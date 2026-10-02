use crate::components::common::calendar_link_modal::CreateCalendarLinkModal;
use crate::components::common::icons::ChevronDownIcon;
use crate::components::common::modal_wrapper::ModalWrapper;
use crate::components::edit_series::episode_details_modal::EpisodeDetailsModal;

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime};
use jumbie_shared::formatting::LabelStyle;
use jumbie_shared::types::CalendarEpisode;
use jumbie_shared::types::EpisodeViewModel;
use leptos::control_flow::Show;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::hooks::use_config::use_config;
use crate::hooks::use_ui_config::{use_time_format, use_ui_config};

use jumbie_shared::config::ui::ReleaseDateDisplayConfig;

const VIEW_MONTH: &str = "month";
const VIEW_WEEK: &str = "week";
const POLL_INTERVAL_SECS: u64 = 60;
/// The calendar grid always renders 42 cells (6 rows × 7 days) so the height
/// stays consistent when the month doesn't fill exactly 6 weeks.
const CALENDAR_GRID_DAYS: i64 = 42;
const MAX_FULL_DISPLAY: usize = 3;
const OVERFLOW_DISPLAY_COUNT: usize = 2;

#[derive(Clone, Copy, PartialEq)]
enum EpisodeStatus {
    Downloaded,
    Missing,
    Upcoming,
}

/// Derive episode status using full datetime comparison.
///
/// SSoT — delegates to the shared `derive_episode_status` in `jumbie_shared`
/// so the backend, frontend calendar, and frontend season accordion all use
/// the same datetime-comparison rule.
fn episode_status(assigned: bool, ep_datetime: NaiveDateTime) -> EpisodeStatus {
    if assigned {
        EpisodeStatus::Downloaded
    } else if jumbie_shared::formatting::derive_episode_status(
        Some(ep_datetime),
        Local::now().naive_local(),
    ) == jumbie_shared::types::EpisodeStatus::Missing
    {
        EpisodeStatus::Missing
    } else {
        EpisodeStatus::Upcoming
    }
}

/// Get the client-side selected date (date-only) for a `CalendarEpisode`.
/// Used for positioning events on the calendar grid.
fn episode_calendar_date(
    ep: &CalendarEpisode,
    config: &ReleaseDateDisplayConfig,
) -> Option<NaiveDate> {
    episode_calendar_datetime(ep, config).map(|dt| dt.date())
}

/// Get the client-side selected full datetime for a `CalendarEpisode`.
/// Used for time-of-day-aware status derivation (missing vs upcoming).
fn episode_calendar_datetime(
    ep: &CalendarEpisode,
    config: &ReleaseDateDisplayConfig,
) -> Option<NaiveDateTime> {
    // Selection-only entry point — no display formatting needed here.
    crate::utils::release_date::pick_release_date_utc(
        config,
        ep.dates.meta_date.as_deref(),
        ep.dates.upload_date.as_deref(),
        ep.dates.est_date.as_deref(),
    )
    .map(|dt| dt.with_timezone(&Local).naive_local())
}

/// Combines `episode_calendar_date` with an optional config — returns `None`
/// when config is unavailable (not loaded yet) or all sources are disabled.
/// Callers that need a fallback date (e.g. for status coloring of known events)
/// can chain `.unwrap_or_else(|| Local::now().date_naive())`.
fn calendar_episode_date(
    ep: &CalendarEpisode,
    config: Option<&ReleaseDateDisplayConfig>,
) -> Option<NaiveDate> {
    config.and_then(|cfg| episode_calendar_date(ep, cfg))
}

/// Combines `episode_calendar_datetime` with an optional config — returns `None`
/// when config is unavailable (not loaded yet) or all sources are disabled.
fn calendar_episode_datetime(
    ep: &CalendarEpisode,
    config: Option<&ReleaseDateDisplayConfig>,
) -> Option<NaiveDateTime> {
    config.and_then(|cfg| episode_calendar_datetime(ep, cfg))
}

/// Filter episodes whose client-side selected date falls within `[start, end]` (inclusive).
/// When all date sources are disabled, `episode_calendar_date` returns `None` for
/// every episode and the result is empty — no fallback to `meta_date`.
/// SSoT — both month grid and list view use this to avoid repeated date-parsing logic.
fn filter_episodes_in_range(
    eps: &[CalendarEpisode],
    start: NaiveDate,
    end: NaiveDate,
    config: &ReleaseDateDisplayConfig,
) -> Vec<CalendarEpisode> {
    eps.iter()
        .filter(|ep| episode_calendar_date(ep, config).is_some_and(|d| d >= start && d <= end))
        .cloned()
        .collect()
}

/// Encode a string for use in a URL query parameter.  SSoT — used by every
/// calendar API call site.
fn uri_encode(s: &str) -> String {
    js_sys::encode_uri_component(s)
        .as_string()
        .unwrap_or_else(|| s.to_string())
}

/// Build a calendar cache key from a date range.  SSoT so every caller
/// produces a consistent key format.
fn calendar_cache_key(start: NaiveDate, end: NaiveDate) -> String {
    format!(
        "fetch_calendar_{}_{}",
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d")
    )
}

/// Calculate the calendar window from a date, convert both ends to RFC3339,
/// URI-encode them, and produce the cache key — in one call.
/// SSoT — prevents drift between the 3 call sites that need this sequence.
fn prepare_calendar_fetch(date: NaiveDate) -> (String, String, String) {
    let (wide_start, wide_end) = crate::utils::calc_calendar_window(date);
    let req_start = crate::utils::calendar::local_date_to_rfc3339(wide_start, false);
    let req_end = crate::utils::calendar::local_date_to_rfc3339(wide_end, true);
    let start_enc = uri_encode(&req_start);
    let end_enc = uri_encode(&req_end);
    let cache_key = calendar_cache_key(wide_start, wide_end);
    (start_enc, end_enc, cache_key)
}

/// Sort episodes by effective time of day (ascending), then by series title
/// as a tiebreaker.  SSoT for in-day event ordering — used by both month grid
/// and list view.
fn sort_episodes_by_effective_time(
    eps: &mut [CalendarEpisode],
    config: Option<&ReleaseDateDisplayConfig>,
) {
    eps.sort_by(|a, b| {
        let da = calendar_episode_datetime(a, config);
        let db = calendar_episode_datetime(b, config);
        da.cmp(&db)
            .then_with(|| jumbie_shared::formatting::natural_cmp(&a.series_title, &b.series_title))
    });
}

#[derive(Clone, PartialEq)]
struct CalendarDayData {
    date: NaiveDate,
    is_current_month: bool,
    is_today: bool,
    events: Vec<CalendarEpisode>,
}

/// Shared calendar event row — SSoT for the HTML structure of a single event row.
/// Used by both the day grid and the overflow modal so their alignment stays consistent.
fn calendar_event_row_view(
    title: String,
    episode_code: String,
    event_class: &str,
    extra_class: &str,
    tooltip: Option<String>,
    on_click: Callback<()>,
) -> impl IntoView {
    let class = if extra_class.is_empty() {
        format!("calendar-event {event_class}")
    } else {
        format!("calendar-event {event_class} {extra_class}")
    };
    view! {
        <div
            class=class
            title=move || tooltip.clone().unwrap_or_default()
            on:click=move |_| on_click.run(())
        >
            <span class="calendar-event-title">{title}</span>
            <span class="calendar-event-episode">{episode_code}</span>
        </div>
    }
}

/// Shared header for a week panel.
///
/// When `collapsed_days` / `week_dates` / `today` are provided, the header
/// becomes clickable and toggles *all* day-content sections in that week
/// between collapsed and expanded — an "expand/collapse all" affordance.
/// When omitted (empty-week view), the header renders as static text.
fn week_header_view(
    week_header: String,
    collapsed_days: Option<RwSignal<std::collections::HashSet<NaiveDate>>>,
    week_dates: Option<Vec<NaiveDate>>,
    today: Option<NaiveDate>,
) -> AnyView {
    match (collapsed_days, week_dates, today) {
        (Some(cd), Some(dates), Some(t)) => {
            let d = dates.clone();
            let d_title = d.clone();
            view! {
                <div
                    class="btn calendar-week-header"
                    on:click=move |_| {
                        // Snapshot: are ALL day sections currently visually collapsed?
                        let all_collapsed = cd.with(|s| {
                            d.iter()
                                .all(|dd| s.contains(dd) != (*dd < t))
                        });
                        // If all collapsed:  expand every day.
                        // If any expanded:  collapse every day.
                        cd.update(|s| {
                            for dd in &d {
                                if all_collapsed != (*dd >= t) {
                                    s.insert(*dd);
                                } else {
                                    s.remove(dd);
                                }
                            }
                        });
                    }
                    title=move || {
                        if cd.with(|s| d_title.iter().all(|dd| s.contains(dd) != (*dd < t))) {
                            "Expand All Days"
                        } else {
                            "Collapse All Days"
                        }
                    }
                >
                    <span class="calendar-week-label">{week_header}</span>
                </div>
            }
            .into_any()
        }
        _ => view! {
            <div class="calendar-week-header">
                <span class="calendar-week-label">{week_header}</span>
            </div>
        }
        .into_any(),
    }
}

/// Empty week: header + "no episodes" message.
/// Reuses `week_header_view` internally.
fn empty_week_view(week_header: String) -> AnyView {
    view! {
        {week_header_view(week_header, None, None, None)}
        <div class="flex flex-col items-center text-muted-color p-md text-center">
            <div>"No episodes this week."</div>
        </div>
    }
    .into_any()
}

#[component]
pub fn Calendar() -> impl IntoView {
    // Default to week view on mobile (<=768px) so the week content is rendered;
    // on desktop default to month.  The view toggle is hidden on mobile via CSS,
    // so the user stays on week view there regardless.
    let default_view = web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|w| {
            Some(
                if w.as_f64()? <= crate::hooks::use_media_query::MOBILE_BREAKPOINT_PX {
                    VIEW_WEEK
                } else {
                    VIEW_MONTH
                },
            )
        })
        .unwrap_or(VIEW_MONTH);
    let (view_mode, set_view_mode) =
        crate::hooks::use_persistent_view_mode::use_persistent_view_mode(
            "calendar".to_string(),
            default_view.to_string(),
        );

    // Reactive mobile detection via matchMedia — used by the week view guard
    // so expensive week rendering is skipped on desktop when in month view.
    let is_mobile = crate::hooks::use_is_mobile();

    let (selected_overflow_events, set_selected_overflow_events) =
        signal(None::<(NaiveDate, Vec<CalendarEpisode>)>);

    let (show_episode_modal, set_show_episode_modal) = signal(false);
    let (selected_episode, set_selected_episode) = signal::<Option<EpisodeViewModel>>(None);
    let (selected_series_id, set_selected_series_id) = signal::<String>(String::new());
    let (selected_series_title, set_selected_series_title) = signal::<String>(String::new());
    let season_overrides = RwSignal::<Vec<jumbie_shared::types::SeasonOverride>>::new(Vec::new());
    let (all_episodes, set_all_episodes) = signal::<Vec<EpisodeViewModel>>(Vec::new());
    let (absolute_numbering, set_absolute_numbering) = signal(false);
    let (series_search_format, set_series_search_format) = signal(String::new());
    let series_metadata_ids = RwSignal::<std::collections::HashMap<String, String>>::new(
        std::collections::HashMap::new(),
    );

    // SSoT: all payload writes into the modal's signals go through the shared
    // `EpisodeModalBindings` helpers — on open AND after every save — so the
    // open modal always reflects the freshest server state.
    let modal_bindings = crate::utils::episode_state::EpisodeModalBindings {
        set_series_id: set_selected_series_id,
        set_series_title: set_selected_series_title,
        season_overrides,
        set_all_episodes,
        set_absolute_numbering,
        set_series_search_format,
        series_metadata_ids,
        set_episode: set_selected_episode,
    };

    let (show_cal_modal, set_show_cal_modal) = signal(false);

    // UI config — used for client-side release date positioning
    let ui_config = use_ui_config().ui_config;
    // App config — the global absolute-numbering default (SSoT: resolves the
    // series tristate the same way edit_series does).
    let global_config = use_config().config;
    let time_format = use_time_format();

    let (cal_quality_profiles, set_cal_quality_profiles) = signal(std::collections::HashMap::<
        String,
        jumbie_shared::types::QualityProfile,
    >::new());
    crate::utils::use_api_cache(
        "fetch_quality_profiles".to_string(),
        || crate::api::fetch_quality_profiles(),
        move |profiles: std::collections::HashMap<String, jumbie_shared::types::QualityProfile>| {
            set_cal_quality_profiles.set(profiles);
        },
    );

    let (cal_metadata_plugins, set_cal_metadata_plugins) =
        signal(Vec::<jumbie_shared::plugin::PluginInstanceInfo>::new());
    let (cal_plugins_cfg, set_cal_plugins_cfg) =
        signal(None::<jumbie_shared::config::PluginsConfig>);

    crate::utils::use_api_cache(
        "fetch_plugins".to_string(),
        || crate::api::fetch_plugins(),
        move |plugins: Vec<jumbie_shared::plugin::PluginInstanceInfo>| {
            let meta = plugins
                .into_iter()
                .filter(|p| {
                    p.capabilities
                        .contains(&jumbie_shared::plugin::Capability::MetadataProviderNormal)
                        || p.capabilities
                            .contains(&jumbie_shared::plugin::Capability::MetadataProviderAbsolute)
                })
                .collect();
            set_cal_metadata_plugins.set(meta);
        },
    );

    crate::utils::use_api_cache(
        "fetch_plugins_cfg".to_string(),
        || crate::api::fetch_plugins_cfg(),
        move |c: jumbie_shared::config::PluginsConfig| {
            set_cal_plugins_cfg.set(Some(c));
        },
    );

    let cal_active_metadata_plugins = Signal::derive(move || {
        let all_plugins = cal_metadata_plugins.get();
        if let Some(cfg) = cal_plugins_cfg.get() {
            crate::utils::resolve_active_metadata_plugins(all_plugins, &cfg)
        } else {
            Vec::new()
        }
    });

    let open_episode_details = move |ep_id: String, series_id: String, s_title: String| {
        // SSoT: open via the shared series-scoped helper — instant render from
        // the series-details cache, then ALWAYS force-revalidate (bypasses the
        // TTL) so the modal never shows a stale status.
        crate::utils::episode_state::reset_episode_modal_bindings(&modal_bindings);
        set_selected_series_id.set(series_id.clone());
        set_selected_series_title.set(s_title);
        let global_abs = global_config
            .get_untracked()
            .map(|c| c.general.absolute_numbering)
            .unwrap_or(false);
        let (global_fmt, global_fmt_abs) = global_config
            .get_untracked()
            .map(|c| {
                (
                    c.organization.search_format.clone(),
                    c.organization.search_format_absolute.clone(),
                )
            })
            .unwrap_or_default();
        crate::utils::episode_state::open_episode_details_modal(
            &modal_bindings,
            &series_id,
            &ep_id,
            global_abs,
            &global_fmt,
            &global_fmt_abs,
            set_show_episode_modal,
        );
    };

    // SSoT for all date-derived values in this component (month grid, week range, today button).
    // Both month and week view derive their display from this single signal reactively — no
    // re-evaluation is needed when switching between views.
    let now = Local::now().date_naive();
    let (current_date, set_current_date) = signal(now);

    let week_range = Signal::derive(move || {
        let base = current_date.get();
        crate::utils::week_range(base)
    });

    let ordinal_day = |d: &NaiveDate| -> String {
        let day = d.day();
        let suffix = match day % 10 {
            1 if day != 11 => "st",
            2 if day != 12 => "nd",
            3 if day != 13 => "rd",
            _ => "th",
        };
        format!("{}{}", day, suffix)
    };

    let week_header = Signal::derive(move || {
        let (ws, we) = week_range.get();
        let current = current_date.get();
        let week_num = crate::utils::week_of_month(ws, (current.year(), current.month()));
        if ws.month() == we.month() && ws.year() == we.year() {
            format!(
                "Week {} ({} {}-{})",
                week_num,
                ws.format("%b"),
                ordinal_day(&ws),
                ordinal_day(&we),
            )
        } else {
            format!(
                "Week {} ({} {} — {} {})",
                week_num,
                ws.format("%b"),
                ordinal_day(&ws),
                we.format("%b"),
                ordinal_day(&we),
            )
        }
    });

    // Episodes signal — populated instantly from cache, background-refreshed on every month change
    let (episodes, set_episodes) = signal(Vec::<CalendarEpisode>::new());

    // Fetch calendar data over a ~3-month window — covers current, previous, and next month.
    // Why a single wide range: eliminates 2 separate preload API calls for adjacent months;
    // episode detail modals fetch on demand via the single-episode endpoint (cached + deduped).
    //
    // Overlap-aware merging: when the user navigates between adjacent months, the new 3-month
    // window overlaps ~67% with the previous one. Instead of always fetching the full range,
    // this Effect scans the cache for overlapping entries, reuses their data for the overlap
    // portion, and only fetches the non-overlapping delta range from the API.
    //
    // This reduces redundant data in API calls while keeping the UI instantly responsive
    // from cached data.
    let calendar_alive = StoredValue::new_local(true);
    on_cleanup(move || calendar_alive.set_value(false));

    Effect::new(move |_| {
        let date = current_date.get();
        let (wide_start, wide_end) = crate::utils::calc_calendar_window(date);

        // Phase 1: Try exact cache hit first (fastest path — key matches exactly).
        // Cache key uses local dates (no timezone) — the overlap merger
        // and preloader also use local-date keys so they match.
        let full_key = calendar_cache_key(wide_start, wide_end);
        if let Some(data) =
            crate::utils::read_cache::<jumbie_shared::types::CalendarResponse>(&full_key)
        {
            set_episodes.set(data.episodes);
            if crate::utils::is_cache_fresh(&full_key) {
                // Exact hit and fresh — no fetch needed.
                return;
            }
            // Exact hit but stale — fall through to background-refresh below.
        }

        // Phase 1b: Scan for overlapping cache entries to reuse for instant render.
        let overlap = crate::utils::find_calendar_cache_overlap(wide_start, wide_end);
        if !overlap.cached_episodes.is_empty() {
            set_episodes.set(overlap.cached_episodes);
        }

        // Phase 2: Determine what range to fetch.
        let (fetch_start, fetch_end) = match overlap.delta_range {
            Some((ds, de)) => (ds, de),
            // Fall back to full range if delta couldn't be determined.
            None => (wide_start, wide_end),
        };
        let fetch_req_start = crate::utils::calendar::local_date_to_rfc3339(fetch_start, false);
        let fetch_req_end = crate::utils::calendar::local_date_to_rfc3339(fetch_end, true);
        let fetch_key = calendar_cache_key(fetch_start, fetch_end);

        // Skip fetch if the delta range is already cached and fresh.
        if crate::utils::is_cache_fresh(&fetch_key) {
            return;
        }

        // Dedup: if a request for this key is already in-flight, register a
        // callback that reads from cache on completion.
        if crate::utils::is_pending(&fetch_key) {
            crate::utils::on_pending_complete(
                &fetch_key,
                Box::new({
                    let set_episodes = set_episodes.clone();
                    move || {
                        if !calendar_alive.get_value() {
                            return;
                        }
                        // Re-run overlap-aware merge so we get any newly cached data.
                        let (ws, we) =
                            crate::utils::calc_calendar_window(current_date.get_untracked());
                        let fresh_overlap = crate::utils::find_calendar_cache_overlap(ws, we);
                        if !fresh_overlap.cached_episodes.is_empty() {
                            set_episodes.set(fresh_overlap.cached_episodes);
                        }
                    }
                }),
            );
            return;
        }

        // Fetch the delta range (or full range if no overlap found).
        crate::utils::mark_pending(&fetch_key);
        let fk = fetch_key.clone();
        let set_episodes_clone = set_episodes.clone();
        let current_date_clone = current_date.clone();
        let start_enc = uri_encode(&fetch_req_start);
        let end_enc = uri_encode(&fetch_req_end);
        spawn_local(async move {
            if let Ok(response) = crate::api::fetch_calendar(&start_enc, &end_enc).await {
                // SSoT change detection: invalidate sibling views (library/wanted)
                // if the fetched portion differs from the cached window.
                crate::utils::write_calendar_cache_with_change_detection(&fk, &response);

                if !calendar_alive.get_value() {
                    crate::utils::resolve_pending(&fk);
                    return;
                }

                // Merge: combine cached episodes (from the overlap portion)
                // with the fresh episodes (from the delta).  Dedup by episode_id
                // so episodes that changed in the overlapping period are
                // superseded by the fresh delta data.
                let (ws, we) =
                    crate::utils::calc_calendar_window(current_date_clone.get_untracked());
                let final_overlap = crate::utils::find_calendar_cache_overlap(ws, we);
                let mut merged = final_overlap.cached_episodes;

                // Add fresh delta episodes. Since the delta is non-overlapping,
                // dedup is just a safety net.
                merged.extend(response.episodes);
                merged.sort_by(|a, b| a.episode_id.cmp(&b.episode_id));
                merged.dedup_by(|a, b| a.episode_id == b.episode_id);

                set_episodes_clone.set(merged);
            }
            crate::utils::resolve_pending(&fk);
        });
    });

    // Re-fetches the current calendar window every 60 seconds so the calendar
    // picks up background changes (download completions, reorganizes, etc.)
    // without a manual refresh.
    //
    // A full re-fetch is used instead of cache-patching because we don't know
    // which episodes changed; the interval costs one small HTTP request and the
    // backend query is fast. The fetch also acts as the shared change-detector:
    // when the window differs from the cached one, the sibling list caches
    // (library, wanted) are invalidated so they re-fetch on next mount (SSoT:
    // `write_calendar_cache_with_change_detection`).
    let run_calendar_poll = move || {
        let date = current_date.get_untracked();
        let (start_enc, end_enc, ck) = prepare_calendar_fetch(date);
        let set_episodes = set_episodes.clone();
        spawn_local(async move {
            if let Ok(response) = crate::api::fetch_calendar(&start_enc, &end_enc).await {
                crate::utils::write_calendar_cache_with_change_detection(&ck, &response);
                set_episodes.try_update(|e| *e = response.episodes.clone());
            }
        });
    };

    // Fire the poll once immediately on a warm re-entry: the main fetch Effect
    // returns early on a fresh cache hit, so without this the grid (and the
    // sibling-cache invalidation above) would wait for the first 60s tick after
    // returning to the tab.  Gated on a fresh cache so a cold mount doesn't
    // double-fetch (the main Effect already fetches then).
    let (_, _, mount_ck) = prepare_calendar_fetch(current_date.get_untracked());
    if crate::utils::is_cache_fresh(&mount_ck) {
        run_calendar_poll();
    }

    let poll_handle = set_interval_with_handle(
        move || run_calendar_poll(),
        std::time::Duration::from_secs(POLL_INTERVAL_SECS),
    )
    .expect("Failed to create calendar poll interval");
    on_cleanup(move || poll_handle.clear());

    // Used by on_override_saved (same-component saves) and the context Effect
    // (cross-component saves): fetches the series' details (shared cache key with
    // the modal) and projects the fresh view model onto the changed episode in the
    // local signal and the calendar cache.
    //
    // Always revalidates (`refresh_cached_with`) so a TTL-fresh-but-stale
    // series-details entry never suppresses the patch; the 60-second poll still
    // handles full reconciliation.
    let fetch_and_patch_episode = {
        let set_episodes = set_episodes.clone();
        move |episode_id: String, series_id: String| {
            if series_id.is_empty() {
                return;
            }
            let ck = format!("fetch_series_details_{}", series_id);
            let set_episodes = set_episodes.clone();
            let bindings = modal_bindings.clone();
            let sid_owned = series_id.clone();
            crate::utils::refresh_cached_with(
                ck,
                move || {
                    let sid = sid_owned.clone();
                    async move { crate::api::fetch_series_details(sid).await }
                },
                move |details: Option<jumbie_shared::types::SeriesDetails>| {
                    let Some(details) = details else {
                        return; // Series gone — nothing to patch.
                    };
                    let global_abs = global_config
                        .get_untracked()
                        .map(|c| c.general.absolute_numbering)
                        .unwrap_or(false);
                    let (global_fmt, global_fmt_abs) = global_config
                        .get_untracked()
                        .map(|c| {
                            (
                                c.organization.search_format.clone(),
                                c.organization.search_format_absolute.clone(),
                            )
                        })
                        .unwrap_or_default();
                    let Some(payload) = crate::utils::episode_state::payload_from_series_details(
                        &details,
                        &episode_id,
                        global_abs,
                        &global_fmt,
                        &global_fmt_abs,
                    ) else {
                        return; // Episode not in this series' list — nothing to patch.
                    };
                    // SSoT: project the freshest view-model fields (dates, status,
                    // assigned, title, eff_date) onto the calendar episode — the
                    // same rule `patch_calendar_cache` applies to cached windows.
                    let eid = payload.episode.unique_id.clone();
                    let series_title = payload.series_title.clone();
                    let config = ui_config.get();
                    let Some(release_date_cfg) = config.as_ref().map(|u| &u.release_date_display)
                    else {
                        return; // Config not loaded yet — grid isn't rendering anyway.
                    };
                    let tf = time_format.get();
                    let (ws, we) = crate::utils::calc_calendar_window(current_date.get_untracked());
                    set_episodes.try_update(|eps| {
                        let Some(idx) = eps.iter().position(|ep| ep.episode_id == eid) else {
                            return; // Not in the current window — cache-only patch below.
                        };
                        crate::utils::refresh_calendar_episode_from_view_model(
                            &mut eps[idx],
                            &payload.episode,
                            &series_title,
                            release_date_cfg,
                            &tf,
                        );
                        // A date change may have moved the episode out of the
                        // window — drop it so no ghost row lingers until the
                        // 60-second poll reconciles.
                        let in_window =
                            match calendar_episode_datetime(&eps[idx], Some(release_date_cfg)) {
                                Some(dt) => {
                                    let d = dt.date();
                                    d >= ws && d <= we
                                }
                                // No effective date — can't judge, keep it.
                                None => true,
                            };
                        if !in_window {
                            eps.remove(idx);
                        }
                    });
                    crate::utils::patch_calendar_cache(
                        &payload.episode,
                        &series_title,
                        release_date_cfg,
                        &tf,
                    );
                    // Keep the open modal (if any) on the freshest server state
                    // — same rule edit_series gets via its refetch.
                    crate::utils::episode_state::apply_episode_details_payload(&bindings, &payload);
                },
            );
        }
    };

    // When the wanted page or edit_series saves an episode, the shared
    // EpisodeStateContext carries the episode_id; this Effect does a targeted
    // fetch + patch so the calendar updates without waiting for the 60-second poll.
    let episode_ctx = crate::utils::episode_state::use_episode_state();
    let fetch_and_patch = fetch_and_patch_episode.clone();
    Effect::new(move |_| {
        if let Some(save) = episode_ctx.pending_save.get() {
            let eid = save.episode_id.clone();
            let sid = save.series_id.clone();
            // Clear immediately so subsequent saves can write a new one.
            episode_ctx.pending_save.set(None);
            fetch_and_patch(eid, sid);
        }
    });

    // Format episode – returns (title, episode_code, tooltip) so the title can shrink independently on overflow
    let format_episode = |ep: &CalendarEpisode| -> (String, String, String) {
        let s_num = jumbie_shared::mapping::parse_season_num(&ep.season).unwrap_or(0);
        let title = ep.series_title.clone();
        let episode_code = jumbie_shared::formatting::fmt_season_episode(
            s_num,
            ep.episode,
            None,
            LabelStyle::Short,
        );
        let tooltip = format!("{} {}", title, episode_code);
        (title, episode_code, tooltip)
    };

    let calendar_days = move || {
        let base_date = current_date.get();
        let eps_data = episodes.get();
        let config = ui_config.get();

        let release_date_cfg = config.as_ref().map(|u| &u.release_date_display);

        let mut days = Vec::new();

        // Always anchor the calendar grid to the 1st of the month,
        // even when current_date has been adjusted to a different day
        // by week-based navigation in week view.
        let month_start = NaiveDate::from_ymd_opt(base_date.year(), base_date.month(), 1).unwrap();
        let days_from_sunday = month_start.weekday().num_days_from_sunday();
        let calendar_start = month_start - Duration::days(days_from_sunday as i64);

        let next_month_start = if month_start.month() == 12 {
            NaiveDate::from_ymd_opt(month_start.year() + 1, 1, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(month_start.year(), month_start.month() + 1, 1).unwrap()
        };
        let month_end = next_month_start - Duration::days(1);

        // The calendar must at least go up to month_end, and finish the week (Saturday)
        let days_to_saturday = 6 - month_end.weekday().num_days_from_sunday();
        let calendar_end = month_end + Duration::days(days_to_saturday as i64);

        let mut total_days = (calendar_end - calendar_start).num_days() + 1;

        total_days = total_days.clamp(CALENDAR_GRID_DAYS, CALENDAR_GRID_DAYS);

        let today = Local::now().date_naive();
        for i in 0..total_days {
            let current_day = calendar_start + Duration::days(i);

            let mut day_events: Vec<CalendarEpisode> = eps_data
                .iter()
                .filter(|ep| {
                    calendar_episode_date(ep, release_date_cfg).is_some_and(|d| d == current_day)
                })
                .cloned()
                .collect();
            sort_episodes_by_effective_time(&mut day_events, release_date_cfg);

            days.push(CalendarDayData {
                date: current_day.clone(),
                is_current_month: current_day.month() == base_date.month(),
                is_today: current_day == today,
                events: day_events,
            });
        }
        days
    };

    // Navigation — arrow keys adapt to the active view mode.
    // In month view: arrows step by month (always landing on the 1st).
    // In week view: arrows step by week (±7 days, landing on the week-start Sunday).
    let go_prev = move || {
        let d = current_date.get();
        if view_mode.get() == VIEW_WEEK {
            let (week_start, _) = crate::utils::week_range(d);
            set_current_date.set(week_start - Duration::days(7));
        } else {
            let new_year = if d.month() == 1 {
                d.year() - 1
            } else {
                d.year()
            };
            let new_month = if d.month() == 1 { 12 } else { d.month() - 1 };
            set_current_date.set(NaiveDate::from_ymd_opt(new_year, new_month, 1).unwrap());
        }
    };

    let go_next = move || {
        let d = current_date.get();
        if view_mode.get() == VIEW_WEEK {
            let (week_start, _) = crate::utils::week_range(d);
            set_current_date.set(week_start + Duration::days(7));
        } else {
            let new_year = if d.month() == 12 {
                d.year() + 1
            } else {
                d.year()
            };
            let new_month = if d.month() == 12 { 1 } else { d.month() + 1 };
            set_current_date.set(NaiveDate::from_ymd_opt(new_year, new_month, 1).unwrap());
        }
    };

    let go_today = move || {
        let now = Local::now().date_naive();
        set_current_date.set(now);
    };

    let get_month_name = move || current_date.get().format("%B %Y").to_string();

    let total_month_episodes = move || {
        let base_date = current_date.get();
        let config = ui_config.get();
        episodes
            .get()
            .into_iter()
            .filter(|ep| {
                calendar_episode_date(ep, config.as_ref().map(|u| &u.release_date_display))
                    .is_some_and(|d| d.month() == base_date.month() && d.year() == base_date.year())
            })
            .count()
    };

    let total_week_episodes = move || {
        let (week_start, week_end) = week_range.get();
        let config = ui_config.get();
        episodes
            .get()
            .into_iter()
            .filter(|ep| {
                calendar_episode_date(ep, config.as_ref().map(|u| &u.release_date_display))
                    .is_some_and(|d| d >= week_start && d <= week_end)
            })
            .count()
    };

    let collapsed_days =
        RwSignal::<std::collections::HashSet<NaiveDate>>::new(std::collections::HashSet::new());

    view! {
            <div class="view active" id="calendar">
                <div class="calendar-header">
                    <div>
                        <h2 class="text-lg font-semibold mb-xs">{get_month_name}</h2>
                        <div class="text-sm text-muted-color">{move || {
                            if view_mode.get() == VIEW_WEEK || is_mobile.get() {
                                format!("{} episodes airing this week", total_week_episodes())
                            } else {
                                format!("{} episodes airing this month", total_month_episodes())
                            }
                        }}</div>
                        <div class="flex gap-md text-xs text-muted-color mt-md desktop-only">
                            {crate::components::edit_series::season_accordion::render_legend_items(true)}
                        </div>
                    </div>
                    <div class="flex gap-md items-center">
                        <div class="view-toggle desktop-only">
                            <button
                                class="view-toggle-btn"
                                class:active=move || view_mode.get() == VIEW_MONTH
                                on:click=move |_| set_view_mode.run(VIEW_MONTH.to_string())
                            >
                                "Month"
                            </button>
                            <button
                                class="view-toggle-btn"
                                class:active=move || view_mode.get() == VIEW_WEEK
                                on:click=move |_| set_view_mode.run(VIEW_WEEK.to_string())
                            >
                                "Week"
                            </button>
                        </div>
                        <div class="calendar-nav">
                            <button class="btn btn-ghost" title="Create iCal Link" on:click=move |_| set_show_cal_modal.set(true)>
                                "iCal"
                            </button>
                            <button class="btn btn-ghost btn-icon" on:click=move |_| go_prev()>"←"</button>
                            <button class="btn btn-ghost" on:click=move |_| go_today()>"Today"</button>
                            <button class="btn btn-ghost btn-icon" on:click=move |_| go_next()>"→"</button>
                        </div>
                    </div>
                </div>

                <div class="desktop-only">
                    <Show when=move || view_mode.get() == VIEW_MONTH && !is_mobile.get()>
                        <div id="calendarMonthView" class="unselectable">
                            <div class="calendar-grid">
                                <div class="calendar-day-header">"Sun"</div>
                                <div class="calendar-day-header">"Mon"</div>
                                <div class="calendar-day-header">"Tue"</div>
                                <div class="calendar-day-header">"Wed"</div>
                                <div class="calendar-day-header">"Thu"</div>
                                <div class="calendar-day-header">"Fri"</div>
                                <div class="calendar-day-header">"Sat"</div>

                                {move || calendar_days().into_iter().map(|day| {
                                    let day_num = day.date.day();
                                    let is_other_month = !day.is_current_month;
                                    let is_today = day.is_today;
                                    let all_events = day.events.clone();
                                    let total_events = all_events.len();
                                    // Show all MAX_FULL_DISPLAY events when total ≤ MAX_FULL_DISPLAY;
                                    // for more, show only OVERFLOW_DISPLAY_COUNT and put the rest into
                                    // overflow (the "N more..." button replaces the last slot).
                                    let (display_events, overflow_count, overflow_events) = if total_events > MAX_FULL_DISPLAY {
                                        let mut events = all_events.clone();
                                        let overflow = events.split_off(OVERFLOW_DISPLAY_COUNT);
                                        (events, total_events - OVERFLOW_DISPLAY_COUNT, overflow)
                                    } else {
                                        (all_events, 0, Vec::new())
                                    };
                                    let day_date_clone = day.date.clone();

                                    view! {
                                        <div class="calendar-day" class:other-month=is_other_month class:today=is_today>
                                            <div class="day-number">{day_num}</div>
                                            <div class="calendar-events-container">
                                                {display_events.into_iter().map(|ev| {
                                                    let (title, episode_code, title_tooltip) = format_episode(&ev);
                                                    let ep_dt = calendar_episode_datetime(
                                                        &ev,
                                                        ui_config.get().as_ref().map(|u| &u.release_date_display),
                                                    )
                                                    .unwrap_or_else(|| Local::now().naive_local());

                                                    let event_class = if is_other_month {
                                                        "out-of-range"
                                                    } else {
                                                        match episode_status(ev.assigned, ep_dt) {
                                                            EpisodeStatus::Downloaded => "downloaded",
                                                            EpisodeStatus::Missing => "missing",
                                                            EpisodeStatus::Upcoming => "upcoming",
                                                        }
                                                    };

                                                    let ep_id = ev.episode_id.clone();
                                                    let s_title = ev.series_title.clone();
                                                    let s_id = ev.series_id.clone();

                                                    calendar_event_row_view(
                                                        title,
                                                        episode_code,
                                                        event_class,
                                                        "",
                                                        Some(title_tooltip),
                                                        Callback::new(move |_| open_episode_details(ep_id.clone(), s_id.clone(), s_title.clone())),
                                                    )
                                                }).collect_view()}

                                                <Show when=move || { overflow_count > 0 }>
                                                    <div
                                                        class="calendar-event-more"
                                                        on:click={
                                                            let ov_events = overflow_events.clone();
                                                            let d = day_date_clone;
                                                            move |_| set_selected_overflow_events.set(Some((d, ov_events.clone())))
                                                        }
                                                    >
                                                        {format!("{} more...", overflow_count)}
                                                    </div>
                                                </Show>
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    </Show>
                </div>

                <div class:hide-week-on-desktop=move || view_mode.get() == VIEW_MONTH>
                    <div id="calendarWeekView" class="unselectable">
                        <div class="calendar-week">
                            {move || {
                                // Skip rendering when the week is hidden AND we're on desktop.
                                // On mobile the CSS always forces the week wrapper visible
                                // (via media query override), so we must render even when
                                // view_mode is "month" to avoid a blank area on resize.
                                if view_mode.get() != VIEW_WEEK && !is_mobile.get() {
                                    return ().into_any();
                                }
                                let (week_start, week_end) = week_range.get();
                                let week_header = week_header.get();
                                let config = ui_config.get();
                                let Some(ui_config_ref) = config.as_ref() else {
                                    return empty_week_view(week_header);
                                };
                                let mut eps = filter_episodes_in_range(
                                    &episodes.get(),
                                    week_start,
                                    week_end,
                                    &ui_config_ref.release_date_display,
                                );
                                // Sort by datetime (then title) so that within each date group
                                // episodes are already in chronological order — no per-group sort needed.
                                sort_episodes_by_effective_time(
                                    &mut eps,
                                    Some(&ui_config_ref.release_date_display),
                                );
                                if eps.is_empty() {
                                    return empty_week_view(week_header);
                                };
                                // Group episodes by date for visual day separators.
                                // When all sources are disabled, filter_episodes_in_range already
                                // returned empty, so this code never runs.
                                let today = Local::now().date_naive();
                                let mut grouped_eps: Vec<(NaiveDate, Vec<(CalendarEpisode, NaiveDateTime)>)> = Vec::new();
                                for ep in eps {
                                    let ep_dt = episode_calendar_datetime(&ep, &ui_config_ref.release_date_display)
                                        .unwrap_or_else(|| Local::now().naive_local());
                                    let ep_date = ep_dt.date();
                                    match grouped_eps.last_mut() {
                                        Some((date, list)) if *date == ep_date => list.push((ep, ep_dt)),
                                        _ => grouped_eps.push((ep_date, vec![(ep, ep_dt)])),
                                    }
                                }

                                let mut all: Vec<AnyView> = Vec::new();
                                let week_dates: Vec<NaiveDate> =
                                    grouped_eps.iter().map(|(d, _)| *d).collect();
                                all.push(week_header_view(
                                    week_header,
                                    Some(collapsed_days),
                                    Some(week_dates),
                                    Some(today),
                                ));
                                for (day_date, day_eps) in grouped_eps {
                                    let day_header = {
                                        let day_name = day_date.format("%A").to_string();
                                        if day_date == today {
                                            format!("Today — {} {}", day_name, day_date.format("%b %d"))
                                        } else if day_date == today + Duration::days(1) {
                                            format!("Tomorrow — {} {}", day_name, day_date.format("%b %d"))
                                        } else {
                                            format!("{} {}", day_name, day_date.format("%b %d"))
                                        }
                                    };
                                    let dd = day_date;
                                    let default_collapsed = dd < today;
                                    let is_collapsed = move || {
                                        if collapsed_days.with(|s| s.contains(&dd)) {
                                            !default_collapsed
                                        } else {
                                            default_collapsed
                                        }
                                    };
                                    all.push(view! {
                                        <div
                                            class="calendar-week-day-separator"
                                            class:collapsed=is_collapsed
                                            on:click=move |_| {
                                                collapsed_days.update(|s| {
                                                    if s.contains(&dd) {
                                                        s.remove(&dd);
                                                    } else {
                                                        s.insert(dd);
                                                    }
                                                })
                                            }
                                        >
                                            <span class="calendar-week-day-label">{day_header}</span>
                                            <span class="accordion-arrow"><ChevronDownIcon/></span>
                                        </div>
                                    }.into_any());
                                    let dd2 = day_date;
                                    let mut day_content: Vec<AnyView> = Vec::new();
                                    for (ep, ep_dt) in day_eps {
                                        let ep_date = ep_dt.date();
                                        let format_day = format!("{:02}", ep_date.day());
                                        let format_month = ep_date.format("%b").to_string();
                                        let release_time =
                                            crate::utils::format_time_local(&ep_dt, &time_format.get());

                                        let release_text = release_time.to_string();

                                        let meta_text = {
                                            let s_num = jumbie_shared::mapping::parse_season_num(&ep.season).unwrap_or(0);
                                            jumbie_shared::formatting::fmt_season_episode(s_num, ep.episode, None, LabelStyle::Human)
                                        };
                                        let (_, _, title_tooltip) = format_episode(&ep);

                                        let (quality_text, quality_class) = match episode_status(ep.assigned, ep_dt) {
                                            EpisodeStatus::Downloaded => ("Downloaded", "text-primary-color"),
                                            EpisodeStatus::Missing => ("Missing", "text-danger"),
                                            EpisodeStatus::Upcoming => ("Airing", "text-muted-color"),
                                        };

                                        let date_class = match episode_status(ep.assigned, ep_dt) {
                                            EpisodeStatus::Downloaded => "date-downloaded",
                                            EpisodeStatus::Missing => "date-missing",
                                            EpisodeStatus::Upcoming => "date-upcoming",
                                        };

                                        let ep_id = ep.episode_id.clone();
                                        let s_title = ep.series_title.clone();
                                        let s_id = ep.series_id.clone();

                                        day_content.push(view! {
                                            <div
                                                class="btn calendar-item"
                                                on:click=move |_| open_episode_details(ep_id.clone(), s_id.clone(), s_title.clone())
                                                title={title_tooltip}
                                            >
                                                <div class="calendar-date">
                                                    <div class=format!("calendar-date-day {}", date_class)>{format_day}</div>
                                                    <div class="calendar-date-month">{format_month}</div>
                                                </div>
                                                <div class="calendar-info">
                                                    <div class="calendar-info-title">{ep.series_title.clone()}</div>
                                                    <div class="calendar-info-meta">{meta_text}</div>
                                                    <div class="calendar-info-meta">{release_text}</div>
                                                </div>
                                                <span class=format!("cell-quality {}", quality_class)>{quality_text}</span>
                                            </div>
                                        }.into_any());
                                    }
                                    all.push(view! {
                                        <div
                                            class="calendar-week-day-content"
                                            class:collapsed=move || {
                                                if collapsed_days.with(|s| s.contains(&dd2)) {
                                                    dd2 >= today
                                                } else {
                                                    dd2 < today
                                                }
                                            }
                                        >
                                            {day_content.into_view()}
                                        </div>
                                    }.into_any());
                                }
                                all.into_view().into_any()
                            }}
                        </div>
                    </div>
                </div>
            </div>

            <ModalWrapper
        show=Signal::derive(move || selected_overflow_events.get().is_some())
        on_close=move |_| set_selected_overflow_events.set(None)
        title=Signal::derive(move || selected_overflow_events.get().map(|(d, _)| d.format("%B %d, %Y").to_string()).unwrap_or_else(|| String::new()))
        size="modal-sm"
        class="calendar-modal-scroll"
    >
        <div class="flex flex-col">
            {move || {
                let events = selected_overflow_events.get().map(|(_, e)| e).unwrap_or_default();
                events.into_iter().map(|ev| {
                    let (title, episode_code, _) = format_episode(&ev);
                    let ep_dt = calendar_episode_datetime(
                        &ev,
                        ui_config.get().as_ref().map(|u| &u.release_date_display),
                    )
                    .unwrap_or_else(|| Local::now().naive_local());

                    let event_class = match episode_status(ev.assigned, ep_dt) {
                        EpisodeStatus::Downloaded => "downloaded",
                        EpisodeStatus::Missing => "missing",
                        EpisodeStatus::Upcoming => "upcoming",
                    };

                            let ep_id = ev.episode_id.clone();
                            let s_title = ev.series_title.clone();

                            calendar_event_row_view(
                                title,
                                episode_code,
                                event_class,
                                "p-sm mb-sm",
                                None,
                                Callback::new({
                                    let ev_id = ep_id.clone();
                                    let ev_sid = ev.series_id.clone();
                                    let ev_title = s_title.clone();
                                    move |_| {
                                        set_selected_overflow_events.set(None);
                                        open_episode_details(ev_id.clone(), ev_sid.clone(), ev_title.clone());
                                    }
                                }),
                            )
                        }).collect_view()
                    }}
                </div>
            </ModalWrapper>

            <EpisodeDetailsModal
                show=show_episode_modal
                set_show=set_show_episode_modal
                episode=selected_episode
                all_episodes=Signal::derive(move || all_episodes.get())
                series_id=Signal::derive(move || selected_series_id.get())
                series_title=Signal::derive(move || selected_series_title.get())
                season_overrides=Signal::derive(move || season_overrides.get())
                absolute_numbering=Signal::derive(move || absolute_numbering.get())
                series_search_format=Signal::derive(move || series_search_format.get())
                series_metadata_ids=Signal::derive(move || series_metadata_ids.get())
                quality_profiles=Signal::derive(move || cal_quality_profiles.get())
                metadata_plugins=cal_active_metadata_plugins
                on_override_saved=Callback::new({
                    let episode_ctx = crate::utils::episode_state::use_episode_state();
                    let fetch_and_patch = fetch_and_patch_episode.clone();
                    move |(episode_id, series_id): (String, String)| {
                        // Broadcast save to shared context so all observing components (wanted, edit_series) can react.
                        crate::utils::episode_state::notify_episode_saved(
                            &episode_ctx,
                            &episode_id,
                            &series_id,
                        );

                        // Targeted series-scoped fetch from the SSoT. Patches only
                        // the changed episode locally; the 60-second poll handles
                        // full reconciliation.
                        fetch_and_patch(episode_id, series_id);
                    }
                })
            />

            <CreateCalendarLinkModal
                show=show_cal_modal
                set_show=set_show_cal_modal
                on_success=Callback::new(move |_token: String| {
                    // Invalidate config cache so ApiSettings token table
                    // shows the new link without requiring a refresh.
                    crate::utils::invalidate_cache_prefix("fetch_config");
                })
            />
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::types::ReleaseDates;

    /// Helper to build a `CalendarEpisode` with the given series title and
    /// an RFC 3339 date string placed in the `meta_date` slot (the default
    /// priority source in `ReleaseDateDisplayConfig`).
    fn ep(title: &str, meta_date: Option<&str>) -> CalendarEpisode {
        CalendarEpisode {
            series_title: title.to_string(),
            series_id: String::new(),
            episode_id: String::new(),
            season: String::new(),
            episode: 1,
            episode_title: None,
            dates: ReleaseDates {
                meta_date: meta_date.map(|s| s.to_string()),
                upload_date: None,
                est_date: None,
            },
            eff_date: String::new(),
            status: String::new(),
            assigned: false,
            disk_present: false,
        }
    }

    #[test]
    fn sort_by_title_when_config_is_none() {
        let mut eps = vec![
            ep("Z Series", Some("2024-01-15T10:00:00Z")),
            ep("A Series", Some("2024-01-15T10:00:00Z")),
            ep("M Series", Some("2024-01-15T10:00:00Z")),
        ];
        sort_episodes_by_effective_time(&mut eps, None);
        let titles: Vec<&str> = eps.iter().map(|e| e.series_title.as_str()).collect();
        assert_eq!(titles, vec!["A Series", "M Series", "Z Series"]);
    }

    #[test]
    fn earlier_time_comes_first() {
        let mut eps = vec![
            ep("Late Show", Some("2024-01-15T14:30:00Z")),
            ep("Early Show", Some("2024-01-15T08:00:00Z")),
            ep("Mid Show", Some("2024-01-15T10:00:00Z")),
        ];
        sort_episodes_by_effective_time(&mut eps, Some(&ReleaseDateDisplayConfig::default()));
        let titles: Vec<&str> = eps.iter().map(|e| e.series_title.as_str()).collect();
        assert_eq!(titles, vec!["Early Show", "Mid Show", "Late Show"]);
    }

    #[test]
    fn same_time_sorted_by_title() {
        let mut eps = vec![
            ep("Z Series", Some("2024-06-01T20:00:00Z")),
            ep("A Series", Some("2024-06-01T20:00:00Z")),
        ];
        sort_episodes_by_effective_time(&mut eps, Some(&ReleaseDateDisplayConfig::default()));
        let titles: Vec<&str> = eps.iter().map(|e| e.series_title.as_str()).collect();
        assert_eq!(titles, vec!["A Series", "Z Series"]);
    }

    #[test]
    fn none_datetime_sorts_before_valid() {
        let mut eps = vec![
            ep("With Date", Some("2024-03-10T12:00:00Z")),
            ep("No Date", None),
        ];
        sort_episodes_by_effective_time(&mut eps, Some(&ReleaseDateDisplayConfig::default()));
        let titles: Vec<&str> = eps.iter().map(|e| e.series_title.as_str()).collect();
        // None < Some, so "No Date" comes first even though "With Date" would
        // be earlier in the alphabet.
        assert_eq!(titles, vec!["No Date", "With Date"]);
    }

    #[test]
    fn stable_for_identical_episodes() {
        let a = ep("Same", Some("2024-05-05T18:00:00Z"));
        let b = ep("Same", Some("2024-05-05T18:00:00Z"));
        let mut eps = vec![a.clone(), b.clone()];
        sort_episodes_by_effective_time(&mut eps, Some(&ReleaseDateDisplayConfig::default()));
        // Rust's sort is stable, so original order is preserved when equal.
        assert_eq!(eps[0].series_title, "Same");
        assert_eq!(eps[1].series_title, "Same");
    }

    #[test]
    fn empty_list_is_noop() {
        let mut eps: Vec<CalendarEpisode> = Vec::new();
        sort_episodes_by_effective_time(&mut eps, Some(&ReleaseDateDisplayConfig::default()));
        assert!(eps.is_empty());
    }
}
