use super::{get, post, post_unit};
use crate::api_client::ApiError as Error;

/// Search for media across all configured indexers. The mode parameter determines
/// search scope (e.g., "season", "episode", "all"). series_id and episode_id are
/// optional — they're provided when the search is scoped to a specific show.
pub async fn search_media(
    query: String,
    mode: String,
    series_id: Option<String>,
    episode_id: Option<String>,
) -> Result<Vec<jumbie_shared::types::SearchResult>, Error> {
    crate::debug_log!("search_media(query={}, mode={})", query, mode);
    let body = jumbie_shared::types::SearchPayload {
        query,
        mode: Some(mode),
        series_id,
        episode_id,
        season: None,
        episode_numbers: None,
        is_user_requested: false,
    };
    let result: Result<Vec<jumbie_shared::types::SearchResult>, Error> =
        post("search", &body).await;
    match &result {
        Ok(results) => crate::debug_log!("search_media: {} results", results.len()),
        Err(e) => crate::debug_error!("search_media failed: {}", e),
    }
    result
}

/// Automatically search for a whole season's worth of episodes. The backend
/// resolves aliases and builds the query.
pub async fn auto_search_season(
    series_id: String,
    season: String,
    episode_numbers: Vec<i32>,
) -> Result<(), Error> {
    crate::debug_log!(
        "auto_search_season(series_id={}, season={}, episodes={:?})",
        series_id,
        season,
        episode_numbers
    );
    let body = jumbie_shared::types::SearchPayload {
        // Unused for auto modes — the backend builds the query.
        query: String::new(),
        mode: Some("auto_season".to_string()),
        series_id: Some(series_id),
        episode_id: None,
        season: Some(season),
        episode_numbers: Some(episode_numbers),
        // User-initiated: the season accordion's Auto Search button.
        is_user_requested: true,
    };
    // Uses "search/auto-season" rather than "search" with mode="auto_season":
    // the auto-season endpoint has compound scope middleware (search + queue:write)
    // enforced at the router level, making the permission boundary visible in the
    // route definition.
    let result: Result<(), Error> = post_unit("search/auto-season", &body).await;
    match &result {
        Ok(_) => crate::debug_log!(
            "auto_search_season({}, {}) succeeded",
            body.series_id.as_deref().unwrap_or("?"),
            body.season.as_deref().unwrap_or("?")
        ),
        Err(e) => crate::debug_error!(
            "auto_search_season({}, {}) failed: {}",
            body.series_id.as_deref().unwrap_or("?"),
            body.season.as_deref().unwrap_or("?"),
            e
        ),
    }
    result
}

/// Response shape for `GET /api/search/auto-season/{series_id}/{season}/status`.
#[derive(serde::Deserialize, Debug, Clone)]
pub struct AutoSearchStatus {
    pub running: bool,
}

/// Poll the backend to check if an auto-season search is still running for a
/// given series + season combination.
///
/// This is a lightweight endpoint (no DB queries) so it can be polled frequently
/// without concern for server load. Called by the season accordion to show a
/// "Searching..." pending state on the Auto Search button.
pub async fn get_auto_season_status(
    series_id: String,
    season: String,
) -> Result<AutoSearchStatus, Error> {
    let path = format!("search/auto-season/{}/{}/status", series_id, season);
    get(&path).await
}

/// Alias-aware auto-search for a single episode. Runs synchronously on the
/// backend and returns the queued result (if found). The backend builds the query.
pub async fn auto_search_episode(
    series_id: String,
    season: String,
    episode_numbers: Vec<i32>,
    is_user_requested: bool,
) -> Result<Vec<jumbie_shared::types::SearchResult>, Error> {
    crate::debug_log!(
        "auto_search_episode(series_id={}, season={}, episodes={:?})",
        series_id,
        season,
        episode_numbers
    );
    let body = jumbie_shared::types::SearchPayload {
        // Unused for auto modes — the backend builds the query.
        query: String::new(),
        mode: Some("auto_episode".to_string()),
        series_id: Some(series_id),
        episode_id: None,
        season: Some(season),
        episode_numbers: Some(episode_numbers),
        is_user_requested,
    };
    let result: Result<Vec<jumbie_shared::types::SearchResult>, Error> =
        post("search", &body).await;
    match &result {
        Ok(results) => crate::debug_log!("auto_search_episode: {} results", results.len()),
        Err(e) => crate::debug_error!("auto_search_episode failed: {}", e),
    }
    result
}

/// Send a download request to the configured download client. The link,
/// download_id, episode_id, title, and score metadata are used for post-download processing
/// and categorization.
pub async fn download_media(
    params: jumbie_shared::types::DownloadMediaPayload,
) -> Result<(), Error> {
    crate::debug_log!(
        "download_media(episode_id={:?}, title={:?}, series_id={:?}, is_season_pack={:?})",
        params.episode_id,
        params.title,
        params.series_id,
        params.is_season_pack
    );
    let result = post_unit("downloads", &params).await;
    match &result {
        Ok(_) => crate::debug_log!("download_media succeeded"),
        Err(e) => crate::debug_error!("download_media failed: {}", e),
    }
    result
}
