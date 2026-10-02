//! Search Queue — serialized, deduplicated dispatch for auto-search operations.
//!
//! ## Design (patterned after `MetadataQueue`)
//!
//! All auto-search paths converge here so that rate-limiting, deduplication,
//! and shutdown handling live in ONE place instead of being scattered across
//! callers:
//!
//!   - **"Search Missing When Added"** (the new checkbox in the Add Series form)
//!   - **Background wanted-episodes auto-search** (CLI recurring task)
//!   - **Displaced episode search** (when a multi-episode download is cancelled)
//!   - **`auto_search_season` dedup check** (the user-facing "Search Season" button
//!     checks this queue's active set to return 409 Conflict when a search for
//!     the same `{series_id}:{season}` is already queued or running)
//!
//! The queue processes one job at a time with a `SEARCH_SEASON_DELAY` pause
//! between consecutive jobs to avoid hammering external indexers.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use crate::db::DbManager;
use crate::organizer::ContentOrganizer;

/// SSoT: delay between consecutive season-search jobs. Both the worker loop and
/// any inline caller rate-limiting read this value.
pub const SEARCH_SEASON_DELAY: Duration = Duration::from_millis(2000);

type SearchFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// A queued search job.
struct SearchJob {
    /// Dedup key: `"{series_id}:{season}"`.
    key: String,
    /// The actual search logic, provided by the submitter.
    fetch_fn: Box<dyn FnOnce() -> SearchFuture + Send>,
}

/// Serialized, deduplicated queue for auto-search operations.
pub struct SearchQueue {
    /// Keys currently queued or in-flight.
    /// Checked by `submit()` for dedup and by external code
    /// (`auto_search_season`) to detect active searches.
    active_searches: Arc<RwLock<HashSet<String>>>,
    /// Signalled during shutdown to stop accepting new work and drain the queue.
    shutdown_token: CancellationToken,
    /// User-initiated jobs (polled first in the biased `select!`).
    manual_tx: mpsc::UnboundedSender<SearchJob>,
    /// Background / automated jobs.
    normal_tx: mpsc::UnboundedSender<SearchJob>,
}

impl SearchQueue {
    /// Create a new search queue with concurrency = 1.
    ///
    /// Spawns a background worker loop that pulls jobs from the priority
    /// channels and executes them one at a time.  The worker runs for the
    /// lifetime of the queue and exits when all channels are closed (which
    /// happens when the `SearchQueue` itself is dropped).
    pub fn new() -> Self {
        let (manual_tx, manual_rx) = mpsc::unbounded_channel();
        let (normal_tx, normal_rx) = mpsc::unbounded_channel();

        let active_searches = Arc::new(RwLock::new(HashSet::new()));
        let shutdown_token = CancellationToken::new();

        let worker_active = active_searches.clone();
        let worker_shutdown = shutdown_token.clone();
        tokio::spawn(Self::worker_loop(
            worker_active,
            worker_shutdown,
            manual_rx,
            normal_rx,
        ));

        Self {
            active_searches,
            shutdown_token,
            manual_tx,
            normal_tx,
        }
    }

    /// Background worker: pulls jobs from the priority channels and executes
    /// them one at a time with a `SEARCH_SEASON_DELAY` pause between jobs.
    ///
    /// Uses a biased `tokio::select!` so Manual is always polled first.
    ///
    /// On shutdown the worker drains all queued items (removing their keys
    /// from `active_searches`) and exits.  Any job that was already dequeued
    /// WILL execute (its key will be removed from `active_searches` after
    /// completion), but no new jobs are started.
    async fn worker_loop(
        active_searches: Arc<RwLock<HashSet<String>>>,
        shutdown_token: CancellationToken,
        mut manual_rx: mpsc::UnboundedReceiver<SearchJob>,
        mut normal_rx: mpsc::UnboundedReceiver<SearchJob>,
    ) {
        loop {
            // ── Shutdown check before receiving ──
            if shutdown_token.is_cancelled() {
                Self::drain_and_exit(&active_searches, &mut manual_rx, &mut normal_rx).await;
                break;
            }

            let job = tokio::select! {
                biased;
                Some(job) = manual_rx.recv() => job,
                Some(job) = normal_rx.recv() => job,
                else => {
                    debug!("Search queue worker: all channels closed, exiting");
                    break;
                }
            };

            // ── Second shutdown check (after receive, before execute) ──
            if shutdown_token.is_cancelled() {
                // This job's key was inserted into active_searches by submit()
                // before being sent to the channel. Since we never ran it,
                // clean it up here.
                active_searches.write().await.remove(&job.key);
                Self::drain_and_exit(&active_searches, &mut manual_rx, &mut normal_rx).await;
                break;
            }

            trace!("Search queue: executing search for {}", job.key);
            (job.fetch_fn)().await;

            // Remove AFTER completion so is_active() stays true during search.
            active_searches.write().await.remove(&job.key);

            trace!("Search queue: completed search for {}, sleeping", job.key);
            tokio::time::sleep(SEARCH_SEASON_DELAY).await;
        }
    }

    /// Drain all remaining queued items and exit.
    async fn drain_and_exit(
        active_searches: &RwLock<HashSet<String>>,
        manual_rx: &mut mpsc::UnboundedReceiver<SearchJob>,
        normal_rx: &mut mpsc::UnboundedReceiver<SearchJob>,
    ) {
        debug!("Search queue worker: shutdown requested, draining remaining jobs");
        let mut active = active_searches.write().await;
        use tokio::sync::mpsc::error::TryRecvError;
        for rx in [manual_rx, normal_rx] {
            loop {
                match rx.try_recv() {
                    Ok(job) => {
                        active.remove(&job.key);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }
    }

    /// Submit a fire-and-forget search job with dedup.
    ///
    /// If a job with the same key (`{series_id}:{season}`) is already queued
    /// or in-flight, this submission is silently skipped (returns `false`).
    /// Returns `true` when the job was accepted.
    pub async fn submit<F, Fut>(&self, key: String, fetch_fn: F) -> bool
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        // Dedup: if already queued or running, skip.
        {
            let mut active = self.active_searches.write().await;
            if active.contains(&key) {
                trace!("Search queue: {} already active, skipping duplicate", key);
                return false;
            }
            active.insert(key.clone());
        }

        if self.shutdown_token.is_cancelled() {
            warn!("Search queue is shut down, rejecting job for {}", key);
            self.active_searches.write().await.remove(&key);
            return false;
        }

        let job = SearchJob {
            key: key.clone(),
            fetch_fn: Box::new(move || Box::pin(fetch_fn())),
        };

        let _ = self.normal_tx.send(job);
        true
    }

    /// Submit a search job bypassing dedup.
    ///
    /// Intended for explicit user actions where the user's intent should
    /// always be honoured even if a background search for the same key
    /// is already queued or running.
    pub async fn submit_override<F, Fut>(&self, key: String, fetch_fn: F) -> bool
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.shutdown_token.is_cancelled() {
            warn!(
                "Search queue is shut down, rejecting override job for {}",
                key
            );
            return false;
        }

        // Bypass dedup, but still insert so subsequent submit() calls see it.
        {
            let mut active = self.active_searches.write().await;
            active.insert(key.clone());
        }

        let job = SearchJob {
            key: key.clone(),
            fetch_fn: Box::new(move || Box::pin(fetch_fn())),
        };

        let _ = self.manual_tx.send(job);
        true
    }

    /// Check whether a key is currently active (queued or in-flight).
    ///
    /// Used by `auto_search_season` to decide whether to return 409 Conflict,
    /// and by `get_auto_season_status` to report search-running state to the
    /// frontend.
    pub async fn is_active(&self, key: &str) -> bool {
        self.active_searches.read().await.contains(key)
    }

    /// Insert a key into the active set without submitting a job.
    ///
    /// Used by `auto_search_season` which manages its own background task
    /// lifecycle via `AutoSearchGuard`.  The guard calls `mark_inactive()`
    /// when the task completes or panics.
    pub async fn mark_active(&self, key: &str) {
        self.active_searches.write().await.insert(key.to_string());
    }

    /// Remove a key from the active set.
    ///
    /// Called by `AutoSearchGuard::drop` to clean up after a
    /// self-managed background task exits.
    pub async fn mark_inactive(&self, key: &str) {
        self.active_searches.write().await.remove(key);
    }

    /// Submit search jobs for ALL monitored-missing episodes of a series.
    ///
    /// Queries the DB for monitored + missing + RELEASED episodes (see
    /// `get_monitored_missing_for_series` — the shared released-date predicate),
    /// groups them by season, and submits each season-group as a single search
    /// job via `submit()`.  Seasons that are already active are silently
    /// skipped (dedup).
    ///
    /// Returns `(seasons_submitted, episodes_total)`.
    ///
    /// Callers: `create_series` (the "Search Missing When Added" checkbox —
    /// runs in the same request, after the metadata sync).
    pub async fn submit_monitored_missing(
        &self,
        series_id: &str,
        db: Arc<DbManager>,
        organizer: &ContentOrganizer,
    ) -> Result<(usize, usize), crate::error::AppError> {
        let episodes = db
            .get_monitored_missing_for_series(series_id)
            .await
            .map_err(|e| {
                crate::error::AppError::Internal(anyhow::anyhow!(
                    "Failed to query monitored missing episodes for {}: {}",
                    series_id,
                    e
                ))
            })?;

        if episodes.is_empty() {
            return Ok((0, 0));
        }

        // Group by season.
        let mut by_season: std::collections::BTreeMap<i32, Vec<i32>> =
            std::collections::BTreeMap::new();
        for (season, episode) in &episodes {
            by_season.entry(*season).or_default().push(*episode);
        }

        let total_episodes = episodes.len();
        let mut submitted = 0usize;

        let organizer = organizer.clone();
        let sid = series_id.to_string();

        for (season, episode_numbers) in &by_season {
            let key = format!("{}:{}", sid, season);
            let org = organizer.clone();
            let s = season.to_string();
            let eps = episode_numbers.clone();
            let db_clone = db.clone();
            let sid_for_closure = sid.clone();

            if self
                .submit(key, move || {
                    let org = org;
                    let sid = sid_for_closure;
                    let s = s;
                    let eps = eps;
                    let db_clone = db_clone;
                    async move {
                        if let Err(e) = org.auto_search_missing(&sid, &s, &eps).await {
                            tracing::error!(
                                "search_monitored_missing: error searching series={} season={}: {}",
                                sid,
                                s,
                                e
                            );
                        }

                        // Track the last-attempt timestamp so the background
                        // wanted-task doesn't immediately re-search the same
                        // episodes on its next poll cycle.
                        let state_key = format!("auto_search_last_attempt_{}_{}", sid, s);
                        let state_value = serde_json::json!({
                            "timestamp": crate::datetime::UtcDateTime::now().to_db_string(),
                            "episodes": &eps,
                        });
                        let _ = db_clone
                            .set_system_state(&state_key, &state_value.to_string())
                            .await;
                    }
                })
                .await
            {
                submitted += 1;
            }
        }

        Ok((submitted, total_episodes))
    }

    /// Cancel all pending jobs and prevent new submissions.
    /// Already-running jobs will complete normally (they've been dequeued
    /// and their key removed from `active_searches` after execution).
    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
    }

    /// Create a child token that external code can use to detect shutdown.
    pub fn child_token(&self) -> CancellationToken {
        self.shutdown_token.child_token()
    }
}

impl Default for SearchQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
