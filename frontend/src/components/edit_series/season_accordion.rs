use crate::components::common::icons::*;
use crate::components::common::toast::NotificationType;
use crate::components::edit_series::EditSeriesCtx;
use crate::hooks::use_ui_config::use_ui_config;
use crate::utils::episode_state::ABSOLUTE_SEASON_LABEL;
use crate::utils::release_date::pick_release_date;
use jumbie_shared::types::{EpisodeViewModel, MetadataSeasonInfo, SeasonOverride};
use leptos::prelude::*;

/// Normalize a season label to the key form used for override lookups:
/// `"S01"` → `"1"`; anything without an `S` prefix is left untouched. The numeric
/// conversion is delegated to the shared SSoT parser. Used throughout the
/// component (header display, override matching, metadata comparison).
pub(crate) fn normalize_season(season_str: &str) -> String {
    match season_str.strip_prefix('S') {
        Some(stripped) => jumbie_shared::mapping::parse_season_num(stripped)
            .unwrap_or(1)
            .to_string(),
        None => season_str.to_string(),
    }
}

/// A single entry in the episode status legend.
pub struct LegendItem {
    /// CSS class suffix for the legend dot (e.g. "downloaded", "in-queue", "oor").
    pub dot_class: &'static str,
    /// Human-readable label shown next to the dot.
    pub label: &'static str,
    /// Whether the dot should render a diagonal slash (used for out-of-range).
    pub has_slash: bool,
    /// Whether this status should appear in the calendar legend.
    /// Calendar statuses are based on file presence + air date, not download queue state.
    pub show_in_calendar: bool,
}

/// Returns the list of episode statuses that appear in the legend.
///
/// Adding a new status to `episode_status_class` should include adding it here so
/// the legend always matches the statuses the UI can display.
pub fn episode_legend_items() -> &'static [LegendItem] {
    &[
        LegendItem {
            dot_class: "downloaded",
            label: "Downloaded",
            has_slash: false,
            show_in_calendar: true,
        },
        LegendItem {
            dot_class: "show-only-downloaded",
            label: "Outside Range",
            has_slash: false,
            show_in_calendar: false,
        },
        LegendItem {
            dot_class: "missing",
            label: "Missing",
            has_slash: false,
            show_in_calendar: true,
        },
        LegendItem {
            dot_class: "unreleased",
            label: "Unreleased",
            has_slash: false,
            show_in_calendar: true,
        },
        LegendItem {
            dot_class: "oor",
            label: "Out of Range",
            has_slash: true,
            show_in_calendar: false,
        },
        LegendItem {
            dot_class: "monitored",
            label: "Monitored",
            has_slash: false,
            show_in_calendar: false,
        },
        LegendItem {
            dot_class: "in-queue",
            label: "In Queue",
            has_slash: false,
            show_in_calendar: false,
        },
    ]
}

/// Render the legend items into a Vec of HTML elements (shared by the calendar and
/// the episodes tab, so legend changes live in one place).
///
/// When `calendar_mode` is true:
/// - Only items with `show_in_calendar == true` are rendered.
/// - The slash overlay is omitted (Out of Range is never shown in the calendar).
pub fn render_legend_items(calendar_mode: bool) -> Vec<leptos::prelude::AnyView> {
    episode_legend_items()
        .iter()
        .filter(|item| !calendar_mode || item.show_in_calendar)
        .map(|item| {
            let (dot_class, show_slash) = if calendar_mode {
                (format!("legend-dot legend-dot--{}", item.dot_class), false)
            } else {
                (
                    format!("legend-dot legend-dot--{}", item.dot_class),
                    item.has_slash,
                )
            };

            view! {
                <span class="legend-item flex items-center gap-xs">
                    <span class={dot_class}>
                        {show_slash
                            .then(|| view! { <span class="legend-dot-slash"></span> })}
                    </span>
                    <span class="legend-label">{item.label}</span>
                </span>
            }
            .into_any()
        })
        .collect()
}

/// Classify an episode's status for CSS class assignment. SSoT for what counts as
/// "missing", "downloaded", or "unreleased" — shared by the auto-search filter and
/// the UI rendering.
pub fn episode_status_class(
    status: &str,
    has_path: bool,
    meta_date: &Option<String>,
) -> &'static str {
    let is_future = meta_date
        .as_ref()
        .and_then(|d| crate::utils::parse_timestamp_utc(d))
        .map(|dt| dt > chrono::Utc::now())
        .unwrap_or(false);

    match status {
        "downloaded" | "organized" => "downloaded",
        _ if has_path => "downloaded",
        "out_of_range" => "out-of-range",
        "in_queue" => "in-queue",
        "missing" => "missing",
        "tba" | "unreleased" => "unreleased",
        _ if meta_date.is_none() || is_future => "unreleased",
        _ => "missing",
    }
}

/// Returns `true` when the episode is assigned a main file but that file is no
/// longer present on disk. `assigned` is DB state; `disk_present` is the disk state
/// of the assigned main file(s). This is a derived, runtime-only state — it is not a
/// backend status. All "Not Found" rendering (badge, warning icon, summary counts,
/// path strikethrough) must use this instead of inlining the check.
pub fn episode_is_not_found(assigned: bool, disk_present: bool) -> bool {
    assigned && !disk_present
}

/// Human-readable label for an episode status.
///
/// Delegates to [`EpisodeStatus::display_label`] so backend and frontend use the
/// same labels; falls back to the raw status string if it can't be parsed.
pub fn episode_status_label(status: &str) -> &str {
    status
        .parse::<jumbie_shared::types::EpisodeStatus>()
        .ok()
        .map(|s| s.display_label())
        .unwrap_or(status)
}

#[component]
pub fn SeasonAccordionList(
    // SSoT: pre-grouped by `episode_state::group_episodes_by_season`, computed once in
    // the parent so the expand/collapse-all list and this grid share one grouping.
    #[prop(into)] grouped_episodes: Signal<Vec<(String, Vec<EpisodeViewModel>)>>,
    #[prop(into)] season_overrides: Signal<Vec<SeasonOverride>>,
    series_id: ReadSignal<String>,
    collapsed_seasons: RwSignal<std::collections::HashSet<String>>,
    managing_season: RwSignal<Option<String>>,
    set_selected_episode: WriteSignal<Option<EpisodeViewModel>>,
    set_show_modal: WriteSignal<bool>,
    save_series: Callback<Option<String>, ()>,
    #[prop(into)] metadata_seasons: Signal<Vec<MetadataSeasonInfo>>,
    #[prop(optional)] on_toggle_episode_monitor: Option<Callback<(String, bool), ()>>,
) -> impl IntoView {
    let ui_config = use_ui_config();
    let active_plugin_name = expect_context::<Signal<String>>();
    let edit_series_ctx = expect_context::<EditSeriesCtx>();
    // Multi-episode file detection, derived from the shared grouping (no separate flat
    // episode list needed). Index assignment is stable regardless of grouping order
    // because it enumerates a path-sorted BTreeMap.
    let multiepisode_paths = Memo::new(move |_| {
        grouped_episodes.with(|groups| {
            let mut counts: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for (_, eps) in groups {
                for ep in eps {
                    if let Some(path) = &ep.path {
                        *counts.entry(path.clone()).or_insert(0) += 1;
                    }
                }
            }
            counts
                .into_iter()
                .filter(|(_, v)| *v > 1)
                .enumerate()
                .map(|(i, (k, _))| (k, i))
                .collect::<std::collections::HashMap<String, usize>>()
        })
    });

    view! {
        <For
            // Yield only the season keys: the keyed diff compares keys, not values, so
            // handing it the episode lists would both clone every episode on each
            // refresh and expose a stale (never-updated) list to each row. Each row
            // reads its live episodes from `grouped_episodes` via `eps_memo` below.
            each=move || {
                grouped_episodes.with(|groups| {
                    groups.iter().map(|(season, _)| season.clone()).collect::<Vec<_>>()
                })
            }
            key=|season| season.clone()
            children=move |season| {
                let season_for_collapsed = season.clone();
                let is_expanded =
                    move || !collapsed_seasons.with(|s| s.contains(&season_for_collapsed));

                let season_clone = season.clone();
                let is_absolute_view = season_clone == ABSOLUTE_SEASON_LABEL;
                // Memo to reactively track this season's episodes. Sourced from the
                // already-grouped memo so each season does an O(seasons) lookup instead
                // of re-filtering the full episode list per season (O(seasons × episodes)).
                let eps_memo = Memo::new({
                    let season_clone = season_clone.clone();
                    move |_| {
                        grouped_episodes.with(|groups| {
                            groups
                                .iter()
                                .find(|(s, _)| s == &season_clone)
                                .map(|(_, eps)| eps.clone())
                                .unwrap_or_default()
                        })
                    }
                });

                // Per-season match info memo: resolves override and metadata_season lookups once,
                // so the view closure only does O(1) reads instead of re-scanning both lists.
                let season_match_info = Memo::new({
                    let season_clone = season_clone.clone();
                    move |_| {
                        let raw_season = normalize_season(&season_clone);
                        let overrides = season_overrides.get();
                        let cell_count = overrides.iter()
                            .find(|o| o.season == raw_season)
                            .and_then(|o| o.cell_count);
                        let eps_count = eps_memo.get().len() as i32;
                        let display_count = cell_count.unwrap_or(eps_count);
                        let meta_season = metadata_seasons.get().into_iter()
                            .find(|m| m.season_number.to_string() == raw_season);
                        let metadata_count = meta_season.as_ref().map(|m| m.episode_count);
                        let is_fallback = meta_season.as_ref()
                            .map(|m| m.is_fallback_mode)
                            .unwrap_or(false);
                        let provider_instance_id = meta_season.as_ref()
                            .map(|m| m.provider_instance_id.clone())
                            .unwrap_or_default();
                        (display_count, metadata_count, is_fallback, provider_instance_id)
                    }
                });

                let (pending_auto_search, set_pending_auto_search) = signal(false);

                // Mount-time check: detect searches still running from before page load
                // Without this, a page refresh mid-search would show "Auto Search" instead
                // of "Searching...", and the user could accidentally trigger a duplicate.
                // The Effect uses get_untracked() so it runs exactly once per season.
                Effect::new({
                    let set_pending = set_pending_auto_search.clone();
                    let sid = series_id.get_untracked();
                    let raw = normalize_season(&season_clone);
                    let sn = if season_clone == ABSOLUTE_SEASON_LABEL {
                        jumbie_shared::mapping::ABSOLUTE_SEASON_NUM.to_string()
                    } else {
                        raw
                    };
                    let season_key = sn.clone();
                    move |_| {
                        if sid.is_empty() {
                            return;
                        }
                        let sid = sid.clone();
                        let sn = season_key.clone();
                        leptos::task::spawn_local(async move {
                            if let Ok(status) = crate::api::get_auto_season_status(sid.clone(), sn.clone()).await
                                && status.running {
                                    set_pending.set(true);
                                    poll_auto_season_status(&sid, &sn).await;
                                    set_pending.set(false);
                                    if let Some(ctx) = use_context::<super::EditSeriesCtx>() {
                                        ctx.series_details.refetch();
                                    }
                                }
                        });
                    }
                });

                let season_for_toggle = season.clone();
                view! {
                    <div class="season-header" on:click=move |_| {
                        collapsed_seasons.update(|s| {
                            if s.contains(&season_for_toggle) {
                                s.remove(&season_for_toggle);
                            } else {
                                s.insert(season_for_toggle.clone());
                            }
                        });
                    }>
                        <div class="season-header-content flex flex-wrap gap-sm items-center w-full">
                            <div class="flex items-center gap-xs">
                                <span class="season-header-title min-w-fit">{
                                    let season_clone = season_clone.clone();
                                    move || {
                                        let eps = eps_memo.get();
                                        let count = eps.len();
                                        if season_clone == ABSOLUTE_SEASON_LABEL {
                                            format!("Episodes - {} Total", count)
                                        } else {
                                            let raw_season = normalize_season(&season_clone);
                                            let cell_count = season_overrides.with(|overrides| {
                                                overrides.iter()
                                                    .find(|o| o.season == raw_season)
                                                    .and_then(|o| o.cell_count)
                                            });
                                            if let Some(cc) = cell_count {
                                                format!("{} - {}* episodes", season_clone, cc)
                                            } else {
                                                format!("{} - {} episodes", season_clone, count)
                                            }
                                        }
                                    }
                                }</span>
                                {
                                    let season_clone = season_clone.clone();
                                    move || {
                                        let raw_season = normalize_season(&season_clone);
                                        let is_fallback = metadata_seasons.get().iter()
                                            .any(|m| m.season_number.to_string() == raw_season && m.is_fallback_mode);

                                        if is_fallback {
                                            view! {
                                                <span class="text-xs text-warning ml-2" title="Season metadata is from opposite ordering mode. Refresh metadata to align.">
                                                    "⚠️"
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! { <span class="hidden"></span> }.into_any()
                                        }
                                    }
                                }
                            </div>

                            <div class="flex flex-wrap gap-xs items-center">
                        {
                            let btn_season_clone = season_clone.clone();
                            move || {
                                if is_absolute_view {
                                    return view! { <span class="hidden"></span> }.into_any();
                                }
                                let (display_count, metadata_count, is_fallback, provider_instance_id) = season_match_info.get();
                                if let Some(t_count) = metadata_count {
                                    let active = active_plugin_name.get();
                                    if active != "None configured" && t_count != display_count && !is_fallback && provider_instance_id == active {
                                        let raw_season = normalize_season(&btn_season_clone);
                                        let btn_season = raw_season;
                                        return view! {
                                            <button
                                                class="btn btn-sm btn-primary flex items-center gap-xs"
                                                title=move || format!("Match {} Episodes", t_count)
                                                on:click=move |ev| {
                                                    ev.stop_propagation();
                                                    // Write directly to the context's override signal (not the read-only copy)
                                                    let target = if edit_series_ctx.absolute_numbering.get_untracked() {
                                                        edit_series_ctx.season_absolute
                                                    } else {
                                                        edit_series_ctx.season
                                                    };
                                                    target.update(|overrides| {
                                                        // Match to provider: clear cell_count so the season dynamically
                                                        // shows whatever episodes the provider has in the DB.
                                                        if let Some(o) = overrides.iter_mut().find(|o| o.season == btn_season) {
                                                            o.cell_count = None;
                                                            o.episode_end = None;
                                                        }
                                                    });
                                                    // Suppress the autosave toast — the explicit message handles it.
                                                    save_series.run(Some(String::new()));

                                                    // Ask the backend to match the season to the provider:
                                                    // reset custom episode metadata and unassign/remove any
                                                    // episodes the provider does not have (their files are
                                                    // durably blocked so a rescan cannot re-adopt them).
                                                    let sid = edit_series_ctx.series_id.get_untracked();
                                                    let season_for_api = btn_season.clone();
                                                    let season_label = btn_season.clone();
                                                    let details = edit_series_ctx.series_details.clone();
                                                    crate::utils::spawn_api_toast(
                                                        crate::api::match_season_to_provider(sid, season_for_api),
                                                        None,
                                                        move |_| {
                                                            crate::components::common::toast::show_success(format!("Matched season {} to provider", season_label));
                                                            details.refetch();
                                                        },
                                                    );
                                                }
                                            >
                                                <span class="icon text-xs"><WandSparklesIcon/></span>
                                                <span class="text-xs">{move || format!("Match {}", t_count)}</span>
                                            </button>
                                        }.into_any();
                                    }
                                }
                                view! { <span class="hidden"></span> }.into_any()
                            }
                        }

                        // Auto-Search button — disabled while pending, shows "Searching..."
                        // (same pending-signal pattern as episode_details_modal.rs). Polling is
                        // used because the backend runs auto-search in a background task that
                        // may take 30+ minutes; the status endpoint is a lightweight HashMap
                        // lookup (no DB queries), so polling every 3s is cheap.
                        <button
                            class="btn btn-sm btn-ghost flex items-center gap-xs"
                            class:opacity-50=pending_auto_search
                            disabled=move || pending_auto_search.get()
                            title="Search all missing/unreleased episodes for this season automatically"
                            on:click={
                                let s_id = series_id.get_untracked();
                                let season_str = season_clone.clone();
                                move |ev| {
                                    ev.stop_propagation();
                                    let eps = eps_memo.get_untracked();
                                    let raw_season = normalize_season(&season_str);

                                    // Collect missing episode numbers as integers.
                                    // `eps` comes from `eps_memo`, which is derived from the parent's
                                    // `series_details_view`. The backend's `fill_missing_episodes` already
                                    // limits the returned episodes to the `cell_count` range (when set),
                                    // so auto-search inherently only searches within the configured scope.
                                    // If cell_count is unset, all missing/unreleased episodes are included.
                                    let missing_episodes: Vec<i32> = eps.iter().filter(|ep| {
                                        let status = episode_status_class(&ep.status, ep.path.is_some(), &ep.dates.meta_date);
                                        status == "missing" || status == "unreleased"
                                    }).map(|ep| ep.episode).collect();

                                    if missing_episodes.is_empty() {
                                        crate::components::common::toast::show_toast("No missing episodes to search for", NotificationType::Info);
                                        return;
                                    }

                                    let count = missing_episodes.len();
                                    let req_sid = s_id.clone();
                                    let req_season = if season_str == ABSOLUTE_SEASON_LABEL {
                                        jumbie_shared::mapping::ABSOLUTE_SEASON_NUM.to_string()
                                    } else {
                                        raw_season.clone()
                                    };

                                    set_pending_auto_search.set(true);

                                    let edit_series_ctx = use_context::<super::EditSeriesCtx>();

                                    let sid = req_sid;
                                    let sn = req_season;
                                    let me = missing_episodes;
                                    leptos::task::spawn_local(async move {
                                        match crate::api::auto_search_season(sid.clone(), sn.clone(), me)
                                            .await
                                        {
                                            Ok(()) => {
                                                crate::components::common::toast::show_toast(
                                                    format!("Auto search started for {} missing episode(s). Downloads will happen in the background.", count),
                                                    NotificationType::Info
                                                );
                                                poll_auto_season_status(&sid, &sn).await;
                                                if let Some(ctx) = &edit_series_ctx {
                                                    ctx.series_details.refetch();
                                                }
                                            }
                                            Err(e) => {
                                                let msg = e.to_string();
                                                if msg.contains("already running") || msg.contains("Conflict") {
                                                    crate::components::common::toast::show_warning("Auto-search is already running for this season");
                                                } else {
                                                    crate::components::common::toast::show_error(
                                                        format!("Auto search failed: {}", msg),
                                                    );
                                                }
                                            }
                                        }
                                        set_pending_auto_search.set(false);
                                    });
                                }
                            }
                        >
                            <span class="icon text-xs"><AutoSearchIcon/></span>
                            <span class="text-xs">
                                {move || if pending_auto_search.get() { "Searching..." } else { "Auto Search" }}
                            </span>
                        </button>

                        <button
                            class="btn btn-sm btn-ghost flex items-center gap-xs"
                            on:click={
                                let s = season_clone.clone();
                                move |ev| {
                                    ev.stop_propagation();
                                    let raw_season = if s == ABSOLUTE_SEASON_LABEL {
                                        jumbie_shared::mapping::ABSOLUTE_SEASON_NUM.to_string()
                                    } else {
                                        normalize_season(&s)
                                    };
                                    managing_season.set(Some(raw_season));
                                }
                            }
                        >
                            <span class="icon text-xs"><ManageIcon/></span>
                            <span class="text-xs">"Manage"</span>
                        </button>
                            </div>
                        </div>
                    <span
                        class="icon"
                        style={
                            let ie = is_expanded.clone();
                            move || format!("transform: rotate({}deg); transition: transform 0.2s;", if ie() { 180 } else { 0 })
                        }
                    >
                        <ArrowDownIcon/>
                    </span>
                </div>
                <div class="season-content" class:active=is_expanded>
                    <div class="episodes-grid">
                        {move || {
                            // Read the UI config once per render instead of once per
                            // episode (avoids N signal subscriptions and N clones).
                            let ui = ui_config.ui_config.get();
                            let mut eps = eps_memo.get();
                            eps.reverse();
                            eps.into_iter().map(move |ep| {
                                let effective_date = ui
                                    .as_ref()
                                    .and_then(|u| pick_release_date(
                                        &u.release_date_display,
                                        ep.dates.meta_date.as_deref(),
                                        ep.dates.upload_date.as_deref(),
                                        ep.dates.est_date.as_deref(),
                                        &u.time_format,
                                    ))
                                    .map(|p| p.display_date)
                                    .unwrap_or_else(|| "-".to_string());

                                let status_class = episode_status_class(&ep.status, ep.path.is_some(), &ep.dates.meta_date);
                                let opacity = if ep.status == "out_of_range" { "0.3" } else { "1" };

                                let multi_index = ep.path.as_ref().and_then(|p| multiepisode_paths.with(|m| m.get(p).copied()));
                                let paperclip_view = multi_index.map(|idx| {
                                    view! {
                                        <div class="multiepisode-indicator" style=format!("color: var(--multi-color-{})", idx % 18)>
                                            <PaperclipIcon/>
                                        </div>
                                    }
                                });

                                let file_not_found_icon = if episode_is_not_found(ep.assigned, ep.disk_present) {
                                    Some(view! {
                                        <div class="file-not-found-indicator" title="File is missing from disk">
                                            <AlertTriangleIcon/>
                                        </div>
                                    })
                                } else {
                                    None
                                };

                                view! {
                                    <div
                                        class={format!("episode-cell {}{}{}",
                                            status_class,
                                            if ep.monitored { " monitored" } else { "" },
                                            if ep.show_only_downloaded { " show-only-downloaded" } else { "" },
                                        )}
                                        data-episode-id={ep.unique_id.clone()}
                                        style={format!("opacity: {}", opacity)}
                                        title={format!("{} (Released: {}){}", ep.header, effective_date, if ep.show_only_downloaded { " \u{2014} Outside configured range" } else { "" })}
                                        on:click={
                                            let ep = ep.clone();
                                            let cb = on_toggle_episode_monitor.clone();
                                            move |ev| {
                                                if ev.shift_key() {
                                                    if let Some(cb) = &cb {
                                                        cb.run((ep.unique_id.clone(), !ep.monitored));
                                                    }
                                                } else {
                                                    set_selected_episode.set(Some(ep.clone()));
                                                    set_show_modal.set(true);
                                                }
                                            }
                                        }
                                    >
                                        {paperclip_view}
                                        {file_not_found_icon}
                                        {move || ep.episode.to_string()}
                                    </div>
                                }
                            }).collect_view()
                        }}
                    </div>
                </div>
            }
            }
        />
    }
}

/// Poll the backend's auto-season status endpoint until the search completes.
///
/// Exponential backoff: the status endpoint is a lightweight HashMap lookup (no DB
/// queries), so polling is cheap. Early polls use short intervals to catch quick
/// completions; later polls back off to avoid unnecessary traffic while the search is
/// still running (common for 5-minute retry cycles). Polling stops after 30 minutes to
/// prevent infinite loops from a stale entry.
///
/// Visibility-aware pause: when the user switches tab, the loop enters a 1s spin-wait
/// instead of making 3-60s requests the user won't see; normal polling resumes on
/// return, eliminating background network traffic.
pub async fn poll_auto_season_status(series_id: &str, season: &str) {
    let max_polls = 300; // 300 polls with backoff covers ~30 min
    let mut delay_ms = 3_000u64;
    for _ in 0..max_polls {
        // Pause polling while the tab is hidden (user switched away)
        wait_for_visible().await;

        gloo_timers::future::TimeoutFuture::new(delay_ms as u32).await;
        match crate::api::get_auto_season_status(series_id.to_string(), season.to_string()).await {
            Ok(status) => {
                if !status.running {
                    return;
                }
            }
            Err(_) => return, // Stop polling on error
        }
        // Exponential backoff: 3s → 4.5s → 6.75s → ... → max 60s
        delay_ms = (delay_ms * 3 / 2).min(60_000);
    }
}

/// Spin-wait until the browser tab becomes visible again.
///
/// A `visibilitychange` listener would require storing an async waker or channel,
/// which is overkill for polling; a 1s spin-wait is invisible to the user and avoids
/// bridging the event-driven and async worlds.
async fn wait_for_visible() {
    #[cfg(target_arch = "wasm32")]
    {
        use web_sys::window;
        while window()
            .and_then(|w| w.document())
            .is_some_and(|d| d.hidden())
        {
            gloo_timers::future::TimeoutFuture::new(1_000).await;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Non-WASM: always visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // episode_status_class

    #[test]
    fn test_status_class_in_queue() {
        // Episode in queue with no file and any meta_date
        let result = episode_status_class("in_queue", false, &None);
        assert_eq!(result, "in-queue");
    }

    #[test]
    fn test_status_class_in_queue_with_path_should_stay_downloaded() {
        // If an episode somehow has status "in_queue" but also has a path,
        // it should show as "downloaded" (path takes precedence)
        let result = episode_status_class("in_queue", true, &None);
        assert_eq!(result, "downloaded");
    }

    #[test]
    fn test_status_class_downloaded() {
        assert_eq!(
            episode_status_class("downloaded", false, &None),
            "downloaded"
        );
        assert_eq!(
            episode_status_class("organized", false, &None),
            "downloaded"
        );
    }

    #[test]
    fn test_status_class_missing() {
        // Missing with past meta_date (not future)
        let past = Some("2024-01-01 00:00:00".to_string());
        assert_eq!(episode_status_class("missing", false, &past), "missing");
    }

    #[test]
    fn test_status_class_unreleased() {
        assert_eq!(episode_status_class("tba", false, &None), "unreleased");
        assert_eq!(
            episode_status_class("unreleased", false, &None),
            "unreleased"
        );
    }

    #[test]
    fn test_status_class_out_of_range() {
        assert_eq!(
            episode_status_class("out_of_range", false, &None),
            "out-of-range"
        );
    }

    #[test]
    fn test_status_class_future_date_fallback() {
        // Unknown status with far future date → unreleased (date-based fallback)
        let future = Some("2099-06-15 00:00:00".to_string());
        assert_eq!(
            episode_status_class("some_unknown", false, &future),
            "unreleased"
        );
    }

    #[test]
    fn test_status_class_explicit_missing_with_future_date() {
        // Explicit "missing" status is always "missing", even if date is future
        // (the backend determines status; the frontend only derives for fallback statuses)
        let future = Some("2099-06-15 00:00:00".to_string());
        assert_eq!(episode_status_class("missing", false, &future), "missing");
    }

    #[test]
    fn test_status_class_no_date_is_unreleased() {
        // No meta_date and no explicit status → unreleased
        assert_eq!(episode_status_class("", false, &None), "unreleased");
    }

    #[test]
    fn test_status_class_has_path_overrides_anything() {
        // has_path=true always returns "downloaded" regardless of status
        assert_eq!(episode_status_class("missing", true, &None), "downloaded");
        assert_eq!(
            episode_status_class("out_of_range", true, &None),
            "downloaded"
        );
        assert_eq!(
            episode_status_class("unreleased", true, &None),
            "downloaded"
        );
    }

    // episode_is_not_found

    #[test]
    fn test_is_not_found_no_assignment() {
        // No assignment → not not-found even if disk_present is false
        assert!(!episode_is_not_found(false, false));
        assert!(!episode_is_not_found(false, true));
    }

    #[test]
    fn test_is_not_found_assigned_and_present() {
        // Assigned and present on disk → not not-found
        assert!(!episode_is_not_found(true, true));
    }

    #[test]
    fn test_is_not_found_assigned_but_missing() {
        // Assigned but missing from disk → not-found
        assert!(episode_is_not_found(true, false));
    }
}
