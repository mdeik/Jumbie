use axum::{Json, extract::State, http::StatusCode};
use std::sync::Arc;

use crate::api::AppState;
use crate::error::AppError;

/// GET /api/queue
pub async fn get_queue(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<jumbie_shared::types::DownloadQueueItem>>, AppError> {
    tracing::debug!("get_queue called");
    match state.db.get_download_queue().await {
        Ok(mut queue) => {
            // Live progress is queried from the downloader client rather than the
            // DB snapshot, so the UI stays accurate without a DB write on every tick.
            let downloader_opt = state.downloader.clone();
            if let Some(downloader_lock) = downloader_opt {
                let downloader = downloader_lock.read().await;

                // Only in-flight items have live progress; completed/paused/queued
                // items keep their stored value, and items without a downloader_id
                // (manually queued) cannot report progress.
                for item in queue
                    .iter_mut()
                    .filter(|i| i.status == "Downloading" && i.downloader_id.is_some())
                {
                    let dl_id = item.downloader_id.as_ref().unwrap();
                    if let Ok(Some(progress)) = downloader.get_download_progress(dl_id).await {
                        item.progress = Some(progress);
                    }
                }
            }

            // Pause/Resume support varies by client (torrent clients do, DDL clients
            // typically don't). Without this the UI would show buttons that silently
            // no-op.
            if let Some(downloader_lock) = state.downloader.clone() {
                let downloader = downloader_lock.read().await;
                // `supports_pause_resume` walks the downloader plugin list, so it is
                // memoized per client to avoid O(items × downloaders) work.
                let mut supports_cache: std::collections::HashMap<String, bool> =
                    std::collections::HashMap::new();
                for item in queue.iter_mut().filter(|i| i.client_id.is_some()) {
                    if let Some(cid) = &item.client_id {
                        let supported = match supports_cache.get(cid) {
                            Some(v) => *v,
                            None => {
                                let v = downloader.supports_pause_resume(cid).await;
                                supports_cache.insert(cid.clone(), v);
                                v
                            }
                        };
                        item.supports_pause_resume = supported;
                    }
                }
            }

            // No-progress is derived at read time (not stored as a status) using the
            // same threshold the orchestrator detects stalls with.
            {
                let now = chrono::Utc::now().naive_utc();
                for item in queue.iter_mut().filter(|i| i.status == "Downloading") {
                    if let Some(elapsed) =
                        crate::organizer::no_progress_minutes(item.no_progress_since, now)
                    {
                        item.no_progress = true;
                        item.no_progress_minutes = Some(elapsed);
                    }
                }
            }

            Ok(Json(queue))
        }
        Err(e) => {
            tracing::error!("Failed to fetch download queue: {}", e);
            Err(AppError::Internal(anyhow::anyhow!(format!(
                "Failed to fetch queue: {}",
                e
            ))))
        }
    }
}

/// DELETE /api/queue/:id
pub async fn remove_from_queue(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<StatusCode, AppError> {
    tracing::debug!("remove_from_queue called: id={}", id);
    if id <= 0 {
        return Err(AppError::BadRequest("Invalid queue item ID".to_string()));
    }
    match state.db.remove_from_download_queue(id).await {
        Ok(_) => Ok(StatusCode::OK),
        Err(e) => {
            tracing::error!("Failed to remove item {} from queue: {}", id, e);
            Err(AppError::Internal(anyhow::anyhow!(format!(
                "Failed to remove item: {}",
                e
            ))))
        }
    }
}

/// POST /api/queue/:id/pause
pub async fn pause_download_queue(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<StatusCode, AppError> {
    tracing::debug!("pause_download_queue called: id={}", id);
    if id <= 0 {
        return Err(AppError::BadRequest("Invalid queue item ID".to_string()));
    }
    // Fetch-then-modify: the downloader_id needed to tell the client to pause comes
    // from the DB queue row, avoiding a separate store that could drift out of sync.
    let mut queue = state
        .db
        .get_download_queue()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
    let item = queue
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or_else(|| AppError::NotFound("Item not found in queue".to_string()))?;

    // Items added to the queue may not yet have been dispatched to a client (e.g.
    // all downloaders disabled), and you can't pause a download never started.
    if let Some(hash) = &item.downloader_id
        && let Some(downloader_lock) = &state.downloader
    {
        let downloader = downloader_lock.read().await;
        // Only the client that owns this item can pause it, and only when that
        // client supports pause/resume (mirrors the per-item button visibility
        // in the queue UI — a direct API call must not bypass the capability).
        if let Some(cid) = item.client_id.as_deref()
            && !downloader.supports_pause_resume(cid).await
        {
            return Err(AppError::BadRequest(
                "This download's client does not support pause/resume".to_string(),
            ));
        }
        downloader
            .pause_download(hash, item.client_id.as_deref())
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable(
                    "Failed to pause torrent — check that the download client is running"
                        .to_string(),
                )
            })?;
        // Status is persisted only after the client confirms the pause; if the
        // client is unreachable, "Downloading" remains correct — it wasn't paused.
        item.status = "Paused".to_string();
        state
            .db
            .update_download_queue_item(item)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
        return Ok(StatusCode::OK);
    }
    Err(AppError::BadRequest(
        "Item does not have a downloader ID".to_string(),
    ))
}

/// POST /api/queue/:id/resume
pub async fn resume_download_queue(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<StatusCode, AppError> {
    tracing::debug!("resume_download_queue called: id={}", id);
    if id <= 0 {
        return Err(AppError::BadRequest("Invalid queue item ID".to_string()));
    }
    let mut queue = state
        .db
        .get_download_queue()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
    let item = queue
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or_else(|| AppError::NotFound("Item not found in queue".to_string()))?;

    if let Some(hash) = &item.downloader_id
        && let Some(downloader_lock) = &state.downloader
    {
        let downloader = downloader_lock.read().await;
        // Same guard as pause: the owning client must support pause/resume.
        if let Some(cid) = item.client_id.as_deref()
            && !downloader.supports_pause_resume(cid).await
        {
            return Err(AppError::BadRequest(
                "This download's client does not support pause/resume".to_string(),
            ));
        }
        downloader
            .resume_download(hash, item.client_id.as_deref())
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable(
                    "Failed to resume torrent — check that the download client is running"
                        .to_string(),
                )
            })?;
        item.status = "Downloading".to_string();
        state
            .db
            .update_download_queue_item(item)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
        let _ = state.db.reset_progress_tracking(item.id).await;
        return Ok(StatusCode::OK);
    }
    Err(AppError::BadRequest(
        "Item does not have a downloader ID".to_string(),
    ))
}

/// POST /api/queue/:id/retry
///
/// Retry a failed item. With a `downloader_id` (already dispatched), the client's
/// retry mechanism runs first (e.g. qBittorrent removes the stale torrent without
/// deleting files) before the item is set to "Downloading" for re-dispatch by the
/// orchestrator. Without one (never dispatched), it is re-queued as "Queued".
///
/// `retry_download` is invoked here rather than left to the polling loop: the
/// orchestrator only calls the retry hook when it finds the item in an error or
/// missing state. If a download completed but organizing failed (e.g. missing
/// series mapping), the torrent stays "seeding" and the orchestrator would retry
/// organizing without ever calling the hook. Calling it synchronously lets the
/// client clean up regardless of the torrent's state so a fresh dispatch is clean.
pub async fn retry_download_queue(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<StatusCode, AppError> {
    tracing::debug!("retry_download_queue called: id={}", id);
    if id <= 0 {
        return Err(AppError::BadRequest("Invalid queue item ID".to_string()));
    }
    let mut queue = state
        .db
        .get_download_queue()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
    let item = queue
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or_else(|| AppError::NotFound("Item not found in queue".to_string()))?;

    if item.status != "Failed" {
        return Err(AppError::BadRequest(format!(
            "Cannot retry item with status '{}'. Only failed items can be retried.",
            item.status
        )));
    }

    if let Some(hash) = &item.downloader_id {
        // Ask the client to clean up before re-entering dispatch: qBittorrent
        // removes the stale torrent (without deleting files), others may re-check
        // or no-op. Skipping this lets `add_download` collide with the stale torrent
        // or the client keep reporting the same error.
        // The downloader_id implies a client_id was set during dispatch.
        if let Some(cid) = &item.client_id {
            if let Some(downloader_lock) = &state.downloader {
                let downloader = downloader_lock.read().await;
                if let Err(e) = downloader.retry_download(hash, cid).await {
                    tracing::warn!(
                        "retry_download for item {} (hash: {}) failed: {}. Proceeding with re-queue.",
                        id,
                        hash,
                        e
                    );
                }
            }
        } else {
            tracing::warn!(
                "Item {} has a downloader_id but no client_id — skipping retry hook.",
                id
            );
        }

        tracing::info!(
            "Retry: item {} was dispatched (hash: {:?}). Resuming poll.",
            id,
            item.downloader_id
        );
        item.status = "Downloading".to_string();
        item.error_message = None;
    } else {
        tracing::info!("Retry: item {} was never dispatched. Re-queuing.", id);
        item.status = "Queued".to_string();
        item.downloader_id = None;
        item.progress = None;
        item.error_message = None;
    }

    state
        .db
        .update_download_queue_item(item)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;

    // Reset retry budget: an organizing failure may have accumulated a high
    // retry_count, but this restart is a fresh download lifecycle.
    let _ = state.db.reset_queue_item_retry(item.id).await;
    let _ = state.db.reset_progress_tracking(item.id).await;

    Ok(StatusCode::OK)
}

/// POST /api/queue/:id/delete
pub async fn delete_download_queue(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(payload): Json<jumbie_shared::types::DeleteTorrentPayload>,
) -> Result<StatusCode, AppError> {
    tracing::debug!(
        "delete_download_queue called: id={}, delete_files={}",
        id,
        payload.delete_files
    );
    if id <= 0 {
        return Err(AppError::BadRequest("Invalid queue item ID".to_string()));
    }
    let mut queue = state
        .db
        .get_download_queue()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
    let item = queue
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or_else(|| AppError::NotFound("Item not found in queue".to_string()))?;

    if let Some(hash) = &item.downloader_id
        && let Some(downloader_lock) = &state.downloader
    {
        let downloader = downloader_lock.read().await;

        // Capture the download's on-disk location before the client removes it, so
        // the per-download staging folder can be culled afterwards if left empty
        // (qBittorrent removes the torrent's files but may keep the savepath folder).
        let content_path = if payload.delete_files {
            downloader.get_download_content_path(hash).await.ok()
        } else {
            None
        };

        // "Deleting" shows immediately (rather than "Downloading") while a slow
        // client responds, preventing repeat clicks; the row is removed afterwards.
        item.status = "Deleting".to_string();
        let _ = state.db.update_download_queue_item(item).await;

        // `delete_files` lets the user also remove the downloaded files from disk;
        // it is passed through to the client (e.g. qBittorrent's deleteFiles flag).
        downloader
            .delete_download(hash, payload.delete_files, item.client_id.as_deref())
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable(
                    "Failed to delete torrent — check that the download client is running"
                        .to_string(),
                )
            })?;
        state
            .db
            .remove_from_download_queue(id)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;

        // The files are gone; cull the staging folder if the client left it empty.
        if let Some(content_path) = content_path {
            let roots = downloader.get_download_roots().await;
            crate::file_manager::file_ops::remove_empty_download_dir(
                std::path::Path::new(&content_path),
                &roots,
            )
            .await;
        }

        return Ok(StatusCode::OK);
    }
    Err(AppError::BadRequest(
        "Item does not have a downloader ID".to_string(),
    ))
}
