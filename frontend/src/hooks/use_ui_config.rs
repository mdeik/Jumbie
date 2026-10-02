use crate::api::save_ui_preferences;
use jumbie_shared::config::{TimeFormat, UIConfig};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[derive(Clone, Copy)]
pub struct UIConfigContext {
    pub ui_config: ReadSignal<Option<UIConfig>>,
    pub set_ui_config: WriteSignal<Option<UIConfig>>,
}

/// Provides the UI Configuration context at the root of the application.
///
/// On creation, populates the signal synchronously from the API cache (written by
/// the preloader) so components reading it on first render get the cached value
/// instantly — no loading flash. A standalone background fetch also fires
/// immediately for the latest `ui_preferences` (including persisted sort state), so
/// tables render with the correct sort from the start; this is separate from the
/// bootstrap (which has a 500 ms+ delay) to avoid a flash of default sort.
///
/// The auth gate is intentionally absent: the API returns 401 on the login screen,
/// which the `if let Ok(fresh)` guard handles.
///
/// Returns `set_ui_config` so the caller (e.g. `app.rs`) can push updates
/// imperatively, though components normally write via `update_ui_config`,
/// `use_filter_override`, or `use_persistent_table_state`.
pub fn provide_ui_config_context() -> WriteSignal<Option<UIConfig>> {
    let (ui_config, set_ui_config) = signal(None);

    // Instant render from cache. The preloader writes this key after fetching the
    // bootstrap; on a fresh page load the cache is empty (WASM was just loaded), but
    // on subsequent navigations within the same session it is available immediately.
    //
    // Request construction does NOT depend on this being populated — components omit
    // sort/filter params until the user overrides them and the backend applies the
    // stored preference (see `resolve_table_sort`).
    let cached = crate::utils::read_cache::<UIConfig>("fetch_ui_preferences");
    if let Some(ui) = cached {
        set_ui_config.set(Some(ui));
    }

    // Background refresh (immediate, no auth gate): fetches the latest
    // ui_preferences independently so persisted sort state arrives before the
    // bootstrap (which is delayed 500 ms+). On the login screen this 401s, ignored.
    if !crate::utils::is_cache_fresh("fetch_ui_preferences") {
        spawn_local(async move {
            if let Ok(fresh) = crate::api::fetch_ui_preferences().await {
                crate::utils::write_cache("fetch_ui_preferences", &fresh);
                set_ui_config.set(Some(fresh));
            }
        });
    }

    provide_context(UIConfigContext {
        ui_config,
        set_ui_config,
    });

    set_ui_config
}

/// Access the global UI configuration state.
pub fn use_ui_config() -> UIConfigContext {
    use_context::<UIConfigContext>().expect("UIConfigContext should be provided in App")
}

/// Reactive 12h/24h display preference — the single source of truth for the
/// time-format preference and its fallback. Components must use this instead of
/// re-deriving `ui_config.time_format` or hard-coding a default. Falls back to
/// [`TimeFormat::Hour12`] before `UIConfig` has loaded.
pub fn use_time_format() -> Signal<TimeFormat> {
    let UIConfigContext { ui_config, .. } = use_ui_config();
    Signal::derive(move || {
        ui_config
            .get()
            .map(|u| u.time_format)
            .unwrap_or(TimeFormat::Hour12)
    })
}

/// Persist the current UI configuration to the backend and update the cache.
pub async fn persist_ui_config(ui: UIConfig) {
    if save_ui_preferences(&ui).await.is_ok() {
        crate::utils::write_cache("fetch_ui_preferences", &ui);
    }
}

/// Mutate a UI preference and persist it — the SSoT for "change a setting and save it".
///
/// Updates the context signal synchronously (so the fetcher and any derived signal
/// observe the new value in the same tick) and spawns the async save.
///
/// When the background load hasn't populated the context yet, the mutation is skipped
/// rather than persisting a partial/default config, which would clobber unrelated
/// stored preferences. Callers keep their own local override, so the current session
/// stays correct either way; only persistence is deferred to the next change.
pub fn update_ui_config(ctx: &UIConfigContext, mutate: impl FnOnce(&mut UIConfig)) {
    let Some(mut updated) = ctx.ui_config.get_untracked() else {
        return;
    };
    mutate(&mut updated);
    ctx.set_ui_config.set(Some(updated.clone()));
    spawn_local(async move {
        persist_ui_config(updated).await;
    });
}
