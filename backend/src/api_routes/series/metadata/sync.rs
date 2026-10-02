use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

use crate::api::AppState;

use crate::error::{AppError, IntoApiResponse};
use jumbie_shared::formatting::{LabelStyle, fmt_absolute_episode_id, fmt_episode_id_num};

/// `POST /api/series/:id/sync_metadata`
///
/// Upsert metadata episodes for a specific series into the DB.
pub async fn sync_metadata(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::SyncMetadataRequest>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Syncing {} metadata episodes for series {}",
        payload.episodes.len(),
        id
    );

    let mapping = match state.db.get_series_mapping(&id).await.ok().flatten() {
        Some(m) => m,
        None => return Err(AppError::NotFound(format!("Series {} not found", id))),
    };
    let series_id = mapping.series_id.clone();
    let absolute_numbering = state.effective_absolute_numbering(&mapping).await;

    // SSoT: resolve the provider instance (priority-ordered, mode-filtered).
    let required = Some(if absolute_numbering {
        jumbie_shared::plugin::Capability::MetadataProviderAbsolute
    } else {
        jumbie_shared::plugin::Capability::MetadataProviderNormal
    });
    let provider = crate::api_routes::series::metadata::fetch::resolve_provider_for_series(
        &state, &mapping, None, required,
    )
    .await?;
    // `metadata_ids` (episode column) and `metadata_source` are both keyed by the
    // provider INSTANCE id — not the backend type id.
    let instance_id = provider.instance_id;

    // Pre-validate the whole batch and reject on any invalid or duplicate
    // episode: catching duplicates here avoids partial inserts that would
    // violate the unique (season, episode) constraint and surface as a
    // confusing later error. The HashSet avoids O(N²) comparison.
    let mut seen_episodes = std::collections::HashSet::new();
    let mut seen_ids = std::collections::HashSet::new();
    for ep in &payload.episodes {
        if ep.season < 0 {
            return Err(AppError::BadRequest(format!(
                "Season must not be negative, got {}",
                ep.season
            )));
        }
        crate::validation::validate_episode_number(ep.episode)
            .map_err(|e| AppError::BadRequest(e.0))?;
        if !seen_episodes.insert((ep.season, ep.episode)) {
            return Err(AppError::BadRequest(format!(
                "Duplicate episode detected in metadata payload: {}",
                jumbie_shared::formatting::fmt_season_episode(
                    ep.season,
                    ep.episode,
                    None,
                    LabelStyle::Short
                )
            )));
        }
        if !ep.metadata_id.is_empty() && !seen_ids.insert(&ep.metadata_id) {
            return Err(AppError::BadRequest(format!(
                "Duplicate metadata ID detected in payload: {}",
                ep.metadata_id
            )));
        }
    }

    // One transaction for the whole batch: a season can hold hundreds of
    // episodes, and per-row inserts would mean N round-trips. Atomic — either
    // all metadata is saved or none is.
    let inserts: Vec<(String, crate::plugins::metadata::EpisodeMetadata)> = payload
        .episodes
        .into_iter()
        .map(|ep| {
            // Accept "YYYY-MM-DD" or an ISO/RFC 3339 datetime (SSoT parser).
            let meta_date = ep
                .meta_date
                .as_deref()
                .and_then(|d| crate::datetime::parse_utc(d).ok());
            (
                if absolute_numbering {
                    fmt_absolute_episode_id(ep.episode, &series_id)
                } else {
                    fmt_episode_id_num(ep.season, ep.episode, &series_id)
                },
                crate::plugins::metadata::EpisodeMetadata {
                    unique_id: ep.metadata_id,
                    season: ep.season,
                    episode: ep.episode,
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
            &series_id,
            inserts,
            absolute_numbering as i32,
        )
        .await
        .into_status_response()?;

    // Re-estimate release dates after metadata sync — new episode dates
    // from metadata may fill gaps that improve estimation accuracy.
    if let Err(e) = crate::release_estimator::run_release_date_estimation(
        &state.db,
        &series_id,
        absolute_numbering,
    )
    .await
    {
        tracing::warn!(
            "Failed to run release date estimator after metadata sync for {} ({}): {}",
            mapping.target_title,
            series_id,
            e
        );
    }

    // Refresh monitor status — newly populated `meta_date` / `upload_date`
    // may now give the episode an effective date, which affects `Future` mode.
    if let Ok(Some(_)) = state.db.get_series_mapping(&series_id).await {
        crate::source_processor::reapply_monitor_for_series(&state.db, &series_id, false, {
            let c = state.cfg.read().await;
            c.general.absolute_numbering
        })
        .await;
    } else {
        tracing::warn!("sync_metadata: series {} not found in mappings", series_id);
    }

    Ok(StatusCode::OK)
}
