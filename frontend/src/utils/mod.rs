pub mod auth;
pub mod cache;
pub mod calendar;
pub mod episode_search;
pub mod episode_state;
pub mod operation_ledger;
pub mod plugins;
pub mod preload;

pub mod debug;
pub mod indicators;
pub mod pagination_query;
pub mod release_date;
pub mod sorting;

#[cfg(test)]
mod tests;

// Re-exports so existing `crate::utils::*` imports continue to work
pub use auth::*;
pub use cache::*;
pub use calendar::*;
pub use episode_search::resolve_manual_search_query;
pub use pagination_query::ListQueryParams;
pub use plugins::*;
pub use preload::*;

// Remaining imports
use crate::api_client::ApiError;
use crate::components::common::form_fields::SelectOption;
use crate::components::common::toast::{NotificationType, show_error, show_success, show_toast};
use chrono::{DateTime, Duration, Local, NaiveDateTime, Utc};
use jumbie_shared::config::{Config, TimeFormat};
use jumbie_shared::types::MonitorMode;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use wasm_bindgen::JsValue;

/// Update a config field and sync the API cache in one call. Every settings page
/// that writes to the API config should use this instead of manually repeating
/// `set_config.update(|c| { ...; write_cache("fetch_config", c); })`.
pub fn write_config_field(set_config: &WriteSignal<Option<Config>>, f: impl FnOnce(&mut Config)) {
    set_config.update(|c| {
        if let Some(c) = c {
            f(c);
            write_cache("fetch_config", c);
        }
    });
}

// Auto-save with debouncing

// Creates a debounced auto-save mechanism for the settings Config.
//
// A token counter is used instead of clearing + rescheduling a timer: each pending
// save captures the token value when scheduled and checks, before executing, whether
// the token has advanced (meaning a newer save superseded it). This guarantees
// conflicting saves never happen even under high-frequency updates or races between
// the effect and the manual trigger_save_immediate path.
pub fn use_autosave<F, Fut>(config: ReadSignal<Option<Config>>, save_fn: F) -> impl Fn(bool) + Clone
where
    F: Fn(Config) -> Fut + Clone + 'static,
    Fut: std::future::Future<Output = Result<(), ApiError>> + 'static,
{
    let last_saved = StoredValue::new_local(Option::<Value>::None);
    // Counter; when it changes, pending ops holding an old token are invalid.
    let save_token = StoredValue::new_local(0usize);

    // Initialize last_saved when config loads for the first time
    Effect::new(move |_| {
        if let Some(c) = config.get() {
            last_saved.update_value(|l| {
                if l.is_none()
                    && let Ok(v) = serde_json::to_value(&c)
                {
                    *l = Some(v);
                }
            });
        }
    });

    let save_action = Action::new_local(move |(c, show_toast): &(Config, bool)| {
        let c_clone = c.clone();
        let save_fn = save_fn.clone();
        let show_toast = *show_toast;

        async move {
            match save_fn(c_clone.clone()).await {
                Ok(_) => {
                    if let Ok(v) = serde_json::to_value(&c_clone) {
                        *last_saved.write_value() = Some(v);
                    }
                    // Keep the API cache in sync so re-mounting any settings page
                    // shows the latest values immediately instead of stale data.
                    write_cache("fetch_config", &c_clone);
                    if show_toast {
                        show_success("Settings saved successfully!");
                    }
                }
                Err(e) => {
                    if show_toast {
                        show_error(format!("Failed to save settings: {}", e));
                    } else {
                        crate::debug_error!("Auto-save failed: {}", e);
                    }
                }
            }
        }
    });

    Effect::new(move |_| {
        if let Some(current) = config.get() {
            // Check if dirty
            let is_dirty = last_saved.with_value(|l| {
                if let Some(last) = l {
                    if let Ok(curr_val) = serde_json::to_value(&current) {
                        last != &curr_val
                    } else {
                        true
                    }
                } else {
                    false
                }
            });

            if is_dirty {
                // Increment token to invalidate any previous pending timers
                save_token.update_value(|t| *t += 1);
                let my_token = *save_token.read_value();
                let current_for_timer = current.clone();

                set_timeout(
                    move || {
                        // Only save if token matches (i.e. no new changes or manual saves happened)
                        if *save_token.read_value() == my_token {
                            // Debounced saves are silent to prevent toasts while typing
                            save_action.dispatch((current_for_timer, false));
                        }
                    },
                    std::time::Duration::from_millis(500),
                );
            }
        }
    });

    move |show_toast: bool| {
        if let Some(current) = config.get() {
            let is_dirty = last_saved.with_value(|l| {
                if let Some(last) = l {
                    if let Ok(curr_val) = serde_json::to_value(&current) {
                        last != &curr_val
                    } else {
                        false
                    }
                } else {
                    false
                }
            });

            if is_dirty {
                // Invalidate pending timers by incrementing token
                save_token.update_value(|t| *t += 1);
                save_action.dispatch((current, show_toast));
            }
        }
    }
}

// API action wrappers with toast feedback

// Wraps an async operation in a Leptos Action with built-in toast feedback,
// eliminating the repetitive match-on-Result + show_error pattern at each call site.
pub fn create_api_action<I, O, F, Fut>(
    action_fn: F,
    success_message: Option<&'static str>,
) -> Action<I, Option<O>>
where
    I: Clone + 'static,
    O: Clone + 'static,
    F: Fn(&I) -> Fut + Clone + 'static,
    Fut: std::future::Future<Output = Result<O, ApiError>> + 'static,
{
    Action::new_local(move |input: &I| {
        let input = input.clone();
        let action_fn_clone = action_fn.clone();

        async move {
            match action_fn_clone(&input).await {
                Ok(res) => {
                    if let Some(msg) = success_message {
                        show_success(msg);
                    }
                    Some(res)
                }
                Err(e) => {
                    match e {
                        ApiError::Api { body, .. } => show_error(&body),
                        _ => show_error(e.to_string()),
                    }
                    None
                }
            }
        }
    })
}

// Fires a fire-and-forget async operation with optional toast feedback — for one-shot
// side effects where the caller doesn't need to track loading state (pre-save hooks,
// background cleanup). Unlike create_api_action, it returns no Action.
pub fn spawn_api_toast<Fut, O, F>(future: Fut, success_message: Option<&'static str>, on_success: F)
where
    Fut: std::future::Future<Output = Result<O, ApiError>> + 'static,
    F: FnOnce(O) + 'static,
{
    spawn_api_toast_with_level(future, success_message, on_success, NotificationType::Error);
}

/// Like [`spawn_api_toast`] but allows controlling the notification level for errors.
pub fn spawn_api_toast_with_level<Fut, O, F>(
    future: Fut,
    success_message: Option<&'static str>,
    on_success: F,
    error_level: NotificationType,
) where
    Fut: std::future::Future<Output = Result<O, ApiError>> + 'static,
    F: FnOnce(O) + 'static,
{
    spawn_local(async move {
        match future.await {
            Ok(res) => {
                if let Some(msg) = success_message {
                    show_success(msg);
                }
                on_success(res);
            }
            Err(e) => match &e {
                ApiError::Api { .. } => show_toast(e.user_message(), error_level),
                _ => show_toast(e.to_string(), error_level),
            },
        }
    });
}

// Resource helpers

/// Creates a resource that loads once on component mount
pub fn create_mount_resource<T, Fu>(fetcher: impl Fn() -> Fu + 'static) -> LocalResource<T>
where
    T: 'static,
    Fu: std::future::Future<Output = T> + 'static,
{
    LocalResource::new(move || fetcher())
}

/// Creates a resource with automatic error handling and toast notifications
pub fn create_api_resource<T, E, Fu>(
    fetcher: impl Fn() -> Fu + Clone + 'static,
    error_message: &'static str,
) -> LocalResource<Option<T>>
where
    T: 'static,
    E: std::fmt::Display + 'static,
    Fu: std::future::Future<Output = Result<T, E>> + 'static,
{
    LocalResource::new(move || {
        let fetch = fetcher.clone();
        async move {
            match fetch().await {
                Ok(data) => Some(data),
                Err(e) => {
                    show_error(format!("{}: {}", error_message, e));
                    None
                }
            }
        }
    })
}

// Formatting helpers

pub fn format_size(bytes: u64) -> String {
    const KILOBYTE: u64 = 1024;
    const MEGABYTE: u64 = KILOBYTE * 1024;
    const GIGABYTE: u64 = MEGABYTE * 1024;
    const TERABYTE: u64 = GIGABYTE * 1024;

    if bytes < KILOBYTE {
        format!("{} B", bytes)
    } else if bytes < MEGABYTE {
        format!("{:.2} KB", bytes as f64 / KILOBYTE as f64)
    } else if bytes < GIGABYTE {
        format!("{:.2} MB", bytes as f64 / MEGABYTE as f64)
    } else if bytes < TERABYTE {
        format!("{:.2} GB", bytes as f64 / GIGABYTE as f64)
    } else {
        format!("{:.2} TB", bytes as f64 / TERABYTE as f64)
    }
}

/// Build a sorted list of [SelectOption] from profile name tuples: the
/// `placeholder` first (value=""), then the profiles sorted by label. When
/// `clear_option` is true, a final option with value="__clear__", label="None" is
/// appended (for bulk-edit use, where the user must explicitly clear).
///
/// SSoT for building profile-select options from an unsorted map of (id, name)
/// pairs — used by `derive_quality_options`, `derive_release_options`, and
/// `series_library.rs` directly.
pub fn build_sorted_profile_options(
    profiles: &[(String, String)],
    placeholder: &str,
    clear_option: bool,
) -> Vec<crate::components::common::form_fields::SelectOption> {
    let mut opts = vec![SelectOption::from((
        "".to_string(),
        placeholder.to_string(),
    ))];
    let mut sorted: Vec<&(String, String)> = profiles.iter().collect();
    sorted.sort_by(|a, b| a.1.cmp(&b.1));
    for (id, name) in sorted {
        opts.push(SelectOption::from((id.clone(), name.clone())));
    }
    if clear_option {
        opts.push(SelectOption::from((
            "__clear__".to_string(),
            "None".to_string(),
        )));
    }
    opts
}

/// Parse a monitor-mode value string (from a `<Select>` dropdown) into a `MonitorMode`.
///
/// Returns `None` for the empty string (used in bulk-edit to signal "no change") and
/// for unknown strings, so this pairs safely with `.unwrap_or(MonitorMode::None)`.
pub fn parse_monitor_mode(s: &str) -> Option<MonitorMode> {
    match s {
        "All" => Some(MonitorMode::All),
        "Future" => Some(MonitorMode::Future),
        "Missing" => Some(MonitorMode::Missing),
        "Existing" => Some(MonitorMode::Existing),
        "Pilot" => Some(MonitorMode::Pilot),
        "FirstSeason" => Some(MonitorMode::FirstSeason),
        "Specials" => Some(MonitorMode::Specials),
        "None" => Some(MonitorMode::None),
        _ => None,
    }
}

/// Derive quality profile select options from a reactive signal map. Shared by
/// `add_series.rs` and `edit_series.rs`.
pub fn derive_quality_options(
    quality_profiles: leptos::prelude::Signal<
        std::collections::HashMap<String, jumbie_shared::types::QualityProfile>,
    >,
) -> leptos::prelude::Signal<Vec<crate::components::common::form_fields::SelectOption>> {
    leptos::prelude::Signal::derive(move || {
        let profiles: Vec<(String, String)> = quality_profiles
            .get()
            .iter()
            .map(|(id, p)| (id.clone(), p.name.clone()))
            .collect();
        build_sorted_profile_options(&profiles, "None", false)
    })
}

/// Derive release profile select options from a reactive signal map. Shared by
/// `add_series.rs` and `edit_series.rs`.
pub fn derive_release_options(
    release_profiles: leptos::prelude::Signal<
        std::collections::HashMap<String, jumbie_shared::types::ReleaseProfile>,
    >,
) -> leptos::prelude::Signal<Vec<crate::components::common::form_fields::SelectOption>> {
    leptos::prelude::Signal::derive(move || {
        let profiles: Vec<(String, String)> = release_profiles
            .get()
            .iter()
            .map(|(id, p)| (id.clone(), p.name.clone()))
            .collect();
        build_sorted_profile_options(&profiles, "None", false)
    })
}

/// Resolve a quality profile UUID to its display name, falling back to the UUID if
/// not found. Used by `series_library.rs` table cells and cards, mirroring the
/// resolution `Select` options provide in `edit_series/general_tab.rs`.
pub fn resolve_quality_profile_name(
    uuid: &str,
    profiles: &std::collections::HashMap<String, jumbie_shared::types::QualityProfile>,
) -> String {
    profiles
        .get(uuid)
        .map(|p| p.name.as_str())
        .unwrap_or(uuid)
        .to_string()
}

/// Resolve a release profile UUID to its display name, falling back to the UUID if
/// not found. Used by `series_library.rs` table cells and cards.
pub fn resolve_release_profile_name(
    uuid: &str,
    profiles: &std::collections::HashMap<String, jumbie_shared::types::ReleaseProfile>,
) -> String {
    profiles
        .get(uuid)
        .map(|p| p.name.as_str())
        .unwrap_or(uuid)
        .to_string()
}

/// Resolve known template variables in a path or text string using the series title
/// (currently `${series}`).
///
/// Delegates to `jumbie_shared::paths::resolve_template` — the SSoT shared with the
/// backend — so the value is sanitized per the org illegal-char policy and the
/// SERVER's OS, matching the folder that actually exists on disk.
pub fn resolve_path_template(
    text: &str,
    series_title: &str,
    org: &jumbie_shared::config::organization::OrganizationConfig,
    os: jumbie_shared::patterns::PlatformOs,
) -> String {
    jumbie_shared::paths::resolve_template(text, series_title, org, os)
        .to_string_lossy()
        .to_string()
}

/// Check whether an entity (episode or season) can be "matched" to its metadata provider.
///
/// `active_plugins` MUST be pre-filtered to metadata-only providers, in priority order
/// (use `EditSeriesCtx.active_metadata_plugins`, which preserves the backend's order).
///
/// Matchability is defined against the **current active provider** — the
/// highest-priority active metadata provider. A cache entry belonging to a non-active
/// provider does NOT make the entity matchable.
///
/// Returns `true` when: `metadata_source` is `"custom"` or `"cleared"` (not currently
/// using live provider data); an active metadata provider is configured; the entity
/// has cached metadata in `metadata_ids` for that provider; and (season-level only)
/// `season_cache_exists` confirms cached data.
///
/// Pass `season_cache_exists: true` for episode-level checks; for season-level checks,
/// compute it from `metadata_seasons` before calling.
///
/// SSoT for match-button visibility across `EpisodeDetailsModal` and
/// `SeasonAccordionList`. For the series-level "Match All" button in `EpisodesTab`,
/// use the `season_mismatch` memo instead — it diffs `metadata_seasons` against
/// loaded seasons, which is O(seasons) rather than O(episodes).
pub fn has_matchable_metadata(
    metadata_source: Option<&str>,
    metadata_ids: &std::collections::HashMap<String, String>,
    active_plugins: &[jumbie_shared::plugin::PluginInstanceInfo],
    season_cache_exists: bool,
) -> bool {
    // 1. Source guard — only custom/cleared metadata can be re-matched
    if !matches!(metadata_source, Some("custom" | "cleared")) {
        return false;
    }

    // 2. Season-level cache gate (always passes for episode-level callers)
    if !season_cache_exists {
        return false;
    }

    // 3. Current active provider = highest-priority active metadata provider.
    let Some(instance_id) = active_plugins
        .first()
        .and_then(|p| p.instance_id.as_deref())
        .filter(|pid| !pid.is_empty())
    else {
        return false;
    };

    // 4. Cache check against the ACTIVE provider's id only.
    metadata_ids
        .get(instance_id)
        .filter(|mid| !mid.is_empty())
        .is_some()
}

/// A season-level discrepancy between provider metadata and loaded episodes.
#[derive(Clone, PartialEq, Debug)]
pub struct SeasonMismatch {
    /// Provider seasons with no loaded episodes: `(season label, provider episode count)`.
    pub missing: Vec<(String, i32)>,
    /// Season overrides the provider does not have.
    pub extra_overrides: Vec<String>,
}

/// Compute the season mismatch that drives the "Match Seasons" affordance.
///
/// SSoT for the comparison in `EpisodesTab`: both inputs are pre-derived by the caller
/// so this stays pure and testable.
///
/// Suppressed (deliberately deleted) seasons are intentionally INCLUDED in `missing`:
/// "Match Seasons" is the intended way to restore a deleted season (it re-adds the
/// season cell and rehydrates from the provider cache), so a deleted-but-cached season
/// must surface here. Callers must NOT filter suppressed seasons out.
pub fn season_mismatch(
    metadata_seasons: &[jumbie_shared::types::MetadataSeasonInfo],
    loaded_seasons: &std::collections::HashSet<String>,
    override_seasons: &[String],
) -> Option<SeasonMismatch> {
    if metadata_seasons.is_empty() {
        return None;
    }

    let provider_seasons: std::collections::HashSet<String> = metadata_seasons
        .iter()
        .map(|t| t.season_number.to_string())
        .collect();

    let missing: Vec<(String, i32)> = metadata_seasons
        .iter()
        .map(|t| (t.season_number.to_string(), t.episode_count))
        .filter(|(season, _)| !loaded_seasons.contains(season))
        .collect();

    let extra_overrides: Vec<String> = override_seasons
        .iter()
        .filter(|season| !provider_seasons.contains(*season))
        .cloned()
        .collect();

    if missing.is_empty() && extra_overrides.is_empty() {
        None
    } else {
        Some(SeasonMismatch {
            missing,
            extra_overrides,
        })
    }
}

/// SSoT — parse a timestamp emitted by the API into a UTC instant.
///
/// Delegates to [`jumbie_shared::datetime::parse_utc`] — the same lenient parser the
/// backend uses — so both sides accept exactly the same inputs: RFC 3339 with any
/// offset, naive `"YYYY-MM-DD HH:MM:SS"` / `"YYYY-MM-DDTHH:MM:SS"` (treated as UTC,
/// since the database stores UTC), and date-only `"YYYY-MM-DD"` (anchored at midnight
/// UTC). Fractional seconds are discarded (display is minute-precision).
///
/// Returns `None` for empty/unparseable input; callers fall back to a raw rendering
/// via [`unparseable_display`].
pub fn parse_timestamp_utc(ts: &str) -> Option<DateTime<Utc>> {
    jumbie_shared::datetime::parse_utc(ts)
        .ok()
        .map(|dt| dt.to_chrono_utc())
}

/// SSoT — format a timestamp for display in the browser's local timezone.
/// Accepts RFC 3339 and naive-UTC inputs (see [`parse_timestamp_utc`]).
/// Time portion uses 12hr or 24hr based on `time_format`.
pub fn format_datetime_local(ts: &str, time_format: &TimeFormat) -> String {
    match parse_timestamp_utc(ts) {
        Some(dt) => {
            let local_dt = dt.with_timezone(&Local);
            match time_format {
                TimeFormat::Hour12 => local_dt.format("%m-%d-%y %I:%M %p").to_string(),
                TimeFormat::Hour24 => local_dt.format("%m-%d-%y %H:%M").to_string(),
            }
        }
        None => unparseable_display(ts),
    }
}

/// SSoT — format a local time-of-day honoring the user's 12hr/24hr preference.
///
/// Takes an already-local `NaiveDateTime` (e.g. the `.with_timezone(&Local).naive_local()`
/// conversion) so callers holding a local instant don't round-trip through a string.
/// Used by the calendar, which would otherwise hardcode 24-hour time.
pub fn format_time_local(dt: &NaiveDateTime, time_format: &TimeFormat) -> String {
    match time_format {
        TimeFormat::Hour12 => dt.format("%I:%M %p").to_string(),
        TimeFormat::Hour24 => dt.format("%H:%M").to_string(),
    }
}

/// Fallback rendering for a value [`parse_timestamp_utc`] rejected: strip the
/// zone markers so a raw timestamp is still readable. SSoT for the `None` arm of
/// [`format_datetime_local`] and [`format_age`].
fn unparseable_display(ts: &str) -> String {
    ts.replace("T", " ").replace("Z", "")
}

/// Format a `chrono::Duration` as a relative age string (e.g. "3h ago",
/// "5d ago", "30+ days"). Handles future dates, sub-hour, hours, days, and the
/// 30+ cap. Shared by [`format_age`] and other age displays.
fn format_duration_age(duration: Duration) -> String {
    let hours = duration.num_hours();
    let days = duration.num_days();

    if hours < 0 {
        "Not yet released".to_string()
    } else if days < 1 {
        if hours < 1 {
            "Just now".to_string()
        } else {
            format!("{}h ago", hours)
        }
    } else if days > 30 {
        "30+ days".to_string()
    } else {
        format!("{}d ago", days)
    }
}

/// SSoT — format a timestamp as a relative age string (e.g. "3h ago", "5d ago",
/// "30+ days"). Accepts RFC 3339 and naive-UTC inputs (see [`parse_timestamp_utc`]).
/// Used by the activity list and the wanted page.
pub fn format_age(ts: &str) -> String {
    match parse_timestamp_utc(ts) {
        Some(dt) => format_duration_age(Utc::now().signed_duration_since(dt)),
        None => unparseable_display(ts),
    }
}

/// Build a calendar iCal link URL from an origin and a token. SSoT for iCal URL
/// construction, used by both `calendar_link_modal.rs` and `api_settings.rs`.
pub fn build_calendar_link_url(origin: &str, token: &str) -> String {
    format!("{}/api/calendar/ical?token={}", origin, token)
}

/// Attempt to write `text` to the system clipboard via the Clipboard API.
///
/// The Clipboard API requires a secure context (HTTPS or localhost); over plain HTTP
/// (e.g. Docker) `navigator.clipboard` is undefined and this returns `false` without
/// crashing. Returns `true` if the API was available and the write was initiated.
pub fn write_clipboard(text: &str) -> bool {
    if let Some(window) = web_sys::window() {
        let navigator = window.navigator();
        let js_nav: &JsValue = navigator.as_ref();
        let has_clipboard = js_sys::Reflect::get(js_nav, &"clipboard".into())
            .map(|v| v.is_object())
            .unwrap_or(false);

        if has_clipboard {
            let _ = navigator.clipboard().write_text(text);
            return true;
        }
    }
    false
}

/// Select the entire text content of an HTML element, replacing the current browser
/// selection. Useful for "click to select" UX on code blocks and URLs.
/// SSoT for in-element text selection, called from `api_settings.rs`.
pub fn select_element_contents(el: &web_sys::HtmlElement) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document())
        && let Ok(range) = doc.create_range()
        && range.select_node_contents(el).is_ok()
        && let Ok(Some(selection)) = doc.get_selection()
    {
        let _ = selection.remove_all_ranges();
        let _ = selection.add_range(&range);
    }
}

/// Clear the current browser text selection — useful when toggling visible content
/// so a stale selection on now-hidden text doesn't persist. Pairs with
/// `select_element_contents`.
pub fn clear_selection() {
    if let Some(selection) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_selection().ok().flatten())
    {
        let _ = selection.remove_all_ranges();
    }
}
