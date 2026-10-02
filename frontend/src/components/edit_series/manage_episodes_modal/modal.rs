use crate::api::{
    assign_series_file, batch_assign_series_files, delete_series_files, fetch_series_files,
    unassign_series_files,
};
use crate::components::common::confirmation_modal::ConfirmationModal;
use crate::components::common::icons::{CheckIcon, EditIcon, TrashIcon, UnassignIcon, XIcon};
use crate::components::common::standard_modal::StandardModal;
use crate::components::common::table_builder::{ManagedColumn, TableBuilder};
use crate::hooks::use_table_selection::{TableSelection, use_table_selection};
use crate::utils::sorting::apply_sort;
use jumbie_shared::media_format::FileKind;
use jumbie_shared::types::{
    AssignFilePayload, BatchAssignPayload, BatchDeletePayload, SeriesFileViewModel,
};
use leptos::prelude::*;
use std::collections::HashMap;

use leptos::control_flow::Show;
use leptos::task::spawn_local;

use super::batch_plan::{PlanError, plan_batch_targets, slot_conflict};
use super::filename_parser::{format_episode_display, parse_season_episode_from_filename};

/// Lowercased file extension shown in (and sorted by) the "Ext" column.
/// Empty string when the filename has no extension.
fn file_ext(filename: &str) -> String {
    std::path::Path::new(filename)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

#[component]
pub fn ManageEpisodesModal(
    show: ReadSignal<bool>,
    set_show: WriteSignal<bool>,
    series_id: ReadSignal<String>,
    #[prop(into)] series_details: Signal<Option<jumbie_shared::types::SeriesDetails>>,
    absolute_numbering: ReadSignal<bool>,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let (sort_column, set_sort_column) = signal("filename".to_string());
    let (sort_desc, set_sort_desc) = signal(false);

    let (assigning_path, set_assigning_path) = signal(None::<String>);
    let (assign_season, set_assign_season) = signal(String::new());
    let (assign_episode, set_assign_episode) = signal(String::new());

    let (batch_start_season, set_batch_start_season) = signal(String::new());
    let (batch_start_episode, set_batch_start_episode) = signal(String::new());
    let (batch_is_multipart, set_batch_is_multipart) = signal(false);
    let (batch_processing, set_batch_processing) = signal(false);

    let (show_delete_modal, set_show_delete_modal) = signal(false);

    let files_resource = LocalResource::new(move || {
        let id = series_id.get();
        let is_shown = show.get();
        async move {
            if !is_shown || id.is_empty() {
                return vec![];
            }
            fetch_series_files(id).await.unwrap_or_default()
        }
    });

    let TableSelection {
        selected: selected_files,
        is_select_all,
        disabled: select_all_disabled,
        select_all,
        toggle: toggle_selection,
        clear: clear_selection,
    } = use_table_selection(
        Signal::derive(move || files_resource.get().unwrap_or_default()),
        |f: &SeriesFileViewModel| f.path.clone(),
    );

    let refetch_files = move || {
        files_resource.refetch();
        clear_selection.run(());
    };

    let sorted_files = Memo::new(move |_| {
        if !show.get() {
            clear_selection.run(());
            return vec![];
        }
        let mut files = files_resource.get().unwrap_or_default();
        let column = sort_column.get();
        let desc = sort_desc.get();

        apply_sort(&mut files, &column, !desc, |a, b, c| match c {
            "filename" => jumbie_shared::formatting::natural_cmp(&a.filename, &b.filename),
            "path" => jumbie_shared::formatting::natural_cmp(&a.path, &b.path),
            "size" => a.size.cmp(&b.size),
            // Extension: case-insensitive natural order, with a case-sensitive
            // natural filename tie-break so equal extensions keep a stable,
            // deterministic order.
            "ext" => jumbie_shared::formatting::natural_cmp(
                &file_ext(&a.filename),
                &file_ext(&b.filename),
            )
            .then_with(|| {
                jumbie_shared::formatting::natural_cmp_case_sensitive(&a.filename, &b.filename)
            }),
            "assigned" => a
                .assigned_id
                .is_some()
                .cmp(&b.assigned_id.is_some())
                .then_with(|| {
                    jumbie_shared::formatting::natural_cmp(
                        a.assigned_header.as_deref().unwrap_or(""),
                        b.assigned_header.as_deref().unwrap_or(""),
                    )
                }),
            _ => std::cmp::Ordering::Equal,
        });
        files
    });

    // Reactive preview map: path -> (label, is_conflict_active).
    // Derived from the shared batch planner (SSoT) so the preview can never
    // disagree with what `handle_batch_assign` actually assigns.
    let preview_map = Memo::new(move |_| {
        let files = sorted_files.get();
        let selected = selected_files.get();
        let start_s = batch_start_season.get();
        let start_e = batch_start_episode.get();
        let is_multipart = batch_is_multipart.get();

        let settings = series_details
            .get()
            .map(|d| d.config.settings)
            .unwrap_or_default();
        let absolute = absolute_numbering.get();
        let parse = move |p: &str| parse_season_episode_from_filename(p, &settings, absolute);

        // Selected files in display order.
        let ordered: Vec<String> = files
            .iter()
            .filter(|f| selected.contains(&f.path))
            .map(|f| f.path.clone())
            .collect();

        let planned = plan_batch_targets(&ordered, &start_s, &start_e, is_multipart, &parse);

        // Reverse: assigned_header -> its assigned video files, in list order. A label
        // can hold several language variants of one artifact, so a candidate must be
        // checked against all of them. Auxiliary sidecars (subtitle/nfo) share an
        // episode with their video rather than occupying a slot of their own, so they
        // must never be treated as slot occupants for conflict detection.
        let mut assigned_by_label: HashMap<String, Vec<(&str, bool)>> = HashMap::new();
        for f in files.iter().filter(|f| f.kind == FileKind::Video) {
            if let Some(header) = f.assigned_header.as_ref() {
                assigned_by_label
                    .entry(header.clone())
                    .or_default()
                    .push((f.path.as_str(), selected.contains(&f.path)));
            }
        }

        let kind_by_path: HashMap<&str, FileKind> =
            files.iter().map(|f| (f.path.as_str(), f.kind)).collect();

        let mut result: HashMap<String, (String, bool)> = HashMap::new();
        for (path, target) in planned {
            let (label, is_conflict) = match target {
                Err(_) => ("-".to_string(), false),
                Ok(t) => {
                    let label = format_episode_display(&t.season, &t.episode, t.part);
                    // Conflict: the episode already holds a video this file cannot
                    // attach alongside. Rule lives in `slot_conflict` (SSoT).
                    let occupants: &[(&str, bool)] = assigned_by_label
                        .get(&label)
                        .map(|v| v.as_slice())
                        .unwrap_or(&[]);
                    let is_conflict = slot_conflict(
                        kind_by_path.get(path.as_str()) == Some(&FileKind::Video),
                        occupants,
                        &path,
                    );
                    (label, is_conflict)
                }
            };
            result.insert(path, (label, is_conflict));
        }
        result
    });

    // Whether any active (unsuppressed) conflict exists — drives footer warning
    let has_conflicts =
        Memo::new(move |_| preview_map.get().values().any(|(_, conflict)| *conflict));

    let handle_assign = move |_| {
        let path = assigning_path.get();
        if path.is_none() {
            return;
        }
        let path_str = path.unwrap();
        let s_id = series_id.get();
        let s = assign_season.get();
        let e = assign_episode.get();
        let season_str = if s.trim().is_empty() {
            "1".to_string()
        } else {
            s
        };
        if e.trim().is_empty() {
            return;
        }
        let on_refresh = on_refresh;
        let payload = AssignFilePayload {
            path: path_str.clone(),
            season: season_str,
            episode: e,
        };
        crate::utils::spawn_api_toast(assign_series_file(s_id, payload), None, move |_| {
            crate::components::common::toast::show_success("File assigned successfully");
            set_assigning_path.set(None);
            refetch_files();
            on_refresh.run(());
        });
    };

    // Batch assign: selected files in sorted order.
    //
    // The shared planner derives a target per file for every mode. Only a
    // BOTH-provided **multipart** batch is delegated to the backend endpoint,
    // because that is the one path that owns sequential part numbering; every
    // other mode assigns per file so the preview and the result stay identical.
    let handle_batch_assign = move |_| {
        set_batch_processing.set(true);
        let s_id = series_id.get();
        let selected = selected_files.get();
        let on_refresh = on_refresh;

        // Series files: preserve sorted display order
        let ordered_paths: Vec<String> = sorted_files
            .get()
            .into_iter()
            .filter(|f| selected.contains(&f.path))
            .map(|f| f.path)
            .collect();

        let start_season = batch_start_season.get();
        let start_episode_str = batch_start_episode.get();
        let is_multipart = batch_is_multipart.get();
        let both_provided = !start_season.trim().is_empty() && !start_episode_str.trim().is_empty();

        if both_provided && is_multipart {
            let start_episode = start_episode_str.parse::<i32>().unwrap_or(1);
            spawn_local(async move {
                if !ordered_paths.is_empty() {
                    let payload = BatchAssignPayload {
                        paths: ordered_paths.clone(),
                        start_season: start_season.clone(),
                        start_episode,
                        is_multipart,
                    };
                    if let Err(e) = batch_assign_series_files(s_id.clone(), payload).await {
                        crate::components::common::toast::show_error(format!(
                            "Failed to assign series files: {}",
                            e
                        ));
                    }
                }
                refetch_files();
                on_refresh.run(());
                set_batch_processing.set(false);
            });
            return;
        }

        // Every other mode: plan each file via the shared planner.
        let settings = series_details
            .get()
            .map(|d| d.config.settings)
            .unwrap_or_default();
        let absolute = absolute_numbering.get();
        let parse = move |p: &str| parse_season_episode_from_filename(p, &settings, absolute);
        let planned = plan_batch_targets(
            &ordered_paths,
            &start_season,
            &start_episode_str,
            is_multipart,
            &parse,
        );
        let total_count = planned.len();

        spawn_local(async move {
            let mut errors: Vec<String> = Vec::new();
            for (path, target) in planned {
                match target {
                    Ok(t) => {
                        let payload = AssignFilePayload {
                            path: path.clone(),
                            season: t.season,
                            episode: t.episode,
                        };
                        if let Err(e) = assign_series_file(s_id.clone(), payload).await {
                            errors.push(format!("{}: {}", path, e));
                        }
                    }
                    Err(PlanError::Unparseable) => {
                        errors.push(format!("Could not parse season/episode for {}", path));
                    }
                }
            }
            let succeeded = total_count.saturating_sub(errors.len());
            if errors.is_empty() {
                crate::components::common::toast::show_success(format!(
                    "Assigned {} file(s)",
                    succeeded
                ));
            } else {
                crate::components::common::toast::show_error(format!(
                    "Assigned {} / {} files. {} error(s): {}",
                    succeeded,
                    total_count,
                    errors.len(),
                    errors.join("; ")
                ));
            }
            refetch_files();
            on_refresh.run(());
            set_batch_processing.set(false);
        });
    };

    let handle_delete_confirm = move || {
        let s_id = series_id.get();
        let paths: Vec<String> = selected_files.get().into_iter().collect();
        let on_refresh = on_refresh;
        let path_count = paths.len();
        spawn_local(async move {
            let mut errors: Vec<String> = Vec::new();
            if !paths.is_empty() {
                let payload = BatchDeletePayload { paths };
                if let Err(e) = delete_series_files(s_id.clone(), payload).await {
                    errors.push(format!("Known files: {}", e));
                }
            }
            if errors.is_empty() {
                crate::components::common::toast::show_success(format!(
                    "Deleted {} file(s)",
                    path_count
                ));
            } else {
                crate::components::common::toast::show_error(format!(
                    "Deleted {} / {} files. {} error(s): {}",
                    path_count.saturating_sub(errors.len()),
                    path_count,
                    errors.len(),
                    errors.join("; ")
                ));
            }
            refetch_files();
            on_refresh.run(());
        });
    };

    let _on_sort_main = Callback::new(move |sort_key: &'static str| {
        if sort_column.get() == sort_key {
            set_sort_desc.update(|d| *d = !*d);
        } else {
            set_sort_column.set(sort_key.to_string());
            set_sort_desc.set(false);
        }
    });

    // Closing always clears every transient input so the next open starts fresh
    // (the start fields, multipart flag, inline assign fields, and selection).
    let close_modal = move || {
        set_assigning_path.set(None);
        set_assign_season.set(String::new());
        set_assign_episode.set(String::new());
        set_batch_start_season.set(String::new());
        set_batch_start_episode.set(String::new());
        set_batch_is_multipart.set(false);
        set_show_delete_modal.set(false);
        clear_selection.run(());
        set_show.set(false);
    };

    view! {
        <StandardModal
            show=show
            on_close=move |_| close_modal()
            title=Signal::derive(move || "Manage Series Files".to_string())
            size="modal-full"
            body_class="p-0"
            class="modal-body--flush"
            footer=view! {

                    <Show when=move || absolute_numbering.get()>
                        <div class="absolute-numbering-warning">
                            "ℹ️ Absolute numbering is enabled for this series. Reassignment still applies to the normal season and episode numbers, not the absolute number."
                        </div>
                    </Show>
                    // Conflict warning bar — only shown when conflicts exist
                    <Show when=move || has_conflicts.get()>
                        <div class="batch-conflict-warning">
                            "⚠ Some preview slots conflict with already-assigned files. Select those files too, or adjust the start episode."
                        </div>
                    </Show>
                    <div id="modal-footer--manage-series-files" class="modal-footer justify-between justify-evenly-mobile">
                        <div class="flex gap-md items-center">
                            <button class="btn btn-danger footer-delete-btn"
                                disabled={move || selected_files.get().is_empty()}
                                on:click=move |_| set_show_delete_modal.set(true)
                            >
                                <span class="delete-label">"Delete"</span><span class="delete-icon"><TrashIcon /></span>
                            </button>
                            <button class="btn btn-warning modal-footer-btn modal-footer-btn--wide"
                                disabled={move || selected_files.get().is_empty()}
                                on:click=move |_| {
                                    let s_id = series_id.get();
                                    let paths: Vec<String> = selected_files.get().into_iter().collect();
                                    let on_refresh = on_refresh;
                                    let payload = BatchDeletePayload { paths };
                                    crate::utils::spawn_api_toast(
                                        unassign_series_files(s_id, payload),
                                        None,
                                        move |_| {
                                            crate::components::common::toast::show_success("Files unassigned successfully");
                                            refetch_files();
                                            on_refresh.run(());
                                        }
                                    );
                                }
                            >
                                "Unassign"
                            </button>
                        </div>
                        <div class="flex gap-md items-center">
                            <div class="text-sm text-secondary-color">
                                {move || {
                                    let n = selected_files.get().len();
                                    format!("{} selected", n)
                                }}
                            </div>
                            <div class="flex gap-sm">
                                <div class="flex flex-col items-center gap-xs">
                                    <input
                                        id="manage-series-season-input"
                                        type="number"
                                        aria-label="Start Season"
                                        class="input-assign-season"
                                        placeholder="S"
                                        prop:value=batch_start_season
                                        on:input=move |ev| set_batch_start_season.set(event_target_value(&ev))
                                        min="0"
                                    />
                                    <span class="text-xs text-secondary-color">"Season"</span>
                                </div>
                                <div class="flex flex-col items-center gap-xs">
                                    <input
                                        id="manage-series-episode-input"
                                        type="number"
                                        aria-label="Start Episode"
                                        class="input-assign-episode"
                                        placeholder="E"
                                        prop:value=batch_start_episode
                                        on:input=move |ev| set_batch_start_episode.set(event_target_value(&ev))
                                        min="0"
                                    />
                                    <span class="text-xs text-secondary-color">"Episode"</span>
                                </div>
                                <div class="flex flex-col items-center gap-xs ml-sm self-center">
                                    <input
                                        id="manage-series-multipart-input"
                                        type="checkbox"
                                        aria-label="Multipart Episode"
                                        class="cursor-pointer"
                                        prop:checked=batch_is_multipart
                                        on:change=move |_| set_batch_is_multipart.update(|v| *v = !*v)
                                    />
                                    <span class="text-xs text-secondary-color">"Multipart"</span>
                                </div>
                            </div>
                        </div>
                        <div class="flex gap-md items-center">
                        <button class="btn btn-primary modal-footer-btn modal-footer-btn--wide"
                            disabled={move || batch_processing.get() || selected_files.get().is_empty()}
                            title={move || if batch_processing.get() { "Assigning..." } else { "Assign selected files" }}
                            on:click=handle_batch_assign
                        >
                            {move || if batch_processing.get() { "Assigning..." } else { "Assign" }}
                        </button>
                        </div>
                    </div>
            }.into_any()
        >
            <div class="flex-1 manage-table-container">
                {
                    let columns: Vec<ManagedColumn<SeriesFileViewModel>> = vec![
                        ManagedColumn {
                            id: "filename".into(), label: "Filename".into(), sortable: true, class: "col-manage-filename".into(),
                            cell_render: Callback::new(|file: SeriesFileViewModel| {
                                let filename = file.filename.clone();
                                let file_path = file.path.clone();
                                view! {
                                    <div class="flex items-center gap-xs min-w-0">
                                        <span class="truncate block" title=file_path>{filename}</span>
                                    </div>
                                }.into_any()
                            })
                        },
                        ManagedColumn {
                            id: "size".into(), label: "Size".into(), sortable: true, class: "text-xs".into(),
                            cell_render: Callback::new(|file: SeriesFileViewModel| {
                                view! { <span>{crate::utils::format_size(file.size)}</span> }.into_any()
                            })
                        },
                        ManagedColumn {
                            id: "ext".into(), label: "Ext".into(), sortable: true, class: "text-xs col-manage-ext".into(),
                            cell_render: Callback::new(|file: SeriesFileViewModel| {
                                let ext = file_ext(&file.filename);
                                let display = if ext.is_empty() { "-".to_string() } else { ext };
                                view! { <span>{display}</span> }.into_any()
                            })
                        },
                        ManagedColumn {
                            id: "assigned".into(), label: "Assigned To".into(), sortable: true, class: "".into(),
                            cell_render: Callback::new(|file: SeriesFileViewModel| {
                                if let Some(h) = file.assigned_header {
                                    view! { <span class="badge badge-success">{h}</span> }.into_any()
                                } else {
                                    view! { <span class="text-muted italic text-xs">"-"</span> }.into_any()
                                }
                            })
                        },
                        ManagedColumn {
                            id: "preview".into(), label: "Preview".into(), sortable: false, class: "col-manage-preview".into(),
                            cell_render: Callback::new({
                                let preview_map = preview_map.clone();
                                move |file: SeriesFileViewModel| {
                                    let map = preview_map.get();
                                    match map.get(&file.path) {
                                        Some((label, true)) => {
                                            let label = label.clone();
                                            view! {
                                                <span class="badge badge-warning preview-conflict" title="Already occupied by another file">
                                                    "⚠ " {label}
                                                </span>
                                            }.into_any()
                                        },
                                        Some((label, false)) => {
                                            let label = label.clone();
                                            view! { <span class="badge badge-blue">{label.clone()}</span> }.into_any()
                                        },
                                        None => {
                                            // Show inline-assign values in preview when editing a single file
                                            if assigning_path.get() == Some(file.path.clone()) {
                                                let s = assign_season.get();
                                                let e = assign_episode.get();
                                                if !e.trim().is_empty() {
                                                    let season_str = if s.trim().is_empty() { "1".to_string() } else { s };
                                                    let label = format_episode_display(&season_str, &e, None);
                                                    view! { <span class="badge badge-blue">{label}</span> }.into_any()
                                                } else {
                                                    view! { <span class="text-secondary-color">"-"</span> }.into_any()
                                                }
                                            } else {
                                                view! { <span class="text-secondary-color">"-"</span> }.into_any()
                                            }
                                        },
                                    }
                                }
                            })
                        },
                        ManagedColumn {
                            id: "actions".into(), label: "Actions".into(), sortable: false, class: "col-actions".into(),
                            cell_render: Callback::new({
                                let series_id = series_id.clone();
                                let on_refresh = on_refresh.clone();
                                move |file: SeriesFileViewModel| {
                                    let path = file.path.clone();
                                    let path_for_assigning = path.clone();
                                    let is_assigning = move || assigning_path.get() == Some(path_for_assigning.clone());

                                    view! {
                                        <div class="action-cell">
                                            {move || if is_assigning() {
                                                view! {
                                                    <div class="flex gap-xs items-center">
                                                        <input
                                                            id="manage-series-assign-season-input"
                                                            type="number"
                                                            aria-label="Assign Season"
                                                            class="input-assign-season"
                                                            placeholder="S"
                                                            prop:value=assign_season
                                                            on:input=move |ev| set_assign_season.set(event_target_value(&ev))
                                                            on:click=move |ev| ev.stop_propagation()
                                                        />
                                                        <input
                                                            id="manage-series-assign-episode-input"
                                                            type="number"
                                                            aria-label="Assign Episode"
                                                            class="input-assign-episode"
                                                            placeholder="E"
                                                            prop:value=assign_episode
                                                            on:input=move |ev| set_assign_episode.set(event_target_value(&ev))
                                                            on:click=move |ev| ev.stop_propagation()
                                                        />
                                                        <button class="btn btn-sm btn-primary" title="Assign"
                                                            on:click=move |ev| {
                                                                ev.stop_propagation();
                                                                handle_assign(ev);
                                                            }
                                                        >
                                                            <span class="icon"><CheckIcon /></span>
                                                        </button>
                                                        <button class="btn btn-sm btn-secondary" title="Cancel"
                                                            on:click=move |ev| {
                                                                ev.stop_propagation();
                                                                set_assigning_path.set(None);
                                                            }
                                                        >
                                                            <span class="icon"><XIcon /></span>
                                                        </button>
                                                    </div>
                                                }.into_any()
                                            } else {
                                                let path_for_assign = path.clone();
                                                let path_for_unassign = path.clone();
                                                let s_id = series_id.get();
                                                let show_unassign = file.assigned_id.is_some();

                                                view! {
                                                    <div class="flex gap-xs">
                                                        {if show_unassign {
                                                            view! {
                                                                <button class="btn btn-sm-danger" title="Unassign"
                                                                    on:click=move |e| {
                                                                        e.stop_propagation();
                                                                        let p = path_for_unassign.clone();
                                                                        let sid = s_id.clone();
                                                                        let id_for_unassign = p.clone();
                                                                        crate::utils::spawn_api_toast(
                                                                            unassign_series_files(sid, BatchDeletePayload { paths: vec![id_for_unassign] }),
                                                                            None,
                                                                            move |_| {
                                                                                crate::components::common::toast::show_success("Unassigned".to_string());
                                                                                refetch_files();
                                                                                on_refresh.run(());
                                                                            }
                                                                        );
                                                                    }
                                                                >
                                                                    <span class="icon"><UnassignIcon /></span>
                                                                </button>
                                                            }.into_any()
                                                        } else { view! {}.into_any() }}

                                                        <button class="btn btn-sm btn-primary" title="Assign Manually"
                                                            on:click=move |e| {
                                                                e.stop_propagation();
                                                                set_assigning_path.set(Some(path_for_assign.clone()));
                                                                let settings = series_details.get().map(|d| d.config.settings).unwrap_or_default();
                                                                let absolute = absolute_numbering.get();
                                                                let (s, ep, _) = parse_season_episode_from_filename(&path_for_assign, &settings, absolute);
                                                                set_assign_season.set(s);
                                                                set_assign_episode.set(ep);
                                                            }
                                                        >
                                                            <span class="icon"><EditIcon /></span>
                                                        </button>
                                                    </div>
                                                }.into_any()
                                            }}
                                        </div>
                                    }.into_any()
                                }
                            })
                        },
                    ];

                    TableBuilder::new(sort_column.into(), Signal::derive(move || !sort_desc.get()))
                        .table_class("w-full manage-table")
                        .loading(Signal::derive(move || files_resource.get().is_none()))
                        .empty_message("No files found for this series.")
                        .selection_mode(is_select_all, select_all)
                        .select_all_disabled(select_all_disabled)
                        .on_sort(Callback::new(move |col| {
                            if sort_column.get() == col {
                                set_sort_desc.update(|d| *d = !*d);
                            } else {
                                set_sort_column.set(col);
                                set_sort_desc.set(false);
                            }
                        }))
                        .build_managed(
                            Signal::derive(move || sorted_files.get()),
                            columns,
                            Callback::new(|(a, b, col): (SeriesFileViewModel, SeriesFileViewModel, String)| {
                                match col.as_str() {
                                    "filename" => jumbie_shared::formatting::natural_cmp(&a.filename, &b.filename),
                                    "path" => jumbie_shared::formatting::natural_cmp(&a.path, &b.path),
                                    "size" => a.size.cmp(&b.size),
                                    "assigned" => a.assigned_id.is_some().cmp(&b.assigned_id.is_some())
                                        .then_with(|| {
                                            jumbie_shared::formatting::natural_cmp(
                                                a.assigned_header.as_deref().unwrap_or(""),
                                                b.assigned_header.as_deref().unwrap_or(""),
                                            )
                                        }),
                                    _ => std::cmp::Ordering::Equal,
                                }
                            }),
                            Some(Callback::new(move |file: SeriesFileViewModel| {
                                toggle_selection.run(file.path.clone());
                            })),
                            Some(Callback::new(move |file: SeriesFileViewModel| {
                                selected_files.get().contains(&file.path)
                            })),
                            None::<Callback<SeriesFileViewModel, AnyView>>
                        )
                }
            </div>

            <ConfirmationModal
                show=Signal::from(show_delete_modal)
                set_show=set_show_delete_modal
                title="Delete Files"
                on_confirm=handle_delete_confirm.into()
            >
                <div class="p-md">
                    {move || {
                        let n = selected_files.get().len();
                        format!("Are you sure you want to delete {} file(s)? This cannot be undone.", n)
                    }}
                </div>
            </ConfirmationModal>
        </StandardModal>
    }
}
