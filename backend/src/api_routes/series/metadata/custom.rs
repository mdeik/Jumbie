use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::error::AppError;

use crate::api_routes::series::helpers::get_series_mapping_or_404;

use jumbie_shared::formatting::{fmt_absolute_episode_id, fmt_episode_id_num};

use super::fetch::fetch_metadata_for_series;

// Custom episode metadata management

/// Payload for saving custom metadata on a single episode.
#[derive(Debug, Deserialize)]
pub struct CustomMetadataPayload {
    pub title: Option<String>,
    pub description: Option<String>,
    pub runtime: Option<i32>,
    pub image_url: Option<String>,
    /// "YYYY-MM-DD" format
    pub meta_date: Option<String>,
}

/// `PUT /api/series/{id}/episodes/{episode_id}/metadata`
///
/// Save custom metadata for one episode. Updates the episode row with the
/// provided values and sets `metadata_source = 'custom'` so the background
/// poller will not overwrite them.
pub async fn save_custom_metadata(
    State(state): State<Arc<AppState>>,
    Path((id, episode_id)): Path<(String, String)>,
    Json(payload): Json<CustomMetadataPayload>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Saving custom metadata for episode {} of series {}",
        episode_id,
        id
    );

    crate::validation::validate_id(&episode_id, "Episode ID").map_err(|e| {
        tracing::debug!("save_custom_metadata: invalid episode ID: {}", e);
        AppError::BadRequest(e.0)
    })?;

    // Also validates the series ID.
    get_series_mapping_or_404(&state, &id).await?;

    if let Some(runtime) = payload.runtime {
        crate::validation::validate_runtime(runtime).map_err(|e| {
            tracing::error!("Invalid runtime in custom metadata: {}", e);
            AppError::BadRequest(e.0)
        })?;
    }

    if let Some(ref title) = payload.title {
        crate::validation::validate_max_length(title, "Episode title", 500).map_err(|e| {
            tracing::error!("Invalid episode title length: {}", e);
            AppError::BadRequest(e.0)
        })?;
    }

    if let Some(ref description) = payload.description {
        crate::validation::validate_max_length(description, "Episode description", 5000).map_err(
            |e| {
                tracing::error!("Invalid episode description length: {}", e);
                AppError::BadRequest(e.0)
            },
        )?;
    }

    if let Some(ref url) = payload.image_url
        && !url.is_empty()
    {
        crate::validation::validate_url(url).map_err(|e| {
            tracing::error!("Invalid image_url in custom metadata: {}", e);
            AppError::BadRequest(e.0)
        })?;
    }

    // Strict boundary: `meta_date` must be RFC 3339 with an explicit offset.
    // The stored column is always a full UTC datetime.
    let meta_date = match payload.meta_date.as_deref() {
        Some(date) if !date.is_empty() => {
            Some(crate::datetime::parse_request_utc(date).map_err(AppError::BadRequest)?)
        }
        _ => None,
    };

    let found = state
        .db
        .save_custom_episode_metadata(crate::db::episodes::SaveCustomMetadataParams {
            episode_id: &episode_id,
            series_id: &id,
            title: payload.title.as_deref(),
            description: payload.description.as_deref(),
            runtime: payload.runtime,
            image_url: payload.image_url.as_deref(),
            meta_date,
        })
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("Failed to save custom metadata: {}", e))
        })?;

    if !found {
        return Err(AppError::NotFound(format!(
            "Episode {} not found in series {}",
            episode_id, id
        )));
    }

    tracing::info!(
        "Custom metadata saved for episode {} of series {}",
        episode_id,
        id
    );

    Ok(StatusCode::OK)
}

/// `DELETE /api/series/{id}/episodes/{episode_id}/metadata`
///
/// Clear metadata for one episode. Nulls out all metadata fields and sets
/// `metadata_source = 'cleared'` so the background poller will not re-populate.
pub async fn clear_episode_metadata(
    State(state): State<Arc<AppState>>,
    Path((id, episode_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Clearing metadata for episode {} of series {}",
        episode_id,
        id
    );

    crate::validation::validate_id(&episode_id, "Episode ID").map_err(|e| {
        tracing::debug!("clear_episode_metadata: invalid episode ID: {}", e);
        AppError::BadRequest(e.0)
    })?;

    // Also validates the series ID.
    get_series_mapping_or_404(&state, &id).await?;

    let found = state
        .db
        .clear_episode_metadata(&episode_id, &id)
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("Failed to clear episode metadata: {}", e))
        })?;

    if !found {
        return Err(AppError::NotFound(format!(
            "Episode {} not found in series {}",
            episode_id, id
        )));
    }

    tracing::info!(
        "Metadata cleared for episode {} of series {}",
        episode_id,
        id
    );

    Ok(StatusCode::OK)
}

/// `POST /api/series/{id}/season/{season}/match`
///
/// Reset all episodes in a season back to provider state. Nulls out custom
/// metadata fields and sets `metadata_source = NULL` so the background poller
/// will re-fill them as Missing → Provider on the next cycle.
pub async fn match_season_to_provider(
    State(state): State<Arc<AppState>>,
    Path((id, season)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let mapping = get_series_mapping_or_404(&state, &id).await?;

    tracing::debug!(
        "Matching season {} of series {} ({}) back to provider metadata",
        season,
        mapping.target_title,
        id
    );

    // The season comes from the URL path, so an unresolvable label is a bad
    // request — never coerce it into some other season. Absolute numbering is
    // canonically season 1 (`ABSOLUTE_SEASON_NUM`), so the label is not
    // consulted in that mode.
    let absolute = state.effective_absolute_numbering(&mapping).await;
    let season_num = jumbie_shared::mapping::resolve_season_num(&season, absolute)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    state
        .db
        .match_season_to_provider(&id, season_num)
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("Failed to match season to provider: {}", e))
        })?;

    // Explicit re-adopt: clear the season's suppression so it is not filtered
    // out of the season list / metadata sync.
    if let Err(e) = state
        .db
        .unsuppress_season(&id, season_num, absolute as i32)
        .await
    {
        tracing::warn!(
            "Failed to clear suppression for matched season {} of {}: {}",
            season_num,
            id,
            e
        );
    }

    // Unassign episodes the provider does not have: "match to provider" means the
    // season should contain exactly the provider's episodes. Any extra row is
    // unassigned (its file durably blocked so a rescan can't re-adopt it) and
    // removed so it stops inflating counts. Runs only when the provider's episode
    // list for the season is known from the cache — otherwise "extra" is unknown
    // and the season is left untouched.
    let required = Some(if absolute {
        jumbie_shared::plugin::Capability::MetadataProviderAbsolute
    } else {
        jumbie_shared::plugin::Capability::MetadataProviderNormal
    });
    if let Ok(provider) =
        super::fetch::resolve_provider_for_series(&state, &mapping, None, required).await
    {
        let ordering_mode = if absolute { "absolute" } else { "normal" };
        let provider_eps = state
            .db
            .get_metadata_episodes_cache_for_season(
                &provider.metadata_id,
                &provider.plugin_id,
                &provider.instance_id,
                ordering_mode,
                season_num,
            )
            .await
            .unwrap_or_default();
        if !provider_eps.is_empty() {
            let keep: std::collections::HashSet<i32> =
                provider_eps.iter().map(|e| e.episode_number).collect();
            let rows = state
                .db
                .get_episode_rows_for_season(&id, season_num, absolute as i32)
                .await
                .unwrap_or_default();
            let mut extra_ids: Vec<String> = Vec::new();
            for (episode_id, episode_num, file_path) in &rows {
                if keep.contains(episode_num) {
                    continue;
                }
                if let Some(p) = file_path
                    && std::path::Path::new(p).exists()
                    && let Some((size, hash)) = state.db.path_file_identity(p).await
                    && let Some(name) = std::path::Path::new(p).file_name().and_then(|n| n.to_str())
                {
                    let _ = state.db.block_file(&id, name, size, hash.as_deref()).await;
                }
                extra_ids.push(episode_id.clone());
            }
            if !extra_ids.is_empty() {
                tracing::debug!(
                    "Matching season {} of {} ({}) to provider: unassigning {} extra episode(s)",
                    season_num,
                    mapping.target_title,
                    id,
                    extra_ids.len()
                );
                if let Err(e) = state.db.delete_episodes(&extra_ids).await {
                    tracing::warn!(
                        "Failed to remove extra episodes for season {} of {}: {}",
                        season_num,
                        id,
                        e
                    );
                }
            }
        }
    }

    // Reset this provider's `metadata_last_synced_at` so the background refresh
    // loop re-fetches metadata on its next tick instead of thinking everything
    // is up-to-date.
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
        mapping.settings.metadata_last_synced_at.clear();
        if let Err(e) = state.db.upsert_series_mapping(&id, &mapping).await {
            tracing::error!(
                "Failed to reset metadata_last_synced_at after matching season to provider: {}",
                e
            );
        }

        // Invalidate rename plan cache — season metadata rematched
    }

    tracing::debug!(
        "Season {} of series {} ({}) matched back to provider",
        season,
        mapping.target_title,
        id
    );

    Ok(StatusCode::OK)
}

/// `POST /api/series/{id}/metadata/match`
///
/// Series-wide version of the season match operation. Resets all episodes
/// for the series back to provider state.
pub async fn match_series_to_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mapping = get_series_mapping_or_404(&state, &id).await?;

    tracing::info!(
        "Matching series {} ({}) back to provider metadata",
        mapping.target_title,
        id
    );

    state.db.match_series_to_provider(&id).await.map_err(|e| {
        AppError::Internal(anyhow::anyhow!("Failed to match series to provider: {}", e))
    })?;

    // Explicit re-adopt: clear ALL season suppressions for this series.
    if let Err(e) = state.db.unsuppress_all_seasons(&id).await {
        tracing::warn!("Failed to clear season suppressions for {}: {}", id, e);
    }

    // Reset all `metadata_last_synced_at` entries so the background refresh
    // loop re-fetches metadata on its next tick instead of thinking everything
    // is up-to-date.
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
        mapping.settings.metadata_last_synced_at.clear();
        if let Err(e) = state.db.upsert_series_mapping(&id, &mapping).await {
            tracing::error!(
                "Failed to reset metadata_last_synced_at after matching series to provider: {}",
                e
            );
        }

        // Invalidate rename plan cache — series metadata rematched
    }

    tracing::info!(
        "Series {} ({}) matched back to provider",
        mapping.target_title,
        id
    );

    Ok(StatusCode::OK)
}

/// `POST /api/series/{id}/episodes/{episode_id}/restore_metadata`
///
/// Restore a single episode's metadata back to provider state. Looks up the
/// episode in the per-provider metadata cache and re-inserts the cached values.
/// Falls back to a full metadata fetch if no cache entry is found.
pub async fn restore_episode_metadata(
    State(state): State<Arc<AppState>>,
    Path((id, episode_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Restoring metadata for episode {} of series {}",
        episode_id,
        id
    );

    let mapping = get_series_mapping_or_404(&state, &id).await?;

    // Try to find the episode to get its season number.
    // If the episode was deleted from the episodes table, parse the season
    // from the episode_id so we can still look up cached metadata.
    let season_number: i32 = match state.db.get_episode_by_id(&episode_id).await {
        Ok(Some(row)) => {
            // Null out metadata_source so batch_insert can overwrite.
            let season = row
                .season
                .ok_or_else(|| AppError::BadRequest("Episode has no season number".to_string()))?;
            state
                .db
                .reset_episode_metadata_source(&episode_id, &id)
                .await
                .map_err(|e| {
                    AppError::Internal(anyhow::anyhow!(
                        "Failed to reset episode metadata source: {}",
                        e
                    ))
                })?;
            season
        }
        Ok(None) => {
            // Episode was deleted — parse the season from the episode_id.
            // Format: {series_id}_S{season:02}E{episode:02} or {series_id}_ABS{episode:04}.
            tracing::info!(
                "Episode {} not found in episodes table, attempting restore from cache",
                episode_id
            );
            // SSoT: jumbie_shared::formatting (the inverse of fmt_episode_id).
            jumbie_shared::formatting::parse_season_from_episode_id(&episode_id).ok_or_else(
                || {
                    AppError::BadRequest(format!(
                        "Could not parse season from episode_id: {}",
                        episode_id
                    ))
                },
            )?
        }
        Err(e) => {
            return Err(AppError::Internal(anyhow::anyhow!(
                "Failed to look up episode: {}",
                e
            )));
        }
    };

    // SSoT: resolve the provider instance + external ID (priority-ordered,
    // mode-filtered) rather than an arbitrary `metadata_ids` entry. Resolved before
    // metadata_id so the external ID is looked up by the active provider's UUID key.
    let abs_mode = state.effective_absolute_numbering(&mapping).await;
    let required = Some(if abs_mode {
        jumbie_shared::plugin::Capability::MetadataProviderAbsolute
    } else {
        jumbie_shared::plugin::Capability::MetadataProviderNormal
    });
    let provider = crate::api_routes::series::metadata::fetch::resolve_provider_for_series(
        &state, &mapping, None, required,
    )
    .await?;
    // Cache key is (metadata_id, plugin_id, instance_id): `plugin_id` is the backend
    // TYPE id (e.g. "jumbie.tvdb"), `instance_id` the instance. The episode
    // `metadata_ids` map / `metadata_source` columns are keyed by the INSTANCE id.
    let instance_id = provider.instance_id;
    let plugin_id = provider.plugin_id;
    let metadata_id = provider.metadata_id;

    let ordering_mode = if abs_mode { "absolute" } else { "normal" };

    // Try fetching cached metadata for the season this episode belongs to.
    // Absolute episodes resolve to season 1 (see ABSOLUTE_SEASON_NUM), which is
    // how absolute metadata is cached.
    let cached_episodes = state
        .db
        .get_metadata_episodes_cache_for_season(
            &metadata_id,
            &plugin_id,
            &instance_id,
            ordering_mode,
            season_number,
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;

    if let Some(cached) = cached_episodes.iter().find(|ce| {
        let expected_id = if abs_mode {
            // Absolute ids have no season component.
            fmt_absolute_episode_id(ce.episode_number, &mapping.series_id)
        } else {
            fmt_episode_id_num(ce.season_number, ce.episode_number, &mapping.series_id)
        };
        expected_id == episode_id
    }) {
        tracing::info!(
            "Restoring cached metadata for episode {} of series {} (provider={})",
            episode_id,
            mapping.target_title,
            plugin_id
        );

        // The cache stores meta_date as a DB-canonical (naive) UTC string — an
        // internal value, so the lenient parser is correct here.
        let meta_date = cached
            .meta_date
            .as_deref()
            .and_then(|d| crate::datetime::parse_utc(d).ok());

        let episode_metadata = crate::plugins::metadata::EpisodeMetadata {
            unique_id: cached.unique_id.clone(),
            season: cached.season_number,
            episode: cached.episode_number,
            title: cached.title.clone(),
            description: cached.description.clone(),
            runtime: cached.runtime,
            image_url: cached.image_url.clone(),
            meta_date,
        };

        state
            .db
            .batch_insert_metadata_episodes(
                &instance_id,
                &mapping.series_id,
                vec![(
                    if abs_mode {
                        fmt_absolute_episode_id(cached.episode_number, &mapping.series_id)
                    } else {
                        // Use the cache row's own season, not the parsed one: the
                        // two are the same entry, and this keeps the restored
                        // episode_id identical to the one matched above.
                        fmt_episode_id_num(
                            cached.season_number,
                            cached.episode_number,
                            &mapping.series_id,
                        )
                    },
                    episode_metadata,
                )],
                abs_mode as i32,
            )
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;

        // Re-apply monitor mode — newly restored metadata may affect
        // whether episodes should be monitored under Future mode.
        // SSoT: monitored is always derived from the series' monitor mode.
        crate::source_processor::reapply_monitor_for_series(
            &state.db,
            &mapping.series_id,
            false, // sweep: preserve manual toggles
            {
                let c = state.cfg.read().await;
                c.general.absolute_numbering
            },
        )
        .await;

        // Re-estimate release dates
        if let Err(e) = crate::release_estimator::run_release_date_estimation(
            &state.db,
            &mapping.series_id,
            abs_mode,
        )
        .await
        {
            tracing::warn!(
                "Failed to run release date estimator after episode restore for {}: {}",
                mapping.series_id,
                e
            );
        }
    } else {
        // Cache miss — trigger a full metadata fetch via the queue (P0 / user-initiated).
        tracing::info!(
            "No cached metadata for episode {} of series {}, submitting to queue",
            episode_id,
            mapping.target_title
        );
        let state_clone = state.clone();
        let series_id_for_log = id.clone();
        let id_for_closure = id.clone();
        state
            .metadata_queue
            .submit_and_wait(series_id_for_log.clone(), move || {
                let state = state_clone.clone();
                let sid = id_for_closure.clone();
                async move { fetch_metadata_for_series(&state, &sid, None).await }
            })
            .await?;
    }

    // Reset all `metadata_last_synced_at` entries so the background refresh
    // loop picks up any remaining changes on its next tick.
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
        mapping.settings.metadata_last_synced_at.clear();
        if let Err(e) = state.db.upsert_series_mapping(&id, &mapping).await {
            tracing::error!(
                "Failed to reset metadata_last_synced_at after restoring episode metadata: {}",
                e
            );
        }

        // Invalidate rename plan cache — episode metadata restored
    }

    tracing::info!(
        "Episode metadata restored for {} of series {}",
        episode_id,
        id
    );

    Ok(StatusCode::OK)
}
