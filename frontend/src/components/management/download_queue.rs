use jumbie_shared::formatting::LabelStyle;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};

use crate::api::{
    delete_download_queue_item, fetch_download_queue, pause_download_queue_item,
    remove_download_queue_item, resume_download_queue_item, retry_download_queue_item,
};
use crate::components::common::icons::{TrashIcon, XIcon};
use crate::components::common::status_badge::{ProgressBadge, StatusBadge};
use crate::components::common::table_builder::TableBuilder;
use crate::components::common::toast::{NotificationType, show_toast};
use crate::components::common::{FormattedTimestamp, SettingsBuilder};
use crate::components::profiles::profile_utils::score_class;
use crate::hooks::use_status_polling::{RefreshIndicators, SyncDownloadQueueIndicators};
use crate::hooks::use_ui_config::use_time_format;
use crate::utils::format_datetime_local;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct DownloadQueueItem {
    pub id: i64,
    pub downloaded_at: String,
    pub media_name: String,
    pub media_link: String,
    pub series_title: String,
    pub season: Option<String>,
    pub episode: Option<i32>,
    #[serde(default)]
    pub episode_end: Option<i32>,
    pub episode_id: Option<String>,
    pub score: i32,
    pub is_user_requested: bool,
    /// True when the user picked this specific release — renders the `Manual` badge.
    #[serde(default)]
    pub is_manual: bool,
    pub status: String,
    pub downloader_id: Option<String>,
    pub progress: Option<f32>,
    /// True when progress has not advanced for the configured window.
    #[serde(default)]
    pub no_progress: bool,
    /// Minutes without progress (warning tooltip).
    #[serde(default)]
    pub no_progress_minutes: Option<i64>,
    pub supports_pause_resume: bool,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub series_id: String,
    #[serde(default)]
    pub is_season_pack: bool,
    #[serde(default)]
    pub category: String,
}

/// Render the queue item's S/E label.
///
/// Once smart-link has resolved the episode, the queue row is authoritative.
/// Series-level search downloads deliberately leave the episode unresolved
/// until their files are mapped on disk, so we fall back to a hint parsed from
/// the release title and, failing that, make the uncertainty explicit with
/// `S??E??` instead of the misleading `S01E00`.
fn episode_label(item: &DownloadQueueItem) -> String {
    if let Some(ep) = item.episode {
        let season = item
            .season
            .as_deref()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(1);
        return jumbie_shared::formatting::fmt_season_episode(
            season,
            ep,
            item.episode_end,
            LabelStyle::Short,
        );
    }

    // Episode unresolved — derive a display hint from the release title.
    if let Some(info) = jumbie_shared::parsing::parse_filename(
        &item.media_name,
        jumbie_shared::parsing::ParseContext::Search,
    ) {
        let season = info.seasons.first().copied();
        let first = info.episodes.first().copied();
        let last = info.episodes.last().copied();
        return match (season, first) {
            // SSoT: the shared formatters own the S/E label shape.
            (Some(s), Some(f)) => {
                jumbie_shared::formatting::fmt_season_episode(s, f, last, LabelStyle::Short)
            }
            (Some(s), None) => jumbie_shared::formatting::fmt_season(s, LabelStyle::Short),
            _ => "S??E??".to_string(),
        };
    }

    "S??E??".to_string()
}

#[component]
pub fn DownloadQueueTable() -> impl IntoView {
    let (queue, set_queue) = signal(Vec::<DownloadQueueItem>::new());
    let (is_loading, set_is_loading) = signal(true);

    // Shared callback that re-fetches fetch_system_status() and updates ALL
    // sidebar indicator signals at once (DownloadQueueFailed, etc.).
    // Call this after any mutation so the breathing indicator clears promptly.
    let refresh_indicators = use_context::<RefreshIndicators>();
    let sync_dl_indicators = use_context::<SyncDownloadQueueIndicators>();

    let load_queue = move || {
        set_is_loading.set(true);
        spawn_local(async move {
            match fetch_download_queue().await {
                Ok(q) => {
                    // Compute indicators before moving q into the closure
                    let failed = q.iter().any(|i| i.status == "Failed");
                    let populated = !q.is_empty();
                    let _ = set_queue.try_update(|s| *s = q);
                    if let Some(s) = sync_dl_indicators {
                        s.0.run((failed, populated));
                    }
                }
                Err(e) => show_toast(
                    format!("Failed to load download queue: {}", e),
                    NotificationType::Error,
                ),
            }
            let _ = set_is_loading.try_update(|s| *s = false);
        });
    };

    Effect::new(move |_| {
        // Load on every mount — no cooldown gating.
        // The 5-second polling below already handles updates.
        load_queue();

        let handle = set_interval_with_handle(
            move || {
                spawn_local(async move {
                    if let Ok(q) = fetch_download_queue().await {
                        let failed = q.iter().any(|i| i.status == "Failed");
                        let populated = !q.is_empty();
                        let _ = set_queue.try_update(|s| *s = q);
                        if let Some(s) = sync_dl_indicators {
                            s.0.run((failed, populated));
                        }
                    }
                });
            },
            std::time::Duration::from_secs(5),
        )
        .expect("Failed to create interval");

        on_cleanup(move || {
            handle.clear();
        });
    });

    let time_format = use_time_format();

    let refresh_indicators_after = move || {
        if let Some(r) = refresh_indicators {
            r.0.run(());
        }
    };

    let remove_item = move |id: i64| {
        let refresh = refresh_indicators_after;
        crate::utils::spawn_api_toast(remove_download_queue_item(id), None, move |_| {
            crate::components::common::toast::show_success("Item removed from queue");
            load_queue();
            refresh();
        });
    };

    let pause_item = move |id: i64| {
        let refresh = refresh_indicators_after;
        crate::utils::spawn_api_toast(pause_download_queue_item(id), None, move |_| {
            crate::components::common::toast::show_success("Torrent paused");
            load_queue();
            refresh();
        });
    };

    let resume_item = move |id: i64| {
        let refresh = refresh_indicators_after;
        crate::utils::spawn_api_toast(resume_download_queue_item(id), None, move |_| {
            crate::components::common::toast::show_success("Torrent resumed");
            load_queue();
            refresh();
        });
    };

    let retry_item = move |id: i64| {
        let refresh = refresh_indicators_after;
        crate::utils::spawn_api_toast(retry_download_queue_item(id), None, move |_| {
            crate::components::common::toast::show_success("Item re-queued for download");
            load_queue();
            refresh();
        });
    };

    let delete_item = move |id: i64, delete_files: bool| {
        let refresh = refresh_indicators_after;
        crate::utils::spawn_api_toast(
            delete_download_queue_item(id, delete_files),
            None,
            move |_| {
                crate::components::common::toast::show_success("Torrent deleted");
                load_queue();
                refresh();
            },
        );
    };

    view! {

        {move || {
            let (sort_col, sort_asc, on_sort) = crate::hooks::use_persistent_table_state("download_queue".to_string(), "date".to_string(), false);

            let sorted_items = Signal::derive(move || {
                let mut items = queue.get();
                let col = sort_col.get();
                let asc = sort_asc.get();
                crate::utils::sorting::apply_sort(&mut items, &col, asc, |a: &DownloadQueueItem, b: &DownloadQueueItem, c: &str| {
                    match c {
                        "date" => jumbie_shared::formatting::natural_cmp(&a.downloaded_at, &b.downloaded_at),
                        "series" => jumbie_shared::formatting::natural_cmp(&a.series_title, &b.series_title),
                        "status" => jumbie_shared::formatting::natural_cmp(&a.status, &b.status),
                        "score" => a.score.cmp(&b.score),
                        _ => std::cmp::Ordering::Equal,
                    }
                });
                items
            });

            TableBuilder::new(sort_col, sort_asc)
                .container_class(Signal::derive(move || "table-container".to_string()))
                .table_variant(crate::components::common::table_builder::TableVariant::Hover)
                .loading(Signal::derive(move || is_loading.get() && queue.get().is_empty()))
                .empty_message("No torrents in the queue.")
                .on_sort(on_sort)
                .column("date", "Added", true)
                .column("series", "Series", true)
                .column("episode", "S/E", false)
                .column("score", "Score", true)
                .column("status", "Status", true)
                .column_with_class("actions", "Actions", false, "col-actions")
                .mobile_view(view! {
                    <div class="flex flex-col gap-md">
                        <For
                            each=move || sorted_items.get()
                            key=|item| (item.id, item.status.clone(), (item.progress.map(|p| (p * 1000.0) as i32)), item.no_progress)
                            children=move |item| {
                                let item_id = item.id;
                                let ep_str = episode_label(&item);

                                view! {
                                    <div class="card p-md flex flex-col gap-sm">
                                        <div class="card-row-with-badge">
                                            <div class="flex flex-col gap-xs min-w-0">
                                                <span class="truncate card-row-title font-bold text-lg">{item.series_title.clone()}</span>
                                                <span class="text-sm text-muted">{item.media_name.clone()}</span>
                                            </div>
                                            <div class="flex flex-col items-end gap-xs flex-shrink-0">
                                                <span class="badge badge-secondary">{ep_str}</span>
                                                {if item.is_manual {
                                                    view! { <StatusBadge variant="warning" label="Manual" /> }.into_any()
                                                } else {
                                                    view! { <span class=format!("badge badge-secondary {}", score_class(item.score))>{
                                                        if item.score > 0 { format!("+{}", item.score) }
                                                        else if item.score < 0 { item.score.to_string() }
                                                        else { "0".to_string() }
                                                    }</span> }.into_any()
                                                }}
                                                <ProgressBadge progress=item.progress status=item.status.clone() error_message=item.error_message.clone() no_progress=item.no_progress no_progress_minutes=item.no_progress_minutes />
                                            </div>
                                        </div>

                                        <div class="flex items-center justify-end gap-sm mt-xs pt-sm border-t">
                                            {if item.downloader_id.is_some() {
                                                view! {
                                                    <button class="btn btn-sm btn-danger" on:click=move |_| delete_item(item_id, true)>
                                                        <span class="icon"><TrashIcon /></span>
                                                        "Delete"
                                                    </button>
                                                    {if item.status == "Downloading" && item.supports_pause_resume && item.progress.is_none_or(|p| p < 1.0) {
                                                        view! {
                                                            <button class="btn btn-sm btn-dl-action btn-warning" on:click=move |_| pause_item(item_id)>
                                                                "Pause"
                                                            </button>
                                                        }.into_any()
                                                    } else if item.status == "Paused" && item.supports_pause_resume {
                                                        view! {
                                                            <button class="btn btn-sm btn-dl-action btn-success" on:click=move |_| resume_item(item_id)>
                                                                "Resume"
                                                            </button>
                                                        }.into_any()
                                                    } else if item.status == "Failed" {
                                                        view! {
                                                            <button class="btn btn-sm btn-dl-action btn-primary" on:click=move |_| retry_item(item_id)>
                                                                "Retry"
                                                            </button>
                                                        }.into_any()
                                                    } else { view! {}.into_any() }}
                                                }.into_any()
                                            } else if item.status == "Failed" {
                                                view! {
                                                    <button class="btn btn-sm btn-danger" on:click=move |_| remove_item(item_id)>
                                                        <span class="icon"><TrashIcon /></span>
                                                        "Remove"
                                                    </button>
                                                    <button class="btn btn-sm btn-dl-action btn-primary" on:click=move |_| retry_item(item_id)>
                                                        "Retry"
                                                    </button>
                                                }.into_any()
                                            } else {
                                                view! {
                                                        <button class="btn btn-sm btn-danger" on:click=move |_| remove_item(item_id)>
                                                            <span class="icon"><TrashIcon /></span>
                                                            "Remove"
                                                        </button>
                                                    }.into_any()
                                            }}
                                        </div>
                                    </div>
                                }
                            }
                        />
                    </div>
                }.into_any())
                .build(view! {
                    <For
                        each=move || sorted_items.get()
                        key=|item| (item.id, item.status.clone(), (item.progress.map(|p| (p * 1000.0) as i32)), item.no_progress)
                        children=move |item| {
                            let item_id = item.id;
                            let ep_str = episode_label(&item);

                            view! {
                                <tr>
                                    <td>
                                        <FormattedTimestamp value={format_datetime_local(&item.downloaded_at, &time_format.get())} />
                                    </td>
                                    <td>
                                        <div class="flex flex-col">
                                            <span class="weight-medium truncate series-title-truncate" title=item.series_title.clone()>{item.series_title.clone()}</span>
                                            <span class="text-xs text-muted truncate media-title-truncate" title=item.media_name.clone()>
                                                {item.media_name.clone()}
                                            </span>
                                        </div>
                                    </td>
                                    <td class="break-normal">
                                        {ep_str}
                                    </td>
                                    <td>
                                        {if item.is_manual {
                                            view! { <StatusBadge variant="warning" label="Manual" title="User-picked release" /> }.into_any()
                                        } else {
                                            view! { <span class=score_class(item.score)>{item.score.to_string()}</span> }.into_any()
                                        }}
                                    </td>
                                    <td class="break-normal">
                                        <ProgressBadge progress=item.progress status=item.status.clone() error_message=item.error_message.clone() no_progress=item.no_progress no_progress_minutes=item.no_progress_minutes />
                                    </td>
                                    <td class="col-actions">
                                        <div class="action-cell">
                                        {if item.downloader_id.is_some() {
                                            view! {
                                                    {if item.status == "Downloading" && item.supports_pause_resume && item.progress.is_none_or(|p| p < 1.0) {
                                                        view! {
                                                            <button class="btn btn-sm btn-dl-action btn-warning" title="Pause Download" on:click=move |_| pause_item(item_id)>
                                                                "Pause"
                                                            </button>
                                                        }.into_any()
                                                    } else if item.status == "Paused" && item.supports_pause_resume {
                                                        view! {
                                                            <button class="btn btn-sm btn-dl-action btn-success" title="Resume Download" on:click=move |_| resume_item(item_id)>
                                                                "Resume"
                                                            </button>
                                                        }.into_any()
                                                    } else if item.status == "Failed" {
                                                        view! {
                                                            <button class="btn btn-sm btn-dl-action btn-primary" title="Retry Download" on:click=move |_| retry_item(item_id)>
                                                                "Retry"
                                                            </button>
                                                        }.into_any()
                                                    } else {
                                                        view! {}.into_any()
                                                    }}
                                                <button class="btn btn-sm btn-icon btn-danger" title="Delete Torrent and Files" on:click=move |_| delete_item(item_id, true)>
                                                    <XIcon />
                                                </button>
                                            }.into_any()
                                        } else if item.status == "Failed" {
                                            view! {
                                                <button class="btn btn-sm btn-dl-action btn-primary" title="Retry Download" on:click=move |_| retry_item(item_id)>
                                                    "Retry"
                                                </button>
                                                <button class="btn btn-sm btn-icon btn-danger" title="Remove from Queue" on:click=move |_| remove_item(item_id)>
                                                    <XIcon />
                                                </button>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <button class="btn btn-sm btn-icon btn-danger" title="Remove from Queue" on:click=move |_| remove_item(item_id)>
                                                    <XIcon />
                                                </button>
                                            }.into_any()
                                        }}
                                        </div>
                                    </td>
                                </tr>
                            }
                        }
                    />
                }.into_any())
        }}
    }
}

#[component]
pub fn SystemDownloadQueue() -> impl IntoView {
    SettingsBuilder::new("download-queue")
        .raw_section(view! {
            <DownloadQueueTable />
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_item(extra: serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({
            "id": 1,
            "downloaded_at": "2026-01-01T00:00:00Z",
            "media_name": "Test S01E01",
            "media_link": "magnet:?xt=urn:btih:abc",
            "series_title": "Test",
            "season": "01",
            "episode": 1,
            "episode_id": "series_S01E01",
            "score": 0,
            "is_user_requested": false,
            "status": "Downloading",
            "downloader_id": null,
            "progress": null,
            "supports_pause_resume": false
        });
        let obj = base.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            obj.insert(k.clone(), v.clone());
        }
        base
    }

    /// Contract test: the queue API's no-progress fields must deserialize into the
    /// frontend item (guards against field-name drift from the shared type).
    #[test]
    fn deserializes_no_progress_fields() {
        let item: DownloadQueueItem = serde_json::from_value(api_item(serde_json::json!({
            "no_progress": true,
            "no_progress_minutes": 31
        })))
        .unwrap();
        assert!(item.no_progress);
        assert_eq!(item.no_progress_minutes, Some(31));
    }

    #[test]
    fn no_progress_defaults_to_false_when_absent() {
        let item: DownloadQueueItem =
            serde_json::from_value(api_item(serde_json::json!({}))).unwrap();
        assert!(!item.no_progress);
        assert_eq!(item.no_progress_minutes, None);
    }

    /// `is_manual` (user-picked release) drives the `Manual` badge. It must
    /// deserialize and default to false when the field is absent.
    #[test]
    fn deserializes_is_manual_flag() {
        let manual: DownloadQueueItem =
            serde_json::from_value(api_item(serde_json::json!({ "is_manual": true }))).unwrap();
        assert!(manual.is_manual);

        let auto: DownloadQueueItem =
            serde_json::from_value(api_item(serde_json::json!({}))).unwrap();
        assert!(!auto.is_manual);
    }
}
