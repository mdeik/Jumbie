use crate::api::{batch_move_series, batch_move_status, validate_path};
use crate::components::common::form_fields::{CheckboxInput, SelectOption, field_validation};
use crate::components::common::standard_modal::StandardModal;
use crate::components::common::toast::{NotificationType, show_toast, use_notification};
use crate::hooks::{EXAMPLE_SERIES_PATH, path_placeholder, use_server_os};
use gloo_timers::future::TimeoutFuture;
use jumbie_shared::config::generate_uuid;
use jumbie_shared::types::PathOperation;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

/// Modal for batch-moving organized series: a destination root select (roots +
/// "Custom Path...") and a PathOperation (Move / Copy / Delete / DoNothing).
#[component]
pub fn BatchMoveModal(
    show: ReadSignal<bool>,
    set_show: WriteSignal<bool>,
    selected_paths: ReadSignal<std::collections::HashSet<String>>,
    dest_roots: ReadSignal<Vec<jumbie_shared::config::DestinationRoot>>,
    on_success: Callback<()>,
) -> impl IntoView {
    let (target_root, set_target_root) = signal(String::new());
    let (use_custom_root, set_use_custom_root) = signal(false);
    let (custom_root_path, set_custom_root_path) = signal(String::new());
    let (file_operation, set_file_operation) = signal(PathOperation::Move);
    let (stop_tracking, set_stop_tracking) = signal(false);
    let (executing, set_executing) = signal(false);
    // Per-series custom path overrides: source_path → custom_destination
    let (custom_paths, set_custom_paths) = signal(HashMap::<String, String>::new());
    let (custom_path_enabled, set_custom_path_enabled) =
        signal(std::collections::HashSet::<String>::new());

    let dest_root_options = Signal::derive(move || {
        let mut opts = Vec::new();
        for root in &dest_roots.get() {
            let p = root.path.to_string_lossy().to_string();
            opts.push(SelectOption::from((p.clone(), p)));
        }
        opts.push(SelectOption::from((
            "__CUSTOM__".to_string(),
            "Custom Path...".to_string(),
        )));
        opts
    });

    let effective_root = move || {
        if use_custom_root.get() && !custom_root_path.get().is_empty() {
            custom_root_path.get()
        } else {
            target_root.get()
        }
    };

    let has_valid_root = move || {
        if use_custom_root.get() {
            !custom_root_path.get().is_empty()
        } else {
            !target_root.get().is_empty() && target_root.get() != "__CUSTOM__"
        }
    };

    let on_root_change = move |val: String| {
        if val == "__CUSTOM__" {
            set_use_custom_root.set(true);
        } else {
            set_use_custom_root.set(false);
            set_target_root.set(val);
        }
    };

    let is_windows = use_server_os();
    let (path_validation_msg, set_path_validation_msg) = signal(String::new());

    // Debounced validation: call the backend whenever the effective root changes
    Effect::new(move |_| {
        let path = effective_root();
        if path.is_empty() {
            set_path_validation_msg.set(String::new());
            return;
        }
        spawn_local(async move {
            match validate_path(jumbie_shared::types::ValidatePathPayload {
                path,
                series_id: None,
                resolve_collisions: false,
            })
            .await
            {
                Ok(res) => {
                    set_path_validation_msg.set(if res.is_valid {
                        String::new()
                    } else {
                        res.message
                    });
                }
                Err(_) => set_path_validation_msg.set(String::new()),
            }
        });
    });

    // Each enabled custom path is validated for empty paths, duplicate
    // destinations, and backend path validity (writable, traversal, etc.).
    let (custom_path_errors, set_custom_path_errors) = signal(HashMap::<String, String>::new());
    let (custom_path_dup_error, set_custom_path_dup_error) = signal(String::new());
    let (custom_path_backend_errors, set_custom_path_backend_errors) =
        signal(HashMap::<String, String>::new());

    // True when any enabled custom path has an empty value: this disables Execute
    // but shows no inline error (empty = not yet configured). Checks both entries
    // with an empty value and enabled paths with no entry yet.
    let has_empty_custom_path = Signal::derive(move || {
        let enabled = custom_path_enabled.get();
        let paths = custom_paths.get();
        for ref source_path in &enabled {
            match paths.get(*source_path) {
                Some(dest) if dest.trim().is_empty() => return true,
                None => return true, // enabled but no value typed yet
                _ => {}
            }
        }
        false
    });

    let has_custom_path_error = Signal::derive(move || {
        if !custom_path_dup_error.get().is_empty() {
            return true;
        }
        if custom_path_errors.get().values().any(|e| !e.is_empty()) {
            return true;
        }
        if custom_path_backend_errors
            .get()
            .values()
            .any(|e| !e.is_empty())
        {
            return true;
        }
        if has_empty_custom_path.get() {
            return true;
        }
        false
    });

    // Debounced: validate each enabled non-empty custom path via backend
    Effect::new(move |_| {
        let enabled = custom_path_enabled.get();
        let paths = custom_paths.get();

        for (source_path, dest) in &paths {
            if !enabled.contains(source_path) || dest.trim().is_empty() {
                continue;
            }
            let sp = source_path.clone();
            let d = dest.clone();
            spawn_local(async move {
                match validate_path(jumbie_shared::types::ValidatePathPayload {
                    path: d,
                    series_id: None,
                    resolve_collisions: false,
                })
                .await
                {
                    Ok(res) => {
                        set_custom_path_backend_errors.update(|errs| {
                            if res.is_valid {
                                errs.remove(&sp);
                            } else {
                                errs.insert(sp.clone(), res.message);
                            }
                        });
                    }
                    Err(_) => {
                        set_custom_path_backend_errors.update(|errs| {
                            errs.remove(&sp);
                        });
                    }
                }
            });
        }
    });

    // Combine frontend and backend errors for display
    // Frontend errors (duplicate) take priority over backend errors (writable, traversal)
    let get_combined_error = move |source_path: &str| -> String {
        let frontend = custom_path_errors
            .get()
            .get(source_path)
            .cloned()
            .unwrap_or_default();
        if !frontend.is_empty() {
            return frontend;
        }
        custom_path_backend_errors
            .get()
            .get(source_path)
            .cloned()
            .unwrap_or_default()
    };

    // Validate enabled custom paths: empty/duplicate frontend checks ONLY
    // (backend validation is handled by the Effect above)
    Effect::new(move |_| {
        // Read signals to track reactivity
        let enabled = custom_path_enabled.get();
        let paths = custom_paths.get();

        let mut new_errors = HashMap::<String, String>::new();
        let mut seen: Vec<String> = Vec::new();
        let mut dup_error = String::new();

        // First pass: check paths that ARE in the HashMap (have a value typed)
        for (source_path, dest) in &paths {
            if !enabled.contains(source_path) {
                continue;
            }
            if dest.trim().is_empty() {
                // Whitespace-only: show error, clear backend errors
                new_errors.insert(source_path.clone(), "Path cannot be empty".to_string());
                set_custom_path_backend_errors.update(|errs| {
                    errs.remove(source_path);
                });
            } else if seen.contains(dest) {
                new_errors.insert(source_path.clone(), "Duplicate destination".to_string());
                dup_error = "Multiple series have the same custom destination path. ".to_string();
            } else {
                new_errors.insert(source_path.clone(), String::new());
                seen.push(dest.clone());
            }
        }

        // Second pass: enabled paths with NO entry in custom_paths (no value typed yet)
        // These have no value → clear any stale backend errors, no frontend error shown
        for ref source_path in &enabled {
            if !paths.contains_key(*source_path) {
                new_errors.insert((*source_path).clone(), String::new());
                set_custom_path_backend_errors.update(|errs| {
                    errs.remove(*source_path);
                });
            }
        }

        set_custom_path_errors.set(new_errors);
        set_custom_path_dup_error.set(dup_error);
    });

    let has_validation_error = Signal::derive(move || {
        !path_validation_msg.get().is_empty() || has_custom_path_error.get()
    });

    let reset_state = move || {
        set_target_root.set(String::new());
        set_use_custom_root.set(false);
        set_custom_root_path.set(String::new());
        set_file_operation.set(PathOperation::Move);
        set_stop_tracking.set(false);
        set_executing.set(false);
        set_custom_paths.set(HashMap::new());
        set_custom_path_enabled.set(std::collections::HashSet::new());
    };

    let do_batch_move = move |_| {
        let paths: Vec<String> = selected_paths.get().into_iter().collect();
        let payload = jumbie_shared::types::BatchMoveOrganizedSeriesPayload {
            paths,
            target_root: effective_root(),
            custom_paths: custom_paths
                .get()
                .into_iter()
                .filter(|(k, _)| custom_path_enabled.get().contains(k))
                .collect(),
            file_operation: file_operation.get(),
            stop_tracking: stop_tracking.get(),
        };
        set_executing.set(true);
        spawn_local(async move {
            match batch_move_series(payload).await {
                Ok(result) => {
                    if let Some(task_id) = &result.task_id {
                        let task_id = task_id.clone();
                        let total_series = selected_paths.get().len();
                        let ctx = use_notification();
                        ctx.show_operation_progress(
                            format!("Batch move: {} series", total_series),
                            task_id.clone(),
                        );
                        // Close the modal — progress is now tracked via toast
                        reset_state();
                        set_show.set(false);

                        let max_polls: u32 = 600; // 5 minutes at 500ms intervals
                        let mut polls: u32 = 0;
                        loop {
                            if polls >= max_polls {
                                ctx.dismiss_all();
                                show_toast(
                                    "Batch move timed out after 5 minutes",
                                    NotificationType::Error,
                                );
                                break;
                            }
                            polls += 1;
                            TimeoutFuture::new(500).await;
                            match batch_move_status(&task_id).await {
                                Ok(progress) => {
                                    if progress.finished {
                                        // SSoT: transform the progress toast into a result toast.
                                        // No second toast is shown here — doing so produced
                                        // duplicate completion notifications.
                                        let ctx = use_notification();
                                        let op =
                                            crate::components::common::structs::ActiveOperation {
                                                id: task_id.clone(),
                                                operation_type: "batch_move".to_string(),
                                                total: progress.total,
                                                completed: progress.completed,
                                                finished: true,
                                                finished_at_ms: None,
                                                success_count: progress.success_count,
                                                failed: progress.failed,
                                                errors: progress.errors.clone(),
                                            };
                                        ctx.finalize_operation(&task_id, &op);
                                        on_success.run(());
                                        break;
                                    } else {
                                        ctx.update_progress(
                                            &format!("Batch move: {} series", total_series),
                                            progress.completed,
                                            progress.total,
                                        );
                                    }
                                }
                                Err(e) => {
                                    ctx.dismiss_all();
                                    show_toast(
                                        format!(
                                            "Failed to poll batch move status: {}",
                                            e.user_message()
                                        ),
                                        NotificationType::Error,
                                    );
                                    break;
                                }
                            }
                        }
                    } else {
                        // Synchronous response — no task_id to poll.
                        if result.failure_count == 0 {
                            show_toast(
                                format!("Successfully moved {} series", result.success_count),
                                NotificationType::Success,
                            );
                        } else {
                            show_toast(
                                format!(
                                    "Moved {} series ({} failed)",
                                    result.success_count, result.failure_count
                                ),
                                if result.success_count > 0 {
                                    NotificationType::Warning
                                } else {
                                    NotificationType::Error
                                },
                            );
                        }
                        on_success.run(());
                        reset_state();
                        set_show.set(false);
                    }
                }
                Err(e) => {
                    show_toast(
                        format!("Batch move failed: {}", e.user_message()),
                        NotificationType::Error,
                    );
                }
            }
            set_executing.set(false);
        });
    };

    let footer = view! {
        <div class="modal-footer flex items-center justify-end gap-md">
            <button class="btn btn-primary"
                disabled=move || executing.get() || !has_valid_root() || has_validation_error.get()
                on:click=do_batch_move
                type="button"
            >
                {move || if executing.get() { "Moving..." } else { "Execute" }}
            </button>
        </div>
    }
    .into_any();

    view! {
        <StandardModal
            show=show
            on_close=move |_| {
                reset_state();
                set_show.set(false);
            }
            title=Signal::derive(move || format!("Batch Move: {} Series", selected_paths.get().len()))
            size="modal-md"
            footer=footer
        >
            <div class="alert alert-info text-sm mb-md">
                "You can safely close this window during the operation.
                Progress notifications will appear here."
            </div>

            <div class="form-group">
                <label class="form-label" for="batchMoveRoot">"Destination"</label>
                {move || {
                    if use_custom_root.get() {
                        view! {
                            <div class="flex flex-col gap-xs">
                                <input
                                    type="text"
                                    id="batchMoveRoot"
                                    class="form-control"
                                    placeholder=move || path_placeholder(is_windows.get(), EXAMPLE_SERIES_PATH)
                                    prop:value=custom_root_path
                                    on:input=move |e| set_custom_root_path.set(event_target_value(&e))
                                />
                                {field_validation(path_validation_msg)}

                                                                    <label class="flex items-center gap-sm cursor-pointer text-sm">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || !use_custom_root.get()
                                        on:change=move |_| set_use_custom_root.set(false)
                                    />
                                    "Pick a destination root instead"
                                </label>
                                <p class="text-xs text-muted mt-xs">
                                    "Series folder name is automatically appended (e.g. /media/tv/My Show)."
                                </p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="flex flex-col gap-xs">
                                <select
                                    id="batchMoveRoot"
                                    class="form-control"
                                    on:change=move |ev| on_root_change(event_target_value(&ev))
                                >
                                    <option value="" disabled selected=move || target_root.get().is_empty()>
                                        "-- Select destination --"
                                    </option>
                                    {move || dest_root_options.get().into_iter().map(|opt| {
                                        match opt {
                                            SelectOption::Single(item) => {
                                                let val = item.value.clone();
                                                let label = item.label.clone();
                                                let val_clone = val.clone();
                                                view! {
                                                    <option
                                                        value=val.clone()
                                                        selected=move || target_root.get() == val_clone
                                                    >
                                                        {label}
                                                    </option>
                                                }.into_any()
                                            }
                                            SelectOption::Group { label, items } => {
                                                view! {
                                                    <optgroup label=label>
                                                        {items.into_iter().map(|item| {
                                                            let val = item.value.clone();
                                                            let label = item.label.clone();
                                                            let val_clone = val.clone();
                                                            view! {
                                                                <option
                                                                    value=val.clone()
                                                                    selected=move || target_root.get() == val_clone
                                                                >
                                                                    {label}
                                                                </option>
                                                            }
                                                        }).collect::<Vec<_>>()}
                                                    </optgroup>
                                                }.into_any()
                                            }
                                        }
                                    }).collect::<Vec<_>>()}
                                </select>
                                {field_validation(path_validation_msg)}
                                <p class="text-xs text-muted mt-xs">
                                    "Series folder name is automatically appended (e.g. /media/tv/My Show)."
                                </p>
                            </div>
                        }.into_any()
                    }
                }}
            </div>
            <details class="border rounded-md p-sm mb-md">
                <summary class="cursor-pointer text-sm font-semibold">
                    "Per-Series Custom Paths "
                    <span class="text-muted">"(optional)"</span>
                </summary>
                <div class="mt-sm flex flex-col gap-xs max-h-48 overflow-y-auto">
                    {move || {
                        let paths: Vec<String> = selected_paths.get().into_iter().collect();
                        paths.into_iter().map(|path| {
                            let p_check = path.clone();
                            let p_input = path.clone();
                            let p_display = path.clone();
                            let p_label = path.clone();
                            let p_title = path.clone();
                            let p_toggle2 = path.clone();
                            let input_id = generate_uuid();
                            let folder_name: String = path.split('/').next_back().unwrap_or(&path).to_string();
                            let default_dest = if use_custom_root.get() && !custom_root_path.get().is_empty() {
                                format!("{}/{}", custom_root_path.get(), folder_name)
                            } else if !target_root.get().is_empty() && target_root.get() != "__CUSTOM__" {
                                format!("{}/{}", target_root.get(), folder_name)
                            } else {
                                String::new()
                            };
                            let placeholder = if default_dest.is_empty() {
                                path_placeholder(is_windows.get(), EXAMPLE_SERIES_PATH)
                            } else {
                                default_dest
                            };
                            let fn_label = folder_name.clone();
                            let p_err = path.clone();
                            let combined_err = Signal::derive(move || {
                                if custom_path_enabled.get().contains(&p_err) {
                                    get_combined_error(&p_err)
                                } else {
                                    String::new()
                                }
                            });
                            let err_view = field_validation(combined_err);
                            view! {
                                <div class="flex flex-col py-xs border-b border-base last:border-b-0">
                                    <div class="flex items-center gap-xs">
                                        <input
                                            type="checkbox"
                                            id=input_id.clone()
                                            prop:checked=move || custom_path_enabled.get().contains(&p_check)
                                            on:change=move |_| {
                                                let p = p_input.clone();
                                                set_custom_path_enabled.update(|set| {
                                                    if set.contains(&p) { set.remove(&p); }
                                                    else { set.insert(p); }
                                                });
                                            }
                                            title="Use custom destination path"
                                        />
                                        <span class="text-xs flex-1 truncate" title=p_title>
                                            {folder_name.clone()}
                                        </span>
                                        <div class="flex flex-col">
                                            <label for=input_id.clone() class="sr-only">{format!("Custom path for {}", fn_label)}</label>
                                            <input
                                                type="text"
                                                id=input_id.clone()
                                                class="form-control form-control-sm w-56"
                                                placeholder=placeholder
                                                prop:value=move || custom_paths.get().get(&p_toggle2).cloned().unwrap_or_default()
                                                prop:disabled=move || !custom_path_enabled.get().contains(&p_label)
                                                on:input=move |e| {
                                                    let val = event_target_value(&e);
                                                    let p = p_display.clone();
                                                    set_custom_paths.update(|map| {
                                                        if val.is_empty() { map.remove(&p); }
                                                        else { map.insert(p, val); }
                                                    });
                                                }
                                            />
                                        </div>
                                    </div>
                                    {err_view}
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}
                </div>
                <p class="text-xs text-muted mt-xs">
                    "Use custom paths to place series at specific locations outside the target root."
                </p>
            </details>

            <div class="form-group">
                <label class="form-label" for="fileOperation">"Existing Files Operation"</label>
                <select
                    id="fileOperation"
                    class="form-control"
                    on:change=move |ev| {
                        let val = event_target_value(&ev);
                        let op = match val.as_str() {
                            "move" => PathOperation::Move,
                            "copy" => PathOperation::Copy,
                            "delete" => PathOperation::Delete,
                            _ => PathOperation::DoNothing,
                        };
                        set_file_operation.set(op);
                    }
                >
                    <option value="move" selected=move || file_operation.get() == PathOperation::Move>
                        "Move existing files to new directory"
                    </option>
                    <option value="copy" selected=move || file_operation.get() == PathOperation::Copy>
                        "Copy existing files to new directory"
                    </option>
                    <option value="delete" selected=move || file_operation.get() == PathOperation::Delete>
                        "Delete existing files in old directory"
                    </option>
                    <option value="none" selected=move || file_operation.get() == PathOperation::DoNothing>
                        "Do nothing (Keep files where they are)"
                    </option>
                </select>
                <p class="text-xs text-muted mt-xs">
                    {move || match file_operation.get() {
                        PathOperation::Move => "Files are moved to the new location and removed from the old one.".to_string(),
                        PathOperation::Copy => "Files are copied to the new location; originals remain in place.".to_string(),
                        PathOperation::Delete => "Files at the old location are permanently deleted. Only the DB path is updated.".to_string(),
                        PathOperation::DoNothing => "Only the database path is updated. Files stay where they are.".to_string(),
                    }}
                </p>
            </div>


            {field_validation(custom_path_dup_error)}

            <CheckboxInput
                label=Signal::stored(
                    "Stop tracking after move".to_string()
                )
                checked=stop_tracking
                set_checked=Callback::new(move |v| set_stop_tracking.set(v))
                help_text=Signal::derive(move || {
                    "Remove the series from the database entirely after the move completes.".to_string()
                })
                id="stopTracking".to_string()
            />
        </StandardModal>
    }
}
