// Shared reactive episode state — carries the episode_id of the most recent save so
// all observing components (calendar, wanted, edit_series) can react without each
// needing its own cache-invalidation + re-fetch logic.
//
// The server remains the source of truth for episode state; this context is a
// lightweight broadcast channel: the modal writes the saved episode_id here, and
// each component's Effect consumes it and refreshes its local view.
//
// This module is also the SSoT for OPENING the episode-details modal with fresh,
// series-scoped data: the payload derives from `fetch_series_details_{sid}` (never
// the per-episode endpoint), so one cache entry per series feeds every episode modal
// of that series, and every open force-revalidates it (bypassing the TTL).

use jumbie_shared::formatting::{LabelStyle, fmt_abs_episode, fmt_episode, fmt_season};
use jumbie_shared::types::{EpisodeViewModel, ReleaseDates, SeasonOverride, SeriesDetails};
use leptos::prelude::*;
use std::collections::HashMap;

/// Display label for the pseudo-season that holds every episode in absolute-numbering
/// mode. SSoT for the string so the grouping helper and the UI checks that special-case
/// it can never drift apart.
pub const ABSOLUTE_SEASON_LABEL: &str = "Absolute";

/// Group a series' episodes into `(season_label, episodes)` buckets, newest season
/// first (descending) — the order the Episodes tab renders top-to-bottom.
///
/// This is the SSoT for how the Episodes tab enumerates seasons: the expand/collapse-all
/// list and `SeasonAccordionList` both derive from it, so they cannot disagree about
/// which seasons exist or their order. Absolute mode collapses everything into one
/// [`ABSOLUTE_SEASON_LABEL`] bucket.
pub fn group_episodes_by_season(
    episodes: Vec<EpisodeViewModel>,
    absolute_numbering: bool,
) -> Vec<(String, Vec<EpisodeViewModel>)> {
    if absolute_numbering {
        vec![(ABSOLUTE_SEASON_LABEL.to_string(), episodes)]
    } else {
        episodes
            .into_iter()
            .fold(
                std::collections::BTreeMap::<String, Vec<EpisodeViewModel>>::new(),
                |mut acc, ep| {
                    acc.entry(ep.season.clone()).or_default().push(ep);
                    acc
                },
            )
            .into_iter()
            .rev()
            .collect()
    }
}

/// A pending save notification carrying the episode_id that was saved.
/// Written by `fire_on_override_saved`, consumed by observing components.
/// Carries `series_id` so consumers can refresh from the series-scoped source.
#[derive(Debug, Clone)]
pub struct PendingEpisodeSave {
    pub episode_id: String,
    pub series_id: String,
}

/// Shared context provided at the app root.
#[derive(Debug, Clone)]
pub struct EpisodeStateContext {
    /// When set, an episode was just saved. Observing components read this
    /// and set it back to `None` after reacting.
    pub pending_save: RwSignal<Option<PendingEpisodeSave>>,
}

/// Provide the `EpisodeStateContext` at the app root.
/// Call this once in `App()` before any route renders.
pub fn provide_episode_state() {
    let ctx = EpisodeStateContext {
        pending_save: RwSignal::new(None),
    };
    provide_context(ctx);
}

/// Access the shared episode state context.
/// Panics if `provide_episode_state()` was not called in an ancestor.
pub fn use_episode_state() -> EpisodeStateContext {
    expect_context::<EpisodeStateContext>()
}

/// Convenience helper — write a pending episode save into the context.
/// The caller must have already obtained the context via `use_episode_state()`
/// at component scope (where `use_context` works).  Use this inside callbacks
/// that may run outside a reactive owner.
pub fn notify_episode_saved(ctx: &EpisodeStateContext, episode_id: &str, series_id: &str) {
    ctx.pending_save.set(Some(PendingEpisodeSave {
        episode_id: episode_id.to_string(),
        series_id: series_id.to_string(),
    }));
}

// Episode details modal bindings.
// SSoT: every payload write into the modal's signals flows through these helpers. The
// calendar and wanted pages each build one `EpisodeModalBindings` and use
// `apply_episode_details_payload` both when OPENING the modal and when a save
// completes — so the open modal always reflects the freshest server state rather
// than its open-time snapshot.

/// The writable signals an `EpisodeDetailsModal` is bound to. Constructed once per
/// page (calendar / wanted) from the page's modal state. `Copy` because every field
/// is a Copy signal — keeping this Copy means the closures that capture it stay `Fn`.
#[derive(Clone, Copy)]
pub struct EpisodeModalBindings {
    pub set_series_id: WriteSignal<String>,
    pub set_series_title: WriteSignal<String>,
    pub season_overrides: RwSignal<Vec<SeasonOverride>>,
    pub set_all_episodes: WriteSignal<Vec<EpisodeViewModel>>,
    /// Effective numbering mode for the modal's series — drives the manual
    /// search default text (absolute mode emits no season marker).
    pub set_absolute_numbering: WriteSignal<bool>,
    /// Series-level search template (series → global for the active mode) that
    /// per-season overrides replace.
    pub set_series_search_format: WriteSignal<String>,
    pub series_metadata_ids: RwSignal<HashMap<String, String>>,
    pub set_episode: WriteSignal<Option<EpisodeViewModel>>,
}

/// Write every field of a fresh details payload into the modal bindings.
pub fn apply_episode_details_payload(
    bindings: &EpisodeModalBindings,
    payload: &EpisodeDetailsPayload,
) {
    bindings.set_series_id.set(payload.series_id.clone());
    bindings.set_series_title.set(payload.series_title.clone());
    bindings
        .season_overrides
        .set(payload.season_overrides.clone());
    bindings
        .set_all_episodes
        .set(payload.all_episodes.clone().unwrap_or_default());
    bindings
        .set_absolute_numbering
        .set(payload.absolute_numbering);
    bindings
        .set_series_search_format
        .set(payload.series_search_format.clone());
    bindings
        .series_metadata_ids
        .set(payload.series_metadata_ids.clone());
    bindings.set_episode.set(Some(payload.episode.clone()));
}

/// Clear every modal binding (used when no payload is available yet — e.g. the
/// wanted page's partial-episode fallback, or a calendar open with no cache).
pub fn reset_episode_modal_bindings(bindings: &EpisodeModalBindings) {
    bindings.set_series_id.set(String::new());
    bindings.set_series_title.set(String::new());
    bindings.season_overrides.set(Vec::new());
    bindings.set_all_episodes.set(Vec::new());
    bindings.set_absolute_numbering.set(false);
    bindings.set_series_search_format.set(String::new());
    bindings.series_metadata_ids.set(HashMap::new());
    bindings.set_episode.set(None);
}

// Series-scoped modal data (SSoT)

/// The episode-details modal's data contract — the full set of series-scoped values
/// the modal needs for one episode. Derived client-side from `SeriesDetails`
/// (see `payload_from_series_details`) and applied to the modal bindings; it is
/// frontend-internal (not a wire type).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EpisodeDetailsPayload {
    pub episode: EpisodeViewModel,
    pub series_id: String,
    pub series_title: String,
    pub season_overrides: Vec<SeasonOverride>,
    #[serde(default)]
    pub all_episodes: Option<Vec<EpisodeViewModel>>,
    /// Effective numbering mode (series tristate resolved against the global
    /// default). Kept so the modal can build its manual search default text.
    pub absolute_numbering: bool,
    /// Series-level search template (series → global for the active mode) that
    /// per-season overrides replace — carried so the calendar/wanted modal derives
    /// its search query from the same source as the edit-series page, without a
    /// separate fetch.
    pub series_search_format: String,
    /// Series-level metadata IDs (plugin UUID → metadata ID) — empty when the
    /// series has never been synced with a provider. Used by the episode modal
    /// to gate the "Fetch Metadata" button.
    #[serde(default)]
    pub series_metadata_ids: HashMap<String, String>,
}

/// Derive the modal's `EpisodeDetailsPayload` from fresh `SeriesDetails`:
/// season overrides picked for the effective numbering mode, and
/// `absolute_numbering` resolved against the global default.
/// Returns `None` when `episode_id` is not in the series' episode list.
pub fn payload_from_series_details(
    details: &SeriesDetails,
    episode_id: &str,
    global_absolute_default: bool,
    global_search_format: &str,
    global_search_format_absolute: &str,
) -> Option<EpisodeDetailsPayload> {
    let episode = details
        .episodes
        .iter()
        .find(|e| e.unique_id == episode_id)?
        .clone();
    let settings = &details.config.settings;
    let absolute_numbering = settings
        .absolute_numbering
        .unwrap_or(global_absolute_default);
    let overrides = if absolute_numbering {
        &settings.season_absolute
    } else {
        &settings.season
    };
    let global_format = if absolute_numbering {
        global_search_format_absolute
    } else {
        global_search_format
    };
    let series_search_format = settings
        .effective_search_format(global_format, absolute_numbering)
        .to_string();
    Some(EpisodeDetailsPayload {
        episode,
        series_id: details.config.series_id.clone(),
        series_title: details.config.target_title.clone(),
        season_overrides: overrides.values().cloned().collect(),
        all_episodes: Some(details.episodes.clone()),
        absolute_numbering,
        series_search_format,
        series_metadata_ids: settings.metadata_ids.clone(),
    })
}

/// SSoT: open the episode-details modal with fresh, series-scoped data.
///
/// Phase 1 — instant render from the `fetch_series_details_{sid}` cache when present.
/// Phase 2 — force-revalidate via `refresh_cached_with` (bypasses the TTL) and
/// re-apply, so the modal never shows a TTL-fresh-but-stale status.
///
/// `set_show` is flipped true once a payload is available, so callers that seed a
/// partial episode may pre-open the modal (wanted) while callers without a seed wait
/// for data (calendar).
pub fn open_episode_details_modal(
    bindings: &EpisodeModalBindings,
    series_id: &str,
    episode_id: &str,
    global_absolute_default: bool,
    global_search_format: &str,
    global_search_format_absolute: &str,
    set_show: WriteSignal<bool>,
) {
    let ck = format!("fetch_series_details_{}", series_id);

    // Phase 1: instant render from cache.
    if let Some(details) = crate::utils::read_cache::<SeriesDetails>(&ck)
        && let Some(payload) = payload_from_series_details(
            &details,
            episode_id,
            global_absolute_default,
            global_search_format,
            global_search_format_absolute,
        )
    {
        apply_episode_details_payload(bindings, &payload);
        set_show.set(true);
    }

    // Phase 2: always revalidate on open — a fresh-but-stale entry must not suppress
    // the fetch (the original bug: the modal showed "missing" for queued episodes
    // until the 5-minute TTL expired).
    let sid_owned = series_id.to_string();
    let eid_owned = episode_id.to_string();
    let global_search_format = global_search_format.to_string();
    let global_search_format_absolute = global_search_format_absolute.to_string();
    let bindings = *bindings;
    crate::utils::refresh_cached_with(
        ck,
        move || {
            let sid = sid_owned.clone();
            async move { crate::api::fetch_series_details(sid).await }
        },
        move |details: Option<SeriesDetails>| {
            if let Some(details) = details
                && let Some(payload) = payload_from_series_details(
                    &details,
                    &eid_owned,
                    global_absolute_default,
                    &global_search_format,
                    &global_search_format_absolute,
                )
            {
                apply_episode_details_payload(&bindings, &payload);
                set_show.set(true);
            }
        },
    );
}

/// Build a partial `EpisodeViewModel` from list-row data for instant modal render
/// (before the full series-scoped payload arrives). Used by both the wanted and
/// calendar open flows.
/// `#[allow]`: the arguments mirror the row fields; a struct wrapper would add noise
/// for a two-call-site fixture builder.
#[allow(clippy::too_many_arguments)]
pub fn partial_episode_view_model(
    episode_id: String,
    season_raw: Option<&str>,
    episode_num: i32,
    title: Option<String>,
    status: String,
    dates: ReleaseDates<String>,
    monitored: bool,
    assigned: bool,
) -> EpisodeViewModel {
    EpisodeViewModel {
        unique_id: episode_id,
        season: season_raw
            .and_then(|s| s.parse::<i32>().ok())
            .map(|n| fmt_season(n, LabelStyle::Short))
            .unwrap_or_default(),
        episode: episode_num,
        header: if season_raw.is_some() {
            fmt_episode(episode_num, LabelStyle::Human)
        } else {
            fmt_abs_episode(episode_num, LabelStyle::Human)
        },
        title,
        status,
        quality_profile_id: None,
        size: 0,
        submitter: None,
        release_title: None,
        path: None,
        original_path: None,
        media_info: None,
        fingerprint: None,
        created_at: None,
        file_acquired_at: None,
        monitored,
        dates,
        metadata_ids: HashMap::new(),
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        parts: Vec::new(),
        auxiliary_files: Vec::new(),
        show_only_downloaded: false,
        assigned,
        disk_present: false,
    }
}
