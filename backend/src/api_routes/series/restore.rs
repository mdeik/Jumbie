use crate::api::AppState;
use crate::api_routes::series::helpers::get_series_mapping_or_404;
use crate::api_routes::series::metadata::fetch_metadata_for_series;

use crate::error::AppError;
use axum::{extract::State, http::StatusCode};
use std::sync::Arc;

/// POST /api/series/{id}/season/{season}/restore_metadata
///
/// Restores a season's episode metadata from the per-provider cache, re-inserting
/// it without calling the external API; falls back to a full metadata fetch on a
/// cache miss.
pub async fn restore_season_metadata(
    State(state): State<Arc<AppState>>,
    axum::extract::Path((id, season)): axum::extract::Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let season_number: i32 = crate::validation::validate_season_number(&season)
        .map_err(|e| AppError::BadRequest(e.0))?;

    let mapping = get_series_mapping_or_404(&state, &id).await?;

    // The provider is resolved before metadata_id so the external ID is looked
    // up by the active provider's UUID key.
    let abs_mode = state.effective_absolute_numbering(&mapping).await;

    // Clear suppression first (the deliberate "re-add") so the rehydrated
    // season is not immediately filtered out of the season list.
    let numbering_mode_i32 = abs_mode as i32;
    if let Err(e) = state
        .db
        .unsuppress_season(&id, season_number, numbering_mode_i32)
        .await
    {
        tracing::warn!(
            "Failed to clear season suppression for {} ({}) S{}: {}",
            mapping.target_title,
            id,
            season,
            e
        );
    }
    // SSoT: resolve the provider instance + external ID (priority-ordered,
    // mode-filtered) instead of taking an arbitrary `metadata_ids` entry.
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

    if !cached_episodes.is_empty() {
        tracing::info!(
            "Restoring {} cached episodes for season {} of series {} (provider={})",
            cached_episodes.len(),
            season,
            mapping.target_title,
            plugin_id
        );

        let episodes: Vec<(String, crate::plugins::metadata::EpisodeMetadata)> = cached_episodes
            .into_iter()
            .map(|ep| {
                let episode_id = if abs_mode {
                    jumbie_shared::formatting::fmt_absolute_episode_id(
                        ep.episode_number,
                        &mapping.series_id,
                    )
                } else {
                    jumbie_shared::formatting::fmt_episode_id_num(
                        ep.season_number,
                        ep.episode_number,
                        &mapping.series_id,
                    )
                };
                // Cache stores meta_date as a UTC string; parse via the SSoT
                // parser (accepts a full datetime or date-only → midnight UTC).
                let meta_date = ep
                    .meta_date
                    .as_deref()
                    .and_then(|d| crate::datetime::parse_utc(d).ok());
                (
                    episode_id,
                    crate::plugins::metadata::EpisodeMetadata {
                        unique_id: ep.unique_id,
                        season: ep.season_number,
                        episode: ep.episode_number,
                        title: ep.title,
                        description: ep.description,
                        runtime: ep.runtime,
                        image_url: ep.image_url,
                        meta_date,
                    },
                )
            })
            .collect();

        state
            .db
            .batch_insert_metadata_episodes(
                &instance_id,
                &mapping.series_id,
                episodes,
                abs_mode as i32,
            )
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;

        // Re-apply monitor mode — newly restored metadata (e.g. meta_date)
        // may affect whether episodes should be monitored under Future mode.
        // SSoT: monitored is always derived from the series' monitor mode,
        // never hardcoded at insert time.
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

        // Re-estimate release dates after restoring cached metadata.
        if let Err(e) =
            crate::release_estimator::run_release_date_estimation(&state.db, &id, abs_mode).await
        {
            tracing::warn!(
                "Failed to run release date estimator after season restore for {}: {}",
                mapping.target_title,
                e
            );
        }

        // Also restore the metadata season count
        if let Ok(season_counts) = state
            .db
            .get_metadata_season_cache(&metadata_id, &plugin_id, &instance_id, ordering_mode)
            .await
        {
            let counts: Vec<crate::plugins::metadata::SeasonMetadata> = season_counts
                .into_iter()
                .filter(|(s, _)| *s == season)
                .map(|(_, count)| crate::plugins::metadata::SeasonMetadata {
                    season: season_number,
                    episode_count: count,
                })
                .collect();
            if !counts.is_empty() {
                let _ = state
                    .db
                    .upsert_metadata_season_cache(
                        &metadata_id,
                        &plugin_id,
                        &instance_id,
                        ordering_mode,
                        &counts,
                    )
                    .await;
            }
        }

        return Ok(StatusCode::OK);
    }

    // Cache miss — fall back to full metadata fetch via the queue (P0 / user-initiated).
    tracing::info!(
        "No cached episodes for season {} of series {}, submitting to queue",
        season,
        mapping.target_title
    );
    let state_clone = state.clone();
    let series_id = id.clone();
    state
        .metadata_queue
        .submit_and_wait(series_id, move || {
            let state = state_clone.clone();
            let sid = id.clone();
            async move { fetch_metadata_for_series(&state, &sid, None).await }
        })
        .await?;
    Ok(StatusCode::OK)
}
