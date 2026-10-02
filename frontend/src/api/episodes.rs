use super::{delete_unit, post_unit, put_unit};
use crate::api_client::ApiError as Error;

/// Save custom episode metadata (title, description, runtime, image_url) via a
/// PUT with a partial JSON payload — only overridden fields are included.
pub async fn save_episode_custom_metadata(
    series_id: String,
    episode_id: String,
    payload: serde_json::Value,
) -> Result<(), Error> {
    crate::debug_log!(
        "save_episode_custom_metadata({}, {})",
        series_id,
        episode_id
    );
    let path = format!("series/{}/episodes/{}/metadata", series_id, episode_id);
    let result = put_unit(&path, &payload).await;
    match &result {
        Ok(_) => crate::debug_log!(
            "save_episode_custom_metadata({}, {}) succeeded",
            series_id,
            episode_id
        ),
        Err(e) => crate::debug_error!(
            "save_episode_custom_metadata({}, {}) failed: {}",
            series_id,
            episode_id,
            e
        ),
    }
    result
}

/// Clear user-defined episode metadata (resets to provider data).
pub async fn clear_episode_custom_metadata(
    series_id: String,
    episode_id: String,
) -> Result<(), Error> {
    crate::debug_log!(
        "clear_episode_custom_metadata({}, {})",
        series_id,
        episode_id
    );
    let path = format!("series/{}/episodes/{}/metadata", series_id, episode_id);
    let result = delete_unit(&path).await;
    match &result {
        Ok(_) => crate::debug_log!(
            "clear_episode_custom_metadata({}, {}) succeeded",
            series_id,
            episode_id
        ),
        Err(e) => crate::debug_error!(
            "clear_episode_custom_metadata({}, {}) failed: {}",
            series_id,
            episode_id,
            e
        ),
    }
    result
}

/// Restore a single episode's metadata back to provider state.
/// Nulls out custom/cleared metadata so the poller re-fills it from the provider.
pub async fn restore_episode_metadata(series_id: String, episode_id: String) -> Result<(), Error> {
    crate::debug_log!("restore_episode_metadata({}, {})", series_id, episode_id);
    let path = format!(
        "series/{}/episodes/{}/restore_metadata",
        series_id, episode_id
    );
    let result = post_unit(&path, &()).await;
    match &result {
        Ok(_) => crate::debug_log!(
            "restore_episode_metadata({}, {}) succeeded",
            series_id,
            episode_id
        ),
        Err(e) => crate::debug_error!(
            "restore_episode_metadata({}, {}) failed: {}",
            series_id,
            episode_id,
            e
        ),
    }
    result
}

/// Toggle a single episode's monitored flag via PUT /api/episodes/{id}/monitor.
pub async fn toggle_episode_monitor(episode_id: String, monitored: bool) -> Result<(), Error> {
    crate::debug_log!(
        "toggle_episode_monitor({}, monitored={})",
        episode_id,
        monitored
    );
    let payload = jumbie_shared::types::EpisodeMonitorTogglePayload { monitored };
    let result = put_unit(&format!("episodes/{}/monitor", episode_id), &payload).await;
    match &result {
        Ok(_) => crate::debug_log!(
            "toggle_episode_monitor({}, monitored={}) succeeded",
            episode_id,
            monitored
        ),
        Err(e) => crate::debug_error!(
            "toggle_episode_monitor({}, monitored={}) failed: {}",
            episode_id,
            monitored,
            e
        ),
    }
    result
}

/// Batch-set monitored flag for a list of episodes via POST /api/episodes/batch-monitor.
pub async fn batch_monitor_episodes(ids: Vec<String>, monitored: bool) -> Result<(), Error> {
    crate::debug_log!(
        "batch_monitor_episodes({} ids, monitored={})",
        ids.len(),
        monitored
    );
    let payload = jumbie_shared::types::BatchMonitorEpisodesPayload { ids, monitored };
    let result: Result<(), Error> = post_unit("episodes/batch-monitor", &payload).await;
    match &result {
        Ok(_) => crate::debug_log!("batch_monitor_episodes succeeded"),
        Err(e) => crate::debug_error!("batch_monitor_episodes failed: {}", e),
    }
    result
}

/// Match a single season's episodes to the active metadata provider.
/// Replaces any custom/cleared metadata with provider data.
pub async fn match_season_to_provider(series_id: String, season: String) -> Result<(), Error> {
    crate::debug_log!("match_season_to_provider({}, {})", series_id, season);
    let path = format!("series/{}/season/{}/match", series_id, season);
    let result = post_unit(&path, &()).await;
    match &result {
        Ok(_) => crate::debug_log!(
            "match_season_to_provider({}, {}) succeeded",
            series_id,
            season
        ),
        Err(e) => crate::debug_error!(
            "match_season_to_provider({}, {}) failed: {}",
            series_id,
            season,
            e
        ),
    }
    result
}
