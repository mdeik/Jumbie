use crate::api::{batch_edit_organized_series, batch_remove_series, remove_series};
use crate::components::common::confirmation_modal::ConfirmationModal;
use crate::components::common::form_fields::CheckboxInput;
use crate::components::common::nested_checkboxes::{NestedCheckboxGroup, NestedCheckboxItem};
use jumbie_shared::types::{BatchEditOrganizedSeriesPayload, BatchRemoveSeriesPayload, SeriesInfo};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashSet;

/// Which kind of deletion this modal performs.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum RemoveModalMode {
    /// Remove tracked series by series ID (uses `remove_series` /
    /// `batch_remove_series` API). Shows the full set of checkboxes
    /// (configurations, episode files, episode data).
    #[default]
    Series,
    /// Delete organized folder paths (uses `batch_edit_organized_series`
    /// API with `operation: "delete"`). Shows only a single
    /// "Delete files from disk" checkbox.
    Folder,
}

/// Removal modal for tracked series (`mode = Series`) and organized folders (`mode = Folder`).
///
/// ## Series mode (default)
///
/// The `delete_episodes` checkbox (main) deletes episode files **and** data
/// from disk. When checked, the children are visually selected but their
/// signals are **not** written to — the parent encompasses them, and the
/// effective child state is derived as `child_signal || parent_checked`.
/// This avoids triggering any per-child reactive side effects (e.g. API calls).
///
/// Behaviour per checkbox combination:
///   - Neither checked:          Just hide the series.
///   - Config only:              Delete settings, mappings. Episode data & files kept.
///   - Data only (sub only):     Delete episode metadata & media scans from DB.
///                                Files remain on disk. Settings kept.
///   - Files (main + sub):       Delete episode data **and** files from disk.
///                                Settings kept.
///   - Config + data:            Delete settings + episode data. Files kept.
///   - Config + files:           Full removal — configs, episode data, and files.
///
/// ## Folder mode
///
/// Only a single "Delete files from disk" checkbox is shown. The `series_ids`
/// signal holds **paths** instead of IDs. Configurations and episode data are
/// always removed from the database; the checkbox controls whether the raw
/// files on disk are also deleted.
#[component]
pub fn RemoveSeriesModal(
    #[prop(into)] show: Signal<bool>,
    set_show: WriteSignal<bool>,
    /// Signal holding the series IDs to delete (Series mode) or **paths** to
    /// delete (Folder mode).
    #[prop(into)]
    series_ids: Signal<Vec<String>>,
    /// Whether to delete configurations (settings, season mappings).
    /// Only used in Series mode.
    #[prop(into)]
    delete_configurations: Signal<bool>,
    set_delete_configurations: WriteSignal<bool>,
    /// Whether to delete episodes — files **and** data from disk (main checkbox).
    /// When checked, `delete_episode_data` is forced to `true`.
    /// Only used in Series mode.
    #[prop(into)]
    delete_episodes: Signal<bool>,
    set_delete_episodes: WriteSignal<bool>,
    /// Whether to delete episode data only (nested checkbox).
    /// Deletes metadata, media scans from DB but keeps files on disk.
    /// When `delete_episodes` is checked, this is forced to `true` and disabled.
    /// Only used in Series mode.
    #[prop(into)]
    delete_episode_data: Signal<bool>,
    set_delete_episode_data: WriteSignal<bool>,
    /// Called after a successful deletion with the list of deleted IDs/paths.
    /// Handles post-deletion actions (e.g. navigation, clearing selection, refresh).
    on_success: Callback<Vec<String>>,
    /// The removal mode — dictates which checkboxes appear, which API is
    /// called, and how the message reads. Defaults to `Series`.
    #[prop(optional)]
    mode: RemoveModalMode,
    /// Whether to delete files from disk (Folder mode only).
    /// `true` = delete files, `false` = keep files on disk.
    /// Mirrors the `delete_files` field in `BatchEditOrganizedSeriesPayload`.
    /// Requires `set_delete_files` to be provided alongside.
    #[prop(optional)]
    delete_files: Option<Signal<bool>>,
    /// Write signal counterpart for `delete_files`.
    #[prop(optional)]
    set_delete_files: Option<WriteSignal<bool>>,
    /// Optional title override. Defaults to "Remove Series" (Series mode) or
    /// "Delete Folder" (Folder mode).
    #[prop(optional)]
    title: Option<&'static str>,
) -> impl IntoView {
    // Derive the count so the message reactively updates.
    let count = Memo::new(move |_| series_ids.get().len());

    // `title` must be &'static str because ConfirmationModal requires it.
    let title: &'static str = title.unwrap_or(match mode {
        RemoveModalMode::Series => "Remove Series",
        RemoveModalMode::Folder => "Delete Folder",
    });

    // The message Memo and `NestedCheckboxGroup`'s internal Effect handle
    // everything reactively — no separate Effect needed here.

    let message = Memo::new(move |_| {
        let n = count.get();

        match mode {
            RemoveModalMode::Folder => {
                if n == 1 {
                    "Delete configurations, episode data, and all metadata for \
                     this folder from the database."
                        .to_string()
                } else {
                    format!(
                        "Delete configurations, episode data, and all metadata \
                         for {} folders from the database.",
                        n
                    )
                }
            }
            RemoveModalMode::Series => {
                let files = delete_episodes.get();
                // Derive effective child state: when parent is checked it
                // encompasses the children without setting their signals.
                let conf = delete_configurations.get() || files;
                let data = delete_episode_data.get() || files;

                let prefix = if n == 1 {
                    String::new()
                } else {
                    format!("{} selected series", n)
                };

                match (conf, data, files) {
                    (false, false, false) => {
                        if n == 1 {
                            "Hide this series from the library. All data is preserved \
                             and will be restored if unhidden."
                                .to_string()
                        } else {
                            format!(
                                "Hide {}. All data is preserved and will be restored if unhidden.",
                                prefix
                            )
                        }
                    }
                    (true, false, false) => {
                        if n == 1 {
                            "Remove all configurations (series settings, season settings, \
                             episode mappings). Episode files and metadata are kept."
                                .to_string()
                        } else {
                            format!(
                                "Remove configurations for {}. Episode files and metadata are kept.",
                                prefix
                            )
                        }
                    }
                    (false, true, false) => {
                        if n == 1 {
                            "Delete episode data (metadata, media scans) from the database. \
                             Episode files remain on disk and series settings are kept."
                                .to_string()
                        } else {
                            format!(
                                "Delete episode data for {}. Episode files remain on \
                                 disk and series settings are kept.",
                                prefix
                            )
                        }
                    }
                    (false, true, true) => {
                        if n == 1 {
                            "Delete episode data and the series directory from disk. \
                             Series settings are kept."
                                .to_string()
                        } else {
                            format!(
                                "Delete episode data and the series directory for {} \
                                 from disk. Series settings are kept.",
                                prefix
                            )
                        }
                    }
                    (true, true, false) => {
                        if n == 1 {
                            "Remove configurations and delete episode data (metadata, media scans). \
                             Episode files remain on disk."
                                .to_string()
                        } else {
                            format!(
                                "Remove configurations and delete episode data for {}. \
                                 Episode files remain on disk.",
                                prefix
                            )
                        }
                    }
                    (true, true, true) => {
                        if n == 1 {
                            "Fully remove this series — delete configurations, episode data, \
                             and the series directory from disk."
                                .to_string()
                        } else {
                            format!(
                                "Fully remove {} — delete configurations, episode data, \
                                 and the series directory from disk.",
                                prefix
                            )
                        }
                    }
                    // `(false, false, true)` is unreachable because the force-effect
                    // above ensures `data=true` when `files=true`.
                    _ => unreachable!(),
                }
            }
        }
    });

    let on_confirm = {
        let set_show = set_show.clone();
        move |_| {
            let ids = series_ids.get();
            let set_show = set_show.clone();

            match mode {
                RemoveModalMode::Folder => {
                    let delete = delete_files.as_ref().map(|s| s.get()).unwrap_or(true);
                    spawn_local(async move {
                        let result = batch_edit_organized_series(BatchEditOrganizedSeriesPayload {
                            paths: ids.clone(),
                            operation: "delete".to_string(),
                            delete_files: delete,
                        })
                        .await;

                        match result {
                            Ok(_) => {
                                set_show.set(false);
                                on_success.run(ids);
                            }
                            Err(e) => {
                                crate::debug_error!("Failed to delete folder: {}", e)
                            }
                        }
                    });
                }
                RemoveModalMode::Series => {
                    let del_files = delete_episodes.get();
                    // Derive effective child state: parent encompasses children.
                    let del_conf = delete_configurations.get() || del_files;
                    let del_data = delete_episode_data.get() || del_files;

                    spawn_local(async move {
                        let result = if ids.len() == 1 {
                            remove_series(ids[0].clone(), del_conf, del_data, del_files)
                                .await
                                .map(|_| ids.clone())
                        } else {
                            batch_remove_series(BatchRemoveSeriesPayload {
                                series_ids: ids.clone(),
                                delete_configurations: del_conf,
                                delete_episode_data: del_data,
                                delete_episodes: del_files,
                            })
                            .await
                            .map(|_| ids.clone())
                        };

                        match result {
                            Ok(deleted_ids) => {
                                set_show.set(false);
                                // Remove deleted series from the cached list so the library reflects it immediately.
                                if let Some(mut series_list) =
                                    crate::utils::read_cache::<Vec<SeriesInfo>>("fetch_series")
                                {
                                    let ids_set: HashSet<String> =
                                        deleted_ids.iter().cloned().collect();
                                    series_list.retain(|s| !ids_set.contains(&s.id));
                                    crate::utils::write_cache("fetch_series", &series_list);
                                }
                                // Drop stale series-scoped caches (details, calendar windows,
                                // wanted lists) so a re-added series never resurfaces stale data.
                                crate::utils::invalidate_series_caches(&deleted_ids);
                                on_success.run(deleted_ids);
                            }
                            Err(e) => {
                                crate::debug_error!("Failed to remove series: {}", e)
                            }
                        }
                    });
                }
            }
        }
    };

    view! {
        <ConfirmationModal
            show=show
            set_show=set_show
            title
            message=message
            on_confirm=Callback::new(on_confirm)
        >
            {move || match mode {
                RemoveModalMode::Folder => {
                    let delete = delete_files.unwrap_or_default();
                    view! {
                        <div class="flex-col gap-sm mt-lg">
                            <CheckboxInput
                                id="remove-delete-files".to_string()
                                checked=delete
                                set_checked=Callback::new(move |v| {
                                    if let Some(ref set_delete) = set_delete_files {
                                        set_delete.set(v);
                                    }
                                })
                                label="Delete folder from disk".to_string()
                            />
                            <p class="text-xs text-muted mt-sm">
                                "Configurations and episode data will always be removed from the \
                                 database. Uncheck this to keep the folder on disk."
                            </p>
                        </div>
                    }
                    .into_any()
                }
                RemoveModalMode::Series => {
                    view! {
                        <NestedCheckboxGroup
                            parent_id="delete-episodes-checkbox"
                            parent_label="Delete series directory"
                            parent_checked=delete_episodes
                            set_parent_checked=set_delete_episodes
                            children=vec![
                                NestedCheckboxItem {
                                    id: "delete-episode-data-checkbox".to_string(),
                                    label: "Delete episode data only (metadata, media scans) — keep files on disk".to_string(),
                                    checked: delete_episode_data,
                                    set_checked: set_delete_episode_data,
                                },
                                NestedCheckboxItem {
                                    id: "delete-config-checkbox".to_string(),
                                    label: "Delete configurations (settings, season/episode mappings)".to_string(),
                                    checked: delete_configurations,
                                    set_checked: set_delete_configurations,
                                },
                            ]
                        />
                    }
                    .into_any()
                }
            }}
        </ConfirmationModal>
    }
}
