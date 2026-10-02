use crate::components::common::SettingsBuilder;
use crate::components::common::form_fields::{CheckboxInput, TextInput, TooltipBuilder};
use crate::components::common::icons::{
    AlertTriangleIcon, FolderIcon, LockIcon, MinusIcon, PlusIcon, TrashIcon,
};
use crate::components::common::standard_modal::StandardModal;
use crate::components::common::table_builder::{TableBuilder, TableVariant};
use crate::components::common::toast::{NotificationType, show_toast};
use crate::components::common::{SearchInput, ViewHeader};
use crate::components::series_library::remove_series_modal::{RemoveModalMode, RemoveSeriesModal};
use crate::hooks::use_config::{ConfigContext, use_config};
use crate::hooks::use_persistent_table_state;
use crate::hooks::use_table_search::use_table_search;
use crate::hooks::use_table_selection::{TableSelection, use_table_selection};
use jumbie_shared::types::{CompletionStatus, DEFAULT_MONITOR_MODE, OrganizedSeriesItem};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Mobile cards let the path wrap around the status/completion badges, so it
/// can't use a CSS single-line ellipsis. Cap it at a generous character count
/// instead and append an ellipsis. Character-based (not byte-based) so
/// multi-byte folder names are never split mid-character.
const CARD_PATH_MAX_CHARS: usize = 120;

fn truncate_card_path(path: &str) -> String {
    if path.chars().count() <= CARD_PATH_MAX_CHARS {
        return path.to_string();
    }
    let mut truncated: String = path.chars().take(CARD_PATH_MAX_CHARS).collect();
    truncated.push('…');
    truncated
}

#[component]
pub fn SystemOrganizedSeries() -> impl IntoView {
    let ConfigContext {
        config: _,
        set_config: global_set_config,
    } = use_config();
    let (items, set_items) = signal(Vec::<jumbie_shared::types::OrganizedSeriesItem>::new());
    let (is_loading, set_is_loading) = signal(true);

    // Folder-name search; select-all uses the filtered view.
    let (set_search_query, filtered_items) =
        use_table_search(items.into(), |i: &OrganizedSeriesItem| {
            i.folder_name.clone()
        });

    let (is_edit_mode, set_is_edit_mode) = signal(false);

    let TableSelection {
        selected: selected_paths,
        is_select_all,
        disabled: select_all_disabled,
        select_all,
        toggle: toggle_selection,
        clear: clear_selection,
    } = use_table_selection(filtered_items, |i: &OrganizedSeriesItem| {
        i.absolute_path.clone()
    });

    let toggle_edit_mode = move |_| {
        if is_edit_mode.get() {
            clear_selection.run(());
        }
        set_is_edit_mode.update(|m| *m = !*m);
    };

    let (sort_col, sort_asc, on_sort) =
        use_persistent_table_state("managed_folders".to_string(), "name".to_string(), true);

    // Fetches the fresh series list from the API and writes it into the
    // `fetch_series` cache so the SeriesLibrary reflects changes (hide/unhide/add/remove)
    // without requiring a manual refresh.
    let refresh_series_cache = move || {
        crate::utils::invalidate_cache_prefix("fetch_series");
        spawn_local(async move {
            if let Ok(data) = crate::api::fetch_series().await {
                crate::utils::write_cache("fetch_series", &data);
            }
        });
    };

    // Background refresh without the loading flash — used after add/remove
    // operations so the table updates smoothly.
    let refresh_items = move || {
        spawn_local(async move {
            match crate::api::fetch_organized_series().await {
                Ok(data) => {
                    set_items.try_update(|i| *i = data.clone());
                    crate::utils::write_cache("fetch_organized_series", &data);
                }
                Err(e) => show_toast(
                    format!("Failed to load series: {}", e),
                    NotificationType::Error,
                ),
            }
        });
    };

    // Refreshes the series table after destination-root changes without hiding the current
    // items. Polls with retries so the backend has time to settle filesystem state.
    let refresh_items_after_root_change = move || {
        crate::utils::invalidate_cache_prefix("fetch_organized_series");
        spawn_local(async move {
            for _ in 0..10 {
                if let Ok(data) = crate::api::fetch_organized_series().await
                    && !data.is_empty()
                {
                    set_items.try_update(|i| *i = data.clone());
                    crate::utils::write_cache("fetch_organized_series", &data);
                    return;
                }
                gloo_timers::future::TimeoutFuture::new(500).await;
            }
            // Fallback: one last attempt, even if empty
            if let Ok(data) = crate::api::fetch_organized_series().await {
                set_items.try_update(|i| *i = data.clone());
                crate::utils::write_cache("fetch_organized_series", &data);
            }
        });
    };

    crate::utils::use_api_cache(
        "fetch_organized_series".to_string(),
        || crate::api::fetch_organized_series(),
        move |data| {
            set_items.try_update(|i| *i = data);
            set_is_loading.try_update(|l| *l = false);
        },
    );

    // Uses the same `batch_edit_organized_series` call as the bulk footer so
    // that hidden series are unhidden (data preserved) rather than deleted and
    // re-created with a fresh UUID as `create_series` would do.
    let add_untracked = move |path: String| {
        let payload = jumbie_shared::types::BatchEditOrganizedSeriesPayload {
            paths: vec![path],
            operation: "add_to_library".to_string(),
            delete_files: false,
        };
        crate::utils::spawn_api_toast(
            crate::api::batch_edit_organized_series(payload),
            None,
            move |_| {
                crate::components::common::toast::show_success("Series added successfully");
                refresh_items();
                refresh_series_cache();
            },
        );
    };

    // Stores the paths queued for deletion — written before opening the modal,
    // read on confirm. Using a signal (not Rc<RefCell>) so it's Send+Sync for
    // use in leptos Signal::derive closures.
    let (remove_paths, set_remove_paths) = signal(Vec::<String>::new());
    let (show_remove_modal, set_show_remove_modal) = signal(false);
    let (remove_delete_files, set_remove_delete_files) = signal(true);

    // Dummy signals for Series-mode props (unused in Folder mode).
    let (_dummy_conf, _set_dummy_conf) = signal(false);
    let (_dummy_eps, _set_dummy_eps) = signal(false);
    let (_dummy_data, _set_dummy_data) = signal(false);

    let open_individual_remove = move |path: String| {
        set_remove_paths.set(vec![path]);
        set_remove_delete_files.set(true);
        set_show_remove_modal.set(true);
    };

    let open_batch_remove = move |_| {
        let paths: Vec<String> = selected_paths.get().into_iter().collect();
        set_remove_paths.set(paths);
        set_remove_delete_files.set(true);
        set_show_remove_modal.set(true);
    };

    let toggle_library_visibility = move |series_id: String, hidden: bool| {
        crate::utils::spawn_api_toast(
            crate::api::toggle_library_visibility(series_id.clone(), hidden),
            None,
            move |_| {
                refresh_items();
                refresh_series_cache();
                // Drop stale series-scoped caches (details, calendar windows,
                // wanted lists) so the hidden/unhidden series never resurfaces
                // stale data in other views.
                crate::utils::invalidate_series_caches(std::slice::from_ref(&series_id));
            },
        );
    };

    // Delete-folder button; `locked` disables it with an explanatory tooltip.
    let render_delete_button = move |path_for_del: String, locked: bool| -> AnyView {
        let title = if locked {
            "Series is currently being modified — cannot delete"
        } else {
            "Delete folder"
        };
        view! {
            <button
                class="btn btn-sm btn-icon btn-danger"
                class:cursor-not-allowed=locked
                prop:disabled=locked
                title=title
                on:click=move |ev| {
                    if !locked {
                        ev.stop_propagation();
                        open_individual_remove(path_for_del.clone())
                    }
                }
            >
                <span class="icon"><TrashIcon /></span>
            </button>
        }
        .into_any()
    };

    // Shared post-operation refresh. `keep_edit_mode` is set for library
    // add/remove so the user can keep batching without re-entering edit mode.
    let post_operation_cleanup = move |keep_edit_mode: bool| {
        refresh_items();
        refresh_series_cache();
        clear_selection.run(());
        if !keep_edit_mode {
            set_is_edit_mode.try_update(|m| *m = false);
        }
    };

    // Status badge: In Library / Hidden / Untracked.
    let render_status_badge = move |is_tracked: bool, hidden_in_library: bool| -> AnyView {
        let (text, class) = if is_tracked {
            if hidden_in_library {
                ("Hidden", "badge badge-secondary")
            } else {
                ("In Library", "badge badge-success")
            }
        } else {
            ("Untracked", "badge badge-secondary")
        };
        view! { <span class=class>{text}</span> }.into_any()
    };

    // Completion badge; `show_placeholder` renders "—" for None in the table column.
    let render_completion_badge =
        move |item: &OrganizedSeriesItem, show_placeholder: bool| -> AnyView {
            match item.completion_status.as_ref() {
                Some(status) => {
                    let (badge_class, label) = match status {
                        CompletionStatus::Complete => ("badge badge-success", "Complete"),
                        CompletionStatus::Partial => ("badge badge-warning", "Partial"),
                        CompletionStatus::NotStarted => ("badge badge-secondary", "Not Started"),
                    };
                    // Per-season counts, newest season first (rather than one
                    // total), so a multi-season series shows each season's progress.
                    let mut tooltip = TooltipBuilder::new().with_title("Episode Progress");
                    if item.season_counts.is_empty() {
                        tooltip = tooltip.with_text("No episodes");
                    } else {
                        let mut seasons: Vec<_> = item.season_counts.iter().collect();
                        seasons.sort_by_key(|a| std::cmp::Reverse(a.season));
                        for season in seasons {
                            tooltip = tooltip.with_section(
                                format!("Season {}", season.season),
                                format!("{} / {}", season.organized, season.expected),
                            );
                        }
                    }
                    tooltip.build_with(badge_class, label).into_any()
                }
                None if show_placeholder => {
                    view! { <span class="badge badge-ghost">{"—"}</span> }.into_any()
                }
                None => view! {}.into_any(),
            }
        };

    // Tracked/untracked actions (Add to Library / Remove from Library / Delete
    // folder); `card` targets card-foot layout instead of table rows.
    let render_action_buttons = move |item: OrganizedSeriesItem, card: bool| -> AnyView {
        let container = if card {
            "flex gap-xs w-full"
        } else {
            "action-cell"
        };
        let btn_outline = if card {
            "btn btn-sm flex-1"
        } else {
            "btn btn-sm"
        };
        let btn_primary = if card {
            "btn btn-sm btn-primary flex-1"
        } else {
            "btn btn-sm btn-primary"
        };

        let is_locked = item.locked;

        if item.is_tracked {
            if let Some(series_id) = item.series_id {
                let hidden = item.hidden_in_library;
                let path_for_del = item.absolute_path.clone();
                let on_toggle_visibility = {
                    let tlv = toggle_library_visibility.clone();
                    move |ev: leptos::ev::MouseEvent| {
                        ev.stop_propagation();
                        tlv(series_id.clone(), !hidden)
                    }
                };
                let toggle_title = if is_locked {
                    "Series is currently being modified by another operation"
                } else {
                    if hidden {
                        "Add to Library"
                    } else {
                        "Remove from Library"
                    }
                };
                view! {
                    <div class={container}>
                        <button
                            class={btn_outline}
                            class=("btn-warning", !hidden)
                            class=("btn-primary", hidden)
                            title=toggle_title
                            on:click=on_toggle_visibility
                        >
                            {if hidden { "Add to Library" } else { "Remove from Library" }}
                        </button>
                        {if hidden {
                            render_delete_button(path_for_del.clone(), is_locked)
                        } else {
                            view!{}.into_any()
                        }}
                    </div>
                }
                .into_any()
            } else {
                view! {}.into_any()
            }
        } else {
            let path_for_untracked = item.absolute_path.clone();
            let path_for_del = item.absolute_path.clone();
            let on_add = {
                let au = add_untracked.clone();
                move |ev: leptos::ev::MouseEvent| {
                    ev.stop_propagation();
                    au(path_for_untracked.clone())
                }
            };
            view! {
                <div class={container}>
                    <button class={btn_primary} on:click=on_add>
                        "Add to Library"
                    </button>
                    {render_delete_button(path_for_del.clone(), false)}
                </div>
            }
            .into_any()
        }
    };

    let (dest_roots, set_dest_roots) = signal(Vec::<jumbie_shared::config::DestinationRoot>::new());
    let (new_dest_root, set_new_dest_root) = signal(String::new());

    spawn_local(async move {
        if let Ok(config) = crate::api::fetch_config().await {
            set_dest_roots.try_update(|r| *r = config.organization.destination_roots.clone());
        }
    });

    let is_windows = crate::hooks::use_server_os();

    let add_dest_root = move |_| {
        let val = new_dest_root.get();
        if val.is_empty() {
            return;
        }

        spawn_local(async move {
            if let Ok(mut config) = crate::api::fetch_config().await {
                let mut current_roots = config.organization.destination_roots.clone();
                if !current_roots
                    .iter()
                    .any(|r| r.path.to_string_lossy() == val)
                {
                    current_roots.push(jumbie_shared::config::DestinationRoot::new(val.clone()));
                    config.organization.destination_roots = current_roots;

                    match crate::api::save_config(config).await {
                        Ok(_) => {
                            show_toast("Added destination root", NotificationType::Success);
                            set_dest_roots.try_update(|r| {
                                r.push(jumbie_shared::config::DestinationRoot::new(val.clone()))
                            });
                            set_new_dest_root.try_update(|r| *r = String::new());
                            // Refresh the global config signal so other components see the change
                            if let Ok(updated) = crate::api::fetch_config().await {
                                global_set_config.try_update(|c| *c = Some(updated));
                            }
                            refresh_items_after_root_change();
                        }
                        Err(e) => {
                            show_toast(
                                format!("Failed to save config: {}", e.user_message()),
                                NotificationType::Error,
                            );
                        }
                    }
                } else {
                    show_toast("Destination root already exists", NotificationType::Warning);
                }
            }
        });
    };

    let remove_dest_root = move |root_to_remove: String| {
        spawn_local(async move {
            if let Ok(mut config) = crate::api::fetch_config().await {
                let mut current_roots = config.organization.destination_roots.clone();

                current_roots.retain(|r| r.path.to_string_lossy() != root_to_remove);
                config.organization.destination_roots = current_roots;

                match crate::api::save_config(config).await {
                    Ok(_) => {
                        show_toast("Removed destination root", NotificationType::Success);
                        set_dest_roots.try_update(|r| {
                            r.retain(|item| item.path.to_string_lossy() != root_to_remove)
                        });
                        // Refresh the global config signal so other components see the change
                        if let Ok(updated) = crate::api::fetch_config().await {
                            global_set_config.try_update(|c| *c = Some(updated));
                        }
                        refresh_items_after_root_change();
                    }
                    Err(e) => {
                        show_toast(
                            format!("Failed to save config: {}", e),
                            NotificationType::Error,
                        );
                    }
                }
            }
        });
    };

    // Toggles whether a root's subdirectories are auto-listed in Managed Folders.
    // The root still appears in destination selectors either way.
    let set_dest_root_flag = move |root_path: String, include_subdirs: bool| {
        spawn_local(async move {
            if let Ok(mut config) = crate::api::fetch_config().await {
                let mut current_roots = config.organization.destination_roots.clone();
                let Some(root) = current_roots
                    .iter_mut()
                    .find(|r| r.path.to_string_lossy() == root_path)
                else {
                    return;
                };
                root.include_subdirs_in_managed = include_subdirs;
                config.organization.destination_roots = current_roots;

                match crate::api::save_config(config).await {
                    Ok(_) => {
                        set_dest_roots.try_update(|roots| {
                            if let Some(root) = roots
                                .iter_mut()
                                .find(|r| r.path.to_string_lossy() == root_path)
                            {
                                root.include_subdirs_in_managed = include_subdirs;
                            }
                        });
                        // Refresh the global config signal so other components see the change
                        if let Ok(updated) = crate::api::fetch_config().await {
                            global_set_config.try_update(|c| *c = Some(updated));
                        }
                        refresh_items_after_root_change();
                    }
                    Err(e) => {
                        show_toast(
                            format!("Failed to save config: {}", e.user_message()),
                            NotificationType::Error,
                        );
                    }
                }
            }
        });
    };

    let batch_edit = move |operation: &'static str| {
        let paths: Vec<String> = selected_paths.get().into_iter().collect();

        let payload = jumbie_shared::types::BatchEditOrganizedSeriesPayload {
            paths,
            operation: operation.to_string(),
            delete_files: false,
        };
        crate::utils::spawn_api_toast(
            crate::api::batch_edit_organized_series(payload),
            None,
            move |_| {
                crate::components::common::toast::show_success("Batch operation successful");
                // Keep edit mode on so the user can continue batching.
                post_operation_cleanup(true);
            },
        );
    };

    let (show_modal, set_show_modal) = signal(false);
    let (custom_path, set_custom_path) = signal(String::new());
    let (is_bulk_import, set_is_bulk_import) = signal(false);

    let (show_confirm_modal, set_show_confirm_modal) = signal(false);
    let (preview_items, set_preview_items) =
        signal(Vec::<jumbie_shared::types::PreviewSeriesItem>::new());

    let (show_batch_move_modal, set_show_batch_move_modal) = signal(false);

    let handle_next_preview = move || {
        let path = custom_path.get();
        if !path.is_empty() {
            spawn_local(async move {
                let req = jumbie_shared::types::PreviewSeriesRequest {
                    path: path.clone(),
                    is_bulk: is_bulk_import.get(),
                };
                match crate::api::preview_series_import(req).await {
                    Ok(items) => {
                        set_preview_items.try_update(|i| *i = items);
                        set_show_modal.try_update(|m| *m = false);
                        set_show_confirm_modal.try_update(|m| *m = true);
                    }
                    Err(e) => show_toast(
                        format!("Failed to preview series: {}", e),
                        NotificationType::Error,
                    ),
                }
            });
        }
    };

    let confirm_add_path = move || {
        // Filter to only selected items before sending to the backend.
        // The backend also filters by `selected`, but doing it here reduces
        // payload size and avoids sending irrelevant data.
        let mut items: Vec<_> = preview_items.get();
        items.retain(|i| i.selected);

        let req = jumbie_shared::types::ConfirmSeriesImportRequest {
            items,
            scan_for_existing: true,
            monitor_mode: Some(DEFAULT_MONITOR_MODE),
            quality_profile: None,
            release_profile: None,
        };
        crate::utils::spawn_api_toast(crate::api::bulk_create_series(req), None, move |_| {
            crate::components::common::toast::show_success("Series added successfully");
            refresh_items();
            refresh_series_cache();
            set_show_confirm_modal.try_update(|m| *m = false);
            set_custom_path.try_update(|p| *p = String::new());
            set_is_bulk_import.try_update(|b| *b = false);
            set_preview_items.try_update(|i| *i = Vec::new());
        });
    };

    pub fn make_id(input: &str) -> String {
        let mut h = DefaultHasher::new();
        input.hash(&mut h);
        format!("{:x}", h.finish())
    }

    // Tracks only whether any destination roots exist, so the list below is
    // rebuilt only when a root is added or removed.
    let has_dest_roots = Memo::new(move |_| !dest_roots.get().is_empty());

    SettingsBuilder::new("organized-series")
        .section("Destination Roots", |section| {
            section.description("Directories where the organizer places downloaded and renamed series. Click a root's path to toggle whether its subfolders appear in Managed Folders & Monitoring — the root stays selectable when adding or moving series, and series in the library still list.")
                   .field(view! {
                {move || {
                    if !has_dest_roots.get() {
                        view! {}.into_any()
                    } else {
                        view! {
                            <div class="flex flex-col gap-sm mb-md">
                                <For
                                    each=move || dest_roots.get()
                                    key=|root| (root.path.clone(), root.include_subdirs_in_managed)
                                    children=move |root| {
                                        let path_str = root.path.to_string_lossy().to_string();
                                        let include = root.include_subdirs_in_managed;
                                        let path_for_toggle = path_str.clone();
                                        let path_for_remove = path_str.clone();
                                        view! {
                                            <div class="flex items-center gap-sm min-w-0 overflow-hidden">
                                                <input
                                                    type="text"
                                                    class=if include { "form-control flex-1 dest-root-path" } else { "form-control flex-1 dest-root-path excluded" }
                                                    id=make_id(&path_str)
                                                    readonly=true
                                                    value=path_str.clone()
                                                    title=if include {
                                                        "Click to hide this root's subfolders from Managed Folders & Monitoring."
                                                    } else {
                                                        "Click to show this root's subfolders in Managed Folders & Monitoring."
                                                    }
                                                    on:click=move |_| set_dest_root_flag(path_for_toggle.clone(), !include)
                                                />
                                                <button
                                                    class="btn btn-danger btn-icon"
                                                    on:click=move |_| remove_dest_root(path_for_remove.clone())
                                                    title="Remove root"
                                                >
                                                    <span class="icon"><TrashIcon /></span>
                                                </button>
                                            </div>
                                        }
                                    }
                                />
                            </div>
                        }
                        .into_any()
                    }
                }}

                <div class="dest-root-add-row flex items-center gap-sm min-w-0 overflow-hidden">
                    <input
                        type="text"
                        class="form-control flex-1"
                        id="new-dest-root"
                        placeholder=move || crate::hooks::path_placeholder(is_windows.get(), crate::hooks::EXAMPLE_MEDIA_PATH)
                        prop:value=new_dest_root
                        on:input=move |e| set_new_dest_root.set(event_target_value(&e))
                        on:keydown=move |e| {
                            if e.key() == "Enter" {
                                add_dest_root(());
                            }
                        }
                    />
                    <button class="btn btn-primary" on:click=move |_| add_dest_root(()) disabled=move || new_dest_root.get().trim().is_empty()>
                        <span class="icon"><PlusIcon /></span>
                        "Add Root"
                    </button>
                </div>
            })
        })
        .section("Managed Folders & Monitoring", |section| {
            section.description("Folders in your organized directory and their monitor status.")
                .field(view! {
                    {move || {
                            let managed_columns: Vec<
                                crate::components::common::table_builder::ManagedColumn<OrganizedSeriesItem>
                            > = vec![
                                crate::components::common::table_builder::ManagedColumn {
                                    id: "name".into(),
                                    label: "Folder Name".into(),
                                    sortable: true,
                                    class: "".into(),
                                    cell_render: Callback::new(move |item: OrganizedSeriesItem| {
                                        let locked = item.locked;
                                        view! {
                                            <div class="managed-folder-name" title={item.folder_name.clone()}>
                                                <FolderIcon/>
                                                {item.folder_name.clone()}
                                                {if locked {
                                                    view! {
                                                        <span class="icon icon-1rem text-warning ml-xs" title="Series is currently being modified by another operation">
                                                            <LockIcon/>
                                                        </span>
                                                    }.into_any()
                                                } else {
                                                    view! {}.into_any()
                                                }}
                                            </div>
                                        }.into_any()
                                    }),
                                },
                                crate::components::common::table_builder::ManagedColumn {
                                    id: "path".into(),
                                    label: "Path".into(),
                                    sortable: true,
                                    class: "".into(),
                                    cell_render: Callback::new(move |item: OrganizedSeriesItem| {
                                        view! {
                                            <span class="path-truncate text-xs text-muted" title=item.absolute_path.clone()>{item.absolute_path.clone()}</span>
                                        }.into_any()
                                    }),
                                },
                                crate::components::common::table_builder::ManagedColumn {
                                    id: "status".into(),
                                    label: "Status".into(),
                                    sortable: true,
                                    class: "".into(),
                                    cell_render: Callback::new(move |item: OrganizedSeriesItem| {
                                        render_status_badge(item.is_tracked, item.hidden_in_library)
                                    }),
                                },
                                crate::components::common::table_builder::ManagedColumn {
                                    id: "completion".into(),
                                    label: "Completion".into(),
                                    sortable: true,
                                    class: "w-32".into(),
                                    cell_render: Callback::new(move |item: OrganizedSeriesItem| {
                                        render_completion_badge(&item, true)
                                    }),
                                },
                                crate::components::common::table_builder::ManagedColumn {
                                    id: "actions".into(),
                                    label: "Actions".into(),
                                    sortable: false,
                                    class: "col-actions".into(),
                                    cell_render: Callback::new(move |item: OrganizedSeriesItem| {
                                        render_action_buttons(item, false)
                                    }),
                                },
                            ];

                            let compare = Callback::new(
                                move |(a, b, col): (OrganizedSeriesItem, OrganizedSeriesItem, String)| {
                                    match col.as_str() {
                                        "name" => jumbie_shared::formatting::natural_cmp(&a.folder_name, &b.folder_name),
                                        "path" => jumbie_shared::formatting::natural_cmp(&a.absolute_path, &b.absolute_path),
                                        "status" => {
                                            let a_rank = if a.is_tracked { if a.hidden_in_library { 2 } else { 1 } } else { 3 };
                                            let b_rank = if b.is_tracked { if b.hidden_in_library { 2 } else { 1 } } else { 3 };
                                            a_rank.cmp(&b_rank)
                                        }
                                        "completion" => {
                                            // Sort by: completion rank → completion ratio → folder name
                                            let a_rank = a.completion_status.as_ref().map(|c| c.rank()).unwrap_or(-1);
                                            let b_rank = b.completion_status.as_ref().map(|c| c.rank()).unwrap_or(-1);
                                            let rank_cmp = a_rank.cmp(&b_rank);
                                            if rank_cmp != std::cmp::Ordering::Equal {
                                                return rank_cmp;
                                            }
                                            // Ratio: organized / expected (higher = more complete)
                                            let a_ratio = if a.total_episodes_expected > 0 {
                                                a.total_episodes_organized as f64 / a.total_episodes_expected as f64
                                            } else if a.total_episodes_organized > 0 {
                                                f64::MAX
                                            } else {
                                                0.0
                                            };
                                            let b_ratio = if b.total_episodes_expected > 0 {
                                                b.total_episodes_organized as f64 / b.total_episodes_expected as f64
                                            } else if b.total_episodes_organized > 0 {
                                                f64::MAX
                                            } else {
                                                0.0
                                            };
                                            let ratio_cmp = a_ratio.partial_cmp(&b_ratio).unwrap_or(std::cmp::Ordering::Equal);
                                            if ratio_cmp != std::cmp::Ordering::Equal {
                                                return ratio_cmp;
                                            }
                                            // Tiebreaker: folder name alphabetically
                                            jumbie_shared::formatting::natural_cmp(&a.folder_name, &b.folder_name)
                                        }
                                        _ => std::cmp::Ordering::Equal,
                                    }
                                },
                            );

                            let on_row_click = Some(Callback::new(move |item: OrganizedSeriesItem| {
                                if is_edit_mode.get() {
                                    toggle_selection.run(item.absolute_path.clone());
                                }
                            }));

                            let is_selected: Option<Callback<OrganizedSeriesItem, bool>> = Some(Callback::new(move |item: OrganizedSeriesItem| {
                                selected_paths.get().contains(&item.absolute_path)
                            }));

                            let card_view = {
                                Some(Callback::new(move |item: OrganizedSeriesItem| {
                                    let path_for_cb = item.absolute_path.clone();
                                    let path_for_click = item.absolute_path.clone();

                                    let on_card_click = {
                                        let p = path_for_click.clone();
                                        move |_| {
                                            if is_edit_mode.get() {
                                                toggle_selection.run(p.clone());
                                            }
                                        }
                                    };

                                    view! {
                                        <div class="card p-md flex flex-col gap-sm"
                                            on:click=on_card_click
                                        >
                                            // The badge block floats so the folder name and path
                                            // flow around it instead of being squeezed into a column.
                                            <div class="card-row-with-badge card-row-with-badge--wrap">
                                                <div class="card-badge-float">
                                                    {render_status_badge(item.is_tracked, item.hidden_in_library)}
                                                    {render_completion_badge(&item, false)}
                                                </div>
                                                <div class=move || if !is_edit_mode.get() {
                                                    "card-text selectable"
                                                } else {
                                                    "card-text"
                                                }>
                                                    {move || {
                                                        if is_edit_mode.get() {
                                                            let path_sel = path_for_cb.clone();
                                                            let path_sel_checked = path_sel.clone();
                                                            let path_sel_change = path_sel.clone();
                                                            Some(view! {
                                                                <input
                                                                    type="checkbox"
                                                                    class="table-checkbox card-text-checkbox"
                                                                    prop:checked=move || selected_paths.get().contains(&path_sel_checked)
                                                                    on:click=move |ev| ev.stop_propagation()
                                                                    on:change=move |_| toggle_selection.run(path_sel_change.clone())
                                                                />
                                                            }.into_any())
                                                        } else {
                                                            None
                                                        }
                                                    }}
                                                    <span class="font-bold">{item.folder_name.clone()}</span>
                                                    {if item.locked {
                                                        view! {
                                                            <span class="icon icon-1rem text-warning align-middle ml-xs" title="Series is currently being modified by another operation">
                                                                <LockIcon/>
                                                            </span>
                                                        }.into_any()
                                                    } else {
                                                        view! {}.into_any()
                                                    }}
                                                    <span class="card-text-path block text-xs text-muted" title=item.absolute_path.clone()>
                                                        {truncate_card_path(&item.absolute_path)}
                                                    </span>
                                                </div>
                                            </div>

                                            <div class="flex justify-end pt-sm border-t mt-xs">
                                                {render_action_buttons(item, true)}
                                            </div>
                                        </div>
                                    }.into_any()
                                }))
                            };

                            // Batch add/remove are no-ops when every selected folder
                            // already has (or lacks) a library entry, so the matching
                            // footer button is greyed out in those cases.
                            let selection_library_counts = move || -> (usize, usize) {
                                let selected = selected_paths.get();
                                if selected.is_empty() {
                                    return (0, 0);
                                }
                                let current = items.get();
                                let in_library = selected
                                    .iter()
                                    .filter(|path| {
                                        current.iter().any(|i| {
                                            &i.absolute_path == *path
                                                && i.is_tracked
                                                && !i.hidden_in_library
                                        })
                                    })
                                    .count();
                                (in_library, selected.len())
                            };

                            let all_selected_in_library = move || {
                                let (in_library, total) = selection_library_counts();
                                total > 0 && in_library == total
                            };

                            let all_selected_not_in_library = move || {
                                let (in_library, total) = selection_library_counts();
                                total > 0 && in_library == 0
                            };

                            TableBuilder::new(sort_col, sort_asc)
                                .container_class(Signal::derive(move || "table-container".to_string()))
                                .table_variant(TableVariant::Hover)
                                .edit_mode(is_edit_mode)
                                .selection_mode(is_select_all, select_all)
                                .select_all_disabled(select_all_disabled)
                                .loading(Signal::derive(move || is_loading.get()))
                                .empty_message("No folders found in the destination directory.")
                                .on_sort(on_sort)
                                .header_view(view! {
                                    <ViewHeader
                                        id="organized-series-header"
                                        search_bar={view! {
                                            <SearchInput
                                                id="organized-series-search"
                                                placeholder="Search folders..."
                                                class="search-input"
                                                on_search=Callback::new(move |v: String| set_search_query.set(v))
                                            />
                                        }.into_any()}
                                        actions={view! {
                                            {TableBuilder::render_edit_button(is_edit_mode, Callback::new(toggle_edit_mode))}
                                            <button class="btn btn-primary" on:click=move |_| set_show_modal.set(true)>
                                                <span class="icon"><PlusIcon /></span>
                                                "Custom Path"
                                            </button>
                                        }.into_any()}
                                    />
                                }.into_any())
                                .footer_view(view! {
                                    <div class="bulk-action-footer organized-series-footer">
                                        <div class="bulk-action-content">
                                            <div class="selection-count">
                                                <strong>{move || format!("{} Folders Selected", selected_paths.get().len())}</strong>
                                            </div>
                                            <div class="bulk-actions-col flex gap-md align-center">
                                                <button class="btn btn-warning"
                                                    disabled=move || selected_paths.get().is_empty() || all_selected_not_in_library()
                                                    on:click=move |_| batch_edit("remove_from_library")
                                                >
                                                    <span class="icon"><MinusIcon /></span>
                                                    "Remove from library"
                                                </button>
                                                <button class="btn btn-primary"
                                                    disabled=move || selected_paths.get().is_empty() || all_selected_in_library()
                                                    on:click=move |_| batch_edit("add_to_library")
                                                >
                                                    <span class="icon"><PlusIcon /></span>
                                                    "Add to library"
                                                </button>
                                                <div class="divider-vertical mx-sm"></div>
                                                <button class="btn btn-danger"
                                                    disabled=move || selected_paths.get().is_empty()
                                                    on:click=open_batch_remove
                                                >
                                                    <span class="icon"><TrashIcon /></span>
                                                    "Delete"
                                                </button>
                                                <button class="btn btn-primary"
                                                    disabled=move || selected_paths.get().is_empty()
                                                    on:click=move |_| set_show_batch_move_modal.set(true)
                                                >
                                                    <span class="icon"><FolderIcon /></span>
                                                    "Move"
                                                </button>
                                            </div>
                                        </div>
                                    </div>
                                }.into_any())
                                .build_managed(
                                                                    filtered_items,
                                                                    managed_columns,
                                                                    compare,
                                                                    on_row_click,
                                                                    is_selected,
                                                                    card_view,
                                                                )
                    }}
                })
        })
        .raw_section(view! {
            <StandardModal
                show=show_modal
                on_close=move |_| {
                    set_show_modal.set(false);
                    set_custom_path.set(String::new());
                    set_is_bulk_import.set(false);
                }
                title="Add Custom Path"
                footer=view! {
                    <div class="modal-footer flex items-center justify-end gap-md">
                        <button class="btn btn-primary" on:click=move |_| handle_next_preview()>
                            "Next"
                        </button>
                    </div>
                }.into_any()
            >
                <TextInput
                    label=Signal::derive(move || if is_bulk_import.get() { "Absolute Path to Parent Folder".to_string() } else { "Absolute Path".to_string() })
                    id="add-path-input".to_string()
                    value=custom_path
                    set_value=Callback::new(move |v| set_custom_path.set(v))
                    placeholder=Signal::derive(move || if is_bulk_import.get() {
                        crate::hooks::path_placeholder(is_windows.get(), crate::hooks::EXAMPLE_SERIES_PARENT_PATH)
                    } else {
                        crate::hooks::path_placeholder(is_windows.get(), crate::hooks::EXAMPLE_SERIES_PATH)
                    })
                />
                <CheckboxInput
                    id="bulk_import_toggle".to_string()
                    checked=is_bulk_import
                    set_checked=Callback::new(move |v| set_is_bulk_import.set(v))
                    label="Bulk import from folder".to_string()
                />
                {move || if is_bulk_import.get() {
                    view! {
                        <p class="text-xs text-muted mt-0">
                            "If checked, the app will read all subdirectories in the path and add them as separate series."
                        </p>
                    }.into_any()
                } else {
                    view! { <span class="hidden"></span> }.into_any()
                }}
            </StandardModal>

            <StandardModal
                show=show_confirm_modal
                on_close=move |_| {
                    set_show_confirm_modal.set(false);
                    set_custom_path.set(String::new());
                    set_is_bulk_import.set(false);
                    set_preview_items.set(Vec::new());
                }
                title="Confirm Import"
                footer=view! {
                    <div class="modal-footer flex items-center justify-end gap-md">
                        <button class="btn btn-primary"
                            disabled=move || preview_items.get().iter().filter(|i| i.selected).count() == 0
                            on:click=move |_| confirm_add_path()>
                            "Import Series"
                        </button>
                    </div>
                }.into_any()
            >
                <div class="mb-md">
                    <p class="text-sm font-semibold mb-sm">"Review series to be imported. You can rename titles or uncheck folders you want to skip."</p>
                        {
                            let result =
                                crate::hooks::use_table_sort::use_table_sort(preview_items.into(), "path".to_string(), true, |a: &jumbie_shared::types::PreviewSeriesItem, b: &jumbie_shared::types::PreviewSeriesItem, col: &str| {
                                        match col {
                                            "path" => jumbie_shared::formatting::natural_cmp(&a.original_folder_name, &b.original_folder_name),
                                            "seasons" => a.season_count.cmp(&b.season_count),
                                            "episodes" => a.episode_count.cmp(&b.episode_count),
                                            "flat" => a.all_files_in_root.cmp(&b.all_files_in_root),
                                            _ => std::cmp::Ordering::Equal,
                                        }
                                    });
                            let sorted_items = result.sorted_data;
                            let toggle_sort = result.toggle_sort;

                            TableBuilder::new(result.sort_column.into(), result.sort_asc.into())
                                .table_class("table hover w-full text-left table-fixed")
                                .column_with_class("select", "", false, "col-checkbox")
                                .column("path", "Detected Path / Series Name", true)
                                .column_with_class("seasons", "Seasons", true, "w-15 text-center")
                                .column_with_class("episodes", "Episodes", true, "w-15 text-center")
                                .column_with_class("flat", "Structure", true, "w-15 text-center")
                                .on_sort(toggle_sort)
                                .build(view! {
                                    <For
                                        each=move || sorted_items.get()
                                        key=|item| item.path.clone()
                                        children=move |item| {
                                            let path_clone = item.path.clone();

                                            let on_toggle = move |e: web_sys::Event| {
                                                let is_checked = event_target_checked(&e);
                                                set_preview_items.update(|items| {
                                                    if let Some(i) = items.iter_mut().find(|x| x.path == path_clone) {
                                                        i.selected = is_checked;
                                                    }
                                                });
                                            };

                                            let path_clone2 = item.path.clone();
                                            let on_title_change = move |e: web_sys::Event| {
                                                let new_val = event_target_value(&e);
                                                set_preview_items.update(|items| {
                                                    if let Some(i) = items.iter_mut().find(|x| x.path == path_clone2) {
                                                        i.final_title = new_val;
                                                    }
                                                });
                                            };

                                            view! {
                                                <tr
                                                    class=if item.already_exists { "opacity-50" } else { "" }
                                                    title=if item.already_exists { Some("Series already exists in managed folders") } else { None::<&'static str> }
                                                >
                                                    <td class="text-center align-middle">
                                                        <input
                                                            type="checkbox"
                                                            class="form-checkbox mx-auto"
                                                            prop:checked=item.selected
                                                            prop:disabled=item.already_exists
                                                            on:change=on_toggle
                                                        />
                                                    </td>
                                                    <td>
                                                        <div class="flex items-center gap-xs mb-1">
                                                            <div class="text-xs text-muted truncate" title=item.path.clone()>{item.original_folder_name.clone()}</div>
                                                            {
                                                                if item.already_exists {
                                                                    view! {
                                                                        <span class="tooltip" data-tooltip="Series already exists in managed folders">
                                                                            <span class="icon text-warning icon-1rem"><AlertTriangleIcon /></span>
                                                                        </span>
                                                                    }.into_any()
                                                                } else {
                                                                    view! { <span class="hidden"></span> }.into_any()
                                                                }
                                                            }
                                                        </div>
                                                        <input
                                                            type="text"
                                                            class=if item.already_exists { "form-control form-control-sm w-full cursor-not-allowed" } else { "form-control form-control-sm w-full" }
                                                            prop:value=item.final_title.clone()
                                                            prop:disabled=item.already_exists
                                                            on:input=on_title_change
                                                        />
                                                    </td>
                                                    <td class="text-center align-middle">{item.season_count}</td>
                                                    <td class="text-center align-middle">{item.episode_count}</td>
                                                    <td class="text-center align-middle">
                                                        {if item.all_files_in_root {
                                                            view! {
                                                                <span class="badge badge-info text-xs" title="All episode files are in the series root folder; Flatten Series will be auto-enabled">
                                                                    "Flat"
                                                                </span>
                                                            }.into_any()
                                                        } else {
                                                            view! {
                                                                <span class="badge badge-secondary text-xs" title="Episode files are organized in season subfolders">
                                                                    "Season Folders"
                                                                </span>
                                                            }.into_any()
                                                        }}
                                                    </td>
                                                </tr>
                                            }
                                        }
                                    />
                                })
                        }
                </div>
            </StandardModal>

            <RemoveSeriesModal
                show=show_remove_modal
                set_show=set_show_remove_modal
                series_ids=Signal::derive(move || remove_paths.get())
                delete_configurations=Signal::derive(move || true)
                set_delete_configurations=_set_dummy_conf
                delete_episodes=Signal::derive(move || true)
                set_delete_episodes=_set_dummy_eps
                delete_episode_data=Signal::derive(move || true)
                set_delete_episode_data=_set_dummy_data
                mode=RemoveModalMode::Folder
                delete_files=Signal::from(remove_delete_files)
                set_delete_files=set_remove_delete_files
                on_success=Callback::new(move |_| {
                    crate::components::common::toast::show_success("Series removed successfully");
                    post_operation_cleanup(false);
                })
            />

            <crate::components::management::batch_move_modal::BatchMoveModal
                show=show_batch_move_modal
                set_show=set_show_batch_move_modal
                selected_paths=selected_paths
                dest_roots=dest_roots
                on_success=Callback::new(move |_| {
                    post_operation_cleanup(false);
                })
            />
        })
        .build()
}
