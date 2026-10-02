use super::{delete_unit, get, post, post_unit, put_unit};
use crate::api_client::ApiError as Error;

// Organized Series Endpoints

pub async fn fetch_organized_series()
-> Result<Vec<jumbie_shared::types::OrganizedSeriesItem>, Error> {
    crate::debug_log!("fetch_organized_series()");
    let result: Result<Vec<jumbie_shared::types::OrganizedSeriesItem>, Error> =
        get("system/organized_series").await;
    match &result {
        Ok(items) => crate::debug_log!("fetch_organized_series: {} items", items.len()),
        Err(e) => crate::debug_error!("fetch_organized_series failed: {}", e),
    }
    result
}

/// Toggle whether a specific series is actively monitored for new episodes.
pub async fn toggle_series_monitor(
    series_id: String,
    monitor_mode: jumbie_shared::types::MonitorMode,
) -> Result<(), Error> {
    crate::debug_log!("toggle_series_monitor({}, {:?})", series_id, monitor_mode);
    let payload = jumbie_shared::types::ToggleMonitorPayload { monitor_mode };
    let result = put_unit(
        &format!("system/organized_series/{}/toggle", series_id),
        &payload,
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!("toggle_series_monitor({}) succeeded", series_id),
        Err(e) => crate::debug_error!("toggle_series_monitor({}) failed: {}", series_id, e),
    }
    result
}

/// Toggle whether a series is hidden (archived) in the main library view.
pub async fn toggle_library_visibility(
    series_id: String,
    hidden_in_library: bool,
) -> Result<(), Error> {
    crate::debug_log!(
        "toggle_library_visibility({}, hidden={})",
        series_id,
        hidden_in_library
    );
    let payload = jumbie_shared::types::ToggleVisibilityPayload { hidden_in_library };
    let result = put_unit(
        &format!("system/organized_series/{}/visibility", series_id),
        &payload,
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!("toggle_library_visibility({}) succeeded", series_id),
        Err(e) => crate::debug_error!("toggle_library_visibility({}) failed: {}", series_id, e),
    }
    result
}

/// Batch-edit multiple organized series' settings at once (monitor, profile, etc.).
pub async fn batch_edit_organized_series(
    payload: jumbie_shared::types::BatchEditOrganizedSeriesPayload,
) -> Result<(), Error> {
    crate::debug_log!("batch_edit_organized_series()");
    let result = post_unit("system/organized_series/batch_edit", &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("batch_edit_organized_series succeeded"),
        Err(e) => crate::debug_error!("batch_edit_organized_series failed: {}", e),
    }
    result
}

/// Preview a batch-move operation without making changes.
pub async fn batch_move_series_preview(
    payload: jumbie_shared::types::BatchMoveOrganizedSeriesPayload,
) -> Result<jumbie_shared::types::BatchMovePreviewResponse, Error> {
    crate::debug_log!("batch_move_series_preview()");
    let result: Result<jumbie_shared::types::BatchMovePreviewResponse, Error> =
        post("system/organized_series/batch_move_preview", &payload).await;
    match &result {
        Ok(r) => crate::debug_log!(
            "batch_move_series_preview: {} items, has_collisions={}",
            r.items.len(),
            r.has_collisions
        ),
        Err(e) => crate::debug_error!("batch_move_series_preview failed: {}", e),
    }
    result
}

/// Execute a batch-move operation.
pub async fn batch_move_series(
    payload: jumbie_shared::types::BatchMoveOrganizedSeriesPayload,
) -> Result<jumbie_shared::types::BatchMoveResponse, Error> {
    crate::debug_log!("batch_move_series()");
    let result: Result<jumbie_shared::types::BatchMoveResponse, Error> =
        post("system/organized_series/batch_move", &payload).await;
    match &result {
        Ok(r) => crate::debug_log!(
            "batch_move_series: {} success, {} failures, task_id={:?}",
            r.success_count,
            r.failure_count,
            r.task_id
        ),
        Err(e) => crate::debug_error!("batch_move_series failed: {}", e),
    }
    result
}

/// Poll the status of an async batch-move operation.
pub async fn batch_move_status(
    task_id: &str,
) -> Result<crate::api_client::BatchMoveProgressResponse, Error> {
    let path = format!("system/organized_series/batch_move/{}/status", task_id);
    get(&path).await
}

// Rename Queue

/// Returns the rename queue — series/files pending renaming based on the naming template.
pub async fn fetch_rename_queue() -> Result<jumbie_shared::types::RenameQueueResponse, Error> {
    crate::debug_log!("fetch_rename_queue()");
    let result: Result<jumbie_shared::types::RenameQueueResponse, Error> =
        get("system/rename_queue").await;
    match &result {
        Ok(r) => crate::debug_log!("fetch_rename_queue: {} items", r.items.len()),
        Err(e) => crate::debug_error!("fetch_rename_queue failed: {}", e),
    }
    result
}

pub async fn fetch_rename_queue_detail(
    series_id: &str,
) -> Result<jumbie_shared::types::RenameQueueItemDetail, Error> {
    crate::debug_log!("fetch_rename_queue_detail({})", series_id);
    let result = get(&format!("system/rename_queue/{}", series_id)).await;
    match &result {
        Ok(_) => crate::debug_log!("fetch_rename_queue_detail({}) succeeded", series_id),
        Err(e) => crate::debug_error!("fetch_rename_queue_detail({}) failed: {}", series_id, e),
    }
    result
}

// Download Queue

pub async fn fetch_download_queue()
-> Result<Vec<crate::components::management::download_queue::DownloadQueueItem>, Error> {
    crate::debug_log!("fetch_download_queue()");
    let result: Result<
        Vec<crate::components::management::download_queue::DownloadQueueItem>,
        Error,
    > = get("queue").await;
    match &result {
        Ok(items) => crate::debug_log!("fetch_download_queue: {} items", items.len()),
        Err(e) => crate::debug_error!("fetch_download_queue failed: {}", e),
    }
    result
}

pub async fn remove_download_queue_item(id: i64) -> Result<(), Error> {
    crate::debug_log!("remove_download_queue_item({})", id);
    let result = delete_unit(&format!("queue/{}", id)).await;
    match &result {
        Ok(_) => crate::debug_log!("remove_download_queue_item({}) succeeded", id),
        Err(e) => crate::debug_error!("remove_download_queue_item({}) failed: {}", id, e),
    }
    result
}

/// Pause a download queue item.
/// Uses POST with an empty body as a workaround because the HTTP DELETE method
/// doesn't support a body natively via reqwest's API.
pub async fn pause_download_queue_item(id: i64) -> Result<(), Error> {
    crate::debug_log!("pause_download_queue_item({})", id);
    let result = post_unit(&format!("queue/{}/pause", id), &serde_json::json!({})).await;
    match &result {
        Ok(_) => crate::debug_log!("pause_download_queue_item({}) succeeded", id),
        Err(e) => crate::debug_error!("pause_download_queue_item({}) failed: {}", id, e),
    }
    result
}

/// Resume a download queue item.
pub async fn resume_download_queue_item(id: i64) -> Result<(), Error> {
    crate::debug_log!("resume_download_queue_item({})", id);
    let result = post_unit(&format!("queue/{}/resume", id), &serde_json::json!({})).await;
    match &result {
        Ok(_) => crate::debug_log!("resume_download_queue_item({}) succeeded", id),
        Err(e) => crate::debug_error!("resume_download_queue_item({}) failed: {}", id, e),
    }
    result
}

/// Delete a download queue item with an option to also delete its files.
pub async fn delete_download_queue_item(id: i64, delete_files: bool) -> Result<(), Error> {
    crate::debug_log!(
        "delete_download_queue_item({}, delete_files={})",
        id,
        delete_files
    );
    let payload = jumbie_shared::types::DeleteTorrentPayload { delete_files };
    let result = post_unit(&format!("queue/{}/delete", id), &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("delete_download_queue_item({}) succeeded", id),
        Err(e) => crate::debug_error!("delete_download_queue_item({}) failed: {}", id, e),
    }
    result
}

/// Retry a failed download queue item.
pub async fn retry_download_queue_item(id: i64) -> Result<(), Error> {
    crate::debug_log!("retry_download_queue_item({})", id);
    let result = post_unit(&format!("queue/{}/retry", id), &serde_json::json!({})).await;
    match &result {
        Ok(_) => crate::debug_log!("retry_download_queue_item({}) succeeded", id),
        Err(e) => crate::debug_error!("retry_download_queue_item({}) failed: {}", id, e),
    }
    result
}

// Media Scan

/// Re-scan a specific episode's media file (update file size, quality, etc.).
pub async fn scan_episode_media(episode_id: String) -> Result<(), Error> {
    crate::debug_log!("scan_episode_media({})", episode_id);
    let result = post_unit(&format!("episodes/{}/scan_media", episode_id), &()).await;
    match &result {
        Ok(_) => crate::debug_log!("scan_episode_media({}) succeeded", episode_id),
        Err(e) => crate::debug_error!("scan_episode_media({}) failed: {}", episode_id, e),
    }
    result
}

/// Returns a map of series_id → count of files currently being scanned in the
/// media info scan queue for that series. Only series with non-zero counts are
/// included. The frontend polls this to show live scan progress in the series
/// library and edit-series header.
pub async fn fetch_media_info_scan_counts()
-> Result<std::collections::HashMap<String, usize>, Error> {
    crate::debug_log!("fetch_media_info_scan_counts()");
    let result: Result<std::collections::HashMap<String, usize>, Error> =
        get("media-info-scan/counts").await;
    match &result {
        Ok(counts) => crate::debug_log!(
            "fetch_media_info_scan_counts: {} series with active scans",
            counts.len()
        ),
        Err(e) => crate::debug_error!("fetch_media_info_scan_counts failed: {}", e),
    }
    result
}

/// Returns the count of active scan paths for a single series.
///
/// A lightweight alternative to `fetch_media_info_scan_counts` when only one
/// series needs to be checked (e.g. the edit-series page).
pub async fn fetch_media_info_scan_count_for_series(
    series_id: &str,
) -> Result<std::collections::HashMap<String, usize>, Error> {
    crate::debug_log!("fetch_media_info_scan_count_for_series({})", series_id);
    let result: Result<std::collections::HashMap<String, usize>, Error> =
        get(&format!("media-info-scan/counts/{}", series_id)).await;
    match &result {
        Ok(counts) => crate::debug_log!("fetch_media_info_scan_count_for_series: {:?}", counts),
        Err(e) => crate::debug_error!("fetch_media_info_scan_count_for_series failed: {}", e),
    }
    result
}

// Filesystem Validation

/// Validates a filesystem path on the server (exists, is writable, …). Used in
/// settings forms to give the user immediate feedback before saving.
pub async fn validate_path(
    payload: jumbie_shared::types::ValidatePathPayload,
) -> Result<jumbie_shared::types::ValidatePathResponse, Error> {
    crate::debug_log!("validate_path({})", payload.path);
    let result: Result<jumbie_shared::types::ValidatePathResponse, Error> =
        super::post("system/validate-path", &payload).await;
    match &result {
        Ok(r) => crate::debug_log!(
            "validate_path: is_valid={}, message={}",
            r.is_valid,
            r.message
        ),
        Err(e) => crate::debug_error!("validate_path failed: {}", e),
    }
    result
}
