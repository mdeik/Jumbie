use crate::api::batch_edit_series;
use crate::components::common::form_fields::{Select, SelectOption};
use crate::components::common::icons::{
    AlertTriangleIcon, PlusIcon, SaveIcon, TrashIcon, WandSparklesIcon,
};
use crate::components::common::table_builder::{ManagedColumn, TableBuilder, TableVariant};
use crate::components::common::toast::show_success;
use crate::components::common::{SearchInput, ViewHeader, ViewShell};
use crate::components::series_library::remove_series_modal::RemoveSeriesModal;
use crate::hooks::series::use_series_list;
use crate::hooks::use_status_polling::use_media_info_scan_counts;
use crate::hooks::use_table_search::use_table_search;
use crate::hooks::use_table_selection::{TableSelection, use_table_selection};
use crate::routes::path;
use crate::utils::sorting::table_comparator;
use jumbie_shared::types::{BatchEditSeriesPayload, SeriesInfo};
use leptos::prelude::*;
use leptos_router::hooks::*;
use std::collections::HashMap;

/// SSoT: derive the episode-count label's class + tooltip for the library
/// (shared by the table column and the card view).
/// Precedence: queued (yellow) > missing (red) > complete (green) — a series with
/// episodes both missing and queued shows yellow, because the download already in
/// flight supersedes the missing state it will resolve. "Queued" = episodes in the
/// download queue (monitored or not).
fn episode_count_label(series: &SeriesInfo) -> (&'static str, String) {
    let missing = series.monitored_missing_count;
    let queued = series.queued_count;
    // `>=` not `==`: downloads outside the configured range (e.g. past a season's
    // `cell_count`) still count as downloaded, so a fully-covered series can have
    // more downloads than expected episodes.
    let is_complete =
        series.episodes_counts.0 >= series.episodes_counts.1 && series.episodes_counts.1 > 0;

    let class = if queued > 0 {
        "text-warning"
    } else if missing > 0 {
        "text-danger"
    } else if is_complete {
        "text-success"
    } else {
        ""
    };

    let title = match (missing > 0, queued > 0) {
        (true, true) => format!(
            "{} wanted episode{} · {} queued",
            missing,
            if missing == 1 { "" } else { "s" },
            queued,
        ),
        (true, false) => format!(
            "{} wanted episode{}",
            missing,
            if missing == 1 { "" } else { "s" }
        ),
        (false, true) => format!(
            "{} episode{} in queue",
            queued,
            if queued == 1 { "" } else { "s" }
        ),
        (false, false) => String::new(),
    };

    (class, title)
}

#[component]
pub fn SeriesLibrary() -> impl IntoView {
    let navigate = use_navigate();
    let series_state = use_series_list();
    let series_list = series_state.data;
    let is_loading = series_state.loading;
    let refresh_series = series_state.refresh;

    // Client-side search over title + aliases. Select-all uses the filtered view.
    let (set_search_query, filtered_data) = use_table_search(
        Signal::derive(move || series_list.get().unwrap_or_default()),
        |s: &SeriesInfo| {
            let mut haystack = s.title.clone();
            for alias in &s.aliases {
                haystack.push(' ');
                haystack.push_str(alias);
            }
            haystack
        },
    );

    let scan_counts = use_media_info_scan_counts();

    // Scan counts are the SSoT bridge from the backend scan queue: when the
    // pending count drops from >0 to 0, invalidate the series cache and re-fetch
    // so the user sees final episode data without a manual page refresh. This
    // piggybacks on the existing 15s scan-counts polling instead of polling the
    // series list itself.
    //
    // `Option<usize>` distinguishes first mount (None) from later runs: if the
    // count is already 0 on mount, scans may have finished while the component
    // was unmounted (e.g. a scan triggered from edit-series), so we refresh to
    // avoid showing stale cached data because the >0 → 0 transition was never
    // observed on this instance.
    let prev_scan_total = StoredValue::new_local(Option::<usize>::None);
    Effect::new(move |_| {
        let total: usize = scan_counts.get().values().sum();
        let prev = prev_scan_total.get_value();
        prev_scan_total.set_value(Some(total));

        match prev {
            None if total == 0 => {
                refresh_series.run(());
            }
            // All pending scans completed → refresh the series list.
            Some(prev_val) if prev_val > 0 && total == 0 => {
                refresh_series.run(());
            }
            _ => {}
        }
    });

    // The modal broadcasts every save (calendar / wanted / edit_series) through
    // EpisodeStateContext.  Refreshing the series list here keeps the library's
    // counts (missing/queued/downloaded) fresh without waiting for a remount.
    let episode_ctx = crate::utils::episode_state::use_episode_state();
    Effect::new(move |_| {
        if episode_ctx.pending_save.get().is_some() {
            // Clear immediately so subsequent saves can write a new one.
            episode_ctx.pending_save.set(None);
            refresh_series.run(());
        }
    });

    let (quality_profiles, set_quality_profiles) =
        signal(HashMap::<String, jumbie_shared::types::QualityProfile>::new());

    crate::utils::use_api_cache(
        "fetch_quality_profiles".to_string(),
        || crate::api::fetch_quality_profiles(),
        move |profiles| {
            let _ = set_quality_profiles.try_update(|s| *s = profiles);
        },
    );

    let (release_profiles, set_release_profiles) =
        signal(HashMap::<String, jumbie_shared::scoring::ReleaseProfile>::new());

    crate::utils::use_api_cache(
        "fetch_release_profiles".to_string(),
        || crate::api::fetch_release_profiles(),
        move |p| {
            let _ = set_release_profiles.try_update(|s| *s = p);
        },
    );

    // SSoT: mirrors edit_series.rs's active metadata-provider detection.
    let (_metadata_plugins, set_metadata_plugins) =
        signal(Vec::<jumbie_shared::plugin::PluginTypeListing>::new());
    let (plugins_cfg_sig, set_plugins_cfg_sig) =
        signal(None::<jumbie_shared::config::PluginsConfig>);

    crate::utils::use_api_cache(
        "series_lib_fetch_available_plugins".to_string(),
        || crate::api::fetch_available_plugins(),
        move |plugins: Vec<jumbie_shared::plugin::PluginTypeListing>| {
            let meta = plugins
                .into_iter()
                .filter(|p| {
                    p.capabilities
                        .contains(&jumbie_shared::plugin::Capability::MetadataProviderNormal)
                        || p.capabilities
                            .contains(&jumbie_shared::plugin::Capability::MetadataProviderAbsolute)
                })
                .collect();
            let _ = set_metadata_plugins.try_update(|s| *s = meta);
        },
    );

    crate::utils::use_api_cache(
        "series_lib_fetch_plugins_cfg".to_string(),
        || crate::api::fetch_plugins_cfg(),
        move |c| {
            let _ = set_plugins_cfg_sig.try_update(|s| *s = Some(c));
        },
    );

    let has_active_metadata_provider = Signal::derive(move || {
        if let Some(cfg) = plugins_cfg_sig.get() {
            for instances in cfg.metadata.values() {
                for instance_cfg in instances.values() {
                    let enabled = instance_cfg
                        .as_object()
                        .and_then(|t| t.get("enabled"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if enabled {
                        return true;
                    }
                }
            }
        }
        false
    });

    let (is_edit_mode, set_is_edit_mode) = signal(false);

    let TableSelection {
        selected: selected_series,
        is_select_all,
        disabled: select_all_disabled,
        select_all,
        toggle: toggle_selection,
        clear: clear_selection,
    } = use_table_selection(filtered_data, |s: &SeriesInfo| s.id.clone());

    let (bulk_quality, set_bulk_quality) = signal(String::new());
    let (bulk_release, set_bulk_release) = signal(String::new());
    let (bulk_monitor, set_bulk_monitor) = signal(String::new());

    let has_selection = Signal::derive(move || !selected_series.get().is_empty());
    let has_apply_input = Signal::derive(move || {
        !bulk_quality.get().is_empty()
            || !bulk_release.get().is_empty()
            || !bulk_monitor.get().is_empty()
    });

    let (show_delete_modal, set_show_delete_modal) = signal(false);
    let (delete_configurations, set_delete_configurations) = signal(false);
    let (delete_episodes, set_delete_episodes) = signal(false);
    let (delete_episode_data, set_delete_episode_data) = signal(false);

    let exit_edit_mode = move || {
        clear_selection.run(());
        set_bulk_quality.set(String::new());
        set_bulk_release.set(String::new());
        set_bulk_monitor.set(String::new());
        set_is_edit_mode.set(false);
    };

    let toggle_edit_mode = move |_| {
        if is_edit_mode.get() {
            exit_edit_mode();
        } else {
            set_is_edit_mode.set(true);
        }
    };

    let apply_bulk_edit = move |_| {
        let ids: Vec<String> = selected_series.get().into_iter().collect();
        if ids.is_empty() {
            return;
        }

        let qp = bulk_quality.get();
        let rp = bulk_release.get();
        let mm_str = bulk_monitor.get();

        let mm = crate::utils::parse_monitor_mode(&mm_str);

        let payload = BatchEditSeriesPayload {
            series_ids: ids,
            quality_profile: if qp.is_empty() {
                None
            } else if qp == "__clear__" {
                Some(String::new())
            } else {
                Some(qp)
            },
            release_profile: if rp.is_empty() {
                None
            } else if rp == "__clear__" {
                Some(String::new())
            } else {
                Some(rp)
            },
            monitor_mode: mm,
        };

        crate::utils::spawn_api_toast(batch_edit_series(payload), None, move |_| {
            show_success("Edits applied");
            // Reset field signals so the selectors show blank again for the next batch
            set_bulk_quality.set(String::new());
            set_bulk_release.set(String::new());
            set_bulk_monitor.set(String::new());
            exit_edit_mode();
            refresh_series.run(());
        });
    };

    let apply_bulk_fetch_metadata = move |_| {
        let ids: Vec<String> = selected_series.get().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        crate::utils::spawn_api_toast(
            crate::api::batch_fetch_metadata(ids),
            Some("Metadata fetch initiated successfully for series."),
            move |_| {},
        );
        exit_edit_mode();
    };

    let (sort_col, sort_asc, on_sort) = crate::hooks::use_persistent_table_state(
        "series_library".to_string(),
        "title".to_string(),
        true,
    );

    // Uses the shared `build_sorted_profile_options` pipeline, with a distinctive
    // placeholder and a "__clear__" option for the bulk-edit context.
    let quality_options = Signal::derive(move || {
        let profiles: Vec<(String, String)> = quality_profiles
            .get()
            .iter()
            .map(|(id, p)| (id.clone(), p.name.clone()))
            .collect();
        crate::utils::build_sorted_profile_options(&profiles, "— Quality Profile —", true)
    });

    let release_options = Signal::derive(move || {
        let profiles: Vec<(String, String)> = release_profiles
            .get()
            .iter()
            .map(|(id, p)| (id.clone(), p.name.clone()))
            .collect();
        crate::utils::build_sorted_profile_options(&profiles, "— Release Profile —", true)
    });

    let monitor_options = Signal::derive(move || {
        vec![
            SelectOption::from(("".to_string(), "— Monitor Mode —".to_string())),
            SelectOption::from(("All".to_string(), "All".to_string())),
            SelectOption::from(("Future".to_string(), "Future".to_string())),
            SelectOption::from(("Missing".to_string(), "Missing".to_string())),
            SelectOption::from(("Existing".to_string(), "Existing".to_string())),
            SelectOption::from(("Pilot".to_string(), "Pilot".to_string())),
            SelectOption::from(("FirstSeason".to_string(), "First Season".to_string())),
            SelectOption::from(("Specials".to_string(), "Specials".to_string())),
            SelectOption::from(("None".to_string(), "None".to_string())),
        ]
    });

    let columns: Vec<ManagedColumn<SeriesInfo>> = vec![
        ManagedColumn {
            id: "title".into(),
            label: "Series".into(),
            sortable: true,
            class: "".into(),
            cell_render: Callback::new(move |series: SeriesInfo| {
                view! {
                <span class="flex items-center gap-xs">
                    <strong>{series.title}</strong>
                    {
                        let count = scan_counts.get().get(&series.id).copied().unwrap_or(0);
                        if count > 0 {
                            view! {
                                <span class="series-scanning-indicator" title={format!("{} file(s) being scanned", count)}>
                                    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                        <circle cx="12" cy="12" r="8" stroke-dasharray="40" stroke-linecap="round">
                                            <animateTransform attributeName="transform" type="rotate" from="0 12 12" to="360 12 12" dur="0.8s" repeatCount="indefinite"/>
                                        </circle>
                                    </svg>
                                </span>
                            }.into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }
                    }
                </span>
            }.into_any()
            }),
        },
        ManagedColumn {
            id: "seasons".into(),
            label: "Seasons".into(),
            sortable: true,
            class: "".into(),
            cell_render: Callback::new(move |series: SeriesInfo| {
                let display = if series.absolute_numbering {
                    vec![crate::utils::episode_state::ABSOLUTE_SEASON_LABEL.to_string()]
                } else {
                    series.seasons.clone()
                };
                view! {
                    <div class="season-display">{display.join(", ")}</div>
                }
                .into_any()
            }),
        },
        ManagedColumn {
            id: "progress".into(),
            label: "Episodes".into(),
            sortable: true,
            class: "".into(),
            cell_render: Callback::new(move |series: SeriesInfo| {
                let text = format!("{}/{}", series.episodes_counts.0, series.episodes_counts.1);
                let (label_class, title) = episode_count_label(&series);
                view! {
                    <span class=label_class title=title>
                        {if series.has_not_found_files {
                            view! {
                                <span class="file-not-found-warning">
                                    {AlertTriangleIcon()}
                                </span>
                            }.into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }}
                        {text}
                    </span>
                }
                .into_any()
            }),
        },
        ManagedColumn {
            id: "quality".into(),
            label: "Quality".into(),
            sortable: true,
            class: "".into(),
            cell_render: Callback::new(move |series: SeriesInfo| {
                let quality_name = crate::utils::resolve_quality_profile_name(
                    &series.quality_profile,
                    &quality_profiles.get(),
                );
                view! {
                    <span class="cell-quality">{quality_name}</span>
                }
                .into_any()
            }),
        },
        ManagedColumn {
            id: "size".into(),
            label: "Size".into(),
            sortable: true,
            class: "col-size".into(),
            cell_render: Callback::new(move |series: SeriesInfo| {
                let text = if series.size > 0 {
                    jumbie_shared::parsing::format_bytes(series.size)
                } else {
                    "\u{2014}".to_string()
                };
                view! { <span>{text}</span> }.into_any()
            }),
        },
    ];
    let compare = table_comparator(|a: &SeriesInfo, b: &SeriesInfo, col: &str| match col {
        "title" => jumbie_shared::formatting::natural_cmp(&a.title, &b.title),
        "seasons" => a
            .season_count
            .cmp(&b.season_count)
            .then_with(|| jumbie_shared::formatting::natural_cmp(&a.title, &b.title)),
        "quality" => a.quality_profile.cmp(&b.quality_profile),
        "progress" => {
            let p_a = if a.episodes_counts.1 > 0 {
                a.episodes_counts.0 as f64 / a.episodes_counts.1 as f64
            } else {
                0.0
            };
            let p_b = if b.episodes_counts.1 > 0 {
                b.episodes_counts.0 as f64 / b.episodes_counts.1 as f64
            } else {
                0.0
            };
            p_a.partial_cmp(&p_b).unwrap_or(std::cmp::Ordering::Equal)
        }
        "size" => a.size.cmp(&b.size),
        _ => std::cmp::Ordering::Equal,
    });

    view! {
        <ViewShell id="series">
            {TableBuilder::new(sort_col, sort_asc)
                .container_class(Signal::derive(move || "table-container".to_string()))
                .table_variant(TableVariant::Hover)
                .table_class("unselectable")
                .on_sort(on_sort)
                .loading(is_loading)
                .empty_message("No series in your library yet.")
                .edit_mode(is_edit_mode)
                .selection_mode(is_select_all, select_all)
                .select_all_disabled(select_all_disabled)
                .header_view({
                    let nav = navigate.clone();
                    view! {
                        <ViewHeader
                            id="series-library-header"
                            search_bar={view! {
                                <SearchInput
                                    id="series-library-search"
                                    placeholder="Search series..."
                                    class="search-input"
                                    on_search=Callback::new(move |v: String| set_search_query.set(v))
                                />
                            }.into_any()}
                            actions={view! {
                                {TableBuilder::render_edit_button(is_edit_mode, Callback::new(toggle_edit_mode))}
                                <button class="btn btn-primary" on:click=move |_| nav(&format!("/{}", path::SERIES_ADD), Default::default())>
                                    <span class="icon"><PlusIcon /></span>
                                    "Add New"
                                </button>
                            }.into_any()}
                        />
                    }.into_any()})
                .footer_view(view! {
                    <div class="bulk-action-footer">
                        <div class="bulk-action-content">
                            <div class="bulk-fields-col flex gap-md align-center flex-wrap">
                                <div class="selection-count">
                                    <strong>{move || format!("{} Selected", selected_series.get().len())}</strong>
                                </div>
                                <div class="bulk-selectors flex gap-md align-center flex-wrap">
                                    <Select
                                        value=Signal::derive(move || bulk_quality.get())
                                        set_value=Callback::new(move |v| set_bulk_quality.set(v))
                                        options=quality_options
                                    />
                                    <Select
                                        value=Signal::derive(move || bulk_release.get())
                                        set_value=Callback::new(move |v| set_bulk_release.set(v))
                                        options=release_options
                                    />
                                    <Select
                                        value=Signal::derive(move || bulk_monitor.get())
                                        set_value=Callback::new(move |v| set_bulk_monitor.set(v))
                                        options=monitor_options
                                    />
                                </div>
                            </div>
                            <div class="bulk-actions-col flex gap-md align-center">
                                <button class="btn btn-primary btn-md" on:click=apply_bulk_edit disabled=move || !(has_selection.get() && has_apply_input.get())>
                                    <span class="icon"><SaveIcon /></span> "Apply"
                                </button>
                                {move || has_active_metadata_provider.get().then(|| {
                                    view! {
                                        <button class="btn btn-secondary btn-md" on:click=apply_bulk_fetch_metadata disabled=move || !has_selection.get()>
                                            <span class="icon"><WandSparklesIcon /></span> "Metadata"
                                        </button>
                                    }.into_any()
                                })}
                                <button class="btn btn-danger btn-md" on:click=move |_| set_show_delete_modal.set(true) disabled=move || !has_selection.get()>
                                    <span class="icon"><TrashIcon /></span> "Delete"
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_any())
                .build_managed(
                    filtered_data,
                    columns,
                    compare,
                    Some({
                        let nav = navigate.clone();
                        Callback::new(move |series: SeriesInfo| {
                            if is_edit_mode.get() {
                                toggle_selection.run(series.id.clone());
                            } else {
                                nav(&format!("/{}/{}/edit", path::SERIES, series.id), Default::default());
                            }
                        })
                    }),
                    Some(Callback::new(move |series: SeriesInfo| {
                        selected_series.get().contains(&series.id)
                    })),
                    Some({
                        let nav = navigate.clone();
                        Callback::new(move |series: SeriesInfo| {
                            // Computed before the view macro, which partially moves `series`
                            // (e.g. `series.title` in the card header below).
                            let (label_class, title) = episode_count_label(&series);
                            let quality_name = crate::utils::resolve_quality_profile_name(
                                &series.quality_profile,
                                &quality_profiles.get(),
                            );
                            let s_id = std::sync::Arc::new(series.id.clone());
                            let nav_inner = nav.clone();
                            view! {
                                <div class="series-lib-card card p-md bg-secondary border border-radius flex flex-col gap-sm"
                                    data-series-id={s_id.as_str()}
                                    on:click={
                                        let s_id_card = s_id.clone();
                                        move |_| {
                                            if is_edit_mode.get() {
                                                toggle_selection.run(s_id_card.to_string());
                                            } else {
                                                nav_inner(&format!("/{}/{}/edit", path::SERIES, s_id_card), Default::default());
                                            }
                                        }
                                    }
                                >
                                    <div class="series-lib-card-row flex justify-between items-center">
                                        <span class="series-lib-card-title flex items-center gap-xs">
                                            {
                                                let s_id_arc = s_id.clone();
                                                move || {
                                                    if is_edit_mode.get() {
                                                        let s_id_edit = s_id_arc.clone();
                                                        let s_id_tog = s_id_arc.clone();
                                                        Some(view! {
                                                            <input
                                                                type="checkbox"
                                                                class="table-checkbox"
                                                                prop:checked={
                                                                    let s_id_check = s_id_edit.clone();
                                                                    move || selected_series.get().contains(s_id_check.as_str())
                                                                }
                                                                on:click=move |ev| ev.stop_propagation()
                                                                on:change=move |_| toggle_selection.run(s_id_tog.to_string())
                                                            />
                                                        }.into_any())
                                                    } else {
                                                        None
                                                    }
                                                }
                                            }
                                            <strong class="truncate">
                                                {series.title}
                                            </strong>
                                            {
                                                let count = scan_counts.get().get(&series.id).copied().unwrap_or(0);
                                                if count > 0 {
                                                    view! {
                                                        <span class="series-scanning-indicator" title={format!("{} file(s) being scanned", count)}>
                                                            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                                                <circle cx="12" cy="12" r="8" stroke-dasharray="40" stroke-linecap="round">
                                                                    <animateTransform attributeName="transform" type="rotate" from="0 12 12" to="360 12 12" dur="0.8s" repeatCount="indefinite"/>
                                                                </circle>
                                                            </svg>
                                                        </span>
                                                    }.into_any()
                                                } else {
                                                    view! { <span></span> }.into_any()
                                                }
                                            }
                                        </span>
                                        {if quality_name.is_empty() {
                                            view! {}.into_any()
                                        } else {
                                            view! { <span class="cell-quality truncate">{quality_name}</span> }.into_any()
                                        }}
                                    </div>
                                    <div class="series-lib-card-meta flex justify-between text-muted text-sm">
                                        <span class="truncate series-lib-card-seasons">{series.seasons.join(", ")}</span>
                                        {
                                            view! {
                                                <span class=format!("truncate series-lib-card-episodes {}", label_class) title=title>
                                                    {if series.has_not_found_files {
                                                        view! {
                                                            <span class="file-not-found-warning">
                                                                {AlertTriangleIcon()}
                                                            </span>
                                                        }.into_any()
                                                    } else {
                                                        view! { <span></span> }.into_any()
                                                    }}
                                                    {format!("{}/{} episodes", series.episodes_counts.0, series.episodes_counts.1)}
                                                </span>
                                            }
                                        }
                                    </div>
                                </div>
                            }.into_any()
                        })
                    }),
                )
            }
        </ViewShell>

        {move || {
            if !is_edit_mode.get() {
                return view! {}.into_any();
            }
            view! {
                <RemoveSeriesModal
                    show=Signal::from(show_delete_modal)
                    set_show=set_show_delete_modal
                    series_ids=Signal::derive(move || {
                        selected_series.get().into_iter().collect::<Vec<_>>()
                    })
                    delete_configurations=delete_configurations
                    set_delete_configurations=set_delete_configurations
                    delete_episodes=delete_episodes
                    set_delete_episodes=set_delete_episodes
                    delete_episode_data=delete_episode_data
                    set_delete_episode_data=set_delete_episode_data
                    on_success=Callback::new(move |_| {
                        show_success("Series removed");
                        clear_selection.run(());
                        set_delete_configurations.set(false);
                        set_delete_episodes.set(false);
                        set_delete_episode_data.set(false);
                        refresh_series.run(());
                        // Also invalidate the organized-series cache so that
                        // SystemOrganizedSeries reflects the removal immediately
                        // when the user navigates to that page.
                        crate::utils::invalidate_cache_prefix("fetch_organized_series");
                    })
                />
            }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::types::SeriesInfo;
    use std::cmp::Ordering;

    /// Shorthand helper to build a SeriesInfo with just the fields relevant to sorting.
    fn make_series(id: &str, title: &str, season_count: i32) -> SeriesInfo {
        SeriesInfo {
            id: id.to_string(),
            title: title.to_string(),
            seasons: Vec::new(),
            season_count,
            release_profile: String::new(),
            quality_profile: String::new(),
            episodes_counts: (0, 0),
            monitored_missing_count: 0,
            queued_count: 0,
            size: 0,
            has_not_found_files: false,
            path: String::new(),
            scan_queue_count: 0,
            absolute_numbering: false,
            aliases: Vec::new(),
        }
    }

    fn season_cmp(a: &SeriesInfo, b: &SeriesInfo) -> Ordering {
        a.season_count
            .cmp(&b.season_count)
            .then_with(|| jumbie_shared::formatting::natural_cmp(&a.title, &b.title))
    }

    #[test]
    fn test_season_sort_by_count() {
        let a = make_series("1", "Zeta", 1);
        let b = make_series("2", "Alpha", 3);
        assert_eq!(season_cmp(&a, &b), Ordering::Less);
        assert_eq!(season_cmp(&b, &a), Ordering::Greater);
    }

    #[test]
    fn test_season_sort_equal_count_uses_title_tiebreaker() {
        let a = make_series("1", "Beta", 2);
        let b = make_series("2", "Alpha", 2);
        // Same count → alphabetical by title: Alpha < Beta
        assert_eq!(season_cmp(&a, &b), Ordering::Greater);
        assert_eq!(season_cmp(&b, &a), Ordering::Less);
    }

    #[test]
    fn test_season_sort_title_tiebreaker_case_insensitive() {
        let a = make_series("1", "the bear", 2);
        let b = make_series("2", "The Bear", 2);
        assert_eq!(season_cmp(&a, &b), Ordering::Equal);
    }

    #[test]
    fn test_season_sort_zero_count() {
        let a = make_series("1", "Z", 0);
        let b = make_series("2", "A", 1);
        // 0 < 1 regardless of title
        assert_eq!(season_cmp(&a, &b), Ordering::Less);
    }

    #[test]
    fn test_season_sort_ascending_via_callback() {
        let cmp = table_comparator(|a: &SeriesInfo, b: &SeriesInfo, col: &str| match col {
            "seasons" => a
                .season_count
                .cmp(&b.season_count)
                .then_with(|| jumbie_shared::formatting::natural_cmp(&a.title, &b.title)),
            _ => Ordering::Equal,
        });

        let a = make_series("1", "Zoo", 3);
        let b = make_series("2", "Ant", 1);
        // Ascending: Ant (1 season) < Zoo (3 seasons)
        assert_eq!(
            cmp.run((a.clone(), b.clone(), "seasons".into())),
            Ordering::Greater
        );
        assert_eq!(cmp.run((b, a, "seasons".into())), Ordering::Less);
    }

    #[test]
    fn test_season_sort_descending_via_reverse() {
        let cmp = table_comparator(|a: &SeriesInfo, b: &SeriesInfo, col: &str| match col {
            "seasons" => a
                .season_count
                .cmp(&b.season_count)
                .then_with(|| jumbie_shared::formatting::natural_cmp(&a.title, &b.title)),
            _ => Ordering::Equal,
        });

        let a = make_series("1", "Ant", 1);
        let b = make_series("2", "Zoo", 3);
        let ord = cmp.run((a, b, "seasons".into()));
        assert_eq!(ord, Ordering::Less); // Ant (1) < Zoo (3) ascending
        assert_eq!(ord.reverse(), Ordering::Greater); // reversed: Zoo (3) < Ant (1)
    }

    #[test]
    fn test_season_sort_tiebreaker_descending() {
        let cmp = table_comparator(|a: &SeriesInfo, b: &SeriesInfo, col: &str| match col {
            "seasons" => a
                .season_count
                .cmp(&b.season_count)
                .then_with(|| jumbie_shared::formatting::natural_cmp(&a.title, &b.title)),
            _ => Ordering::Equal,
        });

        let a = make_series("1", "Beta", 2);
        let b = make_series("2", "Alpha", 2);
        // Ascending: Alpha < Beta
        let ord = cmp.run((a, b, "seasons".into()));
        assert_eq!(ord, Ordering::Greater); // Beta > Alpha
        // Descending: Beta < Alpha
        assert_eq!(ord.reverse(), Ordering::Less);
    }

    #[test]
    fn test_label_missing_is_red() {
        let mut s = make_series("1", "A", 1);
        s.monitored_missing_count = 2;
        s.episodes_counts = (4, 6);
        let (class, title) = episode_count_label(&s);
        assert_eq!(class, "text-danger");
        assert_eq!(title, "2 wanted episodes");
    }

    #[test]
    fn test_label_queued_is_yellow() {
        let mut s = make_series("1", "A", 1);
        s.queued_count = 3;
        s.episodes_counts = (2, 5);
        let (class, title) = episode_count_label(&s);
        assert_eq!(class, "text-warning");
        assert_eq!(title, "3 episodes in queue");
    }

    #[test]
    fn test_label_queued_wins_over_missing() {
        // An episode already downloading supersedes the missing state it will resolve.
        let mut s = make_series("1", "A", 1);
        s.monitored_missing_count = 1;
        s.queued_count = 2;
        s.episodes_counts = (2, 5);
        let (class, title) = episode_count_label(&s);
        assert_eq!(class, "text-warning");
        assert_eq!(title, "1 wanted episode · 2 queued");
    }

    #[test]
    fn test_label_complete_is_green() {
        let mut s = make_series("1", "A", 1);
        s.episodes_counts = (6, 6);
        let (class, title) = episode_count_label(&s);
        assert_eq!(class, "text-success");
        assert_eq!(title, "");
    }

    #[test]
    fn test_label_green_when_downloaded_exceeds_expected() {
        // Downloads outside the configured range (e.g. past a season's cell_count)
        // still count as downloaded, so more downloads than expected is complete.
        let mut s = make_series("1", "A", 1);
        s.episodes_counts = (9, 6);
        let (class, title) = episode_count_label(&s);
        assert_eq!(class, "text-success");
        assert_eq!(title, "");
    }

    #[test]
    fn test_label_neither_is_default() {
        let mut s = make_series("1", "A", 1);
        s.episodes_counts = (3, 6);
        let (class, title) = episode_count_label(&s);
        assert_eq!(class, "");
        assert_eq!(title, "");
    }

    #[test]
    fn test_table_comparator_wraps_correctly() {
        let cmp = table_comparator(|a: &i32, b: &i32, col: &str| match col {
            "value" => a.cmp(b),
            _ => Ordering::Equal,
        });

        assert_eq!(cmp.run((5, 3, "value".into())), Ordering::Greater);
        assert_eq!(cmp.run((3, 5, "value".into())), Ordering::Less);
        assert_eq!(cmp.run((3, 3, "value".into())), Ordering::Equal);
        assert_eq!(cmp.run((3, 5, "other".into())), Ordering::Equal);
    }
}
