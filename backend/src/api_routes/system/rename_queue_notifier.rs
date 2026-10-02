//! Rename Queue Notifier — gating logic for rename-queue notifications.
//!
//! Encapsulates:
//!
//!   • State tracking  — `last_fired_state` (series_id → affected count at last fire)
//!   • Cooldown        — minimum ms between notification firings
//!   • Startup seed    — first `evaluate` call silently initialises state so the
//!                       startup warm-cache never fires spurious notifications
//!   • Debounce        — `last_fired_state` is only updated when a notification
//!                       actually fires, so changes accumulated within the cooldown
//!                       window are captured on the next firing
//!
//! Thread safety: all state mutations happen inside a single `Mutex` lock scope, so
//! there is no check-then-act window. `cooldown_ms` is immutable after creation.
//!
//! SSoT: `RenameQueueNotifier` is the only code path that reads or writes
//! `last_fired_state` / `last_fired_at`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::AppState;
use crate::plugins::notifiers::{NotifierContext, NotifierEvent};

struct NotifierInner {
    /// Wall-clock milliseconds (since Unix epoch) of the last fired notification.
    last_fired_at: u64,
    /// Per-series (series_id → affected_episodes) at the moment we last fired.
    /// Updated ONLY when a notification actually fires.  Left unchanged during
    /// the cooldown window so that the same comparator detects all accumulated
    /// changes on the next poll.
    last_fired_state: HashMap<String, usize>,
}

/// Gating logic for rename-queue notifications.
///
/// Create one with [`new`](RenameQueueNotifier::new) and call
/// [`evaluate`](RenameQueueNotifier::evaluate) on every rename-queue poll.
pub struct RenameQueueNotifier {
    inner: Mutex<NotifierInner>,
    /// Whether [`evaluate`] has been called at least once.
    /// Until this is `true`, `evaluate` silently seeds state without firing.
    initialized: AtomicBool,
    /// Minimum milliseconds between notification firings.
    cooldown_ms: u64,
}

impl RenameQueueNotifier {
    /// Create a new notifier with the given cooldown.
    ///
    /// The notifier starts in an un-initialised state — the first call to
    /// [`evaluate`](RenameQueueNotifier::evaluate) will seed the internal
    /// state map without firing.
    pub fn new(cooldown_ms: u64) -> Self {
        Self {
            inner: Mutex::new(NotifierInner {
                last_fired_at: 0,
                last_fired_state: HashMap::new(),
            }),
            initialized: AtomicBool::new(false),
            cooldown_ms,
        }
    }

    /// Evaluate the current rename-queue state and fire a notification if the
    /// state changed since the last fire AND the cooldown has elapsed.
    ///
    /// Returns `true` when a notification was dispatched.
    ///
    /// On the first call the notifier silently records the current state without
    /// firing, so the startup warm-cache cannot spam notifications for
    /// pre-existing items.
    pub async fn evaluate(
        &self,
        items: &[jumbie_shared::types::RenameQueueItem],
        state: &Arc<AppState>,
    ) -> bool {
        let now_ms = now_ms();

        // Check state (single lock scope).
        let changed_titles: Option<Vec<(String, usize)>> = {
            let mut inner = self.inner.lock().unwrap();

            if !self.initialized.load(Ordering::Relaxed) {
                // First call: silently seed — no notification.
                inner.last_fired_state = build_state_map(items);
                self.initialized.store(true, Ordering::Relaxed);
                return false;
            }

            let changed = compute_changed(items, &inner.last_fired_state);
            if changed.is_empty() {
                return false; // nothing to notify about
            }

            if now_ms.saturating_sub(inner.last_fired_at) < self.cooldown_ms {
                // Cooldown NOT elapsed — leave `last_fired_state` untouched
                // so the same (and any additional) changes are re-detected
                // on the next poll.  This gives us natural debouncing.
                return false;
            }

            // Cooldown elapsed: record the state at fire time.
            inner.last_fired_at = now_ms;
            inner.last_fired_state = build_state_map(items);
            Some(changed)
        }; // <-- lock dropped here, race-free

        // Fire outside the lock.
        let Some(changed_titles) = changed_titles else {
            return false;
        };

        let auto_apply = state.cfg.read().await.organization.auto_apply_renames;
        if !auto_apply && let Some(ref notifications) = state.notifications {
            let notifier = notifications.read().await;
            let entries: Vec<(String, i32)> = changed_titles
                .iter()
                .map(|(t, c)| (t.clone(), *c as i32))
                .collect();
            if !entries.is_empty() {
                notifier
                    .notify(
                        NotifierEvent::RenameQueue,
                        &NotifierContext::new().with_rename_queue_entries(entries),
                    )
                    .await;
                return true;
            }
        }

        false
    }
}

/// Build a (series_id → affected_episodes) map from a slice of items.
fn build_state_map(items: &[jumbie_shared::types::RenameQueueItem]) -> HashMap<String, usize> {
    items
        .iter()
        .map(|i| (i.series_id.clone(), i.affected_episodes))
        .collect()
}

/// Compute which series have changed since `last_fired_state`.
///
/// Returns `(series_title, current_affected_episodes)` for every series whose
/// affected count differs from the last-fired record.  Series that are in
/// `last_fired_state` but absent from `items` (queue drained) are NOT included
/// because they no longer require user interaction.
fn compute_changed(
    items: &[jumbie_shared::types::RenameQueueItem],
    last_fired_state: &HashMap<String, usize>,
) -> Vec<(String, usize)> {
    items
        .iter()
        .filter_map(|item| {
            let prev = last_fired_state.get(&item.series_id).copied();
            // usize::MAX sentinel: items absent from the map are treated as "new"
            // (≠ any real count).
            if prev.unwrap_or(usize::MAX) != item.affected_episodes {
                Some((item.series_title.clone(), item.affected_episodes))
            } else {
                None
            }
        })
        .collect()
}

/// Milliseconds since Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(
        series_id: &str,
        title: &str,
        affected: usize,
    ) -> jumbie_shared::types::RenameQueueItem {
        jumbie_shared::types::RenameQueueItem {
            series_id: series_id.to_string(),
            series_title: title.to_string(),
            affected_episodes: affected,
            collision_count: 0,
            causes: vec![],
            absolute_numbering: false,
            has_failed: false,
            processing: false,
        }
    }

    #[test]
    fn compute_changed_new_series() {
        let items = vec![make_item("s1", "Series A", 5)];
        let state = HashMap::new();
        let changed = compute_changed(&items, &state);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0], ("Series A".to_string(), 5));
    }

    #[test]
    fn compute_changed_unchanged_series() {
        let items = vec![make_item("s1", "Series A", 5)];
        let mut state = HashMap::new();
        state.insert("s1".to_string(), 5);
        let changed = compute_changed(&items, &state);
        assert!(changed.is_empty());
    }

    #[test]
    fn compute_changed_series_updated() {
        let items = vec![make_item("s1", "Series A", 8)];
        let mut state = HashMap::new();
        state.insert("s1".to_string(), 5);
        let changed = compute_changed(&items, &state);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0], ("Series A".to_string(), 8));
    }

    #[test]
    fn compute_changed_drained_series_excluded() {
        // Series was in state but is no longer in items → ignored
        let items: Vec<jumbie_shared::types::RenameQueueItem> = vec![];
        let mut state = HashMap::new();
        state.insert("s1".to_string(), 5);
        let changed = compute_changed(&items, &state);
        assert!(changed.is_empty());
    }

    #[test]
    fn build_state_map_basic() {
        let items = vec![make_item("s1", "A", 3), make_item("s2", "B", 7)];
        let map = build_state_map(&items);
        assert_eq!(map.get("s1"), Some(&3));
        assert_eq!(map.get("s2"), Some(&7));
        assert_eq!(map.len(), 2);
    }
}
