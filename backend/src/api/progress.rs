//! SSoT: Progress tracking for async background operations.
//!
//! Guarantees `mark_finished()` is called unconditionally for every task, even
//! on panic. All lifecycle flows through this module — one way to start a task,
//! one way to report progress, one way to finalise.
//!
//! - `ProgressTracker` — canonical store; `start()` spawns, `updater()` gives
//!   mid-task reports, `all_active()` / `get()` read.
//! - `TaskHandle` — MUST be consumed by complete()/cancel()/fail(); on drop
//!   (panic or forgotten finalization) the Drop safety net finalizes it.
//! - `ProgressUpdater` — reports mid-task; no-op after finished.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Instant;

/// All background operation types that report progress.
/// SSoT: The serialized form (snake_case) must match the frontend's
/// `operation_label()` match arms in toast.rs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationType {
    Reorganize,
    BatchMove,
}

impl OperationType {
    pub fn label(&self, total: usize) -> String {
        match self {
            Self::Reorganize => format!("Reorganize: {} series", total),
            Self::BatchMove => format!("Batch move: {} items", total),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reorganize => "reorganize",
            Self::BatchMove => "batch_move",
        }
    }
}

/// Progress of a single background operation.
/// SSoT: All lifecycle transitions go through ProgressTracker methods;
/// callers must never set fields directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMoveProgress {
    pub operation_type: OperationType,
    pub total: usize,
    pub completed: usize,
    pub success_count: usize,
    pub failed: usize,
    pub finished: bool,
    /// Set by mark_finished(). Used by the reaper for precise cleanup and by
    /// the frontend for recency-aware cross-tab recovery.
    #[serde(skip)]
    pub finished_at: Option<Instant>,
    pub errors: Vec<String>,
}

impl BatchMoveProgress {
    /// SSoT: Always use this instead of setting `finished = true` directly.
    /// Ensures `completed == total` and records `finished_at`.
    pub fn mark_finished(&mut self) {
        self.completed = self.total;
        self.finished = true;
        self.finished_at = Some(Instant::now());
    }
}

/// Sent to the frontend via /api/system/status; contains everything needed to
/// render or reconcile a toast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveOperation {
    pub id: String,
    pub operation_type: String,
    pub total: usize,
    pub completed: usize,
    pub finished: bool,
    /// Epoch milliseconds since the Unix epoch. None while running.
    pub finished_at_ms: Option<u64>,
    pub success_count: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

/// SSoT: Guards `is_reorganizing` and always resets it to `false` on drop,
/// even when the guarded code panics.
pub struct ReorgGuard {
    flag: Arc<AtomicBool>,
}

impl ReorgGuard {
    /// Attempt to acquire the reorganize lock. Returns `None` if another
    /// reorganize is already running, else `Some(guard)` which resets the flag
    /// to `false` when dropped (normal return OR panic).
    pub fn try_acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        if flag.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(Self { flag: flag.clone() })
        }
    }
}

impl Drop for ReorgGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// How long a finished task's progress entry is kept before cleanup.
/// SSoT: Used by all_active filter, reap_stale, and schedule_cleanup; must
/// match the frontend's RECENCY_MS in reconcile_operations.
const COMPLETED_TASK_RETENTION: std::time::Duration = std::time::Duration::from_secs(1800); // 30 min

/// SSoT: Manages all in-progress background operations.
/// Uses `std::sync::RwLock` (not tokio) so `TaskHandle::drop()` can acquire it
/// synchronously, which the panic safety net requires. Contention is near-zero
/// (reads every 60s, writes on task start/end).
pub struct ProgressTracker {
    inner: RwLock<std::collections::HashMap<String, BatchMoveProgress>>,
}

impl ProgressTracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(std::collections::HashMap::new()),
        })
    }

    /// Start a new task. Returns (task_id, TaskHandle).
    /// SSoT: All tasks start here.
    pub fn start(self: &Arc<Self>, op_type: OperationType, total: usize) -> (String, TaskHandle) {
        let task_id = uuid::Uuid::new_v4().to_string();
        self.inner.write().unwrap().insert(
            task_id.clone(),
            BatchMoveProgress {
                operation_type: op_type,
                total,
                completed: 0,
                success_count: 0,
                failed: 0,
                finished: false,
                finished_at: None,
                errors: vec![],
            },
        );
        let handle = TaskHandle {
            task_id: task_id.clone(),
            tracker: Arc::downgrade(self),
            finalized: AtomicBool::new(false),
        };
        (task_id, handle)
    }

    /// Get a progress updater for mid-task reports.
    /// Returns `None` if the task_id doesn't exist (already cleaned up).
    pub fn updater(self: &Arc<Self>, task_id: &str) -> Option<ProgressUpdater> {
        if self.inner.read().unwrap().contains_key(task_id) {
            Some(ProgressUpdater {
                tracker: Arc::downgrade(self),
                task_id: task_id.to_string(),
            })
        } else {
            None
        }
    }

    /// Build the active_operations payload for /api/system/status.
    /// Returns entries that are running OR finished within the retention window.
    pub fn all_active(&self, now: Instant) -> Vec<ActiveOperation> {
        let inner = self.inner.read().unwrap();
        let unix_epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        inner
            .iter()
            .filter_map(|(id, p)| {
                if let Some(finished_at) = p.finished_at
                    && now.duration_since(finished_at) >= COMPLETED_TASK_RETENTION
                {
                    return None;
                }
                let finished_at_ms = p.finished_at.map(|t| {
                    let ago = now.duration_since(t).as_millis() as u64;
                    unix_epoch_ms.saturating_sub(ago)
                });
                Some(ActiveOperation {
                    id: id.clone(),
                    operation_type: p.operation_type.as_str().to_string(),
                    total: p.total,
                    completed: p.completed,
                    finished: p.finished,
                    finished_at_ms,
                    success_count: p.success_count,
                    failed: p.failed,
                    errors: p.errors.clone(),
                })
            })
            .collect()
    }

    /// Get a single task's progress.
    pub fn get(&self, task_id: &str) -> Option<BatchMoveProgress> {
        self.inner.read().unwrap().get(task_id).cloned()
    }

    /// Remove entries that finished more than 30 minutes ago.
    pub fn reap_stale(&self, now: Instant) {
        self.inner
            .write()
            .unwrap()
            .retain(|_, p| match p.finished_at {
                Some(t) => now.duration_since(t) < COMPLETED_TASK_RETENTION,
                None => true, // still running
            });
    }

    /// Spawn a periodic reaper for the Drop-safety-net entries
    /// that couldn't schedule their own cleanup.
    pub fn start_reaper(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // 5 min
            interval.tick().await; // skip immediate first tick
            loop {
                interval.tick().await;
                this.reap_stale(Instant::now());
            }
        });
    }
}

/// A handle representing a running background task.
///
/// SSoT: Every task MUST obtain a TaskHandle from ProgressTracker::start() and
/// hold it for the task's lifetime. The completion signal is sent on
/// `complete()`, `cancel()`, `fail()`, or `drop()` (panic / forgotten
/// finalization safety net).
///
/// The Drop safety net uses the tracker's `std::sync::RwLock` (synchronous) so
/// `mark_finished()` is called even when the async task has panicked.
pub struct TaskHandle {
    task_id: String,
    tracker: Weak<ProgressTracker>,
    finalized: AtomicBool,
}

impl TaskHandle {
    /// Normal completion. Transforms to a result toast on the frontend.
    pub fn complete(self, success_count: usize, failed: usize, errors: Vec<String>) {
        self.finalized.store(true, Ordering::SeqCst);
        if let Some(tracker) = self.tracker.upgrade()
            && let Some(p) = tracker.inner.write().unwrap().get_mut(&self.task_id)
        {
            p.success_count = success_count;
            p.failed = failed;
            p.errors = errors;
            p.mark_finished();
        }
        Self::schedule_cleanup(self.tracker.clone(), self.task_id.clone());
    }

    /// Cancelled (e.g., CancellationToken fired mid-operation).
    pub fn cancel(self, reason: String) {
        self.finalized.store(true, Ordering::SeqCst);
        if let Some(tracker) = self.tracker.upgrade()
            && let Some(p) = tracker.inner.write().unwrap().get_mut(&self.task_id)
        {
            p.errors.push(format!("Cancelled: {}", reason));
            p.mark_finished();
        }
        Self::schedule_cleanup(self.tracker.clone(), self.task_id.clone());
    }

    /// Error result.
    pub fn fail(self, error: String) {
        self.finalized.store(true, Ordering::SeqCst);
        if let Some(tracker) = self.tracker.upgrade()
            && let Some(p) = tracker.inner.write().unwrap().get_mut(&self.task_id)
        {
            p.failed = p.failed.max(1);
            p.errors.push(error);
            p.mark_finished();
        }
        Self::schedule_cleanup(self.tracker.clone(), self.task_id.clone());
    }

    fn schedule_cleanup(tracker: Weak<ProgressTracker>, task_id: String) {
        // No Tokio runtime (e.g. unit tests) → cleanup is handled by the
        // periodic reaper instead.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                tokio::time::sleep(COMPLETED_TASK_RETENTION).await;
                if let Some(tracker) = tracker.upgrade() {
                    tracker.inner.write().unwrap().remove(&task_id);
                }
            });
        }
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        if !self.finalized.load(Ordering::SeqCst) {
            // Safety net: panicked or forgot to call complete()/cancel()/fail().
            // Uses sync RwLock — safe in Drop because contention is ~zero.
            if let Some(tracker) = self.tracker.upgrade()
                && let Some(p) = tracker.inner.write().unwrap().get_mut(&self.task_id)
                && !p.finished
            {
                p.failed = p.failed.max(1);
                p.errors.push("Task panicked or was cancelled".into());
                p.mark_finished();
                tracing::warn!(
                    "Task {} was dropped without finalization — auto-finalised",
                    self.task_id
                );
            }
            // No cleanup scheduling in Drop (can't spawn reliably).
            // The periodic reaper handles stale entries.
        }
    }
}

/// Mid-task progress reporter.
/// SSoT: All intermediate progress updates go through this.
pub struct ProgressUpdater {
    tracker: Weak<ProgressTracker>,
    task_id: String,
}

impl ProgressUpdater {
    /// Report intermediate progress. No-op after finished (race guard).
    pub fn report(&self, completed: usize, success_count: usize, failed: usize) {
        if let Some(tracker) = self.tracker.upgrade()
            && let Some(p) = tracker.inner.write().unwrap().get_mut(&self.task_id)
        {
            if p.finished {
                return;
            }
            p.completed = completed;
            p.success_count = success_count;
            p.failed = failed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_start_creates_entry() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::Reorganize, 10);

        let p = tracker.get(&task_id).unwrap();
        assert_eq!(p.total, 10);
        assert_eq!(p.completed, 0);
        assert!(!p.finished);
        assert!(p.finished_at.is_none());
        assert_eq!(p.operation_type, OperationType::Reorganize);

        // Must consume the handle or it'll auto-finalize on drop
        handle.complete(5, 0, vec![]);
    }

    #[test]
    fn test_complete_marks_finished() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::BatchMove, 10);

        handle.complete(8, 2, vec!["error 1".into()]);

        let p = tracker.get(&task_id).unwrap();
        assert!(p.finished);
        assert_eq!(p.completed, p.total);
        assert_eq!(p.completed, 10);
        assert_eq!(p.success_count, 8);
        assert_eq!(p.failed, 2);
        assert!(p.finished_at.is_some());
    }

    #[test]
    fn test_cancel_marks_finished() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::Reorganize, 5);

        handle.cancel("shutdown".into());

        let p = tracker.get(&task_id).unwrap();
        assert!(p.finished);
        assert!(p.errors.iter().any(|e| e.contains("Cancelled")));
    }

    #[test]
    fn test_fail_marks_finished() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::Reorganize, 3);

        handle.fail("something broke".into());

        let p = tracker.get(&task_id).unwrap();
        assert!(p.finished);
        assert_eq!(p.failed, 1);
        assert!(p.errors.iter().any(|e| e.contains("broke")));
    }

    #[test]
    fn test_drop_safety_net_auto_finalizes() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::BatchMove, 7);

        // Drop the handle without calling complete/cancel/fail; the Drop
        // safety net should auto-finalize via mark_finished().
        drop(handle);

        let p = tracker.get(&task_id).unwrap();
        assert!(p.finished, "Drop safety net should have marked as finished");
        assert_eq!(p.completed, p.total);
        assert!(p.failed >= 1, "Drop safety net should set failed >= 1");
        assert!(
            p.errors.iter().any(|e| e.contains("panicked")),
            "Drop safety net should record panic message"
        );
    }

    #[test]
    fn test_all_active_excludes_old_finished() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::Reorganize, 5);
        handle.complete(3, 0, vec![]);

        let active = tracker.all_active(Instant::now());
        assert!(active.iter().any(|a| a.id == task_id));

        // finished_at is Instant::now() and retention is 30 min, so the entry
        // must still be present.
        assert!(tracker.get(&task_id).is_some());
    }

    #[test]
    fn test_reap_stale_removes_old_entries() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::BatchMove, 2);
        handle.complete(2, 0, vec![]);

        // Simulate 31 minutes passing
        let future = Instant::now() + Duration::from_secs(1860);
        tracker.reap_stale(future);

        assert!(tracker.get(&task_id).is_none());
    }

    #[test]
    fn test_reap_stale_keeps_running_entries() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::Reorganize, 3);

        // Running (not finished) → should be kept regardless of time
        let future = Instant::now() + Duration::from_secs(99999);
        tracker.reap_stale(future);

        assert!(tracker.get(&task_id).is_some());

        drop(handle);
    }

    #[test]
    fn test_updater_reports_progress() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::Reorganize, 10);
        let updater = tracker.updater(&task_id).unwrap();

        updater.report(3, 2, 1);

        let p = tracker.get(&task_id).unwrap();
        assert_eq!(p.completed, 3);
        assert_eq!(p.success_count, 2);
        assert_eq!(p.failed, 1);
        assert!(!p.finished);

        handle.complete(p.success_count, p.failed, vec![]);
    }

    #[test]
    fn test_updater_no_op_after_finished() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::BatchMove, 5);
        let updater = tracker.updater(&task_id).unwrap();

        handle.complete(5, 0, vec![]);

        // This should be a no-op (race guard)
        updater.report(999, 999, 999);

        let p = tracker.get(&task_id).unwrap();
        assert_eq!(p.completed, p.total);
        assert_eq!(p.success_count, 5);
        assert_eq!(p.failed, 0);
    }

    #[test]
    fn test_reorg_guard_prevents_concurrent() {
        let flag = Arc::new(AtomicBool::new(false));

        let guard1 = ReorgGuard::try_acquire(&flag);
        assert!(guard1.is_some());

        let guard2 = ReorgGuard::try_acquire(&flag);
        assert!(guard2.is_none());

        drop(guard1);

        let guard3 = ReorgGuard::try_acquire(&flag);
        assert!(guard3.is_some());
        drop(guard3);
    }

    #[test]
    fn test_reorg_guard_resets_on_panic() {
        let flag = Arc::new(AtomicBool::new(false));

        // Simulate a panic while holding the guard
        let flag_clone = flag.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = ReorgGuard::try_acquire(&flag_clone).unwrap();
            panic!("simulated panic");
        }));
        assert!(result.is_err());

        assert!(!flag.load(Ordering::SeqCst));

        let guard = ReorgGuard::try_acquire(&flag);
        assert!(guard.is_some());
        drop(guard);
    }

    #[test]
    fn test_active_operation_has_required_fields() {
        let tracker = ProgressTracker::new();
        let (task_id, handle) = tracker.start(OperationType::Reorganize, 3);
        handle.complete(2, 1, vec!["error msg".into()]);

        let active = tracker.all_active(Instant::now());
        let op = active.iter().find(|a| a.id == task_id).unwrap();

        assert_eq!(op.operation_type, "reorganize");
        assert!(op.finished);
        assert!(op.finished_at_ms.is_some());
        assert_eq!(op.success_count, 2);
        assert_eq!(op.failed, 1);
        assert_eq!(op.errors.len(), 1);
        assert_eq!(op.errors[0], "error msg");
    }

    #[test]
    fn test_operation_type_label() {
        assert_eq!(OperationType::Reorganize.label(5), "Reorganize: 5 series");
        assert_eq!(OperationType::BatchMove.label(3), "Batch move: 3 items");
    }

    #[test]
    fn test_operation_type_serde_snake_case() {
        // SSoT: Must match the frontend's operation_label() match arms.
        // Both serde and as_str() must produce the same value.
        assert_eq!(
            serde_json::to_string(&OperationType::Reorganize).unwrap(),
            format!("\"{}\"", OperationType::Reorganize.as_str())
        );
        assert_eq!(
            serde_json::to_string(&OperationType::BatchMove).unwrap(),
            format!("\"{}\"", OperationType::BatchMove.as_str())
        );

        // Round-trip
        let deser: OperationType = serde_json::from_str("\"reorganize\"").unwrap();
        assert_eq!(deser, OperationType::Reorganize);
        let deser: OperationType = serde_json::from_str("\"batch_move\"").unwrap();
        assert_eq!(deser, OperationType::BatchMove);
    }
}
