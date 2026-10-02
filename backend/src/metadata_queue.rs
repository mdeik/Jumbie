// Priority-aware queue for metadata fetch operations, modeled on ScanQueue
// (channel-based submission + background worker + semaphore-gated concurrency),
// but with four priority tiers: Manual > Bulk > Normal > Background.
//
// Concurrency is 1 globally, so jobs run back-to-back and cannot flood a
// provider even under burst. Dedup is by `series_id` only: the system has at
// most one active metadata provider, so two fetches for a series are always
// redundant. Fire-and-forget submissions dedup; `submit_and_wait` (Manual)
// always queues so an explicit user action is never dropped.

use crate::error::AppError;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

/// Priority level for a metadata fetch job.
///
/// The worker drains channels in order: Manual → Bulk → Normal → Background.
/// Wider spacing between levels allows future insertions without renaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Lowest — background refresh loop.
    Background = 0,
    /// Rename queue, download path, mode switch, proactive metadata.
    Normal = 10,
    /// Bulk user-initiated fetch (batch endpoint, less urgent than single).
    Bulk = 20,
    /// Manual user-initiated fetch (single-series route).
    Manual = 30,
}

type FetchFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

struct MetadataJob {
    /// Dedup key: the series mapping's primary identifier.
    key: String,
    fetch_fn: Box<dyn FnOnce() -> FetchFuture + Send>,
}

/// Receiver side of the four priority channels, drained in order:
/// Manual → Bulk → Normal → Background.
struct PriorityReceivers {
    manual: mpsc::UnboundedReceiver<MetadataJob>,
    bulk: mpsc::UnboundedReceiver<MetadataJob>,
    normal: mpsc::UnboundedReceiver<MetadataJob>,
    background: mpsc::UnboundedReceiver<MetadataJob>,
}

/// SSoT: per-job execution timeout for metadata fetches.
///
/// Bounds a hung provider (dead plugin IPC, stuck HTTP call) so it cannot block
/// a `create_series` request or shutdown indefinitely. On expiry the job is
/// cancelled: `submit_and_wait` callers get an error via the dropped oneshot.
/// Kept comfortably below the frontend's 120s client timeout.
pub const DEFAULT_JOB_TIMEOUT: Duration = Duration::from_secs(90);

/// Priority-aware queue for metadata fetch operations.
pub struct MetadataQueue {
    /// Series IDs currently queued or in-flight.  Checked by submit() for dedup.
    active_series: Arc<RwLock<HashSet<String>>>,
    /// Signalled during shutdown to stop accepting new work.
    shutdown_token: CancellationToken,
    /// Channels for each priority tier.  Drained in order: Manual → Bulk → Normal → Background.
    manual_tx: mpsc::UnboundedSender<MetadataJob>,
    bulk_tx: mpsc::UnboundedSender<MetadataJob>,
    normal_tx: mpsc::UnboundedSender<MetadataJob>,
    background_tx: mpsc::UnboundedSender<MetadataJob>,
}

impl MetadataQueue {
    /// Create a new metadata queue with concurrency = 1 and the default job
    /// timeout ([`DEFAULT_JOB_TIMEOUT`]).
    ///
    /// Spawns a background worker that runs for the lifetime of the queue and
    /// exits when all channels are closed (i.e. the queue is dropped).
    pub fn new() -> Self {
        Self::with_job_timeout(DEFAULT_JOB_TIMEOUT)
    }

    /// Create a queue with a custom per-job timeout (used by tests).
    pub fn with_job_timeout(job_timeout: Duration) -> Self {
        let (manual_tx, manual_rx) = mpsc::unbounded_channel();
        let (bulk_tx, bulk_rx) = mpsc::unbounded_channel();
        let (normal_tx, normal_rx) = mpsc::unbounded_channel();
        let (background_tx, background_rx) = mpsc::unbounded_channel();

        let active_series = Arc::new(RwLock::new(HashSet::new()));
        let semaphore = Arc::new(Semaphore::new(1));
        let shutdown_token = CancellationToken::new();

        let worker_active = active_series.clone();
        let worker_semaphore = semaphore.clone();
        let worker_shutdown = shutdown_token.clone();
        tokio::spawn(Self::worker_loop(
            worker_active,
            worker_semaphore,
            worker_shutdown,
            job_timeout,
            PriorityReceivers {
                manual: manual_rx,
                bulk: bulk_rx,
                normal: normal_rx,
                background: background_rx,
            },
        ));

        Self {
            active_series,
            shutdown_token,
            manual_tx,
            bulk_tx,
            normal_tx,
            background_tx,
        }
    }

    /// Background worker: pulls jobs from the priority channels and executes
    /// them one at a time (gated by the semaphore).
    ///
    /// Uses a biased `tokio::select!` so Manual is always polled first.  Jobs
    /// within the same priority level are processed FIFO.
    async fn worker_loop(
        active_series: Arc<RwLock<HashSet<String>>>,
        semaphore: Arc<Semaphore>,
        shutdown_token: CancellationToken,
        job_timeout: Duration,
        mut rx: PriorityReceivers,
    ) {
        loop {
            let job = tokio::select! {
                biased;
                Some(job) = rx.manual.recv() => job,
                Some(job) = rx.bulk.recv() => job,
                Some(job) = rx.normal.recv() => job,
                Some(job) = rx.background.recv() => job,
                else => {
                    debug!("Metadata queue worker: all channels closed, exiting");
                    break;
                }
            };

            if shutdown_token.is_cancelled() {
                active_series.write().await.remove(&job.key);
                // Drain remaining queued items so `active_series` tracking is
                // fully cleaned up, then exit.
                use tokio::sync::mpsc::error::TryRecvError;
                for receiver in [
                    &mut rx.manual,
                    &mut rx.bulk,
                    &mut rx.normal,
                    &mut rx.background,
                ] {
                    loop {
                        match receiver.try_recv() {
                            Ok(j) => {
                                active_series.write().await.remove(&j.key);
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => break,
                        }
                    }
                }
                debug!("Metadata queue worker: shutdown requested, exiting");
                break;
            }

            let active = active_series.clone();
            let sem = semaphore.clone();
            let shutdown = shutdown_token.clone();

            tokio::spawn(async move {
                let permit = tokio::select! {
                    permit = sem.acquire() => {
                        match permit {
                            Ok(p) => p,
                            Err(_) => {
                                active.write().await.remove(&job.key);
                                return;
                            }
                        }
                    }
                    _ = shutdown.cancelled() => {
                        trace!("Metadata queue: shutdown before acquiring permit for {}", job.key);
                        active.write().await.remove(&job.key);
                        return;
                    }
                };

                trace!("Metadata queue: starting fetch for {}", job.key);
                // Bound the job: a hung provider must not block requests or
                // shutdown indefinitely. On timeout the future is dropped —
                // `submit_and_wait` callers see the error via the closed
                // oneshot, `submit` callers just miss the work (DB writes roll
                // back, cache writes are idempotent, dedup key cleaned up below).
                match tokio::time::timeout(job_timeout, (job.fetch_fn)()).await {
                    Ok(()) => {}
                    Err(_elapsed) => {
                        warn!(
                            "Metadata queue: job for {} exceeded timeout ({:?}), cancelled",
                            job.key, job_timeout
                        );
                    }
                }
                active.write().await.remove(&job.key);
                drop(permit);
                trace!("Metadata queue: completed fetch for {}", job.key);
            });
        }
    }

    /// Submit a fire-and-forget metadata fetch with dedup.
    ///
    /// If a job with the same `series_id` is already queued or in-flight, this
    /// submission is silently skipped (dedup).
    pub async fn submit<F, Fut>(&self, series_id: String, priority: Priority, fetch_fn: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        {
            let mut active = self.active_series.write().await;
            if active.contains(&series_id) {
                trace!(
                    "Metadata queue: {} already queued, skipping duplicate",
                    series_id
                );
                return;
            }
            active.insert(series_id.clone());
        }

        if self.shutdown_token.is_cancelled() {
            debug!(
                "Metadata queue is shut down, rejecting job for {}",
                series_id
            );
            self.active_series.write().await.remove(&series_id);
            return;
        }

        let job = MetadataJob {
            key: series_id,
            fetch_fn: Box::new(move || Box::pin(fetch_fn())),
        };

        let tx = match priority {
            Priority::Manual => &self.manual_tx,
            Priority::Bulk => &self.bulk_tx,
            Priority::Normal => &self.normal_tx,
            Priority::Background => &self.background_tx,
        };

        let _ = tx.send(job);
    }

    /// Submit a metadata fetch and wait for the result.
    ///
    /// This is intended for Manual priority (user-initiated) operations where
    /// the HTTP handler needs the result to respond to the client.
    ///
    /// Unlike `submit`, this does NOT dedup — the user's explicit action
    /// always goes through, even if a background fetch for the same series
    /// is already queued.
    pub async fn submit_and_wait<F, Fut>(
        &self,
        series_id: String,
        fetch_fn: F,
    ) -> Result<String, AppError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<String, AppError>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        // Manual bypasses dedup, but we still insert into active_series so the
        // worker can clean up afterward.
        {
            let mut active = self.active_series.write().await;
            active.insert(series_id.clone());
        }

        let job = MetadataJob {
            key: series_id,
            fetch_fn: Box::new(move || {
                Box::pin(async move {
                    let result = fetch_fn().await;
                    let _ = tx.send(result);
                })
            }),
        };

        let _ = self.manual_tx.send(job);

        rx.await.map_err(|_| {
            AppError::Internal(anyhow::anyhow!("metadata queue: oneshot channel closed"))
        })?
    }

    /// Cancel all pending jobs and prevent new submissions.
    /// Already-running fetches will complete normally.
    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
    }

    /// Check if a series is currently queued or in-flight.
    /// Used by tests to verify that guards successfully submitted or blocked.
    #[cfg(test)]
    pub(crate) async fn contains(&self, series_id: &str) -> bool {
        self.active_series.read().await.contains(series_id)
    }

    /// Create a child token that the run loop can use to detect shutdown.
    pub fn child_token(&self) -> CancellationToken {
        self.shutdown_token.child_token()
    }
}

impl Default for MetadataQueue {
    fn default() -> Self {
        Self::new()
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn test_queue() -> MetadataQueue {
        MetadataQueue::new()
    }

    fn test_queue_with_timeout(timeout: Duration) -> MetadataQueue {
        MetadataQueue::with_job_timeout(timeout)
    }

    // Dedup tests

    #[tokio::test]
    async fn test_submit_dedup_same_series() {
        let queue = Arc::new(test_queue());
        let call_count = Arc::new(AtomicUsize::new(0));

        // Submit the same series_id twice.
        let q1 = queue.clone();
        let c1 = call_count.clone();
        q1.submit("s1".into(), Priority::Normal, move || {
            let c = c1.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        let q2 = queue.clone();
        let c2 = call_count.clone();
        q2.submit("s1".into(), Priority::Normal, move || {
            let c = c2.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_submit_dedup_different_series() {
        let queue = Arc::new(test_queue());
        let call_count = Arc::new(AtomicUsize::new(0));

        let q1 = queue.clone();
        let c1 = call_count.clone();
        q1.submit("s1".into(), Priority::Normal, move || {
            let c = c1.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        let q2 = queue.clone();
        let c2 = call_count.clone();
        q2.submit("s2".into(), Priority::Normal, move || {
            let c = c2.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    // Priority order tests

    /// Shared setup for priority-order tests: creates a queue and submits a "blocker"
    /// Normal-priority job that sleeps for 50 ms before signalling [`blocker_done`].
    /// After a 5 ms yield to let the worker pick up the blocker, returns the
    /// `(queue, execution_order, blocker_done)` triple so the caller can submit
    /// their test jobs and await the blocker.
    async fn setup_priority_test() -> (
        Arc<MetadataQueue>,
        Arc<RwLock<Vec<String>>>,
        Arc<tokio::sync::Notify>,
    ) {
        let queue = Arc::new(test_queue());
        let execution_order = Arc::new(RwLock::new(Vec::new()));

        let q_blk = queue.clone();
        let blocker_done = Arc::new(tokio::sync::Notify::new());
        let bd = blocker_done.clone();
        q_blk
            .submit("blocker".into(), Priority::Normal, move || {
                let d = bd.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    d.notify_one();
                }
            })
            .await;

        tokio::time::sleep(Duration::from_millis(5)).await;

        (queue, execution_order, blocker_done)
    }

    #[tokio::test]
    async fn test_priority_order_manual_before_normal() {
        let (queue, execution_order, blocker_done) = setup_priority_test().await;

        let q1 = queue.clone();
        let o1 = execution_order.clone();
        q1.submit("s1".into(), Priority::Normal, move || {
            let o = o1.clone();
            async move {
                o.write().await.push("normal".to_string());
            }
        })
        .await;

        let q2 = queue.clone();
        let o2 = execution_order.clone();
        q2.submit("s2".into(), Priority::Manual, move || {
            let o = o2.clone();
            async move {
                o.write().await.push("manual".to_string());
            }
        })
        .await;

        blocker_done.notified().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let order = execution_order.read().await;
        assert_eq!(
            *order,
            vec!["manual".to_string(), "normal".to_string()],
            "Manual should execute before Normal when both channels are non-empty"
        );
    }

    #[tokio::test]
    async fn test_priority_order_normal_before_background() {
        let (queue, execution_order, blocker_done) = setup_priority_test().await;

        let q1 = queue.clone();
        let o1 = execution_order.clone();
        q1.submit("s1".into(), Priority::Background, move || {
            let o = o1.clone();
            async move {
                o.write().await.push("background".to_string());
            }
        })
        .await;

        let q2 = queue.clone();
        let o2 = execution_order.clone();
        q2.submit("s2".into(), Priority::Normal, move || {
            let o = o2.clone();
            async move {
                o.write().await.push("normal".to_string());
            }
        })
        .await;

        blocker_done.notified().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let order = execution_order.read().await;
        assert_eq!(
            *order,
            vec!["normal".to_string(), "background".to_string()],
            "Normal should execute before Background when both channels are non-empty"
        );
    }

    #[tokio::test]
    async fn test_all_priorities_respected() {
        let (queue, execution_order, blocker_done) = setup_priority_test().await;

        let q1 = queue.clone();
        let o1 = execution_order.clone();
        q1.submit("s_a".into(), Priority::Background, move || {
            let o = o1.clone();
            async move {
                o.write().await.push("background".to_string());
            }
        })
        .await;

        let q2 = queue.clone();
        let o2 = execution_order.clone();
        q2.submit("s_b".into(), Priority::Normal, move || {
            let o = o2.clone();
            async move {
                o.write().await.push("normal".to_string());
            }
        })
        .await;

        let q3 = queue.clone();
        let o3 = execution_order.clone();
        q3.submit("s_c".into(), Priority::Manual, move || {
            let o = o3.clone();
            async move {
                o.write().await.push("manual".to_string());
            }
        })
        .await;

        blocker_done.notified().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let order = execution_order.read().await;
        assert_eq!(
            *order,
            vec![
                "manual".to_string(),
                "normal".to_string(),
                "background".to_string()
            ],
            "Priority order must be Manual > Normal > Background"
        );
    }

    // Concurrency tests

    #[tokio::test]
    async fn test_single_concurrency() {
        let queue = Arc::new(test_queue());
        let concurrent_max = Arc::new(AtomicUsize::new(0));
        let concurrent_current = Arc::new(AtomicUsize::new(0));
        let finished_count = Arc::new(AtomicUsize::new(0));

        for i in 0..3 {
            let q = queue.clone();
            let cmax = concurrent_max.clone();
            let ccur = concurrent_current.clone();
            let fin = finished_count.clone();
            tokio::spawn(async move {
                q.submit(format!("s{}", i), Priority::Normal, move || {
                    let cmax = cmax.clone();
                    let ccur = ccur.clone();
                    let fin = fin.clone();
                    async move {
                        let prev = ccur.fetch_add(1, Ordering::SeqCst);
                        loop {
                            let current = cmax.load(Ordering::SeqCst);
                            if prev <= current {
                                break;
                            }
                            if cmax
                                .compare_exchange(current, prev, Ordering::SeqCst, Ordering::SeqCst)
                                .is_ok()
                            {
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        ccur.fetch_sub(1, Ordering::SeqCst);
                        fin.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .await;
            });
        }

        for _ in 0..50 {
            if finished_count.load(Ordering::SeqCst) >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert_eq!(finished_count.load(Ordering::SeqCst), 3);
        let max = concurrent_max.load(Ordering::SeqCst);
        assert!(max <= 1, "Max concurrent should be 1, got {}", max);
    }

    // submit_and_wait tests

    #[tokio::test]
    async fn test_submit_and_wait_returns_result() {
        let queue = Arc::new(test_queue());
        let result = queue
            .submit_and_wait("test-series".to_string(), || async {
                Ok("2024-01-01T00:00:00Z".to_string())
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2024-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn test_submit_and_wait_propagates_error() {
        let queue = Arc::new(test_queue());
        let result = queue
            .submit_and_wait("test-series".to_string(), || async {
                Err(AppError::NotFound("series not found".to_string()))
            })
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound(msg) => assert_eq!(msg, "series not found"),
            _ => panic!("Expected NotFound error"),
        }
    }

    #[tokio::test]
    async fn test_submit_and_wait_job_timeout_returns_error_and_cleans_key() {
        // A job that never completes must be cancelled by the per-job timeout,
        // the caller must receive an error (dropped oneshot — no hang), and the
        // dedup key must be released so the series can be fetched again.
        let queue = Arc::new(test_queue_with_timeout(Duration::from_millis(100)));

        let result = queue
            .submit_and_wait("slow-series".to_string(), || async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok::<String, AppError>("never".to_string())
            })
            .await;

        assert!(
            result.is_err(),
            "timed-out job must surface an error to submit_and_wait"
        );

        // Give the worker a moment to clean up the key after cancelling.
        for _ in 0..50 {
            if !queue.contains("slow-series").await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("dedup key was not released after job timeout");
    }

    #[tokio::test]
    async fn test_submit_and_wait_not_blocked_by_previous_dedup() {
        let queue = Arc::new(test_queue());

        // Submit Normal (fire-and-forget) for this series first.
        let q1 = queue.clone();
        q1.submit("same-series".into(), Priority::Normal, || async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
        })
        .await;

        // submit_and_wait should still go through (Manual bypasses dedup).
        let result = queue
            .submit_and_wait("same-series".to_string(), || async {
                Ok("manual-result".to_string())
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "manual-result");
    }

    // Shutdown tests

    #[tokio::test]
    async fn test_shutdown_stops_new_submissions() {
        let queue = Arc::new(test_queue());
        let call_count = Arc::new(AtomicUsize::new(0));

        queue.shutdown();

        let c = call_count.clone();
        queue
            .submit("s1".into(), Priority::Normal, move || {
                let c2 = c.clone();
                async move {
                    c2.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(call_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_submit_and_wait_after_shutdown_panics() {
        let queue = Arc::new(test_queue());
        queue.shutdown();

        let result = queue
            .submit_and_wait("s1".to_string(), || async {
                Ok("should not run".to_string())
            })
            .await;

        assert!(result.is_err());
    }

    // Cleanup tests

    #[tokio::test]
    async fn test_active_series_cleaned_after_completion() {
        let queue = Arc::new(test_queue());
        let sid = "cleanup-test";

        assert!(!queue.contains(sid).await);

        queue
            .submit("cleanup-test".into(), Priority::Normal, || async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
            })
            .await;

        assert!(queue.contains(sid).await);

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(
            !queue.contains(sid).await,
            "active_series should be cleaned up"
        );
    }

    // Race & edge-case tests

    #[tokio::test]
    async fn test_double_click_manual_same_series() {
        // Two concurrent submit_and_wait calls for the same series.
        // Both should return results (Manual bypasses dedup).
        let queue = Arc::new(test_queue());
        let sid = "double-click-series".to_string();
        let sid2 = sid.clone();

        let (r1, r2) = tokio::join!(
            queue.submit_and_wait(sid.clone(), || async { Ok("result-a".to_string()) }),
            queue.submit_and_wait(sid2, || async { Ok("result-b".to_string()) }),
        );

        assert!(r1.is_ok(), "First Manual should succeed");
        assert!(r2.is_ok(), "Second Manual should also succeed");
    }

    #[tokio::test]
    async fn test_manual_submitted_while_normal_running_same_series() {
        // Normal is already running (holds semaphore). Manual submits for the same
        // series. Manual should bypass dedup, wait for Normal to finish, then run.
        // Uses a watch channel to signal Normal to continue — avoids the
        // lost-notification race that Notify has.
        let queue = Arc::new(test_queue());
        let (signal_tx, _signal_rx) = tokio::sync::watch::channel(false);

        // Submit Normal which blocks until signalled.
        let q1 = queue.clone();
        let mut rx1 = signal_tx.subscribe();
        q1.submit("series-a".into(), Priority::Normal, move || {
            async move {
                // Wait for signal: Manual is queued.
                let _ = rx1.changed().await;
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await;

        // Give the worker time to dequeue Normal and acquire the semaphore.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Submit Manual while Normal is running (signalled but sleeping).
        // Manual bypasses dedup, task waits for semaphore.
        let q2 = queue.clone();
        let manual_handle = tokio::spawn(async move {
            q2.submit_and_wait("series-a".to_string(), || async {
                Ok("manual-result".to_string())
            })
            .await
        });

        // Small delay to let Manual's job reach the channel.
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Signal Normal to finish (stops sleeping, releases semaphore).
        let _ = signal_tx.send(true);

        // Manual should get its result.
        let manual_result = manual_handle.await.unwrap();
        assert!(
            manual_result.is_ok(),
            "Manual should succeed even if Normal ran first"
        );
        assert_eq!(manual_result.unwrap(), "manual-result");
    }

    #[tokio::test]
    async fn test_high_volume_mixed_priority() {
        // Stress test: submit 20 jobs across all priorities. All must complete.
        let queue = Arc::new(test_queue());
        let completed = Arc::new(AtomicUsize::new(0));

        for i in 0..20 {
            let q = queue.clone();
            let c = completed.clone();
            let priority = match i % 3 {
                0 => Priority::Manual,
                1 => Priority::Normal,
                _ => Priority::Background,
            };
            let sid = format!("stress-{}", i);
            tokio::spawn(async move {
                q.submit(sid, priority, move || {
                    let c = c.clone();
                    async move {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        c.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .await;
            });
        }

        // Poll until all 20 complete.
        for _ in 0..100 {
            if completed.load(Ordering::SeqCst) >= 20 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(
            completed.load(Ordering::SeqCst),
            20,
            "All 20 jobs must complete under mixed priority"
        );
    }

    #[tokio::test]
    async fn test_manual_dedup_interaction() {
        // Normal is dedup'd for series. Then Manual bypasses dedup and runs.
        // Verify Normal is skipped (dedup) but Manual runs.
        let queue = Arc::new(test_queue());
        let p1_ran = Arc::new(AtomicUsize::new(0));
        let p0_ran = Arc::new(AtomicUsize::new(0));

        let q1 = queue.clone();
        let p1c = p1_ran.clone();
        q1.submit("series-x".into(), Priority::Normal, move || {
            let c = p1c.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                c.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;

        // Same series again at Normal — should be dedup'd.
        let q2 = queue.clone();
        let p1c2 = p1_ran.clone();
        q2.submit("series-x".into(), Priority::Normal, move || {
            let c = p1c2.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;

        // Manual for same series — bypasses dedup.
        let p0c = p0_ran.clone();
        let result = queue
            .submit_and_wait("series-x".to_string(), move || {
                let c = p0c.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok("manual-won".to_string())
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "manual-won");

        // Wait for Normal to finish.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Normal should have run exactly once (second was dedup'd).
        assert_eq!(p1_ran.load(Ordering::SeqCst), 1, "Normal should run once");
        // Manual should have run exactly once.
        assert_eq!(p0_ran.load(Ordering::SeqCst), 1, "Manual should run once");
    }

    #[tokio::test]
    async fn test_worker_exits_on_shutdown() {
        // Verify the worker loop terminates after shutdown, even with
        // items still in the channels. Without this fix, the worker
        // would loop forever blocked on recv().
        let queue = Arc::new(MetadataQueue::new());

        // Submit a blocker that holds the single permit
        let blocker = Arc::new(tokio::sync::Barrier::new(2));
        let b = blocker.clone();
        queue
            .submit("blocker".into(), Priority::Normal, move || {
                let b = b.clone();
                async move {
                    b.wait().await;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            })
            .await;

        // Give the blocker time to start executing (acquire the permit)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Submit queued items across multiple priority levels
        let ran_manual = Arc::new(AtomicUsize::new(0));
        let ran_normal = Arc::new(AtomicUsize::new(0));
        let ran_background = Arc::new(AtomicUsize::new(0));

        let r0 = ran_manual.clone();
        queue
            .submit("queued_manual".into(), Priority::Manual, move || {
                let r = r0.clone();
                async move {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        let r1 = ran_normal.clone();
        queue
            .submit("queued_normal".into(), Priority::Normal, move || {
                let r = r1.clone();
                async move {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        let r2 = ran_background.clone();
        queue
            .submit(
                "queued_background".into(),
                Priority::Background,
                move || {
                    let r = r2.clone();
                    async move {
                        r.fetch_add(1, Ordering::SeqCst);
                    }
                },
            )
            .await;

        // Shutdown — worker should drain queued items from tracking and exit
        queue.shutdown();

        // Give the worker time to process shutdown
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Queued items should be removed from tracking (drained by worker)
        // and none should have actually executed
        assert!(!queue.contains("queued_manual").await);
        assert!(!queue.contains("queued_normal").await);
        assert!(!queue.contains("queued_background").await);
        assert_eq!(
            ran_manual.load(Ordering::SeqCst),
            0,
            "Manual should not run"
        );
        assert_eq!(
            ran_normal.load(Ordering::SeqCst),
            0,
            "Normal should not run"
        );
        assert_eq!(
            ran_background.load(Ordering::SeqCst),
            0,
            "Background should not run"
        );

        // The in-flight blocker is still running (not interrupted)
        assert!(queue.contains("blocker").await);

        // Release blocker so it finishes cleanly
        blocker.wait().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!queue.contains("blocker").await);

        // After shutdown, new submissions are rejected
        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();
        queue
            .submit("post_shutdown".into(), Priority::Normal, move || {
                let r = r.clone();
                async move {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(ran.load(Ordering::SeqCst), 0, "No jobs after shutdown");
    }
}
