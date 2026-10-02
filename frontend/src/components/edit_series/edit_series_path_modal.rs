use crate::components::common::form_fields::{
    CheckboxInput, FormatFieldBuilder, TextInput, TooltipBuilder,
};
use crate::components::common::modal_wrapper::ModalWrapper;
use jumbie_shared::types::PathOperation;
use jumbie_shared::variables::TemplateContext;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Title constant for the Edit Series Path modal, shared with its trigger button.
pub const EDIT_SERIES_PATH_TITLE: &str = "Edit Series Path";

#[component]
pub fn EditSeriesPathModal(
    show: ReadSignal<bool>,
    set_show: WriteSignal<bool>,
    current_path: ReadSignal<String>,
    fallback_path: Signal<String>,
    set_current_path: WriteSignal<String>,
    set_path_operation: WriteSignal<Option<PathOperation>>,
    save_series: Callback<Option<String>, ()>,
    series_title: ReadSignal<String>,
    series_id: ReadSignal<String>,
) -> impl IntoView {
    let (new_path, set_new_path) = signal(String::new());
    let (append_series_name, set_append_series_name) = signal(true);
    let (operation, set_operation) = signal(PathOperation::Move);
    let is_windows = crate::hooks::use_server_os();

    Effect::new(move |_| {
        if show.get() {
            let mut current = current_path.get_untracked();
            // If the series has no explicit custom path yet, use the synthesized fallback
            if current.is_empty() {
                current = fallback_path.get_untracked();
            }

            // If the current path ends with the series title and append_series_name is checked (which it is by default),
            // strip the series title from the current path so the user just edits the base.
            let title = series_title.get_untracked();

            let mut resolved = current.clone();

            if !title.is_empty() && current.ends_with(&title) {
                resolved.truncate(current.len() - title.len());
                if resolved.ends_with('/') || resolved.ends_with('\\') {
                    resolved.pop();
                }
                set_append_series_name.set(true);
            } else {
                set_append_series_name.set(false);
            }

            set_new_path.set(resolved);
            set_operation.set(PathOperation::Move);
        }
    });

    let preview_path = Signal::derive(move || {
        let path = new_path.get();
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let mut preview = trimmed.to_string();
        if append_series_name.get() {
            if !preview.ends_with('/') && !preview.ends_with('\\') {
                preview.push('/');
            }
            preview.push_str(&series_title.get().trim());
        }
        preview
    });

    let (validation_msg, set_validation_msg) = signal(String::new());
    let (is_backend_valid, set_is_backend_valid) = signal(true);
    // Track the org config so a settings change (collision strategy / illegal-char
    // policy) re-validates the path and updates the preview live.
    let config = crate::hooks::use_config().config;
    // Effective path reported by the backend after sanitization + folder-level
    // collision resolution. Keyed by the candidate preview it was computed for
    // so a stale response can never leak in for a different candidate.
    let (resolved_preview_for, set_resolved_preview_for) = signal((String::new(), None::<String>));

    // The path that will ACTUALLY be used — the backend's effective path when a
    // fresh validation response exists, otherwise the locally-derived preview.
    let effective_preview = Signal::derive(move || {
        let raw = preview_path.get();
        if raw.is_empty() {
            return String::new();
        }
        let (for_path, resolved) = resolved_preview_for.get();
        if for_path == raw {
            resolved.unwrap_or(raw)
        } else {
            raw
        }
    });

    let field_error = Signal::derive(move || {
        let path = new_path.get();
        let trimmed = path.trim();
        if trimmed.is_empty() {
            Some("Path cannot be empty".to_string())
        } else if !is_backend_valid.get() {
            let msg = validation_msg.get();
            if msg.is_empty() { None } else { Some(msg) }
        } else {
            None
        }
    });

    Effect::new(move |_| {
        let _ = config.get();
        let path = preview_path.get();
        if path.is_empty() {
            set_validation_msg.set(String::new());
            set_is_backend_valid.set(false);
            return;
        }

        let exclude_id = series_id.get_untracked();
        // Clone because `path` is moved into the request payload; the response
        // must be keyed by the exact candidate it was computed for.
        let key_path = path.clone();
        spawn_local(async move {
            match crate::api::validate_path(jumbie_shared::types::ValidatePathPayload {
                path,
                series_id: Some(exclude_id),
                // Folder-level collision + illegal-char resolution applies to this
                // move flow too — the preview shows the effective destination.
                resolve_collisions: true,
            })
            .await
            {
                Ok(res) => {
                    set_validation_msg.set(if res.is_valid {
                        String::new()
                    } else {
                        res.message
                    });
                    set_is_backend_valid.set(res.is_valid);
                    set_resolved_preview_for.set((key_path, res.resolved_path));
                }
                Err(_e) => {
                    set_validation_msg.set("Failed to validate path".to_string());
                    set_is_backend_valid.set(false);
                    set_resolved_preview_for.set((key_path, None));
                }
            }
        });
    });

    let can_save = Signal::derive(move || {
        let path = new_path.get();
        let trimmed = path.trim();
        // Path is valid if it's not empty, and different from the starting baseline
        !trimmed.is_empty()
            && preview_path.get_untracked() != current_path.get_untracked()
            && is_backend_valid.get()
    });

    let on_save = move |_| {
        if !can_save.get() {
            return;
        }
        let final_path = preview_path.get_untracked();
        if final_path != current_path.get_untracked() {
            set_current_path.set(final_path);
            set_path_operation.set(Some(operation.get_untracked()));
            save_series.run(None);
        }
        set_show.set(false);
    };

    let footer = view! {
        <div class="modal-footer flex items-center justify-end gap-md">
            <button
                class="btn btn-primary"
                class:disabled=move || !can_save.get()
                disabled=move || !can_save.get()
                on:click=on_save
            >"Save"</button>
        </div>
    };

    view! {
        <ModalWrapper
            id="editSeriesPathModal"
            show=show
            on_close=move |_| set_show.set(false)
            title=Signal::derive(move || EDIT_SERIES_PATH_TITLE.to_string())
            size="modal-md"
            footer=footer.into_any()
        >
            <div class="items-center">
                <FormatFieldBuilder
                    id="newBasePath".to_string()
                    label="New Base Path".to_string()
                    value=Signal::derive(move || new_path.get())
                    on_change=Callback::new(move |v| set_new_path.set(v))
                    tooltip=TooltipBuilder::new()
                        .with_context(TemplateContext::SeriesPath)
                    placeholder=Signal::derive(move || crate::hooks::path_placeholder(is_windows.get(), crate::hooks::EXAMPLE_MEDIA_PATH))
                    error=field_error
                />

                <CheckboxInput
                    id="createSeriesFolder".to_string()
                    checked=append_series_name
                    set_checked=Callback::new(move |v| set_append_series_name.set(v))
                    label="Create folder with series name".to_string()
                />

                <TextInput
                    label="Preview Path".to_string()
                    id="previewPath".to_string()
                    value=Signal::derive(move || effective_preview.get())
                    set_value=Callback::new(|_| {})
                    readonly=true
                    disabled=Signal::derive(move || true)
                />

                <div class="form-group">
                    <label class="form-label" for="existingFilesOperation">"Existing Files Operation"</label>
                    <select
                        id="existingFilesOperation"
                        class="form-control"
                        on:change=move |ev| {
                            let val = event_target_value(&ev);
                            let op = match val.as_str() {
                                "move" => PathOperation::Move,
                                "copy" => PathOperation::Copy,
                                "delete" => PathOperation::Delete,
                                _ => PathOperation::DoNothing,
                            };
                            set_operation.set(op);
                        }
                    >
                        <option value="move" selected=move || operation.get() == PathOperation::Move>"Move existing files to new directory"</option>
                        <option value="copy" selected=move || operation.get() == PathOperation::Copy>"Copy existing files to new directory"</option>
                        <option value="delete" selected=move || operation.get() == PathOperation::Delete>"Delete existing files in old directory"</option>
                        <option value="none" selected=move || operation.get() == PathOperation::DoNothing>"Do nothing (Keep files where they are)"</option>
                    </select>
                </div>
            </div>
        </ModalWrapper>
    }
}
