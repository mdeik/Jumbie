use crate::components::common::SearchInput;
use crate::components::common::ViewShell;
use crate::components::common::pagination::PaginationControl;
use crate::components::common::status_badge::StatusBadge;
use crate::components::common::table_builder::{ManagedColumn, TableBuilder, TableVariant};
use crate::components::common::toast::show_error;
use crate::components::edit_series::episode_details_modal::EpisodeDetailsModal;
use crate::components::edit_series::season_accordion::episode_status_label;
use crate::hooks::use_config::use_config;
use crate::hooks::use_paginated_sort_state::use_paginated_sort_state;
use crate::hooks::use_pagination::use_pagination;
use crate::hooks::use_status_polling::WantedHasItems;
use crate::hooks::use_ui_config::use_ui_config;
use crate::utils::ListQueryParams;
use crate::utils::format_age;
use jumbie_shared::formatting::{
    LabelStyle, fmt_abs_episode, fmt_episode, fmt_season, fmt_season_episode,
};
use jumbie_shared::types::{EpisodeViewModel, WantedEpisode};
use leptos::prelude::*;
use std::collections::HashMap;

#[component]
pub fn Wanted() -> impl IntoView {
    let ui_config = use_ui_config().ui_config;
    // App config: global absolute-numbering default (SSoT: resolves the series
    // tristate the same way edit_series does).
    let global_config = use_config().config;

    let sort = use_paginated_sort_state("wanted".to_string(), "age".to_string(), false);
    let sort_col = sort.column;
    let sort_asc = sort.ascending;
    let sort_override = sort.override_state;
    let on_sort = sort.on_sort;

    // Matches series title, episode title, and season/episode numbers on the
    // backend.  No stored default — purely a client override.
    let (search_override, set_search_override) = signal::<Option<String>>(None);

    // Both the key and the request are built from `params`, so they cannot
    // drift apart.  `Signal::derive` has no equality check, so identical
    // override writes are dropped at the setters.
    let params = Signal::derive(move || {
        ListQueryParams::new()
            .sort(sort_override.get())
            .search(search_override.get())
    });
    let query_key = Signal::derive(move || params.get().key());

    let pagination = use_pagination(
        "fetch_wanted_episodes",
        move |page| {
            let p = params.get_untracked();
            async move {
                crate::api::fetch_wanted_episodes(page, 50, p.search_value(), p.sort_state())
                    .await
                    .map_err(|e| {
                        show_error(format!("Failed to fetch wanted episodes: {}", e));
                        e
                    })
            }
        },
        50,
        true,
        Some(30_000),
        query_key,
    );

    // Derived: filter items based on the user's release date display preferences.
    // Items whose effective date (per user's priority/enabled config) is in the
    // future are excluded, even if some other (lower-priority) date is in the
    // past. This way changes to General Settings take effect instantly without
    // cache invalidation.
    let display_items = Signal::derive(move || {
        let items = pagination.items.get();
        let ui = ui_config.get();
        let config = match ui.as_ref() {
            Some(u) => &u.release_date_display,
            None => return items, // config not loaded yet — show everything
        };
        let now_utc = chrono::Utc::now();
        items
            .into_iter()
            .filter(|item| {
                // Selection-only entry point: we only need the instant to compare
                // against now, not a formatted display string.
                match crate::utils::release_date::pick_release_date_utc(
                    config,
                    item.dates.meta_date.as_deref(),
                    item.dates.upload_date.as_deref(),
                    item.dates.est_date.as_deref(),
                ) {
                    Some(dt) => dt < now_utc,
                    None => false, // no effective date → can't confirm released
                }
            })
            .collect()
    });

    // Sync the sidebar indicator with the filtered count.
    let wanted_has_items = use_context::<WantedHasItems>();
    Effect::new(move |_| {
        let filtered = display_items.get();
        if let Some(WantedHasItems(_, setter)) = &wanted_has_items {
            setter.set(!filtered.is_empty());
        }
    });

    let (show_modal, set_show_modal) = signal(false);
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

    // Modal saves in the calendar/edit_series broadcast through
    // EpisodeStateContext; refresh the wanted list so it tracks monitor
    // toggles and metadata edits made elsewhere (same listener as the
    // series library's).
    let episode_ctx = crate::utils::episode_state::use_episode_state();
    let refresh_list = pagination.refresh;
    Effect::new(move |_| {
        if episode_ctx.pending_save.get().is_some() {
            // Clear immediately so subsequent saves can write a new one.
            episode_ctx.pending_save.set(None);
            crate::utils::invalidate_cache_prefix("fetch_wanted_episodes:");
            refresh_list.run(());
        }
    });

    let (wanted_metadata_plugins, set_wanted_metadata_plugins) =
        signal(Vec::<jumbie_shared::plugin::PluginInstanceInfo>::new());
    let (wanted_plugins_cfg, set_wanted_plugins_cfg) =
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
            set_wanted_metadata_plugins.set(meta);
        },
    );

    crate::utils::use_api_cache(
        "fetch_plugins_cfg".to_string(),
        || crate::api::fetch_plugins_cfg(),
        move |c: jumbie_shared::config::PluginsConfig| {
            set_wanted_plugins_cfg.set(Some(c));
        },
    );

    let wanted_active_metadata_plugins = Signal::derive(move || {
        let all_plugins = wanted_metadata_plugins.get();
        if let Some(cfg) = wanted_plugins_cfg.get() {
            crate::utils::resolve_active_metadata_plugins(all_plugins, &cfg)
        } else {
            Vec::new()
        }
    });

    let (wanted_quality_profiles, set_wanted_quality_profiles) = signal(HashMap::new());
    crate::utils::use_api_cache(
        "fetch_quality_profiles".to_string(),
        || crate::api::fetch_quality_profiles(),
        move |profiles: HashMap<String, jumbie_shared::types::QualityProfile>| {
            set_wanted_quality_profiles.set(profiles);
        },
    );

    let open_episode_detail = move |item: WantedEpisode| {
        let ep_id = item.episode_id.clone();
        let series_id = item.series_id.clone();

        // Instant render from the row (partial episode); the shared
        // series-scoped refresh below replaces it with the full payload
        // moments later.
        let partial_episode = crate::utils::episode_state::partial_episode_view_model(
            item.episode_id.clone(),
            item.season.as_deref(),
            item.episode,
            item.title.clone(),
            item.status.clone(),
            item.dates.clone(),
            // Wanted episodes are always monitored (SQL filters e.monitored = 1).
            true,
            false,
        );

        crate::utils::episode_state::reset_episode_modal_bindings(&modal_bindings);
        set_selected_series_id.set(series_id.clone());
        set_selected_series_title.set(item.series_title.clone());
        set_selected_episode.set(Some(partial_episode));
        set_show_modal.set(true);

        // SSoT: force-revalidate the series-scoped details so the modal never
        // shows a TTL-fresh-but-stale status.
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
            set_show_modal,
        );
    };

    // Maps a wanted episode status to its badge colour variant.
    // The backend's get_wanted_episodes already maps raw "unreleased" to "missing",
    // so only "in_queue" and "missing" reach this component.
    // SSoT: mirrors the episode_status_class logic from season_accordion.
    let wanted_status_variant = |status: &str| -> &'static str {
        match status {
            "in_queue" => "warning",
            _ => "danger",
        }
    };

    let columns: Vec<ManagedColumn<WantedEpisode>> = vec![
        ManagedColumn {
            id: "series".into(),
            label: "Series".into(),
            sortable: true,
            class: "font-bold".into(),
            cell_render: Callback::new(move |item: WantedEpisode| {
                view! { <span>{item.series_title}</span> }.into_any()
            }),
        },
        ManagedColumn {
            id: "episode".into(),
            label: "Episode".into(),
            sortable: true,
            class: "".into(),
            cell_render: Callback::new(move |item: WantedEpisode| {
                let ep_str = if let Some(s) = &item.season {
                    s.parse::<i32>()
                        .ok()
                        .map(|n| fmt_season_episode(n, item.episode, None, LabelStyle::Short))
                        .unwrap_or_else(|| {
                            format!(
                                "{}{}",
                                fmt_season(s.parse().unwrap_or(0), LabelStyle::Short),
                                fmt_episode(item.episode, LabelStyle::Short)
                            )
                        })
                } else {
                    fmt_abs_episode(item.episode, LabelStyle::Short)
                };
                view! {
                    <div>
                        {ep_str}
                        {item.title.map(|t| {
                            view! { <span class="text-sm block text-muted">{t}</span> }
                        })}
                    </div>
                }
                .into_any()
            }),
        },
        ManagedColumn {
            id: "age".into(),
            label: "Age".into(),
            sortable: true,
            class: "".into(),
            cell_render: Callback::new(move |item: WantedEpisode| {
                // eff_date is server-resolved via the UI config (compute_effective_date).
                let age = format_age(&item.eff_date);
                view! { <span>{age}</span> }.into_any()
            }),
        },
        ManagedColumn {
            id: "status".into(),
            label: "Status".into(),
            sortable: true,
            class: "w-10".into(),
            cell_render: Callback::new(move |item: WantedEpisode| {
                let variant = wanted_status_variant(&item.status);
                view! { <StatusBadge variant=variant label={episode_status_label(&item.status)} /> }
                    .into_any()
            }),
        },
    ];

    // No-op compare — unused because `.preserve_order()` is set below.
    // Sorting is handled server-side via the sort parameter sent to the API.
    let compare =
        Callback::new(move |_: (WantedEpisode, WantedEpisode, String)| std::cmp::Ordering::Equal);

    view! {
        <ViewShell id="wanted">
            <div class="table-header pt-0">
                <div class="flex items-center gap-md w-full">
                    <SearchInput
                        id="wantedSearchInput"
                        placeholder="Search series, episode or title…"
                        on_search=Callback::new(move |v: String| {
                            set_search_override.set(if v.is_empty() { None } else { Some(v) });
                        })
                    />
                </div>
            </div>
            {
                TableBuilder::new(sort_col, sort_asc)
                    .container_class(Signal::derive(move || "table-container unselectable".to_string()))
                    .table_variant(TableVariant::Hover)
                    .loading(pagination.loading)
                    .empty_message("No missing episodes found")
                    .on_sort(on_sort)
                    .preserve_order()
                    .build_managed(
                        display_items,
                        columns,
                        compare,
                        Some(Callback::new(open_episode_detail)),
                        None,
                        Some(Callback::new(move |item: WantedEpisode| {
                            let ep_str = if let Some(s) = &item.season {
                                s.parse::<i32>()
                                    .ok()
                                    .map(|n| fmt_season_episode(n, item.episode, None, LabelStyle::Short))
                                    .unwrap_or_else(|| {
                                        format!(
                                            "{}{}",
                                            fmt_season(s.parse().unwrap_or(0), LabelStyle::Short),
                                            fmt_episode(item.episode, LabelStyle::Short)
                                        )
                                    })
                            } else {
                                fmt_abs_episode(item.episode, LabelStyle::Short)
                            };
                            let item_clone = item.clone();
                            let age = format_age(&item.eff_date);
                            view! {
                                <div class="card p-md bg-secondary border border-radius flex flex-col gap-xs cursor-pointer" on:click=move |_| open_episode_detail(item_clone.clone())>
                                    <div class="card-row-with-badge">
                                        <strong class="truncate card-row-title text-base">{item.series_title}</strong>
                                        <StatusBadge variant={wanted_status_variant(&item.status)} label={episode_status_label(&item.status)} />
                                    </div>
                                    <div class="flex justify-between text-sm">
                                        <span>{ep_str}</span>
                                        <span class="text-muted">{age}</span>
                                    </div>
                                    {item.title.map(|t| view!{ <div class="text-xs text-muted truncate">{t}</div> })}
                                </div>
                            }.into_any()
                        })),
                    )
            }
            <PaginationControl
                page=pagination.page
                total_pages=pagination.total_pages
                on_page_change=pagination.set_page
            />

            <EpisodeDetailsModal
                show=show_modal
                set_show=set_show_modal
                episode=selected_episode
                all_episodes=Signal::derive(move || all_episodes.get())
                series_id=Signal::derive(move || selected_series_id.get())
                series_title=Signal::derive(move || selected_series_title.get())
                season_overrides=Signal::derive(move || season_overrides.get())
                absolute_numbering=Signal::derive(move || absolute_numbering.get())
                series_search_format=Signal::derive(move || series_search_format.get())
                series_metadata_ids=Signal::derive(move || series_metadata_ids.get())
                metadata_plugins=wanted_active_metadata_plugins
                quality_profiles=Signal::derive(move || wanted_quality_profiles.get())
                on_override_saved=Callback::new({
                    let episode_ctx = crate::utils::episode_state::use_episode_state();
                    let refresh = pagination.refresh;
                    let modal_bindings = modal_bindings.clone();
                    move |(episode_id, series_id): (String, String)| {
                        // Broadcast the save to shared context so all components
                        // watching EpisodeStateContext (calendar, etc.) can react.
                        crate::utils::episode_state::notify_episode_saved(
                            &episode_ctx,
                            &episode_id,
                            &series_id,
                        );

                        // Individual episode cache invalidation is handled by
                        // EpisodeDetailsModal itself (SSoT).
                        // Only invalidate the collection-level cache so the list refreshes.
                        crate::utils::invalidate_cache_prefix("fetch_wanted_episodes:");
                        refresh.run(());

                        // Keep the open modal on the freshest server state (same as
                        // the edit_series page's refetch), via the shared SSoT helper.
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
                            &episode_id,
                            global_abs,
                            &global_fmt,
                            &global_fmt_abs,
                            set_show_modal,
                        );
                    }
                })
            />
        </ViewShell>
    }
}
