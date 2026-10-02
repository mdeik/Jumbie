use crate::components::common::confirmation_modal::ConfirmationModal;
use crate::components::common::icons::*;
use crate::components::common::standard_modal::StandardModal;
use crate::components::edit_series::SearchModal;
use crate::hooks::use_ui_config::{use_time_format, use_ui_config};
use jumbie_shared::formatting::{LabelStyle, fmt_season};
use jumbie_shared::types::EpisodeViewModel;
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

#[component]
pub fn EpisodeDetailsModal(
    show: ReadSignal<bool>,
    set_show: WriteSignal<bool>,
    episode: ReadSignal<Option<EpisodeViewModel>>,
    #[prop(optional, into)] series_id: Option<Signal<String>>,
    #[prop(optional, into)] series_title: Option<Signal<String>>,
    #[prop(optional, into)] season_overrides: Option<
        Signal<Vec<jumbie_shared::types::SeasonOverride>>,
    >,
    /// Series-level search template (series → global for the active mode) that a
    /// per-season override replaces.
    #[prop(optional, into)]
    series_search_format: Option<Signal<String>>,
    #[prop(optional, into)] all_episodes: Option<Signal<Vec<EpisodeViewModel>>>,
    /// Effective numbering mode for the modal's series — drives the manual
    /// search default text (absolute mode emits no season marker).
    #[prop(optional, into)]
    absolute_numbering: Option<Signal<bool>>,
    #[prop(optional)] hide_jump_to_series: Option<bool>,
    /// Fired after every save with `(episode_id, series_id)` — the series_id lets
    /// observing pages refresh from the series-scoped source (SSoT).
    #[prop(optional)]
    on_override_saved: Option<Callback<(String, String)>>,
    /// Resolve metadata provider UUIDs → display names. SSoT: EditSeriesCtx.active_metadata_plugins.
    /// When absent, falls back to `EditSeriesCtx` from context (edit series page) or shows UUID raw.
    #[prop(optional)]
    metadata_plugins: Option<Signal<Vec<jumbie_shared::plugin::PluginInstanceInfo>>>,
    /// Quality profiles to resolve quality_profile_id to display name. SSoT: EditSeriesCtx.quality_profiles.
    /// When absent, falls back to `EditSeriesCtx` from context (edit series page) or shows UUID/raw quality.
    #[prop(optional)]
    quality_profiles: Option<
        Signal<std::collections::HashMap<String, jumbie_shared::types::QualityProfile>>,
    >,
    /// Series-level metadata IDs (plugin UUID → metadata ID). SSoT: EditSeriesCtx.form.
    /// When absent, falls back to `EditSeriesCtx` from context (edit series page).
    /// Used to gate the "Fetch Metadata" button when no ID is configured.
    #[prop(optional)]
    series_metadata_ids: Option<Signal<std::collections::HashMap<String, String>>>,
) -> impl IntoView {
    let config = use_context::<ReadSignal<Option<jumbie_shared::config::Config>>>();

    let (show_search, set_show_search) = signal(false);
    let (show_delete_modal, set_show_delete_modal) = signal(false);
    let (search_query_initial, set_search_query_initial) = signal(String::new());
    let (pending_manual_search, set_pending_manual_search) = signal(false);
    let (pending_auto_search, set_pending_auto_search) = signal(false);

    // Series-level metadata fetch
    let (fetching_series_metadata, set_fetching_series_metadata) = signal(false);

    // Derived: whether a real (non-empty) series_id is available
    let series_id_ready =
        Signal::derive(move || series_id.map(|s| !s.get().is_empty()).unwrap_or(false));

    // Custom metadata editing state
    let (edit_title, set_edit_title) = signal(String::new());
    let (edit_description, set_edit_description) = signal(String::new());
    let (edit_runtime, set_edit_runtime) = signal(String::new());
    let (edit_image_url, set_edit_image_url) = signal(String::new());

    // Guard flag: when set, the debounce auto-save effect will skip saving.
    // Raised before programmatic sync_edit_fields calls (metadata fetch/restore)
    // and lowered after the next reactive cycle so user edits resume working.
    let (suppress_debounce, set_suppress_debounce) = signal(false);

    let ui_config = use_ui_config().ui_config;

    // Reactive time format for local file date display (SSoT)
    let time_format = use_time_format();

    // Format a file timestamp (RFC 3339 UTC from the API) for display in the
    // browser's local timezone, honoring the user's 12h/24h preference.
    let fmt_file_date = move |raw: Option<String>| match raw.as_deref() {
        None | Some("") => "-".to_string(),
        Some(ts) => crate::utils::format_datetime_local(ts, &time_format.get()),
    };

    // FFmpeg availability (from health endpoint)
    let (ffmpeg_installed, set_ffmpeg_installed) = signal(false);
    crate::utils::use_api_cache(
        "fetch_health".to_string(),
        || crate::api::fetch_health(),
        move |h| {
            let _ = set_ffmpeg_installed.try_update(|s| *s = h.ffmpeg_installed);
        },
    );

    // Resolve metadata provider UUIDs → display names.
    // Prefer explicit prop (Calendar/Wanted pages), fall back to EditSeriesCtx context (edit series page).
    let metadata_plugins_signal = metadata_plugins
        .or_else(|| use_context::<super::EditSeriesCtx>().map(|ctx| ctx.active_metadata_plugins));

    // Series-level metadata fetch: resolve plugin name and availability
    let series_metadata_plugin_name = Signal::derive(move || {
        metadata_plugins_signal
            .as_ref()
            .and_then(|plugins| plugins.get().first().map(|p| p.display_name.clone()))
            .unwrap_or_default()
    });
    // Resolve quality profile UUIDs → display names.
    // Prefer explicit prop (Calendar/Wanted pages), fall back to EditSeriesCtx context (edit series page).
    let quality_profiles_signal = quality_profiles
        .or_else(|| use_context::<super::EditSeriesCtx>().map(|ctx| ctx.quality_profiles));

    // Resolve series-level metadata IDs — prefer explicit prop (Calendar/Wanted), fall back to context (edit series).
    let series_metadata_ids_signal = series_metadata_ids.or_else(|| {
        use_context::<super::EditSeriesCtx>()
            .map(|ctx| Signal::derive(move || ctx.form.with(|s| s.metadata_ids.clone())))
    });

    // Whether the first active metadata plugin has a non-empty ID configured on the series.
    // When false, the "Fetch Metadata" button is disabled with a tooltip.
    let has_series_metadata_id = Signal::derive(move || {
        let first_plugin = metadata_plugins_signal
            .as_ref()
            .and_then(|plugins| plugins.get().into_iter().next());
        match first_plugin {
            Some(plugin) => {
                let key = plugin
                    .instance_id
                    .as_deref()
                    .unwrap_or(&plugin.display_name)
                    .to_string();
                series_metadata_ids_signal
                    .as_ref()
                    .map(|ids| ids.get().get(&key).map(|v| !v.is_empty()).unwrap_or(false))
                    .unwrap_or(false)
            }
            None => false,
        }
    });

    // Helper: resolve quality display from quality_profile_id → profile name.
    // Falls back to "-" if no profile is available.
    let quality_display = move |ep: &EpisodeViewModel| {
        if let Some(ref qp_signal) = quality_profiles_signal {
            let profiles = qp_signal.get();
            if let Some(ref qpid) = ep.quality_profile_id
                && !qpid.is_empty()
            {
                return crate::utils::resolve_quality_profile_name(qpid, &profiles);
            }
        }
        "-".to_string()
    };

    // Small helpers used by effects and button handlers below
    let get_sid = move || {
        series_id
            .and_then(|s| {
                let id = s.get();
                if id.is_empty() { None } else { Some(id) }
            })
            .unwrap_or_default()
    };

    let sync_edit_fields = move |ep: &EpisodeViewModel| {
        set_edit_title.set(ep.title.clone().unwrap_or_default());
        set_edit_description.set(ep.description.clone().unwrap_or_default());
        set_edit_runtime.set(ep.runtime.map(|r| r.to_string()).unwrap_or_default());
        set_edit_image_url.set(ep.image_url.clone().unwrap_or_default());
    };

    // Wrapper: raise the suppress flag, sync, then lower it after a microtask
    // so the debounce effect sees the flag and skips saving programmatic updates.
    let sync_edit_fields_programmatic = {
        let sync = sync_edit_fields.clone();
        move |ep: &EpisodeViewModel| {
            set_suppress_debounce.set(true);
            sync(ep);
            // Lower on next tick so queued reactive effects see the raised flag.
            leptos::task::spawn_local(async move {
                // Wait a tick for the reactive system to process the suppress flag.
                // Using a 0ms timeout as a portable yield-between-microtasks pattern.
                gloo_timers::future::TimeoutFuture::new(0).await;
                set_suppress_debounce.set(false);
            });
        }
    };

    // Local writable copy of the episode for immediate display updates on save/clear.
    // The parent's `form_initialized` guard prevents the prop signal from updating
    // after `on_override_saved` → refetch completes, so we manage our own copy.
    let (display_episode, set_display_episode) = signal(None::<EpisodeViewModel>);

    // SSoT: single place that invalidates the series cache and fires the parent callback.
    // All 7 call sites in this component must use this instead of calling cb.run(()) directly.
    // This ensures the parent never needs to invalidate the individual episode cache —
    // each parent only handles its own collection-level refresh in on_override_saved.
    let fire_on_override_saved = move || {
        let ep_id = display_episode
            .get_untracked()
            .or_else(|| episode.get_untracked())
            .map(|ep| ep.unique_id.clone());

        let sid = get_sid();
        // The per-episode details cache is no longer read by any caller — the
        // series-scoped cache is the source of truth for modal data.
        if ep_id.is_some() && !sid.is_empty() {
            crate::utils::invalidate_cache_key(&format!("fetch_series_details_{}", sid));
        }

        if let (Some(cb), Some(eid)) = (&on_override_saved, ep_id) {
            cb.run((eid, sid));
        }
    };

    // Shared: refresh display_episode from server after metadata change
    // Used by both the "Match to Provider" button and the "Fetch Metadata" button
    // to show updated data immediately instead of requiring close/reopen.
    let refresh_display_episode = move || {
        let sid = get_sid();
        if sid.is_empty() {
            return;
        }
        let current_eid = display_episode
            .with_untracked(|opt| opt.as_ref().map(|e| e.unique_id.clone()))
            .or_else(|| episode.get_untracked().map(|e| e.unique_id));
        let set_disp = set_display_episode;
        leptos::task::spawn_local(async move {
            if let Ok(Some(details)) = crate::api::fetch_series_details(sid).await
                && let Some(ref eid) = current_eid
                && let Some(updated) = details.episodes.iter().find(|e| e.unique_id == *eid)
            {
                set_disp.set(Some(updated.clone()));
                sync_edit_fields_programmatic(&updated);
            }
        });
    };

    // Metadata fetch completion coordinator
    // Signal + Effect pattern: The spawn_local inside on:click runs after .await
    // without a reactive owner, so any call to use_context/expect_context panics.
    // Instead, the async task stores its result here, and the Effect below reacts
    // in the proper reactive scope to show toasts and call refresh/save callbacks.
    let (fetch_result, set_fetch_result) = signal(Option::<Result<(), String>>::None);
    Effect::new(move |_| {
        if let Some(result) = fetch_result.get() {
            match result {
                Ok(()) => {
                    crate::components::common::toast::show_success(
                        "Metadata fetched and applied to this episode.",
                    );
                    refresh_display_episode();
                    fire_on_override_saved();
                }
                Err(msg) => {
                    if msg.starts_with("Metadata synced") {
                        // sync succeeded but restore failed — still refresh
                        crate::components::common::toast::show_warning(msg);
                        refresh_display_episode();
                        fire_on_override_saved();
                    } else {
                        crate::components::common::toast::show_error(msg);
                    }
                }
            }
            set_fetch_result.set(None);
        }
    });

    // Reactive release date display
    // Memo that recomputes whenever the episode OR ui_config changes.
    // This ensures reordering priorities in General Settings takes effect
    // immediately in the open modal without requiring a refresh.
    // SSoT — delegates to `crate::utils::release_date::pick_release_date`.
    // Returns the full PickedReleaseDate so callers can read p.source
    // (needed by the cycling logic) without re-running selection.
    let release_date_display = Memo::new(move |_| {
        let ui = ui_config.get();
        let ep = display_episode.get();
        let ep_ref = match ep.as_ref() {
            Some(e) => e,
            None => return None,
        };
        let config = match ui.as_ref() {
            Some(c) => &c.release_date_display,
            None => return None,
        };
        let tf = &ui
            .as_ref()
            .map(|u| u.time_format.clone())
            .unwrap_or_default();
        crate::utils::release_date::pick_release_date(
            config,
            ep_ref.dates.meta_date.as_deref(),
            ep_ref.dates.upload_date.as_deref(),
            ep_ref.dates.est_date.as_deref(),
            tf,
        )
    });

    // Release date cycling state
    // Clicking the release date cycles: effective date → remaining enabled
    // sources in config order → back to effective date.
    // `None` = show effective date (current behavior).
    let (cycle_source, set_cycle_source) = signal(None::<jumbie_shared::types::ReleaseDateSource>);

    // File Path cycling: click to reveal the original content path.
    let (show_original_path, set_show_original_path) = signal(false);
    // Part File Path cycling: part numbers currently showing their original path.
    let (show_part_original, set_show_part_original) =
        signal(std::collections::HashSet::<u32>::new());

    // The date/source cycle and the original-path reveals are component-local view
    // state, so — unlike the fields carried on `display_episode` (which all switch
    // together when the modal opens a different episode) — they must be reset
    // explicitly. Reset on close and whenever the episode identity changes, but not
    // on same-episode refreshes (post-save payloads, cache revalidation) so the
    // user's chosen view survives a background update.
    let viewed_episode_id = StoredValue::new_local(None::<String>);
    Effect::new(move |_| {
        let current_id = episode.get().map(|e| e.unique_id);
        if !show.get() || current_id != viewed_episode_id.get_value() {
            set_cycle_source.set(None);
            set_show_original_path.set(false);
            set_show_part_original.set(std::collections::HashSet::new());
            viewed_episode_id.set_value(current_id);
        }
    });

    // Enabled sources in config priority order — filtered by user preferences.
    let enabled_sources = Signal::derive(move || {
        let ui = ui_config.get();
        let config = match ui.as_ref() {
            Some(c) => &c.release_date_display,
            None => return Vec::new(),
        };
        config
            .order
            .iter()
            .filter_map(|s| {
                let parsed = s.parse::<jumbie_shared::types::ReleaseDateSource>().ok()?;
                match parsed {
                    jumbie_shared::types::ReleaseDateSource::MetaDate
                        if config.metadata_enabled =>
                    {
                        Some(parsed)
                    }
                    jumbie_shared::types::ReleaseDateSource::UploadDate
                        if config.source_enabled =>
                    {
                        Some(parsed)
                    }
                    jumbie_shared::types::ReleaseDateSource::EstDate
                        if config.estimated_enabled =>
                    {
                        Some(parsed)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
    });

    // Cycle order: walks through `enabled_sources` circularly starting from
    // the effective source's position.  Example: config [source, estimated,
    // metadata], dates [estimated, metadata] → cycle [estimated, metadata,
    // source].  The effective date is always first, so the first click always
    // shows the *next* enabled source in config order (not a duplicate).
    let cycle_order = Signal::derive(move || {
        let effective = release_date_display.get();
        let enabled = enabled_sources.get();
        let Some(picked) = effective.as_ref() else {
            // all disabled → nothing to cycle through
            return Vec::new();
        };
        let Some(first_pos) = enabled.iter().position(|s| *s == picked.source) else {
            // effective source not in enabled list (shouldn't happen), fall
            // back to plain enabled list
            return enabled;
        };
        let len = enabled.len();
        let mut order = Vec::with_capacity(len);
        for i in 0..len {
            let idx = (first_pos + i) % len;
            order.push(enabled[idx]);
        }
        order
    });

    // Final display: effective date or cycle override
    // Formatting delegates to crate::utils::release_date::format_source_date (SSoT).
    let release_date_final = Memo::new(move |_| {
        let effective = release_date_display.get();
        let ov = cycle_source.get();
        let tf = time_format.get();

        match (effective, ov) {
            // All sources disabled — nothing to show
            (None, _) => None,
            // Effective date (no cycle active)
            (Some(picked), None) => Some((picked.label.to_string(), picked.display_date)),
            // Cycle override — show that specific source
            (Some(_), Some(source)) => {
                let ep = display_episode.get();
                ep.as_ref().map(|ep_ref| {
                    let result = crate::utils::release_date::format_source_date(
                        source,
                        ep_ref.dates.meta_date.as_deref(),
                        ep_ref.dates.upload_date.as_deref(),
                        ep_ref.dates.est_date.as_deref(),
                        &tf,
                    );
                    (result.label.to_string(), result.display_date)
                })
            }
        }
    });

    // Click handler: cycle through [effective, ...remaining_enabled_sources]
    let cycle_next_date = move |_| {
        let order = cycle_order.get();
        if order.len() <= 1 {
            return; // nothing to cycle
        }
        match cycle_source.get_untracked() {
            None => set_cycle_source.set(Some(order[1])),
            Some(current) => {
                if let Some(pos) = order.iter().position(|s| *s == current) {
                    if pos + 1 < order.len() {
                        set_cycle_source.set(Some(order[pos + 1]));
                    } else {
                        // Wrap back to effective date
                        set_cycle_source.set(None);
                    }
                } else {
                    // Current source no longer in cycle (config changed mid-cycle)
                    set_cycle_source.set(Some(order[1]));
                }
            }
        }
    };
    Effect::new(move |_| {
        if let Some(ep) = episode.get() {
            set_display_episode.set(Some(ep));
        }
    });

    let debounce_timer = StoredValue::new_local(None::<TimeoutHandle>);

    // Auto-save custom metadata with debounce when title changes
    Effect::new(move |_| {
        let _ = edit_title.get();
        let _ = on_override_saved;

        if let Some(handle) = *debounce_timer.read_value() {
            handle.clear();
        }

        // Skip save if a programmatic metadata operation is in progress
        if suppress_debounce.get_untracked() {
            return;
        }

        // Skip save if none of the edit fields differ from the episode's original values
        let Some(ref ep) = episode.get_untracked() else {
            return;
        };
        let title_match = edit_title.get_untracked() == ep.title.clone().unwrap_or_default();
        let desc_match =
            edit_description.get_untracked() == ep.description.clone().unwrap_or_default();
        let rt_match =
            edit_runtime.get_untracked() == ep.runtime.map(|r| r.to_string()).unwrap_or_default();
        let img_match = edit_image_url.get_untracked() == ep.image_url.clone().unwrap_or_default();
        if title_match && desc_match && rt_match && img_match {
            return; // Nothing changed, skip
        }

        let sid = get_sid();
        if sid.is_empty() {
            return;
        }
        let eid = ep.unique_id.clone();
        let set_disp = set_display_episode;

        // Only send fields that actually changed — never blindly round-trip
        // meta_date, description, etc. when only the title was edited.
        let handle_result = set_timeout_with_handle(
            move || {
                let mut payload = serde_json::Map::new();
                let new_title = edit_title.get_untracked();
                if !title_match {
                    payload.insert("title".to_string(), serde_json::Value::String(new_title));
                }
                if !desc_match {
                    let desc = edit_description.get_untracked();
                    if !desc.is_empty() {
                        payload.insert("description".to_string(), serde_json::Value::String(desc));
                    }
                }
                if !rt_match {
                    let rt_str = edit_runtime.get_untracked();
                    if let Ok(rt) = rt_str.parse::<i32>() {
                        payload.insert("runtime".to_string(), serde_json::Value::Number(rt.into()));
                    }
                }
                if !img_match {
                    let img = edit_image_url.get_untracked();
                    if !img.is_empty() {
                        payload.insert("image_url".to_string(), serde_json::Value::String(img));
                    }
                }

                crate::utils::spawn_api_toast_with_level(
                    crate::api::save_episode_custom_metadata(
                        sid.clone(),
                        eid.clone(),
                        serde_json::Value::Object(payload),
                    ),
                    None,
                    move |_| {
                        set_disp.update(|opt_ep| {
                            if let Some(ep) = opt_ep {
                                ep.metadata_source = Some("custom".to_string());
                            }
                        });
                        fire_on_override_saved();
                    },
                    crate::components::common::toast::NotificationType::Error,
                );
            },
            std::time::Duration::from_millis(500),
        );

        if let Ok(handle) = handle_result {
            *debounce_timer.write_value() = Some(handle);
        }
    });

    Effect::new(move |_| {
        if let Some(ep) = episode.get() {
            sync_edit_fields(&ep);
        }
    });

    // The manual Search box is seeded with `{title} {rendered search key}`.
    // SSoT: `episode_search`.
    let manual_search_query = move || {
        let Some(ep) = episode.get() else {
            return String::new();
        };
        let title = series_title.map(|s| s.get()).unwrap_or_default();
        let overrides = season_overrides.map(|s| s.get()).unwrap_or_default();
        let series_search_format = series_search_format.map(|s| s.get()).unwrap_or_default();
        let is_absolute = absolute_numbering.map(|s| s.get()).unwrap_or(false);

        crate::utils::resolve_manual_search_query(
            &ep,
            &title,
            &overrides,
            &series_search_format,
            is_absolute,
        )
    };

    // Dispatch pending searches when series_id becomes available (e.g. after
    // the background fetch in wanted.rs completes).
    Effect::new(move |_| {
        if !series_id_ready.get() {
            return;
        }

        if pending_manual_search.get_untracked() {
            set_pending_manual_search.set(false);
            set_search_query_initial.set(manual_search_query());
            set_show_search.set(true);
        }

        if pending_auto_search.get_untracked() {
            set_pending_auto_search.set(false);
            // Auto-search is alias-aware and built backend-side — the frontend
            // supplies only the episode.
            let (Some(sid), Some(ep)) = (series_id.map(|s| s.get()), episode.get()) else {
                return;
            };
            let season_str = ep.season.clone();
            let episode_num = ep.episode;
            let ep_name = ep.header.clone();
            leptos::task::spawn_local(async move {
                match crate::api::auto_search_episode(sid, season_str, vec![episode_num], true)
                    .await
                {
                    Ok(results) => {
                        if let Some(item) = results.first() {
                            if item.queue_action == "skipped" {
                                crate::components::common::toast::show_info(format!(
                                    "Not queued: {}",
                                    item.title
                                ));
                            } else {
                                crate::components::common::toast::show_success(format!(
                                    "Download started: {}",
                                    item.title
                                ));
                            }
                        } else {
                            crate::components::common::toast::show_warning(format!(
                                "No results found for {}",
                                ep_name
                            ));
                        }
                    }
                    Err(e) => crate::components::common::toast::show_error(format!(
                        "Auto search failed: {}",
                        e
                    )),
                }
            });
        }
    });

    // Keep display_episode in sync when series details refetch
    // (15-second interval, on_override_saved → refetch, etc.).
    // Without this, the open modal would only see the one-time snapshot
    // from the initial open or from refresh_display_episode().
    // Only track all_episodes — the sibling Effect above (L243) already
    // handles updating display_episode when the episode prop changes.
    Effect::new(move |_| {
        if let Some(ref all_eps_sig) = all_episodes {
            let all_eps = all_eps_sig.get();
            let current_id =
                display_episode.with_untracked(|opt| opt.as_ref().map(|e| e.unique_id.clone()));
            if let Some(ref eid) = current_id
                && let Some(updated) = all_eps.iter().find(|e| e.unique_id == *eid)
            {
                set_display_episode.set(Some(updated.clone()));
            }
        }
    });

    // Reset pending state when modal closes or episode changes so stale
    // searches from a previous episode never fire.
    Effect::new(move |_| {
        show.get();
        episode.get();
        set_pending_manual_search.set(false);
        set_pending_auto_search.set(false);
    });

    let refresh_rename_queue = use_context::<crate::hooks::RenameQueueRefresh>();

    let scan_action = crate::utils::create_api_action(
        move |ep_id: &String| crate::api::scan_episode_media(ep_id.clone()),
        Some("Media info scanned."),
    );

    Effect::new(move |_| {
        if scan_action.value().get().is_some() {
            fire_on_override_saved();
        }
    });

    let is_scanning_media = scan_action.pending();

    let scan_media_info = move |_| {
        if is_scanning_media.get() {
            return;
        }

        if let Some(ep) = episode.get() {
            crate::components::common::toast::show_info("Scanning media info...");
            scan_action.dispatch(ep.unique_id.clone());
        }
    };

    // Delete file action
    let delete_file_action = crate::utils::create_api_action(
        move |(series_id, paths): &(String, Vec<String>)| {
            let payload = jumbie_shared::types::BatchDeletePayload {
                paths: paths.clone(),
            };
            crate::api::delete_series_files(series_id.clone(), payload)
        },
        Some("File deleted."),
    );

    Effect::new(move |_| {
        if delete_file_action.value().get().is_some() {
            // Don't close the modal — let the user see the updated status.
            //
            // Clear file-related fields on the local display copy so the badge
            // updates instantly.  The derive rules are:
            //   — no file_path + no parts → status becomes "missing" (meta_date is past)
            //   — file-related fields clear (quality, size, media_info, etc.)
            //   — metadata fields (metadata_ids, metadata_source, title, etc.) keep
            // This is safe because the backend derives status from file presence;
            // the refetch triggered by `fire_on_override_saved` reconciles with
            // the server.
            if let Some(mut ep) = display_episode.get_untracked() {
                ep.path = None;
                ep.parts = Vec::new();
                ep.size = 0;
                ep.quality_profile_id = None;
                ep.submitter = None;
                ep.media_info = None;
                ep.fingerprint = None;
                ep.created_at = None;
                ep.file_acquired_at = None;
                ep.status = "missing".to_string();
                // Also patch the calendar cache so the calendar view reflects
                // the file deletion instantly without a re-fetch.
                let config = ui_config.get();
                if let Some(u) = config.as_ref() {
                    let tf = time_format.get();
                    let s_title = series_title.clone().map(|s| s.get()).unwrap_or_default();
                    crate::utils::patch_calendar_cache(&ep, &s_title, &u.release_date_display, &tf);
                }
                set_display_episode.set(Some(ep));
            }
            fire_on_override_saved();
            if let Some(r) = refresh_rename_queue {
                r.1.run(());
            }
        }
    });

    let is_deleting_file = delete_file_action.pending();

    // Store pending delete info so ConfirmationModal can access it
    let (pending_delete_s_id, set_pending_delete_s_id) = signal(String::new());
    let (pending_delete_paths, set_pending_delete_paths) = signal(Vec::<String>::new());
    let (pending_delete_msg, set_pending_delete_msg) = signal(String::new());

    let delete_episode_file = move |_| {
        if is_deleting_file.get() {
            return;
        }

        let ep = if let Some(e) = episode.get() {
            e
        } else {
            return;
        };

        let s_id = get_sid();
        if s_id.is_empty() {
            crate::components::common::toast::show_error("No series ID available");
            return;
        }

        let mut paths: Vec<String> = ep.path.into_iter().collect();
        for part in &ep.parts {
            paths.push(part.file_path.clone());
        }

        if paths.is_empty() {
            crate::components::common::toast::show_error("No files to delete");
            return;
        }

        let file_count = paths.len();
        let msg = if file_count == 1 {
            "Delete this episode's file from disk? Its subtitles and metadata are removed too. This cannot be undone.".to_string()
        } else {
            format!(
                "Delete this episode's {} files from disk? Its subtitles and metadata are removed too. This cannot be undone.",
                file_count
            )
        };

        set_pending_delete_s_id.set(s_id);
        set_pending_delete_paths.set(paths);
        set_pending_delete_msg.set(msg);
        set_show_delete_modal.set(true);
    };

    let handle_delete_confirm = move || {
        let s_id = pending_delete_s_id.get();
        let paths = pending_delete_paths.get();
        if !paths.is_empty() && !s_id.is_empty() {
            delete_file_action.dispatch((s_id, paths));
        }
    };

    // Local view helpers (avoid repeating label/value structure)
    let info_section = |label: &'static str, value: String| {
        view! {
            <div class="info-section">
                <div class="info-label">{label}</div>
                <div class="info-value">{value}</div>
            </div>
        }
    };

    let info_section_cls = |label: &'static str, value: String, extra: &'static str| {
        view! {
            <div class={format!("info-section {}", extra)}>
                <div class="info-label">{label}</div>
                <div class="info-value path">{value}</div>
            </div>
        }
    };

    let info_row = |l1: &'static str, v1: String, l2: &'static str, v2: String| {
        view! {
            <div class="info-row">
                <div>
                    <div class="info-label">{l1}</div>
                    <div class="info-value">{v1}</div>
                </div>
                <div>
                    <div class="info-label">{l2}</div>
                    <div class="info-value">{v2}</div>
                </div>
            </div>
        }
    };

    let media_info_item = |label: &'static str, value: String, title: Option<String>| {
        view! {
            <div class="media-info-item">
                <div class="media-info-label">{label}</div>
                <div class="media-info-value" title=title>{value}</div>
            </div>
        }
    };

    // Helper: render a media-info grid from an optional MediaInfo reference
    // (replaces the 6-item inline blocks that were duplicated for episode + parts)
    let build_media_info_grid = move |info: &Option<jumbie_shared::types::MediaInfo>| {
        info.as_ref().map(|info| {
            let audio_val = info.audio.clone().unwrap_or_else(|| "-".to_string());
            let sub_val = info.subtitles.clone().unwrap_or_else(|| "-".to_string());
            let audio_title = if info.audio_languages.is_empty() {
                None
            } else {
                Some(info.audio_languages.join(", ").to_uppercase())
            };
            let sub_title = if info.subtitle_languages.is_empty() {
                None
            } else {
                Some(info.subtitle_languages.join(", ").to_uppercase())
            };
            // For single-track streams, show the language inline (usually 1 language, compact).
            // Multi-track streams use the count format, so languages live in the title tooltip.
            let audio_display = match (&audio_title, info.audio_track_count) {
                (Some(langs), 1) => format!("{} ({})", audio_val, langs),
                _ => audio_val,
            };
            let sub_display = match (&sub_title, info.subtitle_track_count) {
                (Some(langs), 1) => format!("{} ({})", sub_val, langs),
                _ => sub_val,
            };
            view! {
                {media_info_item("Codec",      info.codec.clone().unwrap_or_else(|| "-".to_string()), None)}
                {media_info_item("Resolution", info.resolution.clone().unwrap_or_else(|| "-".to_string()), None)}
                {media_info_item("Bitrate",    info.bitrate.clone().unwrap_or_else(|| "-".to_string()), None)}
                {media_info_item("Duration",   info.duration.clone().unwrap_or_else(|| "-".to_string()), None)}
                {media_info_item("Audio",      audio_display, audio_title)}
                {media_info_item("Subtitles",  sub_display, sub_title)}
            }
        })
    };

    // Shared media-info scan section (avoids duplicating the Show/header/button skeleton).
    // - Config toggle gates the entire section (user preference to hide media info).
    // - Scan/re-scan button is additionally gated on ffmpeg being installed.
    // - Existing media info grid is always shown when available (it's still valid data).
    macro_rules! media_info_section {
        ($grid:expr, $compact:expr, $has_info:expr) => {
            {
                let grid_class = if $compact {
                    "media-info-grid media-info-grid--compact mt-sm"
                } else {
                    "media-info-grid"
                };
                let button_class = if $has_info {
                    "btn btn-sm btn-ghost mb-sm"
                } else {
                    "btn btn-sm btn-secondary mb-sm"
                };
                view! {
                    // Show the section if the config allows it AND (there's data to display OR ffmpeg is available to scan).
                    // When ffmpeg is missing and no data exists, the section would be an empty header — hide it.
                    <Show when=move || (config.and_then(|sig| sig.get()).map(|c| c.general.media_info_scan_enabled).unwrap_or(true)) && ($has_info || ffmpeg_installed.get())>
                        <div class="info-section">
                            <div class="flex items-center gap-sm mb-sm">
                                <div class="info-label mb-none">"Media Information"</div>
                                <Show when=move || ffmpeg_installed.get()>
                                    <button class={button_class} on:click=scan_media_info disabled=is_scanning_media>
                                        <span class="icon">
                                            {if $has_info { view! { <RefreshIcon/> }.into_any() } else { view! { <SearchIcon/> }.into_any() }}
                                        </span>
                                        {move || if is_scanning_media.get() { "Scanning..." } else if $has_info { "Re-scan" } else { "Scan" }}
                                    </button>
                                </Show>
                            </div>
                            {($has_info).then(|| view! {
                                <div class={grid_class}>
                                    {$grid}
                                </div>
                            })}
                        </div>
                    </Show>
                }
            }
        };
    }

    view! {
        <StandardModal
            show=Signal::from(show)
            on_close=move |_| set_show.set(false)
            title=move || series_title.clone().map(|s| s.get()).unwrap_or_default()
            subtitle="Episode Details"
            size="modal-min-height-70"
            footer=view! {
                <div class="modal-footer justify-between episode-details-footer">
                    // Left side — destructive action + jump to series
                    <div class="flex gap-md">
                        <Show when=move || {
                            display_episode.get().map(|ep| ep.status == "downloaded" || ep.status == "organized").unwrap_or(false)
                        }>
                            <button class="btn btn-danger" on:click=delete_episode_file disabled=is_deleting_file>
                                <span class="icon"><TrashIcon/></span> {move || if is_deleting_file.get() { "Deleting..." } else { "Episode" }}
                            </button>
                        </Show>
                        <Show when=move || {
                            !hide_jump_to_series.unwrap_or(false)
                                && series_id.as_ref().map(|s| !s.get().is_empty()).unwrap_or(false)
                        }>
                            <button class="btn btn-primary" on:click=move |_| {
                                if let Some(sid) = series_id.as_ref() {
                                    let id = sid.get();
                                    if !id.is_empty() {
                                        use_navigate()(&format!("/series/{}/edit", id), Default::default());
                                    }
                                }
                            }>
                                "Jump to Series"
                            </button>
                        </Show>
                    </div>
                    // Right side — metadata & search actions
                    <div class="flex gap-md">
                        <button class="btn btn-primary"
                            disabled=move || pending_manual_search.get()
                            on:click=move |_| {
                                if series_id_ready.get() {
                                    set_search_query_initial.set(manual_search_query());
                                    set_show_search.set(true);
                                } else {
                                    set_pending_manual_search.set(true);
                                }
                            }
                        >
                            <span class="icon"><SearchIcon/></span>
                            {move || if pending_manual_search.get() { "Pending..." } else { "Search" }}
                        </button>
                        <button class="btn btn-secondary"
                            disabled=move || pending_auto_search.get()
                            on:click=move |_| {
                                if series_id_ready.get() {
                                    if let Some(ep) = episode.get() {
                                        let s_id = series_id.map(|s| s.get());
                                        if let Some(sid) = s_id {
                                            set_pending_auto_search.set(true);
                                            let season_str = ep.season.clone();
                                            let episode_num = ep.episode;
                                            let ep_name = ep.header.clone();

                                            leptos::task::spawn_local({
                                                let sid = sid.clone();
                                                async move {
                                                    match crate::api::auto_search_episode(
                                                        sid,
                                                        season_str,
                                                        vec![episode_num],
                                                        true, // is_user_requested
                                                    )
                                                    .await
                                                    {
                                                        Ok(results) => {
                                                            if let Some(item) = results.first() {
                                                                match item.queue_action.as_str() {
                                                                    "added" | "replaced" | "merged" => {
                                                                        crate::components::common::toast::show_success(
                                                                            format!(
                                                                                "Download started: {}",
                                                                                item.title
                                                                            ),
                                                                        );
                                                                    }
                                                                    "skipped" => {
                                                                        crate::components::common::toast::show_info(
                                                                            format!(
                                                                                "Not queued: {} — better release already queued or downloaded",
                                                                                item.title
                                                                            ),
                                                                        );
                                                                    }
                                                                    _ => {
                                                                        crate::components::common::toast::show_success(
                                                                            format!(
                                                                                "Download started: {}",
                                                                                item.title
                                                                            ),
                                                                        );
                                                                    }
                                                                }
                                                            } else {
                                                                crate::components::common::toast::show_warning(
                                                                    format!(
                                                                        "No results found for {}",
                                                                        ep_name
                                                                    ),
                                                                );
                                                            }
                                                            if let Some(ctx) = use_context::<super::EditSeriesCtx>() {
                                                                ctx.series_details.refetch();
                                                            }
                                                        }
                                                        Err(e) => {
                                                            crate::components::common::toast::show_error(
                                                                format!("Auto search failed: {}", e),
                                                            );
                                                        }
                                                    }
                                                    set_pending_auto_search.set(false);
                                                }
                                            });
                                        }
                                    }
                                } else {
                                    set_pending_auto_search.set(true);
                                }
                            }
                        >
                            <span class="icon"><AutoSearchIcon/></span>
                            {move || if pending_auto_search.get() { "Pending..." } else { "Auto Search" }}
                        </button>
                    </div>
                </div>
            }.into_any()
        >
            {move || display_episode.get().map(|ep| view! {
                // Status + Monitored (full width) — uses the SSoT episode_status_class function
                <div class="info-section">
                    <div class="info-label">"Status"</div>
                    <div class="flex gap-xs items-center">
                        <span
                            title={{
                                let nf = crate::components::edit_series::season_accordion::episode_is_not_found(
                                    ep.assigned, ep.disk_present,
                                );
                                if nf {
                                    "File missing from disk".to_string()
                                } else {
                                    format!("Episode is {}", crate::components::edit_series::season_accordion::episode_status_label(&ep.status))
                                }
                            }}
                            class={{
                                let c = crate::components::edit_series::season_accordion::episode_status_class(
                                    &ep.status,
                                    ep.path.is_some(),
                                    &ep.dates.meta_date,
                                );
                                let nf = crate::components::edit_series::season_accordion::episode_is_not_found(
                                    ep.assigned, ep.disk_present,
                                );
                                format!("btn status-badge {} {}", c, if nf { "not-found" } else { "" })
                            }}
                        >
                            {if crate::components::edit_series::season_accordion::episode_is_not_found(ep.assigned, ep.disk_present) {
                                "Not Found".to_string()
                            } else {
                                crate::components::edit_series::season_accordion::episode_status_label(&ep.status).to_string()
                            }}
                        </span>
                        {
                            let ep_monitored = ep.monitored;
                            let eid = ep.unique_id.clone();
                            let set_disp = set_display_episode;
                            let fire_saved = fire_on_override_saved;
                            view! {
                                <span class={
                                    if ep_monitored {
                                        "btn status-badge monitored"
                                    } else {
                                        "btn status-badge unmonitored"
                                    }
                                }
                                    title=move || if ep_monitored { "Click to unmonitor" } else { "Click to monitor" }
                                    on:click=move |ev| {
                                        ev.stop_propagation();
                                        let new_val = !ep_monitored;
                                        // Optimistic update so the badge flips immediately
                                        set_disp.update(|opt| {
                                            if let Some(e) = opt {
                                                e.monitored = new_val;
                                            }
                                        });
                                        let captured_eid = eid.clone();
                                        leptos::task::spawn_local(async move {
                                            if let Err(err) = crate::api::toggle_episode_monitor(captured_eid, new_val).await {
                                                crate::components::common::toast::show_error(
                                                    format!("Failed to toggle monitoring: {}", err.user_message()),
                                                );
                                                // Revert optimistic update on failure
                                                set_disp.update(|opt| {
                                                    if let Some(e) = opt {
                                                        e.monitored = !new_val;
                                                    }
                                                });
                                            }
                                        });
                                        // Invalidate caches so other views (accordion, calendar) reflect the change
                                        fire_saved();
                                    }
                                >
                                    {if ep_monitored { "monitored" } else { "unmonitored" }}
                                </span>
                            }
                        }
                        {
                            let source = ep.metadata_source.as_deref();
                            match source {
                                Some("custom") => view! { <span class="btn status-badge badge-blue" title="Custom metadata">"custom"</span> }.into_any(),
                                Some("cleared") => view! { <span>""</span> }.into_any(),
                                Some(uuid) => {
                                    // Resolve UUID to a human-readable provider name
                                    let name = metadata_plugins_signal
                                        .as_ref()
                                        .and_then(|plugins| {
                                            plugins
                                                .get()
                                                .into_iter()
                                                .find(|p| p.instance_id.as_deref() == Some(uuid))
                                                .map(|p| p.display_name.clone())
                                        })
                                        .unwrap_or_else(|| uuid.to_string());
                                    let name_lower = name.to_lowercase();
                                    view! {
                                        {move || {
                                            let can_fetch = has_series_metadata_id.get();
                                            let is_fetching = fetching_series_metadata.get();
                                            let pn = series_metadata_plugin_name.get();
                                            if can_fetch && !is_fetching {
                                                view! {
                                                    <button class="btn status-badge badge-green"
                                                        title=format!("Fetch {} metadata", pn)
                                                        on:click=move |_| {
                                                            let sid = get_sid();
                                                            if sid.is_empty() { return; }
                                                            let ep_id = display_episode.with_untracked(|opt| {
                                                                opt.as_ref().map(|e| e.unique_id.clone())
                                                            });
                                                            set_fetching_series_metadata.set(true);
                                                            leptos::task::spawn_local(async move {
                                                                let result = match crate::api::sync_metadata(sid.clone(), None).await {
                                                                    Ok(_) => {
                                                                        match ep_id.as_ref() {
                                                                            Some(eid) => {
                                                                                match crate::api::restore_episode_metadata(sid, eid.clone()).await {
                                                                                    Ok(_) => Ok(()),
                                                                                    Err(e) => {
                                                                                        Err(format!("Metadata synced, but failed to apply: {}", e.user_message()))
                                                                                    }
                                                                                }
                                                                            }
                                                                            None => Ok(()),
                                                                        }
                                                                    }
                                                                    Err(e) => {
                                                                        Err(format!("Failed to fetch metadata: {}", e.user_message()))
                                                                    }
                                                                };
                                                                set_fetch_result.set(Some(result));
                                                                set_fetching_series_metadata.set(false);
                                                            });
                                                        }
                                                    >
                                                        {name_lower.clone()}
                                                    </button>
                                                }.into_any()
                                            } else if can_fetch && is_fetching {
                                                view! { <button class="btn status-badge badge-green" disabled title="">{name_lower.clone()}</button> }.into_any()
                                            } else {
                                                view! { <span class="btn status-badge badge-green">{name_lower.clone()}</span> }.into_any()
                                            }
                                        }}
                                    }.into_any()
                                                                    }
                                None => view! { <span></span> }.into_any(),
                            }
                        }
                    </div>
                </div>

                // Episode + Title
                {info_section("Episode", {
                    let season_num = jumbie_shared::mapping::parse_season_num(&ep.season).unwrap_or(0);
                    format!("{}, {}", fmt_season(season_num, LabelStyle::Human), ep.header)
                })}

                // Title — editable for all episodes
                {
                    // Show editable title for all episodes (downloaded, missing, upcoming)
                    view! {
                        <div class="info-section">
                            <div class="info-label">"Title"</div>
                            <div class="flex gap-sm items-center flex-wrap">
                                <input type="text" id="ep-title-field" class="input flex-1" placeholder="Empty" prop:value=edit_title on:input=move |ev| set_edit_title.set(event_target_value(&ev)) />
                            </div>
                            // Metadata action buttons
                            {
                                // Cancel existing debounce so a pending auto-save doesn't
                                // race with the explicit clear or restore.
                                if let Some(handle) = *debounce_timer.read_value() {
                                    handle.clear();
                                }
                                let episode_id_for_clear = ep.unique_id.clone();
                                let episode_id_for_restore = ep.unique_id.clone();

                                view! {
                                    <div class="flex gap-md mt-sm items-center flex-wrap">
                                        <button
                                            class="btn btn-sm btn-ghost"
                                            on:click=move |_| {
                                                let sid = get_sid();
                                                let eid = episode_id_for_clear.clone();
                                                let set_disp = set_display_episode;
                                                crate::utils::spawn_api_toast(
                                                    crate::api::clear_episode_custom_metadata(sid, eid),
                                                    Some("Metadata cleared."),
                                                    move |_| {
                                                        // Update local copy and edit fields so badge and display fields update immediately
                                                        set_edit_title.set(String::new());
                                                        set_edit_description.set(String::new());
                                                        set_edit_runtime.set(String::new());
                                                        set_edit_image_url.set(String::new());
                                                        set_disp.update(|opt_ep| {
                                                            if let Some(ep) = opt_ep {
                                                                ep.title = None;
                                                                ep.description = None;
                                                                ep.runtime = None;
                                                                ep.image_url = None;
                                                                ep.dates.meta_date = None;
                                                                ep.metadata_source = Some("cleared".to_string());
                                                            }
                                                        });
                                                        fire_on_override_saved();
                                                    },
                                                );
                                            }
                                        >
                                            "Clear Metadata"
                                        </button>
                                        {move || {
                                            // Read from reactive display_episode so the match button
                                            // reappears immediately after clearing/restoring metadata.
                                            let ep = display_episode.get();
                                            let meta_source = ep.as_ref().and_then(|e| e.metadata_source.as_deref().map(|s| s.to_string()));
                                            let metadata_ids = ep.as_ref().map(|e| e.metadata_ids.clone()).unwrap_or_default();
                                            let has_matchable = metadata_plugins_signal
                                                .as_ref()
                                                .map(|plugins| {
                                                    crate::utils::has_matchable_metadata(
                                                        meta_source.as_deref(),
                                                        &metadata_ids,
                                                        &plugins.get(),
                                                        true, // episode-level, no season gate
                                                    )
                                                })
                                                .unwrap_or(false);
                                            if has_matchable {
                                                        let eid_restore = episode_id_for_restore.clone();
                                                        view! {
                                                            <button
                                                                class="btn btn-sm flex gap-xs items-center"
                                                                on:click=move |ev| {
                                                                    ev.stop_propagation();
                                                                    // Cancel pending auto-save
                                                                    if let Some(handle) = *debounce_timer.read_value() {
                                                                        handle.clear();
                                                                    }
                                                                    let sid = get_sid();
                                                                    let eid_restore = eid_restore.clone();
                                                                    crate::utils::spawn_api_toast(
                                                                        crate::api::restore_episode_metadata(sid, eid_restore),
                                                                        Some("Restoring metadata..."),
                                                                        move |_| {
                                                                            crate::components::common::toast::show_success("Episode metadata restored from provider.");
                                                                            refresh_display_episode();
                                                                            fire_on_override_saved();
                                                                        },
                                                                    );
                                                                }
                                                            >
                                                                <span class="icon"><WandSparklesIcon/></span>
                                                                "Match to Provider"
                                                            </button>
                                                        }.into_any()
                                                    } else {
                                                        view! { <span></span> }.into_any()
                                                    }
                                        }}
                                    </div>
                                }.into_any()
                            }
                        </div>
                    }.into_any()
                }


                {
                    // Release Date display — click to cycle through enabled
                    // date sources in config priority order.
                    let display = release_date_final.get();
                    let is_cycleable = cycle_order.get().len() > 1;
                    match display {
                        Some((release_label, display_date)) => {
                            let title = if is_cycleable {
                                "Click to cycle through available dates"
                            } else {
                                ""
                            };
                            let style = if is_cycleable {
                                "cursor: pointer;"
                            } else {
                                ""
                            };
                            view! {
                                <div
                                    class="info-section"
                                    on:click=cycle_next_date
                                    title={title}
                                    style={style}
                                >
                                    <div class="info-label">{release_label.clone()}</div>
                                    <div class="info-value">{display_date.clone()}</div>
                                </div>
                            }.into_any()
                        }
                        None => view! { <span></span> }.into_any(),
                    }
                }

                    // File Path — click to reveal the original content path. Rendered
                    // only when the episode currently has a file path.
                    {if ep.path.is_some() {
                        let original = ep.original_path.clone().filter(|o| Some(o) != ep.path.as_ref());
                        let cycleable = original.is_some();
                        let showing_original = show_original_path.get() && cycleable;
                        let display = if showing_original {
                            original.clone().unwrap_or_default()
                        } else {
                            ep.path.clone().unwrap_or_default()
                        };
                        let is_not_found = !showing_original
                            && crate::components::edit_series::season_accordion::episode_is_not_found(
                                ep.assigned, ep.disk_present,
                            );
                        let label = if showing_original { "Original Path" } else { "File Path" };
                        let title = if cycleable { "Click to show the original content path" } else { "" };
                        let style = if cycleable { "cursor: pointer;" } else { "" };
                        Some(view! {
                            <div
                                class={format!("info-section {}", if is_not_found { "not-found" } else { "" })}
                                title={title}
                                style={style}
                                on:click=move |_| {
                                    if cycleable {
                                        set_show_original_path.update(|s| *s = !*s);
                                    }
                                }
                            >
                                <div class="info-label">{label}</div>
                                <div class="info-value path" class:line-through=is_not_found>{display}</div>
                            </div>
                        }.into_any())
                    } else { None }}

                    // Auxiliary Files — subtitle and nfo sidecars attached to the
                    // episode under its playable file. Shown only when present.
                    {if ep.auxiliary_files.is_empty() {
                        None
                    } else {
                        Some(view! {
                            <div class="info-section">
                                <div class="info-label">"Auxiliary Files"</div>
                                <div class="flex flex-col gap-xs">
                                    {ep.auxiliary_files.clone().into_iter().map(|aux| {
                                        view! {
                                            <div class="info-value path" title=aux.path.clone()>
                                                {aux.path.to_string()}
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                            </div>
                        }.into_any())
                    }}

                    // File-specific fields — shown when episode has a single direct file
                    {if ep.path.is_some() { Some(view! {
                        // Release Name (from fingerprint/download) — shown when available
                        // The original title from release (source)
                        {ep.release_title.clone().filter(|rt| !rt.is_empty()).map(|rt| {
                            info_section_cls("Release Name", rt, "")
                        })}

                        // Size + Downloaded (paired)
                        {info_row(
                            "Size",
                            if ep.size > 0 { jumbie_shared::parsing::format_bytes(ep.size) } else { "-".to_string() },
                            "Created",
                            fmt_file_date(ep.created_at.clone()),
                        )}

                        // File Acquired — shown only when value exists
                        {ep.file_acquired_at.clone().map(|acquired_at| {
                            info_section("File Acquired", fmt_file_date(Some(acquired_at)))
                        })}

                        // Quality (resolved from profile) + Release Group (paired)
                        {let quality = quality_display(&ep); info_row(
                            "Quality Profile",
                            quality,
                            "Submitter",
                            ep.submitter.clone().unwrap_or_else(|| "-".to_string()),
                        )}

                        // Media Info
                        {
                            let has_media_info = ep.media_info.is_some();
                            let media_info_grid = build_media_info_grid(&ep.media_info);
                            {media_info_section!(media_info_grid.clone(), false, has_media_info)}
                        }


                    }) } else { None }}

                    // Parts section — shown when episode consists of multiple physical files
                    {if !ep.parts.is_empty() {
                        let parts = ep.parts.clone();
                        Some(view! {
                            <div class="info-section">
                                <div class="info-label">"Multi-Part Files"</div>
                                <div class="episode-parts-list">
                                    {parts.into_iter().map(|part| {
                                        let size_display = if part.size > 0 {
                                            jumbie_shared::parsing::format_bytes(part.size)
                                        } else {
                                            "-".to_string()
                                        };
                                        view! {
                                            <details class="episode-part-card">
                                                <summary class="episode-part-header cursor-pointer">
                                                    <div>
                                                        <span class="badge badge-blue">{format!("Part {}", part.part_number)}</span>
                                                    </div>
                                                    <span class="text-xs text-secondary-color">{size_display.clone()}</span>
                                                </summary>
                                                <div class="episode-part-content mt-md pl-md pr-md pb-md">
                                                    {
                                                        let pn = part.part_number;
                                                        let current = part.file_path.clone();
                                                        let original = part
                                                            .original_path
                                                            .clone()
                                                            .filter(|o| Some(o) != Some(&current));
                                                        let cycleable = original.is_some();
                                                        let set_parts = set_show_part_original;
                                                        move || {
                                                            let showing = show_part_original.get().contains(&pn);
                                                            let value = if showing {
                                                                original.clone().unwrap_or_default()
                                                            } else {
                                                                current.clone()
                                                            };
                                                            let label = if showing { "Original Path" } else { "File Path" };
                                                            let title = if cycleable { "Click to show the original content path" } else { "" };
                                                            let style = if cycleable { "cursor: pointer;" } else { "" };
                                                            view! {
                                                                <div
                                                                    class="info-section"
                                                                    title={title}
                                                                    style={style}
                                                                    on:click=move |_| {
                                                                        if cycleable {
                                                                            set_parts.update(|s| {
                                                                                if !s.remove(&pn) {
                                                                                    s.insert(pn);
                                                                                }
                                                                            });
                                                                        }
                                                                    }
                                                                >
                                                                    <div class="info-label">{label}</div>
                                                                    <div class="info-value path">{value}</div>
                                                                </div>
                                                            }
                                                        }
                                                    }
                                                    {info_row(
                                                        "Size",
                                                        size_display,
                                                        "Created",
                                                        fmt_file_date(ep.created_at.clone()),
                                                    )}

                                                    // File Acquired — shown only when value exists
                                                    {ep.file_acquired_at.clone().map(|acquired_at| {
                                                        info_section("File Acquired", fmt_file_date(Some(acquired_at)))
                                                    })}
                                                    {let quality = quality_display(&ep); info_row(
                                                        "Quality",
                                                        quality,
                                                        "Submitter",
                                                        ep.submitter.clone().unwrap_or_else(|| "-".to_string()),
                                                    )}
                                                    {part.fingerprint.clone().map(|fp| info_section_cls("File Fingerprint (xxHash)", fp, ""))}
                                                    {
                                                        let has_part_media = part.media_info.is_some();
                                                        let part_media_grid = build_media_info_grid(&part.media_info);
                                                        {media_info_section!(part_media_grid.clone(), true, has_part_media)}
                                                    }
                                                </div>
                                            </details>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>
                        })
                    } else { None }}


            })}

            <SearchModal
                show=show_search
                set_show=set_show_search
                id="episode-details-search"
                initial_query=search_query_initial.into()
                series_id=Signal::derive(move || series_id.map(|s| s.get()))
                episode_id=Signal::derive(move || episode.get().map(|e| e.unique_id))
            />

            <ConfirmationModal
                show=Signal::from(show_delete_modal)
                set_show=set_show_delete_modal
                title="Delete Episode"
                message={Memo::new(move |_| pending_delete_msg.get())}
                on_confirm=handle_delete_confirm.into()
            />
        </StandardModal>
    }
}
