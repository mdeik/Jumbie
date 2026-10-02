// use_filter_override — persisted, optional filter (SSoT for filter overrides).
//
// Each paginated list has filters (log min-level, activity types, …) that also have
// a stored default on the backend. The pattern is always: a LOCAL `Option<T>` override
// (because the pagination fetcher reads it via `get_untracked()` right after a change,
// when a signal derived from `ui_config` would not yet have updated); a DISPLAY value
// (override → stored preference → literal default); and a guarded SETTER that stores
// the override and persists it as the new server-side default.
//
// The override stays `None` until the user changes something: the request then omits
// the parameter and the backend applies the stored preference (see `resolve_*` in
// `backend/src/api_routes/system/mod.rs`), so the first request after a hard refresh
// is correct without a second round-trip.

use crate::hooks::use_ui_config::{update_ui_config, use_ui_config};
use jumbie_shared::config::UIConfig;
use leptos::prelude::*;

/// Reactive handle for one persisted filter override.
pub struct FilterOverride<T>
where
    T: Send + Sync + 'static,
{
    /// Explicit override.  `None` = "defer to the stored preference" (the param
    /// is omitted from the request).  Feed straight to the fetcher / query key.
    pub state: ReadSignal<Option<T>>,
    /// Display value for the UI control: override → stored preference → default.
    pub display: Signal<T>,
    /// Commit a value: no-op if unchanged, otherwise stores the override and
    /// persists it as the new server-side default.
    pub set: Callback<T>,
}

/// Build a [`FilterOverride`].
///
/// * `stored` — reads the persisted value out of `UIConfig` (the fallback used
///   when there is no override).
/// * `default` — literal fallback when neither an override nor a stored value
///   exists.
/// * `persist` — writes the committed value back into `UIConfig` before it is
///   saved.
pub fn use_filter_override<T, S, P>(stored: S, default: T, persist: P) -> FilterOverride<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
    S: Fn(&UIConfig) -> T + Send + Sync + 'static,
    P: Fn(&mut UIConfig, T) + Send + Sync + 'static,
{
    let ctx = use_ui_config();
    let (state, set_state) = signal::<Option<T>>(None);

    // Display: override wins; otherwise the stored preference; otherwise default.
    let display = Signal::derive(move || {
        state
            .get()
            .or_else(|| ctx.ui_config.get().map(|ui| stored(&ui)))
            .unwrap_or_else(|| default.clone())
    });

    let set = Callback::new(move |value: T| {
        // No-op if unchanged — re-notifying the pagination effect (whose derived
        // query key has no equality check) would fire a redundant request.
        if state.get_untracked().as_ref() == Some(&value) {
            return;
        }
        set_state.set(Some(value.clone()));
        // Not `move` — the inner closure must borrow `persist` (captured by the
        // outer `Fn` closure), not move it out.
        update_ui_config(&ctx, |ui| persist(ui, value));
    });

    FilterOverride {
        state,
        display,
        set,
    }
}
