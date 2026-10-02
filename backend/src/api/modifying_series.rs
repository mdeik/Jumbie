//! Per-series modification lock (SSoT for preventing concurrent structural
//! modifications to a series). All operations that move/copy/delete series
//! files or update DB paths must acquire this lock first.
//!
//! Usage: check `try_lock_series`, and **always** pair a successful lock with
//! `unlock_series`, even on failure. Read-only callers use `is_series_locked`
//! and defer/skip rather than block.

use crate::api::AppState;
use std::sync::Arc;

/// Acquire the modification lock for a series. Returns true if the lock was
/// acquired (series was not already locked), false if another operation is
/// already modifying it.
///
/// The caller MUST pair this with a call to `unlock_series` when done.
pub async fn try_lock_series(state: &Arc<AppState>, series_id: &str) -> bool {
    state
        .modifying_series
        .write()
        .await
        .insert(series_id.to_string())
}

/// Release the modification lock for a series.
/// Safe to call even if the series is not currently locked (no-op).
pub async fn unlock_series(state: &Arc<AppState>, series_id: &str) {
    state.modifying_series.write().await.remove(series_id);
}

/// Check whether a series is currently locked without acquiring the lock.
///
/// Used by operations that should defer or skip (e.g., organize-from-download
/// will requeue, media scans will skip this series' files).
pub fn is_series_locked(state: &Arc<AppState>, series_id: &str) -> bool {
    // `try_read` avoids blocking; if the lock is contended we return true (a
    // safe false positive) rather than wait.
    state
        .modifying_series
        .try_read()
        .map(|guard| guard.contains(series_id))
        .unwrap_or(true) // conservative if we can't read
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    type LockSet = Arc<RwLock<HashSet<String>>>;

    fn new_set() -> LockSet {
        Arc::new(RwLock::new(HashSet::new()))
    }

    async fn try_lock(set: &LockSet, id: &str) -> bool {
        set.write().await.insert(id.to_string())
    }

    async fn unlock(set: &LockSet, id: &str) {
        set.write().await.remove(id);
    }

    fn is_locked(set: &LockSet, id: &str) -> bool {
        set.try_read().map(|g| g.contains(id)).unwrap_or(true)
    }

    #[tokio::test]
    async fn test_lock_unlocked_returns_true() {
        let set = new_set();
        assert!(try_lock(&set, "s1").await);
    }

    #[tokio::test]
    async fn test_lock_locked_returns_false() {
        let set = new_set();
        try_lock(&set, "s1").await;
        assert!(!try_lock(&set, "s1").await);
    }

    #[tokio::test]
    async fn test_unlock_releases() {
        let set = new_set();
        try_lock(&set, "s1").await;
        assert!(is_locked(&set, "s1"));
        unlock(&set, "s1").await;
        assert!(!is_locked(&set, "s1"));
    }

    #[tokio::test]
    async fn test_independent_series() {
        let set = new_set();
        try_lock(&set, "a").await;
        try_lock(&set, "b").await;
        assert!(is_locked(&set, "a"));
        assert!(is_locked(&set, "b"));
        unlock(&set, "a").await;
        assert!(!is_locked(&set, "a"));
        assert!(is_locked(&set, "b"));
    }

    #[tokio::test]
    async fn test_double_unlock_noop() {
        let set = new_set();
        try_lock(&set, "s1").await;
        unlock(&set, "s1").await;
        unlock(&set, "s1").await;
        assert!(!is_locked(&set, "s1"));
    }

    #[tokio::test]
    async fn test_relock_after_unlock() {
        let set = new_set();
        try_lock(&set, "s1").await;
        unlock(&set, "s1").await;
        assert!(try_lock(&set, "s1").await);
    }

    #[tokio::test]
    async fn test_is_locked_false_for_unknown() {
        let set = new_set();
        assert!(!is_locked(&set, "unknown"));
    }
}
