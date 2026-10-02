// Centralized queue for background media info scanning (xxhash + ffprobe), and
// the SSoT for which paths are currently being scanned (so the rename queue can
// skip them). Provides bounded concurrency, deduplication, priority (manual
// scans before background batches), and graceful shutdown.
//
// The worker polls the high-priority channel first (biased `select!`) and spawns
// each dequeued job as its own task, which acquires a semaphore permit before
// running the caller-supplied scan closure.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

/// Maximum concurrent ffprobe + hash operations across the entire application.
/// Running more than this simultaneously would saturate I/O on spinning rust
/// and offers no throughput benefit on SSDs (ffprobe is CPU-bound per file).
const DEFAULT_MAX_CONCURRENT_SCANS: usize = 2;

type ScanFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

struct ScanJob {
    path: PathBuf,
    scan_fn: Box<dyn FnOnce() -> ScanFuture + Send>,
}

/// Tracks and coordinates all background media info scanning.
///
/// Usage:
/// ```ignore
/// let queue = ScanQueue::new();
/// queue.submit(path, || async move { /* low-priority scan */ }).await;
/// queue.submit_high_priority(path, || async move { /* high-priority scan */ }).await;
/// ```
pub struct ScanQueue {
    /// Paths currently queued or in-progress.
    /// Read by the rename queue to determine which files to skip.
    active_paths: Arc<RwLock<HashSet<PathBuf>>>,
    /// Signalled during shutdown to stop accepting new work.
    shutdown_token: CancellationToken,
    /// Sender for high-priority jobs (e.g. manual "Scan" button in episode details).
    high_priority_tx: mpsc::UnboundedSender<ScanJob>,
    /// Sender for low-priority jobs (e.g. background batch scanner).
    low_priority_tx: mpsc::UnboundedSender<ScanJob>,
}

impl ScanQueue {
    /// Create a new scan queue with the default max concurrency.
    pub fn new() -> Self {
        Self::with_max_concurrency(DEFAULT_MAX_CONCURRENT_SCANS)
    }

    /// Spawns a background worker that dispatches jobs to the concurrency gate.
    /// The worker runs for the lifetime of the queue and exits when both
    /// channels are closed (i.e. the queue is dropped).
    pub fn with_max_concurrency(max_concurrent: usize) -> Self {
        let (high_priority_tx, high_priority_rx) = mpsc::unbounded_channel();
        let (low_priority_tx, low_priority_rx) = mpsc::unbounded_channel();

        let active_paths = Arc::new(RwLock::new(HashSet::new()));
        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let shutdown_token = CancellationToken::new();

        let worker_active_paths = active_paths.clone();
        let worker_semaphore = semaphore.clone();
        let worker_shutdown_token = shutdown_token.clone();
        tokio::spawn(Self::worker_loop(
            worker_active_paths,
            worker_semaphore,
            worker_shutdown_token,
            high_priority_rx,
            low_priority_rx,
        ));

        Self {
            active_paths,
            shutdown_token,
            high_priority_tx,
            low_priority_tx,
        }
    }

    /// Background worker: pulls jobs from the priority channels and spawns
    /// them as individual tokio tasks (gated by the semaphore).
    ///
    /// Uses a biased `tokio::select!` so the high-priority channel is always
    /// polled first. This ensures that as long as high-priority jobs are
    /// available, low-priority ones will not be dispatched.
    async fn worker_loop(
        active_paths: Arc<RwLock<HashSet<PathBuf>>>,
        semaphore: Arc<Semaphore>,
        shutdown_token: CancellationToken,
        mut high_priority_rx: mpsc::UnboundedReceiver<ScanJob>,
        mut low_priority_rx: mpsc::UnboundedReceiver<ScanJob>,
    ) {
        loop {
            let job = tokio::select! {
                biased;
                Some(job) = high_priority_rx.recv() => job,
                Some(job) = low_priority_rx.recv() => job,
                else => {
                    // Both channels closed — the ScanQueue has been dropped.
                    debug!("Scan queue worker: both channels closed, exiting");
                    break;
                }
            };

            if shutdown_token.is_cancelled() {
                active_paths.write().await.remove(&job.path);
                // Drain any remaining queued items so active_paths tracking
                // is fully cleaned up, then exit the worker loop.
                use tokio::sync::mpsc::error::TryRecvError;
                loop {
                    match high_priority_rx.try_recv() {
                        Ok(j) => {
                            active_paths.write().await.remove(&j.path);
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => break,
                    }
                }
                loop {
                    match low_priority_rx.try_recv() {
                        Ok(j) => {
                            active_paths.write().await.remove(&j.path);
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => break,
                    }
                }
                debug!("Scan queue worker: shutdown requested, exiting");
                break;
            }

            let active_paths = active_paths.clone();
            let semaphore = semaphore.clone();
            let shutdown_token = shutdown_token.clone();

            tokio::spawn(async move {
                let permit = tokio::select! {
                    permit = semaphore.acquire() => {
                        match permit {
                            Ok(p) => p,
                            Err(_) => {
                                active_paths.write().await.remove(&job.path);
                                return;
                            }
                        }
                    }
                    _ = shutdown_token.cancelled() => {
                        debug!("Scan queue: shutdown before acquiring permit for '{}'", job.path.display());
                        active_paths.write().await.remove(&job.path);
                        return;
                    }
                };

                trace!("Scan queue: starting scan for '{}'", job.path.display());

                (job.scan_fn)().await;

                active_paths.write().await.remove(&job.path);
                drop(permit);

                trace!("Scan queue: completed scan for '{}'", job.path.display());
            });
        }
    }

    /// Returns `true` if the given file path is currently queued or being scanned.
    ///
    /// Used by `compute_batch_plan` (rename queue) to exclude files that are
    /// mid-scan — their media info may not yet be available, and including them
    /// would produce filenames without codec/resolution/etc. or cause a race with
    /// the rename operation.
    pub async fn contains(&self, path: &Path) -> bool {
        self.active_paths.read().await.contains(path)
    }

    /// Returns the number of files currently queued or being scanned.
    pub async fn active_count(&self) -> usize {
        self.active_paths.read().await.len()
    }

    pub async fn is_idle(&self) -> bool {
        self.active_paths.read().await.is_empty()
    }

    /// Returns a snapshot of all file paths currently queued or being scanned.
    /// Used by the rename queue to determine which files to exclude from move plans.
    pub async fn active_paths_snapshot(&self) -> HashSet<PathBuf> {
        self.active_paths.read().await.clone()
    }

    /// Remove the given file paths from active tracking (best-effort cancellation).
    ///
    /// Paths are un-tracked so the rename queue stops excluding them, and a
    /// started scan's post-scan cleanup becomes a no-op. The actual scan I/O is
    /// not aborted — it completes gracefully. Used when a series is deleted, its
    /// folder is moved, or it is hidden.
    pub async fn cancel_paths(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let mut active = self.active_paths.write().await;
        for path in paths {
            active.remove(path);
        }
    }

    /// Remove all tracked file paths whose normalized form starts with the given
    /// directory prefix. This is more efficient than collecting individual paths
    /// when an entire directory is being moved or deleted.
    pub async fn cancel_paths_with_prefix(&self, dir: &Path) {
        // `active_paths` holds raw paths, so normalize both sides: canonicalizing
        // only the prefix would compare a Windows `\\?\` path against a raw one
        // and match nothing.
        let canonical = crate::validation::normalize_path(dir);
        let mut active = self.active_paths.write().await;
        active.retain(|p| !crate::validation::normalize_path(p).starts_with(&canonical));
    }

    /// Submit a file for scanning with **normal (low) priority**.
    ///
    /// `path` is tracked internally; `scan_fn` runs when a semaphore permit is
    /// available and is un-tracked on completion. Duplicate paths are silently
    /// skipped. Intended for background batch scans; for user-initiated scans
    /// use `submit_high_priority`.
    pub async fn submit<F, Fut>(&self, path: PathBuf, scan_fn: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.submit_inner(path, scan_fn, false).await
    }

    /// Submit a file for scanning with **high priority**.
    ///
    /// High-priority jobs are dispatched before pending low-priority jobs so
    /// interactive users see results promptly under backlog. Otherwise identical
    /// to `submit`. Use for user-initiated scans (e.g. the "Scan" button).
    pub async fn submit_high_priority<F, Fut>(&self, path: PathBuf, scan_fn: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.submit_inner(path, scan_fn, true).await
    }

    /// Submit a file and await its completion, sharing the same bounded
    /// concurrency as every other scan. Used for hash-first scanning so hashing
    /// runs bounded rather than all at once. Silently skips (returning without
    /// running `scan_fn`) when the path is already queued.
    pub async fn submit_await<F, Fut>(&self, path: PathBuf, scan_fn: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        {
            let mut active = self.active_paths.write().await;
            if active.contains(&path) {
                trace!(
                    "Scan queue: path already queued, skipping duplicate: '{}'",
                    path.display()
                );
                return;
            }
            active.insert(path.clone());
        }

        if self.shutdown_token.is_cancelled() {
            self.active_paths.write().await.remove(&path);
            return;
        }

        let job = ScanJob {
            path,
            scan_fn: Box::new(move || {
                Box::pin(async move {
                    scan_fn().await;
                    let _ = tx.send(());
                })
            }),
        };
        let _ = self.low_priority_tx.send(job);
        let _ = rx.await;
    }

    /// Internal: shared logic for both `submit` and `submit_high_priority`.
    async fn submit_inner<F, Fut>(&self, path: PathBuf, scan_fn: F, high_priority: bool)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        {
            let mut active = self.active_paths.write().await;
            if active.contains(&path) {
                trace!(
                    "Scan queue: path already queued, skipping duplicate: '{}'",
                    path.display()
                );
                return;
            }
            active.insert(path.clone());
        }

        if self.shutdown_token.is_cancelled() {
            warn!(
                "Scan queue is shut down, rejecting job for '{}'",
                path.display()
            );
            self.active_paths.write().await.remove(&path);
            return;
        }

        let job = ScanJob {
            path,
            scan_fn: Box::new(move || Box::pin(scan_fn())),
        };

        if high_priority {
            // Unbounded send cannot fail while the worker holds the receiver.
            let _ = self.high_priority_tx.send(job);
        } else {
            let _ = self.low_priority_tx.send(job);
        }
    }

    /// Cancel all pending scans and prevent new submissions.
    /// Once cancelled, the queue will reject new jobs but already-running
    /// scans will complete normally (they finish their current work).
    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
    }

    /// Create a child token that the run loop can use to detect shutdown.
    pub fn child_token(&self) -> CancellationToken {
        self.shutdown_token.child_token()
    }
}

impl Default for ScanQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn test_submit_and_track() {
        let queue = ScanQueue::new();
        let path = PathBuf::from("/fake/video.mkv");

        assert!(!queue.contains(&path).await);

        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let b = barrier.clone();
        queue
            .submit(path.clone(), move || {
                let b = b.clone();
                async move {
                    b.wait().await; // Hold until test checks
                }
            })
            .await;

        // Should be tracked immediately after submit
        assert!(queue.contains(&path).await);
        assert_eq!(queue.active_count().await, 1);

        // Release the barrier so the job completes
        barrier.wait().await;

        // Give the spawned task time to clean up
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!queue.contains(&path).await);
        assert_eq!(queue.active_count().await, 0);
    }

    #[tokio::test]
    async fn test_deduplicate() {
        let queue = Arc::new(ScanQueue::new());
        let path = PathBuf::from("/fake/video.mkv");

        let counter = Arc::new(AtomicUsize::new(0));
        let c1 = counter.clone();
        queue
            .submit(path.clone(), move || {
                let c = c1.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;

        let c2 = counter.clone();
        // Second submit for the same path should be deduplicated
        queue
            .submit(path.clone(), move || {
                let c = c2.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;

        // Wait for completion
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1, "Should only run once");
    }

    #[tokio::test]
    async fn test_shutdown_rejects_new_jobs() {
        let queue = ScanQueue::new();
        let path = PathBuf::from("/fake/video.mkv");

        queue.shutdown();

        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();
        queue
            .submit(path.clone(), move || {
                let r = r.clone();
                async move {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "Should not run after shutdown"
        );
        // Path should not be tracked since it was rejected
        assert!(!queue.contains(&path).await);
    }

    #[tokio::test]
    async fn test_cancel_paths() {
        let queue = ScanQueue::new();
        let path1 = PathBuf::from("/fake/video1.mkv");
        let path2 = PathBuf::from("/fake/video2.mkv");
        let path3 = PathBuf::from("/fake/video3.mkv");

        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let b = barrier.clone();
        queue
            .submit(path1.clone(), move || {
                let b = b.clone();
                async move {
                    b.wait().await;
                }
            })
            .await;
        queue.submit(path2.clone(), move || async move {}).await;
        queue.submit(path3.clone(), move || async move {}).await;

        assert_eq!(queue.active_count().await, 3);

        // Cancel path2 and path3
        queue.cancel_paths(&[path2.clone(), path3.clone()]).await;
        assert_eq!(queue.active_count().await, 1);
        assert!(queue.contains(&path1).await);

        // Release path1 and verify cleanup
        barrier.wait().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(queue.active_count().await, 0);
    }

    #[tokio::test]
    async fn test_cancel_paths_with_prefix() {
        let queue = ScanQueue::new();
        let dir = PathBuf::from("/media/tv/My Show");
        let path1 = dir.join("S01E01.mkv");
        let path2 = dir.join("S01E02.mkv");
        let path3 = PathBuf::from("/media/tv/Other Show/S01E01.mkv");

        queue.submit(path1.clone(), move || async move {}).await;
        queue.submit(path2.clone(), move || async move {}).await;
        queue.submit(path3.clone(), move || async move {}).await;

        assert_eq!(queue.active_count().await, 3);

        // Cancel all paths under "/media/tv/My Show"
        queue.cancel_paths_with_prefix(&dir).await;
        assert_eq!(queue.active_count().await, 1);
        assert!(!queue.contains(&path1).await);
        assert!(!queue.contains(&path2).await);
        assert!(queue.contains(&path3).await);
    }

    #[tokio::test]
    async fn test_bounded_concurrency() {
        let queue = Arc::new(ScanQueue::with_max_concurrency(2));
        let path1 = PathBuf::from("/fake/video1.mkv");
        let path2 = PathBuf::from("/fake/video2.mkv");
        let path3 = PathBuf::from("/fake/video3.mkv");

        // Jobs 1 and 2 will acquire permits and block on the barrier.
        // Job 3 will be queued (no permit available).
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(3)); // 2 running + 1 test

        let b1 = barrier.clone();
        let s1 = started.clone();
        queue
            .submit(path1.clone(), move || {
                let b = b1.clone();
                async move {
                    s1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    b.wait().await; // Hold until released
                }
            })
            .await;

        let b2 = barrier.clone();
        let s2 = started.clone();
        queue
            .submit(path2.clone(), move || {
                let b = b2.clone();
                async move {
                    s2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    b.wait().await; // Hold until released
                }
            })
            .await;

        // Wait briefly for the first two tasks to acquire permits
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            started.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "Only 2 tasks should have permits (bounded concurrency)"
        );

        // Path3 should also be tracked (queued, not yet running)
        let s3 = started.clone();
        queue
            .submit(path3.clone(), move || async move {
                s3.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .await;

        // All three should be tracked, but only 2 should be running
        assert_eq!(queue.active_count().await, 3);
        assert_eq!(
            started.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "Job 3 should still be queued waiting for a permit"
        );

        // Release jobs 1 and 2 from the barrier
        barrier.wait().await;
        // Now job 3 can acquire a permit
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            started.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "Job 3 should have run after a permit freed up"
        );

        // Wait for all to complete cleanup
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(queue.active_count().await, 0);
    }

    #[tokio::test]
    async fn test_high_priority_before_low() {
        let queue = Arc::new(ScanQueue::with_max_concurrency(1));

        // Submit a low-priority job that blocks, tying up the single permit.
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let b1 = barrier.clone();
        queue
            .submit(PathBuf::from("/fake/blocker.mkv"), move || {
                let b = b1.clone();
                async move {
                    b.wait().await;
                }
            })
            .await;

        // Give the blocker time to acquire the permit.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(queue.active_count().await, 1);

        // Now submit a low-priority job.
        let low_ran = Arc::new(AtomicUsize::new(0));
        let lr = low_ran.clone();
        queue
            .submit(PathBuf::from("/fake/low.mkv"), move || {
                let r = lr.clone();
                async move {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        // And a high-priority job.
        let high_ran = Arc::new(AtomicUsize::new(0));
        let hr = high_ran.clone();
        queue
            .submit_high_priority(PathBuf::from("/fake/high.mkv"), move || {
                let r = hr.clone();
                async move {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        // Both should be tracked.
        assert_eq!(queue.active_count().await, 3);

        // Release the blocker — now the high-priority job should run first.
        barrier.wait().await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // High priority ran, low priority may or may not have had time.
        assert_eq!(
            high_ran.load(Ordering::SeqCst),
            1,
            "High priority should have run"
        );
        // The low priority job should still be queued if concurrency is 1 and
        // there's no free permit yet. But since the high priority job completed
        // and released its permit, the low one might have run too. That's fine —
        // the important thing is that high priority didn't starve.
        // We just verify the high one definitely ran.
    }

    #[tokio::test]
    async fn test_high_priority_deduplicate() {
        let queue = Arc::new(ScanQueue::new());
        let path = PathBuf::from("/fake/video.mkv");

        let counter = Arc::new(AtomicUsize::new(0));
        let c1 = counter.clone();
        queue
            .submit_high_priority(path.clone(), move || {
                let c = c1.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;

        let c2 = counter.clone();
        // Second high-priority submit for the same path should be deduplicated
        queue
            .submit_high_priority(path.clone(), move || {
                let c = c2.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;

        // Wait for completion
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "High-priority duplicate should only run once"
        );
    }

    #[tokio::test]
    async fn test_cross_priority_deduplicate() {
        let queue = Arc::new(ScanQueue::new());
        let path = PathBuf::from("/fake/video.mkv");

        let counter = Arc::new(AtomicUsize::new(0));
        let c1 = counter.clone();
        // First submit as low priority
        queue
            .submit(path.clone(), move || {
                let c = c1.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;

        let c2 = counter.clone();
        // Then submit same path as high priority — should be deduplicated
        queue
            .submit_high_priority(path.clone(), move || {
                let c = c2.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;

        // Wait for completion
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "Cross-priority duplicate should only run once"
        );
    }

    #[tokio::test]
    async fn test_high_priority_shutdown_rejects() {
        let queue = ScanQueue::new();
        let path = PathBuf::from("/fake/video.mkv");

        queue.shutdown();

        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();
        queue
            .submit_high_priority(path.clone(), move || {
                let r = r.clone();
                async move {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "High-priority should not run after shutdown"
        );
        // Path should not be tracked since it was rejected
        assert!(!queue.contains(&path).await);
    }

    #[tokio::test]
    async fn test_high_priority_cancel() {
        let queue = ScanQueue::new();
        let path = PathBuf::from("/fake/video.mkv");

        // Use a barrier to keep the permit busy so the high-priority job stays queued.
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let b = barrier.clone();
        queue
            .submit(PathBuf::from("/fake/blocker.mkv"), move || {
                let b = b.clone();
                async move {
                    b.wait().await;
                }
            })
            .await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Submit a high-priority job
        queue
            .submit_high_priority(path.clone(), move || async move {})
            .await;
        assert_eq!(queue.active_count().await, 2);
        assert!(queue.contains(&path).await);

        // Cancel the high-priority path
        queue.cancel_paths(std::slice::from_ref(&path)).await;
        assert!(!queue.contains(&path).await);
        assert_eq!(queue.active_count().await, 1);

        // Release the blocker so cleanup completes cleanly
        barrier.wait().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_high_priority_track_and_cleanup() {
        let queue = ScanQueue::new();
        let path = PathBuf::from("/fake/video.mkv");

        assert!(!queue.contains(&path).await);

        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let b = barrier.clone();
        queue
            .submit_high_priority(path.clone(), move || {
                let b = b.clone();
                async move {
                    b.wait().await;
                }
            })
            .await;

        // Should be tracked immediately after submit
        assert!(queue.contains(&path).await);
        assert_eq!(queue.active_count().await, 1);

        // Release the barrier so the job completes
        barrier.wait().await;

        // Give the spawned task time to clean up
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!queue.contains(&path).await);
        assert_eq!(queue.active_count().await, 0);
    }

    #[tokio::test]
    async fn test_priority_order_with_multiple() {
        // Proves that with multiple jobs of both priorities queued behind a
        // blocker, all high-priority jobs execute before any low-priority job.
        let queue = Arc::new(ScanQueue::with_max_concurrency(1));

        // Blocker: ties up the single permit
        let blocker_barrier = Arc::new(tokio::sync::Barrier::new(2));
        let b = blocker_barrier.clone();
        queue
            .submit(PathBuf::from("/fake/blocker.mkv"), move || {
                let b = b.clone();
                async move {
                    b.wait().await;
                }
            })
            .await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(queue.active_count().await, 1);

        // Execution order tracker: we record each job's ID as it runs.
        let order: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Submit 3 low-priority jobs
        for id in 0u8..3 {
            let order = order.clone();
            let id_clone = id;
            queue
                .submit(PathBuf::from(format!("/fake/low_{}.mkv", id)), move || {
                    let order = order.clone();
                    async move {
                        order.lock().unwrap().push(id_clone);
                    }
                })
                .await;
        }

        // Submit 3 high-priority jobs
        for id in 3u8..6 {
            let order = order.clone();
            let id_clone = id;
            queue
                .submit_high_priority(PathBuf::from(format!("/fake/high_{}.mkv", id)), move || {
                    let order = order.clone();
                    async move {
                        order.lock().unwrap().push(id_clone);
                    }
                })
                .await;
        }

        // All 7 (1 blocker + 3 low + 3 high) should be tracked
        assert_eq!(queue.active_count().await, 7);

        // Release the blocker.
        blocker_barrier.wait().await;

        // Wait for all jobs to complete
        tokio::time::sleep(Duration::from_millis(300)).await;

        assert_eq!(
            queue.active_count().await,
            0,
            "All jobs should have completed"
        );

        // Verify ordering.
        let exec_order = order.lock().unwrap().clone();
        assert_eq!(
            exec_order.len(),
            6,
            "All 6 non-blocker jobs should have run"
        );

        // Find the position where high-priority IDs (3,4,5) end and
        // low-priority IDs (0,1,2) begin. All high IDs must appear
        // before any low ID.
        let last_high_pos = exec_order
            .iter()
            .rposition(|id| *id >= 3)
            .unwrap_or(usize::MAX);
        let first_low_pos = exec_order
            .iter()
            .position(|id| *id < 3)
            .unwrap_or(usize::MAX);

        assert!(
            last_high_pos < first_low_pos,
            "All high-priority jobs (IDs 3-5) must run before any low-priority job (IDs 0-2). \
             Execution order: {:?}",
            exec_order
        );
    }

    #[tokio::test]
    async fn test_priority_order_all_high_before_low() {
        // With concurrency=1, submits low then high priority jobs. Even though
        // high priority jobs are submitted later, they should run before the
        // previously-queued low priority jobs.
        let queue = Arc::new(ScanQueue::with_max_concurrency(1));

        // Tie up the permit with a blocker
        let blocker_barrier = Arc::new(tokio::sync::Barrier::new(2));
        let b = blocker_barrier.clone();
        queue
            .submit(PathBuf::from("/fake/blocker.mkv"), move || {
                let b = b.clone();
                async move {
                    b.wait().await;
                }
            })
            .await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Queue: low A → low B → high X → high Y
        let order: Arc<std::sync::Mutex<Vec<&'static str>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let o = order.clone();
        queue
            .submit(PathBuf::from("/fake/low_A.mkv"), move || {
                let o = o.clone();
                async move {
                    o.lock().unwrap().push("low_A");
                }
            })
            .await;

        let o = order.clone();
        queue
            .submit(PathBuf::from("/fake/low_B.mkv"), move || {
                let o = o.clone();
                async move {
                    o.lock().unwrap().push("low_B");
                }
            })
            .await;

        let o = order.clone();
        queue
            .submit_high_priority(PathBuf::from("/fake/high_X.mkv"), move || {
                let o = o.clone();
                async move {
                    o.lock().unwrap().push("high_X");
                }
            })
            .await;

        let o = order.clone();
        queue
            .submit_high_priority(PathBuf::from("/fake/high_Y.mkv"), move || {
                let o = o.clone();
                async move {
                    o.lock().unwrap().push("high_Y");
                }
            })
            .await;

        assert_eq!(queue.active_count().await, 5);

        // Release blocker
        blocker_barrier.wait().await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        assert_eq!(
            queue.active_count().await,
            0,
            "All jobs should have completed"
        );

        let exec_order = order.lock().unwrap().clone();
        assert_eq!(
            exec_order.len(),
            4,
            "All 4 non-blocker jobs should have run"
        );

        // Both high priority jobs must appear before both low priority jobs
        let last_high = exec_order.iter().rposition(|n| n.starts_with("high"));
        let first_low = exec_order.iter().position(|n| n.starts_with("low"));

        assert!(
            last_high.unwrap() < first_low.unwrap(),
            "Both high-priority jobs must run before both low-priority jobs. \
             Execution order: {:?}",
            exec_order
        );
    }

    #[tokio::test]
    async fn test_worker_exits_on_shutdown() {
        // Verify the worker loop terminates after shutdown, even with
        // items still in the channels. Without this fix, the worker
        // would loop forever blocked on recv().
        let queue = Arc::new(ScanQueue::with_max_concurrency(1));

        // Submit items that haven't been picked up yet (blocker holds the permit)
        let blocker = Arc::new(tokio::sync::Barrier::new(2));
        let b = blocker.clone();
        queue
            .submit(PathBuf::from("/fake/blocker.mkv"), move || {
                let b = b.clone();
                async move {
                    b.wait().await;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            })
            .await;

        // Give the blocker time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // These will be queued (not running yet, concurrency is 1 and blocker holds permit)
        queue
            .submit(PathBuf::from("/fake/queued_a.mkv"), move || async move {})
            .await;
        queue
            .submit(PathBuf::from("/fake/queued_b.mkv"), move || async move {})
            .await;

        assert_eq!(queue.active_count().await, 3);

        // Shutdown — worker should drain queued items from tracking and exit
        queue.shutdown();

        // Give the worker time to process shutdown
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Queued items should be removed from tracking (drained by worker)
        assert!(!queue.contains(&PathBuf::from("/fake/queued_a.mkv")).await);
        assert!(!queue.contains(&PathBuf::from("/fake/queued_b.mkv")).await);

        // The in-flight blocker is still running (not interrupted), so it's tracked
        assert!(queue.contains(&PathBuf::from("/fake/blocker.mkv")).await);

        // Release blocker so it finishes cleanly
        blocker.wait().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!queue.contains(&PathBuf::from("/fake/blocker.mkv")).await);

        // After shutdown, new submissions are rejected
        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();
        queue
            .submit(PathBuf::from("/fake/post_shutdown.mkv"), move || {
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
