use super::{get, post, post_unit};
use crate::api_client::{ApiError as Error, api_client};

/// Returns all tracked series — used for the main library view and sidebar counts.
pub async fn fetch_series() -> Result<Vec<jumbie_shared::types::SeriesInfo>, Error> {
    crate::debug_log!("fetch_series()");
    let result: Result<Vec<jumbie_shared::types::SeriesInfo>, Error> = get("series").await;
    match &result {
        Ok(series) => crate::debug_log!("fetch_series: found {} series", series.len()),
        Err(e) => crate::debug_error!("fetch_series failed: {}", e),
    }
    result
}

/// Fetches full detail for a single series, including episode-level info.
/// Returns None (not 404) so the caller can distinguish "series not found" from
/// other HTTP errors — this avoids a separate error type just for not-found.
pub async fn fetch_series_details(
    id: String,
) -> Result<Option<jumbie_shared::types::SeriesDetails>, Error> {
    crate::debug_log!("fetch_series_details({})", id);
    let path = format!("series/{}", id);
    let result: Result<Option<jumbie_shared::types::SeriesDetails>, Error> = get(&path).await;
    match &result {
        Ok(details) => crate::debug_log!(
            "fetch_series_details({}): {}",
            id,
            if details.is_some() {
                "found"
            } else {
                "not found"
            }
        ),
        Err(e) => crate::debug_error!("fetch_series_details({}) failed: {}", id, e),
    }
    result
}

/// Fetches details for multiple series in a single request.
/// Used by the preload system to avoid N individual HTTP round-trips.
pub async fn fetch_series_details_batch(
    ids: Vec<String>,
) -> Result<std::collections::HashMap<String, jumbie_shared::types::SeriesDetails>, Error> {
    crate::debug_log!("fetch_series_details_batch({} ids)", ids.len());
    #[derive(serde::Serialize)]
    struct BatchDetailsRequest {
        ids: Vec<String>,
    }
    let result: Result<
        std::collections::HashMap<String, jumbie_shared::types::SeriesDetails>,
        Error,
    > = post("series/details/batch", &BatchDetailsRequest { ids }).await;
    match &result {
        Ok(map) => crate::debug_log!("fetch_series_details_batch: got {} results", map.len()),
        Err(e) => crate::debug_error!("fetch_series_details_batch failed: {}", e),
    }
    result
}

/// Updates series metadata (monitor mode, quality profile, etc).
/// Uses the full builder because the response type differs from the request type.
pub async fn update_series(
    id: String,
    payload: jumbie_shared::types::UpdateSeriesPayload,
) -> Result<jumbie_shared::types::UpdateSeriesResponse, Error> {
    crate::debug_log!("update_series({})", id);
    let result: Result<jumbie_shared::types::UpdateSeriesResponse, Error> = api_client()
        .put_request(format!("series/{}", id))
        .json(&payload)
        .send()
        .await;
    {
        let series_id = id.clone();
        match &result {
            Ok(_) => crate::debug_log!("update_series({}) succeeded", series_id),
            Err(e) => crate::debug_error!("update_series({}) failed: {}", series_id, e),
        }
    }
    result
}

/// Batch-upsert seasons for a series.
///
/// Minimal payload — only sends the seasons to add/update, the episode count,
/// and the numbering mode flag. Avoids sending the entire series configuration.
pub async fn batch_upsert_seasons(
    id: String,
    payload: jumbie_shared::types::BatchUpsertSeasonsRequest,
) -> Result<Vec<String>, Error> {
    let result: Result<Vec<String>, Error> = api_client()
        .post_request(format!("series/{}/seasons", id))
        .json(&payload)
        .send()
        .await;
    result
}

/// Quick-add a series by path only, with backend defaults for monitor mode,
/// quality profile, etc.
///
/// `no_retry` (same as `create_series_with_request`): `POST /api/series` performs
/// the metadata sync inside the request and can take a long time; a timeout-triggered
/// retry would re-POST the same logical add, and the backend's visible-series claim
/// would reject it with a confusing collision error.
pub async fn create_series(path: String) -> Result<(), Error> {
    crate::debug_log!("create_series({})", path);
    let body = jumbie_shared::types::CreateSeriesRequest {
        path,
        series_name: None,
        scan_for_existing: None,
        monitor_mode: None,
        quality_profile: None,
        release_profile: None,
        metadata_ids: Default::default(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: Default::default(),
    };
    let result = api_client()
        .post_request("series")
        .json(&body)
        .no_retry()
        .send_unit()
        .await;
    match &result {
        Ok(_) => crate::debug_log!("create_series succeeded"),
        Err(e) => crate::debug_error!("create_series failed: {}", e),
    }
    result
}

/// Add a series with full control over all parameters (used by the advanced form).
/// The backend performs the metadata sync (and optional search-on-add) inside this
/// one request and returns the new series UUID.
///
/// `no_retry`: the request can take a long time (metadata sync inside it); a
/// timeout-triggered retry would re-POST and the backend's folder collision
/// handling would create a duplicate "Show (2)" series.
pub async fn create_series_with_request(
    request: jumbie_shared::types::CreateSeriesRequest,
) -> Result<String, Error> {
    crate::debug_log!("create_series_with_request({:?})", request.series_name);
    let result = api_client()
        .post_request("series")
        .json(&request)
        .no_retry()
        .send()
        .await;
    match &result {
        Ok(id) => crate::debug_log!("create_series_with_request: created series {}", id),
        Err(e) => crate::debug_error!("create_series_with_request failed: {}", e),
    }
    result
}

/// Preview what would happen when importing a batch of series from the filesystem.
/// Returns a diff-like preview before the user confirms.
pub async fn preview_series_import(
    req: jumbie_shared::types::PreviewSeriesRequest,
) -> Result<Vec<jumbie_shared::types::PreviewSeriesItem>, Error> {
    crate::debug_log!("preview_series_import()");
    let result: Result<Vec<jumbie_shared::types::PreviewSeriesItem>, Error> =
        post("system/organized_series/preview", &req).await;
    match &result {
        Ok(items) => {
            crate::debug_log!("preview_series_import: {} items to import", items.len())
        }
        Err(e) => crate::debug_error!("preview_series_import failed: {}", e),
    }
    result
}

/// Confirm the bulk import after previewing. This is the commit step.
pub async fn bulk_create_series(
    req: jumbie_shared::types::ConfirmSeriesImportRequest,
) -> Result<(), Error> {
    crate::debug_log!("bulk_create_series()");
    let result = post_unit("system/organized_series/bulk", &req).await;
    match &result {
        Ok(_) => crate::debug_log!("bulk_create_series succeeded"),
        Err(e) => crate::debug_error!("bulk_create_series failed: {}", e),
    }
    result
}

/// Remove a series from tracking, with an option to also delete its files from disk.
pub async fn remove_series(
    id: String,
    delete_configurations: bool,
    delete_episode_data: bool,
    delete_episodes: bool,
) -> Result<(), Error> {
    crate::debug_log!(
        "remove_series({}, config={}, data={}, files={})",
        id,
        delete_configurations,
        delete_episode_data,
        delete_episodes
    );
    let payload = jumbie_shared::types::RemoveSeriesPayload {
        delete_configurations,
        delete_episode_data,
        delete_episodes,
    };
    let result = api_client()
        .delete_request(format!("series/{}", id))
        .json(&payload)
        .send_unit()
        .await;
    match &result {
        Ok(_) => crate::debug_log!("remove_series({}) succeeded", id),
        Err(e) => crate::debug_error!("remove_series({}) failed: {}", id, e),
    }
    result
}

pub async fn delete_season(series_id: String, season: String) -> Result<(), Error> {
    crate::debug_log!("delete_season({}, {})", series_id, season);
    let result = super::delete_unit(&format!("series/{}/season/{}", series_id, season)).await;
    match &result {
        Ok(_) => crate::debug_log!("delete_season({}, {}) succeeded", series_id, season),
        Err(e) => crate::debug_error!("delete_season({}, {}) failed: {}", series_id, season, e),
    }
    result
}

/// Delete episode data (metadata, media scans) for a single season from the database.
/// Episode files remain on disk.
pub async fn delete_season_episode_data(series_id: String, season: String) -> Result<(), Error> {
    crate::debug_log!("delete_season_episode_data({}, {})", series_id, season);
    let result = post_unit(
        &format!(
            "series/{}/season/{}/actions/delete_episode_data",
            series_id, season
        ),
        &(),
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!(
            "delete_season_episode_data({}, {}) succeeded",
            series_id,
            season
        ),
        Err(e) => crate::debug_error!(
            "delete_season_episode_data({}, {}) failed: {}",
            series_id,
            season,
            e
        ),
    }
    result
}

/// Restore episode metadata for a single season from the per-provider cache.
/// If the cache has the provider's episode data for this season, it re-inserts
/// the metadata into the episodes table without calling the external API.
/// If no cached data exists, falls back to a full metadata fetch.
pub async fn restore_season_metadata(series_id: String, season: String) -> Result<(), Error> {
    crate::debug_log!("restore_season_metadata({}, {})", series_id, season);
    let result = post_unit(
        &format!("series/{}/season/{}/restore_metadata", series_id, season),
        &(),
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!(
            "restore_season_metadata({}, {}) succeeded",
            series_id,
            season
        ),
        Err(e) => crate::debug_error!(
            "restore_season_metadata({}, {}) failed: {}",
            series_id,
            season,
            e
        ),
    }
    result
}

/// Reset all configuration for a single season to defaults.
/// Season overrides (aliases, offsets, episode range, etc.) are removed.
/// Episode data for the season is kept.
pub async fn reset_season_configuration(series_id: String, season: String) -> Result<(), Error> {
    crate::debug_log!("reset_season_configuration({}, {})", series_id, season);
    let result = post_unit(
        &format!(
            "series/{}/season/{}/actions/reset_configuration",
            series_id, season
        ),
        &(),
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!(
            "reset_season_configuration({}, {}) succeeded",
            series_id,
            season
        ),
        Err(e) => crate::debug_error!(
            "reset_season_configuration({}, {}) failed: {}",
            series_id,
            season,
            e
        ),
    }
    result
}

/// Reorganize files for a single series (renaming/moving according to the naming template).
pub async fn reorganize_series(id: String, target_absolute: bool) -> Result<(), Error> {
    crate::debug_log!(
        "reorganize_series({}, target_absolute={})",
        id,
        target_absolute
    );
    let payload = jumbie_shared::types::ReorganizeSeriesPayload { target_absolute };
    let result = post_unit(&format!("series/{}/actions/reorganize", id), &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("reorganize_series({}) succeeded", id),
        Err(e) => crate::debug_error!("reorganize_series({}) failed: {}", id, e),
    }
    result
}

/// Reorganize all series at once — a bulk action triggered from the management page.
/// Synchronous reorganize-all. Blocks the HTTP response until complete.
/// Use `reorganize_all_series_async` for progress notifications.
pub async fn reorganize_all_series() -> Result<(), Error> {
    crate::debug_log!("reorganize_all_series()");
    let payload = jumbie_shared::types::ReorganizeAllPayload {};
    let result = post_unit("series/actions/reorganize_all", &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("reorganize_all_series succeeded"),
        Err(e) => crate::debug_error!("reorganize_all_series failed: {}", e),
    }
    result
}

/// Async reorganize-all. Spawns the operation in the background and returns
/// a task_id immediately. The frontend should poll `reorganize_all_status`
/// to track progress and display a progress toast.
pub async fn reorganize_all_series_async() -> Result<jumbie_shared::types::BatchMoveResponse, Error>
{
    crate::debug_log!("reorganize_all_series_async()");
    let result: Result<jumbie_shared::types::BatchMoveResponse, Error> =
        crate::api_client::api_client()
            .post("series/actions/reorganize_all_async", &())
            .await;
    match &result {
        Ok(r) => crate::debug_log!("reorganize_all_series_async: task_id={:?}", r.task_id),
        Err(e) => crate::debug_error!("reorganize_all_series_async failed: {}", e),
    }
    result
}

/// Poll the status of an async reorganize operation.
pub async fn reorganize_all_status(
    task_id: &str,
) -> Result<crate::api_client::BatchMoveProgressResponse, Error> {
    crate::debug_log!("reorganize_all_status({})", task_id);
    let path = format!("series/actions/reorganize_all/{}/status", task_id);
    let result: Result<crate::api_client::BatchMoveProgressResponse, Error> =
        crate::api_client::api_client().get(&path).await;
    match &result {
        Ok(p) => crate::debug_log!("reorganize_all_status: {}/{}", p.completed, p.total),
        Err(e) => crate::debug_error!("reorganize_all_status failed: {}", e),
    }
    result
}

pub async fn batch_edit_series(
    payload: jumbie_shared::types::BatchEditSeriesPayload,
) -> Result<(), Error> {
    crate::debug_log!("batch_edit_series()");
    let result = post_unit("series/batch-edit", &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("batch_edit_series succeeded"),
        Err(e) => crate::debug_error!("batch_edit_series failed: {}", e),
    }
    result
}

pub async fn batch_remove_series(
    payload: jumbie_shared::types::BatchRemoveSeriesPayload,
) -> Result<(), Error> {
    crate::debug_log!("batch_remove_series()");
    let result = post_unit("series/batch-delete", &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("batch_remove_series succeeded"),
        Err(e) => crate::debug_error!("batch_remove_series failed: {}", e),
    }
    result
}

/// The refresh endpoint returns a structured summary rather than just a boolean,
/// so the UI can display "Found X new episodes, Y downloads, Z total episodes."
/// This type is defined here because it's specific to this frontend module's
/// deserialization needs — the backend may return these fields in any shape.
#[derive(serde::Deserialize)]
pub struct RefreshResult {
    pub series_found: usize,
    pub downloads_found: usize,
    pub total_episodes: usize,
}

/// Triggers a backend refresh of a series' metadata from its source, returning a
/// summary of what was found (new episodes, downloads, etc.).
///
/// Uses a full builder because the response type differs from the request type.
/// The builder sends the empty body (`()`) automatically for non-GET requests.
pub async fn refresh_series(id: String) -> Result<RefreshResult, Error> {
    crate::debug_log!("refresh_series({})", id);
    let result: Result<RefreshResult, Error> = api_client()
        .post_request(format!("series/{}/refresh", id))
        .send()
        .await;
    {
        let series_id = id.clone();
        match &result {
            Ok(r) => crate::debug_log!(
                "refresh_series({}): series_found={}, downloads_found={}, total_episodes={}",
                series_id,
                r.series_found,
                r.downloads_found,
                r.total_episodes
            ),
            Err(e) => crate::debug_error!("refresh_series({}) failed: {}", series_id, e),
        }
    }
    result
}

/// Forces a metadata sync for a single series (re-fetches from TMDB/TheTVDB/etc.).
/// The front-end calls this after the user clicks "Sync Metadata" on the edit page.
pub async fn sync_metadata(
    series_id: String,
    metadata_id: Option<String>,
) -> Result<jumbie_shared::types::MetadataSyncStatus, Error> {
    crate::debug_log!("sync_metadata({}, {:?})", series_id, metadata_id);
    let mut builder = api_client().post_request(format!("series/{}/fetch_metadata", series_id));
    if let Some(mid) = metadata_id {
        builder = builder.json(&serde_json::json!({ "metadata_id": mid }));
    }
    let result = builder.send().await;
    match &result {
        Ok(status) => crate::debug_log!("sync_metadata({}) succeeded: {:?}", series_id, status),
        Err(e) => crate::debug_error!("sync_metadata({}) failed: {}", series_id, e),
    }
    result
}

/// Fetches the series canonical title from the metadata provider and optionally
/// updates the series title and/or merges aliases into the series settings.
/// `update_title`: update the series title with the canonical name from the provider.
/// `merge_aliases`: merge provider aliases into the series alias list (deduplicated).
pub async fn fetch_series_info(
    series_id: String,
    update_title: bool,
    merge_aliases: bool,
    provider_instance_id: Option<String>,
) -> Result<serde_json::Value, Error> {
    crate::debug_log!(
        "fetch_series_info({}, title={}, aliases={}, provider={:?})",
        series_id,
        update_title,
        merge_aliases,
        provider_instance_id
    );
    let mut url = format!("series/{}/fetch_series_info", series_id);
    if let Some(pid) = provider_instance_id.as_deref().filter(|s| !s.is_empty()) {
        url.push_str(&format!("?provider_instance_id={pid}"));
    }
    let result = api_client()
        .post_request(url)
        .json(&serde_json::json!({
            "update_title": update_title,
            "merge_aliases": merge_aliases,
        }))
        .send()
        .await;
    match &result {
        Ok(v) => crate::debug_log!("fetch_series_info({}) succeeded: {:?}", series_id, v),
        Err(e) => crate::debug_error!("fetch_series_info({}) failed: {}", series_id, e),
    }
    result
}

/// Fetches series aliases from the metadata provider and merges them into the
/// series settings, deduplicating against existing aliases.
pub async fn fetch_series_aliases(
    series_id: String,
    provider_instance_id: Option<String>,
) -> Result<serde_json::Value, Error> {
    crate::debug_log!(
        "fetch_series_aliases({}, provider={:?})",
        series_id,
        provider_instance_id
    );
    let mut url = format!("series/{}/fetch_series_aliases", series_id);
    if let Some(pid) = provider_instance_id.as_deref().filter(|s| !s.is_empty()) {
        url.push_str(&format!("?provider_instance_id={pid}"));
    }
    let result: Result<serde_json::Value, Error> = api_client().post_request(url).send().await;
    match &result {
        Ok(v) => crate::debug_log!(
            "fetch_series_aliases({}) succeeded: {} aliases",
            series_id,
            v.get("count").and_then(|c| c.as_u64()).unwrap_or(0)
        ),
        Err(e) => crate::debug_error!("fetch_series_aliases({}) failed: {}", series_id, e),
    }
    result
}

/// Delete all episode data for a series (rescrape metadata).
pub async fn delete_series_data(series_id: String) -> Result<(), Error> {
    crate::debug_log!("delete_series_data({})", series_id);
    let result = post_unit(
        &format!("series/{}/actions/delete_episode_data", series_id),
        &(),
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!("delete_series_data({}) succeeded", series_id),
        Err(e) => crate::debug_error!("delete_series_data({}) failed: {}", series_id, e),
    }
    result
}

/// Reset series configuration to defaults.
pub async fn reset_series_configuration(series_id: String) -> Result<(), Error> {
    crate::debug_log!("reset_series_configuration({})", series_id);
    let result = post_unit(
        &format!("series/{}/actions/reset_configuration", series_id),
        &(),
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!("reset_series_configuration({}) succeeded", series_id),
        Err(e) => {
            crate::debug_error!("reset_series_configuration({}) failed: {}", series_id, e)
        }
    }
    result
}

// Series Files Endpoints

/// Fetch all files associated with a series (episode files, metadata, etc.).
pub async fn fetch_series_files(
    id: String,
) -> Result<Vec<jumbie_shared::types::SeriesFileViewModel>, Error> {
    crate::debug_log!("fetch_series_files({})", id);
    let path = format!("series/{}/files", id);
    let result: Result<Vec<jumbie_shared::types::SeriesFileViewModel>, Error> = get(&path).await;
    match &result {
        Ok(files) => crate::debug_log!("fetch_series_files({}): {} files", id, files.len()),
        Err(e) => crate::debug_error!("fetch_series_files({}) failed: {}", id, e),
    }
    result
}

/// Manually assign a discovered file to a specific episode.
pub async fn assign_series_file(
    id: String,
    payload: jumbie_shared::types::AssignFilePayload,
) -> Result<(), Error> {
    crate::debug_log!("assign_series_file({})", id);
    let result = post_unit(&format!("series/{}/files/assign", id), &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("assign_series_file({}) succeeded", id),
        Err(e) => crate::debug_error!("assign_series_file({}) failed: {}", id, e),
    }
    result
}

/// Batch-delete files from a series. Uses a full builder because it's a DELETE with
/// a JSON body — the 1-line delete_unit helper only sends a bare DELETE.
pub async fn delete_series_files(
    id: String,
    payload: jumbie_shared::types::BatchDeletePayload,
) -> Result<(), Error> {
    crate::debug_log!("delete_series_files({})", id);
    let result = api_client()
        .delete_request(format!("series/{}/files", id))
        .json(&payload)
        .send_unit()
        .await;
    match &result {
        Ok(_) => crate::debug_log!("delete_series_files({}) succeeded", id),
        Err(e) => crate::debug_error!("delete_series_files({}) failed: {}", id, e),
    }
    result
}

/// Remove the association between a file and an episode without deleting the file.
pub async fn unassign_series_files(
    id: String,
    payload: jumbie_shared::types::BatchDeletePayload,
) -> Result<(), Error> {
    crate::debug_log!("unassign_series_files({})", id);
    let result = post_unit(&format!("series/{}/files/unassign", id), &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("unassign_series_files({}) succeeded", id),
        Err(e) => crate::debug_error!("unassign_series_files({}) failed: {}", id, e),
    }
    result
}

/// Assign multiple files in one request (import optimization).
pub async fn batch_assign_series_files(
    id: String,
    payload: jumbie_shared::types::BatchAssignPayload,
) -> Result<(), Error> {
    crate::debug_log!("batch_assign_series_files({})", id);
    let result = post_unit(&format!("series/{}/files/batch-assign", id), &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("batch_assign_series_files({}) succeeded", id),
        Err(e) => crate::debug_error!("batch_assign_series_files({}) failed: {}", id, e),
    }
    result
}

// Metadata Cache & Batch Endpoints (series-scoped)

/// Clear cached metadata for a series, optionally scoped to a specific provider.
pub async fn clear_metadata_cache(
    series_id: String,
    provider: Option<String>,
) -> Result<(), Error> {
    crate::debug_log!("clear_metadata_cache({}, {:?})", series_id, provider);
    let mut url = format!("series/{}/metadata-cache", series_id);
    if let Some(ref p) = provider {
        url = format!("{}?provider={}", url, p);
    }
    let result = api_client().delete_request(url).send_unit().await;
    match &result {
        Ok(_) => crate::debug_log!("clear_metadata_cache({}) succeeded", series_id),
        Err(e) => crate::debug_error!("clear_metadata_cache({}) failed: {}", series_id, e),
    }
    result
}

/// Match the entire series to the active metadata provider.
/// Re-imports episode metadata, overwriting any custom/cleared tags.
pub async fn match_series_to_provider(series_id: String) -> Result<(), Error> {
    crate::debug_log!("match_series_to_provider({})", series_id);
    let path = format!("series/{}/metadata/match", series_id);
    let result = post_unit(&path, &()).await;
    match &result {
        Ok(_) => crate::debug_log!("match_series_to_provider({}) succeeded", series_id),
        Err(e) => crate::debug_error!("match_series_to_provider({}) failed: {}", series_id, e),
    }
    result
}

/// Clear file_path for missing episode files on disk.
/// Returns the number of episodes that were cleared.
pub async fn clear_not_found_episode_files(series_id: &str) -> Result<usize, Error> {
    crate::debug_log!("clear_not_found_episode_files({})", series_id);
    let result: Result<serde_json::Value, Error> = post(
        &format!("series/{}/clear-not-found-files", series_id),
        &serde_json::json!({}),
    )
    .await;
    match result {
        Ok(v) => {
            let count = v.get("cleared").and_then(|c| c.as_u64()).unwrap_or(0) as usize;
            crate::debug_log!(
                "clear_not_found_episode_files({}): cleared {}",
                series_id,
                count
            );
            Ok(count)
        }
        Err(e) => {
            crate::debug_error!("clear_not_found_episode_files({}) failed: {}", series_id, e);
            Err(e)
        }
    }
}

/// Triggers a metadata fetch for multiple series at once.
/// Only processes series that have a metadata ID — others are silently skipped.
pub async fn batch_fetch_metadata(series_ids: Vec<String>) -> Result<(), Error> {
    crate::debug_log!("batch_fetch_metadata({} series)", series_ids.len());
    let result = post_unit(
        "series/batch-fetch-metadata",
        &serde_json::json!({ "series_ids": series_ids }),
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!("batch_fetch_metadata succeeded"),
        Err(e) => crate::debug_error!("batch_fetch_metadata failed: {}", e),
    }
    result
}
