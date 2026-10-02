// Per-tab ledger for background operations (batch move / reorganize).
//
// sessionStorage is scoped to the initiating tab but survives a reload, so a
// refresh of that tab can resume an in-flight operation without ever replaying
// a result it already showed. Other tabs and other devices do not share it, so
// they never render a toast for an operation they did not start.
//
//   * `jb_my_operations`        — id -> { label, started_at_ms }
//   * `jb_delivered_operations` — id -> delivered_at_ms

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MY_OPERATIONS_KEY: &str = "jb_my_operations";
const DELIVERED_OPERATIONS_KEY: &str = "jb_delivered_operations";

// Ownership records expire so an abandoned operation cannot leak entries.
const MY_OPERATIONS_TTL_MS: u64 = 24 * 60 * 60 * 1000;
// Backend retains finished operations for 30 min; keep delivery records longer.
const DELIVERED_TTL_MS: u64 = 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MyOperation {
    label: String,
    started_at_ms: u64,
}

/// Snapshot of the ledger passed into reconciliation.
#[derive(Debug, Clone, Default)]
pub struct OperationScope {
    /// Operations initiated by this tab: id -> display label.
    pub mine: HashMap<String, String>,
    /// Operations whose result has already been delivered in this tab.
    pub delivered: HashSet<String>,
}

fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.session_storage().ok().flatten())
}

fn load_json<T: serde::de::DeserializeOwned + Default>(key: &str) -> T {
    storage()
        .and_then(|s| s.get_item(key).ok().flatten())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn save_json<T: Serialize>(key: &str, value: &T) {
    if let Some(s) = storage()
        && let Ok(json) = serde_json::to_string(value)
    {
        let _ = s.set_item(key, &json);
    }
}

/// Load both maps with expired entries dropped.
fn load(now: u64) -> (HashMap<String, MyOperation>, HashMap<String, u64>) {
    let mut mine: HashMap<String, MyOperation> = load_json(MY_OPERATIONS_KEY);
    let mut delivered: HashMap<String, u64> = load_json(DELIVERED_OPERATIONS_KEY);
    mine.retain(|_, op| now.saturating_sub(op.started_at_ms) < MY_OPERATIONS_TTL_MS);
    delivered.retain(|_, at| now.saturating_sub(*at) < DELIVERED_TTL_MS);
    (mine, delivered)
}

fn save(mine: &HashMap<String, MyOperation>, delivered: &HashMap<String, u64>) {
    save_json(MY_OPERATIONS_KEY, mine);
    save_json(DELIVERED_OPERATIONS_KEY, delivered);
}

/// Record that this tab initiated `id`; `label` is reused for recovery toasts
/// so a reloaded tab shows the same wording.
pub fn register_operation(id: &str, label: &str) {
    let now = now_ms();
    let (mut mine, delivered) = load(now);
    mine.insert(
        id.to_string(),
        MyOperation {
            label: label.to_string(),
            started_at_ms: now,
        },
    );
    save(&mine, &delivered);
}

/// Record that `id`'s result has been delivered so it is never shown again.
pub fn mark_delivered(id: &str) {
    let now = now_ms();
    let (mine, mut delivered) = load(now);
    delivered.insert(id.to_string(), now);
    save(&mine, &delivered);
}

/// Load the ledger snapshot for reconciliation.
pub fn scope() -> OperationScope {
    let (mine, delivered) = load(now_ms());
    OperationScope {
        mine: mine.into_iter().map(|(id, op)| (id, op.label)).collect(),
        delivered: delivered.into_keys().collect(),
    }
}

/// Drop all ledger state (session teardown).
pub fn clear() {
    if let Some(s) = storage() {
        let _ = s.remove_item(MY_OPERATIONS_KEY);
        let _ = s.remove_item(DELIVERED_OPERATIONS_KEY);
    }
}
