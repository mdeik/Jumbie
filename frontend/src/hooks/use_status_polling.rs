use crate::hooks::use_config::ConfigContext;

use leptos::prelude::*;
use leptos::task::spawn_local;

#[derive(Clone, Copy)]
pub struct WantedHasItems(pub ReadSignal<bool>, pub WriteSignal<bool>);

#[derive(Clone, Copy)]
pub struct RenameQueuePopulated(pub ReadSignal<bool>, pub WriteSignal<bool>);

#[derive(Clone, Copy)]
pub struct RenameQueueFailed(pub ReadSignal<bool>, pub WriteSignal<bool>);

#[derive(Clone, Copy)]
pub struct DownloaderDisabled(pub ReadSignal<bool>, pub WriteSignal<bool>);

#[derive(Clone, Copy)]
pub struct DownloadQueueFailed(pub ReadSignal<bool>, pub WriteSignal<bool>);

#[derive(Clone, Copy)]
pub struct DownloadQueuePopulated(pub ReadSignal<bool>, pub WriteSignal<bool>);

#[derive(Clone, Copy)]
pub struct RenameQueueRefresh(pub ReadSignal<i32>, pub Callback<(), ()>);

/// Request an immediate re-fetch of ALL indicator-level booleans from
/// `fetch_system_status()`. Any page that performs a mutation (retry,
/// delete, rename, etc.) calls this so the sidebar indicators update
/// promptly instead of waiting for the 60-second polling interval.
#[derive(Clone, Copy)]
pub struct RefreshIndicators(pub Callback<(), ()>);

/// Sync the two download-queue-specific indicators from already-fetched queue
/// data, avoiding a redundant `fetch_system_status()`. Called by the 5-second
/// poll so the sidebar stays fresh even for backend-driven state changes.
#[derive(Clone, Copy)]
pub struct SyncDownloadQueueIndicators(pub Callback<(bool, bool), ()>);

pub fn use_media_info_scan_counts() -> ReadSignal<std::collections::HashMap<String, usize>> {
    let (counts, set_counts) = signal(std::collections::HashMap::<String, usize>::new());

    Effect::new(move |_| {
        // Initial fetch — gated by a 5-second cooldown so rapid mount/unmount cycles
        // (e.g. flipping between sections) don't fire redundant API calls. The polling
        // interval below handles periodic refreshes regardless.
        if crate::utils::check_cooldown("fetch_media_info_scan_counts", 5_000.0) {
            spawn_local(async move {
                if let Ok(c) = crate::api::fetch_media_info_scan_counts().await {
                    set_counts.set(c);
                }
            });
        }

        // Poll every 15 seconds, cleaned up on component unmount
        if let Ok(handle) = set_interval_with_handle(
            move || {
                // Polling fetches are also gated — if the user navigated back within
                // the cooldown window, we wait for the next interval tick instead of
                // firing immediately.
                if crate::utils::check_cooldown("fetch_media_info_scan_counts", 5_000.0) {
                    spawn_local(async move {
                        if let Ok(c) = crate::api::fetch_media_info_scan_counts().await {
                            set_counts.set(c);
                        }
                    });
                }
            },
            std::time::Duration::from_secs(15),
        ) {
            on_cleanup(move || handle.clear());
        }
    });

    counts
}

pub fn use_status_polling(is_authenticated: ReadSignal<bool>) {
    let ConfigContext {
        config,
        set_config: _,
    } = use_context::<ConfigContext>().expect("ConfigContext not found");

    let (rename_queue_populated, set_rename_queue_populated) = signal(false);
    crate::debug_log!("Providing RenameQueuePopulated context in use_status_polling");
    provide_context(RenameQueuePopulated(
        rename_queue_populated,
        set_rename_queue_populated,
    ));

    let (rename_queue_failed, set_rename_queue_failed) = signal(false);
    provide_context(RenameQueueFailed(
        rename_queue_failed,
        set_rename_queue_failed,
    ));

    let (downloader_disabled, set_downloader_disabled) = signal(false);
    provide_context(DownloaderDisabled(
        downloader_disabled,
        set_downloader_disabled,
    ));

    let (download_queue_failed, set_download_queue_failed) = signal(false);
    provide_context(DownloadQueueFailed(
        download_queue_failed,
        set_download_queue_failed,
    ));

    let (wanted_has_items, set_wanted_has_items) = signal(false);
    provide_context(WantedHasItems(wanted_has_items, set_wanted_has_items));

    let (download_queue_populated, set_download_queue_populated) = signal(false);
    provide_context(DownloadQueuePopulated(
        download_queue_populated,
        set_download_queue_populated,
    ));

    let (refresh_trigger, set_refresh_trigger) = signal(0);
    provide_context(RenameQueueRefresh(
        refresh_trigger,
        Callback::new(move |_| set_refresh_trigger.update(|v| *v += 1)),
    ));

    // Shared closure for a single status-fetch tick — used both for the
    // immediate fetch on mount and for each 60-second interval tick.
    let fetch_status = {
        let config = config.clone();
        move || {
            let config = config.clone();
            spawn_local(async move {
                if let Ok(status) = crate::api::fetch_system_status().await {
                    set_download_queue_failed.try_update(|d| *d = status.download_queue_has_failed);
                    set_download_queue_populated
                        .try_update(|d| *d = status.download_queue_has_items);
                    set_rename_queue_failed.try_update(|d| *d = status.rename_queue_has_failed);
                    set_wanted_has_items.try_update(|d| *d = status.wanted_has_items);
                    set_downloader_disabled.try_update(|d| *d = status.downloader_disabled);

                    // Apply auto_apply adjustment for rename queue populated
                    // (same logic as update_rename_status)
                    if status.rename_queue_has_failed {
                        set_rename_queue_populated.try_update(|p| *p = true);
                    } else {
                        let auto_apply = config
                            .get_untracked()
                            .map(|c| c.organization.auto_apply_renames)
                            .unwrap_or(false);
                        set_rename_queue_populated
                            .try_update(|p| *p = status.rename_queue_populated && !auto_apply);
                    }

                    // ── Reconcile progress toasts with backend operations ──
                    if !status.active_operations.is_empty() {
                        let ctx = crate::components::common::toast::use_notification();
                        ctx.reconcile_operations(status.active_operations);
                    }
                }
            });
        }
    };

    // Expose a trigger for on-demand indicator refresh, so pages can request an
    // immediate re-fetch after mutations instead of waiting 60 seconds.
    let refresh_indicators = RefreshIndicators(Callback::new(move |_| fetch_status()));
    provide_context(refresh_indicators);

    // Same for download-queue indicators — sync from already-fetched poll
    // data so the sidebar stays responsive even for background changes.
    let sync_dl_q = SyncDownloadQueueIndicators(Callback::new(move |(failed, populated)| {
        set_download_queue_failed.try_update(|f| *f = failed);
        set_download_queue_populated.try_update(|p| *p = populated);
    }));
    provide_context(sync_dl_q);

    Effect::new(move |_| {
        if is_authenticated.get() {
            // Only fetch full rename queue on manual refresh — the 60s polling
            // handles lightweight status updates via fetch_system_status.
            let trigger = refresh_trigger.get();
            if trigger > 0 {
                spawn_local(async move {
                    update_rename_status(
                        config,
                        set_rename_queue_populated,
                        set_rename_queue_failed,
                    )
                    .await;
                });
            }

            // Immediate fetch on mount — without this the indicators stay false
            // for up to 60 seconds after page load until the first interval tick.
            fetch_status();

            // Poll every 60 seconds — unified status endpoint. Deferring the initial
            // full rename-queue fetch: fetch_system_status() already provides accurate
            // indicator booleans from lightweight in-memory state; the full rename-queue
            // computation is only needed when the user views the queue page and refreshes.
            match set_interval_with_handle(
                move || {
                    fetch_status();
                },
                std::time::Duration::from_secs(60),
            ) {
                Ok(handle) => on_cleanup(move || handle.clear()),
                Err(_) => crate::debug_warn!("Failed to set interval for interval polling"),
            }
        }
    });
}

async fn update_rename_status(
    config: ReadSignal<Option<jumbie_shared::config::Config>>,
    set_populated: WriteSignal<bool>,
    set_failed: WriteSignal<bool>,
) {
    crate::debug_log!("Running update_rename_status polling...");
    match crate::api::fetch_rename_queue().await {
        Ok(res) => {
            crate::debug_log!(
                "update_rename_status fetch successful. total_affected: {}, has_failed: {}",
                res.total_affected_episodes,
                res.has_failed_renames
            );
            let auto_apply = config
                .get_untracked()
                .map(|c| c.organization.auto_apply_renames)
                .unwrap_or(false);

            if res.has_failed_renames {
                set_populated.try_update(|p| *p = true);
                set_failed.try_update(|f| *f = true);
            } else {
                set_populated.try_update(|p| *p = res.total_affected_episodes > 0 && !auto_apply);
                set_failed.try_update(|f| *f = false);
            }
        }
        Err(e) => {
            crate::debug_error!("update_rename_status failed: {:?}", e);
        }
    }
}
