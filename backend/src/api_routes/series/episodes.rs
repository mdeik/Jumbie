use crate::api::AppState;
use crate::error::AppError;
use crate::error::IntoApiResponse;
use crate::models::activity::ActivityEvent;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use jumbie_shared::types::{BatchMonitorEpisodesPayload, EpisodeStatus};
use std::sync::Arc;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct UpdateEstimatedReleasePayload {
    /// RFC 3339 timestamp with an explicit offset (e.g. "2026-06-15T00:00:00Z").
    /// `None` or `null` clears the estimated release date.
    pub date: Option<String>,
}

pub async fn update_est_date(
    State(state): State<Arc<AppState>>,
    Path(episode_id): Path<String>,
    Json(payload): Json<UpdateEstimatedReleasePayload>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::validation::validate_id(&episode_id, "Episode ID").map_err(|e| {
        tracing::debug!("update_est_date: invalid episode ID: {}", e);
        AppError::BadRequest(e.0)
    })?;
    tracing::debug!(
        "update_est_date called: episode_id={}, date={:?}",
        episode_id,
        payload.date
    );

    // Strict boundary: the timestamp must be RFC 3339 with an explicit offset.
    let date = payload
        .date
        .map(|d| -> Result<crate::datetime::UtcDateTime, AppError> {
            crate::datetime::parse_request_utc(&d).map_err(AppError::BadRequest)
        })
        .transpose()?;

    state
        .db
        .update_est_date(&episode_id, date)
        .await
        .into_status_response()?;

    // Trigger re-estimation after manual date change — the new date may
    // serve as an anchor for neighbouring episodes.
    let series_id = if let Ok(Some(ep)) = state.db.get_episode_by_id(&episode_id).await {
        if let Err(e) =
            crate::release_estimator::trigger_estimation_for_series(&state.db, &ep.series_id).await
        {
            tracing::warn!(
                "Failed to re-estimate release dates for series {} after manual update: {}",
                ep.series_id,
                e
            );
        }
        ep.series_id
    } else {
        return Ok(StatusCode::OK);
    };

    // Refresh monitor status — the new/changed estimated date may affect
    // `Future` mode decisions now that the effective date computation
    // considers est_date.
    crate::source_processor::reapply_monitor_for_series(&state.db, &series_id, false, {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    })
    .await;

    Ok(StatusCode::OK)
}

pub async fn scan_episode_media(
    State(state): State<Arc<AppState>>,
    Path(episode_id): Path<String>,
) -> Result<StatusCode, AppError> {
    crate::validation::validate_id(&episode_id, "Episode ID").map_err(|e| {
        tracing::debug!("scan_episode_media: invalid episode ID: {}", e);
        AppError::BadRequest(e.0)
    })?;
    tracing::info!(
        "Manual media info scan requested for episode {}",
        episode_id
    );

    // Defence in depth: the frontend also gates the button, but the API must
    // stand on its own.
    {
        let cfg = state.cfg.read().await;
        if !cfg.general.media_info_scan_enabled {
            return Err(AppError::BadRequest(
                "Media info scanning is disabled in settings.".to_string(),
            ));
        }
    }
    if crate::utils::media_info::ffprobe_version().is_none() {
        return Err(AppError::BadRequest(
            "Media info scanning requires FFmpeg/FFprobe to be installed on the server."
                .to_string(),
        ));
    }

    // The episode's primary file, resolved through the SSoT association join. A
    // missing path becomes a precise 404 before ffprobe runs; the fingerprint is
    // then marked failed so the UI can show a "file missing" indicator.
    let file_path = state
        .db
        .get_episode_file_path(&episode_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
        .filter(|p| !p.is_empty());

    let path_str = match file_path {
        Some(p) => p,
        None => {
            return Err(AppError::NotFound(
                "No file assigned to this episode".to_string(),
            ));
        }
    };

    let pathbuf = std::path::PathBuf::from(&path_str);
    if !pathbuf.exists() {
        let _ = state.db.mark_media_info_scan_failed(&path_str).await;
        return Err(AppError::NotFound(
            "File does not exist on disk".to_string(),
        ));
    }

    // Submit to the scan queue at high priority so user-initiated scans are
    // dispatched ahead of background batch scans; the queue also deduplicates and
    // bounds concurrency. Runs in the background so the API returns immediately.
    //
    // The closure uses `scan_file_fingerprint`, which computes the xxhash, reads
    // file metadata (inode, dev, size, mtime), extracts media info via ffprobe, and
    // saves the full row with an upsert — unlike a bare UPDATE, this creates the
    // fingerprint row when none exists.
    let scan_queue = state.scan_queue.clone();
    let db = state.db.clone();
    let pathbuf_for_closure = pathbuf.clone();
    let path_str_for_closure = path_str.clone();
    // Episode info for the activity feed, queried before the closure.
    let ep_info = sqlx::query_as::<_, (String, i32, i32)>(
        "SELECT COALESCE(sm.target_title, ''), e.season, e.episode FROM episodes e LEFT JOIN series_mappings sm ON e.series_id = sm.id WHERE e.episode_id = ?",
    )
    .bind(&episode_id)
    .fetch_optional(state.db.get_pool())
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
    let (series_title, season, episode) = ep_info.unwrap_or_default();
    scan_queue
        .submit_high_priority(pathbuf, move || {
            let db = db.clone();
            let pathbuf = pathbuf_for_closure;
            let path_str = path_str_for_closure;
            let series_title = series_title.clone();
            async move {
                let (_hash, _, media_info) = db
                    .scan_file_fingerprint(&pathbuf, EpisodeStatus::Organized.as_str())
                    .await;

                if media_info.is_some() {
                    tracing::debug!("Successfully scanned media info for {}", path_str);
                    let _ = sqlx::query(
                        "INSERT INTO file_event_log (event_type, source_path, status) VALUES ('analyze', ?, 'completed')",
                    )
                    .bind(&path_str)
                    .execute(db.get_pool())
                    .await;

                    // Codec + resolution; the title already shows the episode.
                    let media_summary = media_info.as_ref().map(|mi| {
                        let parts: Vec<&str> = [mi.codec.as_deref(), mi.resolution.as_deref()]
                            .into_iter()
                            .flatten()
                            .collect();
                        if parts.is_empty() {
                            path_str.clone()
                        } else {
                            parts.join(" / ")
                        }
                    }).unwrap_or_else(|| path_str.clone());

                    let _ = db.record_activity(ActivityEvent {
                        event_type: jumbie_shared::types::ActivityType::Analyze,
                        series_title,
                        season: Some(season.to_string()),
                        episode: Some(episode),
                        episode_end: None,
                        title: None,
                        details: Some(media_summary),
                        status: "Success".to_string(),
                    })
                    .await;
                } else {
                    tracing::error!("Failed to extract media info for {}", path_str);
                }
            }
        })
        .await;

    tracing::info!(
        "Manual media info scan queued for episode {} ({})",
        episode_id,
        path_str
    );
    Ok(StatusCode::OK)
}

pub async fn monitor_episodes(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::MonitorEpisodesPayload>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Updating monitor status for series {} with mode {:?}",
        id,
        payload.mode
    );

    // Persist the mapping BEFORE applying: the mapping row is the source of
    // truth for monitor mode. Crashing between apply and save would re-derive
    // episode states from the old mode on restart; saving first means episodes
    // are only ever brought into line with a persisted mode.
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
        mapping.settings.monitor_mode = Some(payload.mode);
        if let Err(e) = state.db.upsert_series_mapping(&id, &mapping).await {
            tracing::error!(
                "Failed to save monitor mode update to DB for {} ({}): {}",
                mapping.target_title,
                id,
                e
            );
        }
    }

    crate::api_routes::series::apply_monitor_mode(&state, &id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    tracing::debug!("monitor_episodes completed for series {}", id);
    Ok(StatusCode::OK)
}

/// Toggle a single episode's monitored flag.
///
/// SSoT: Direct DB write — sets `monitor_override=1` so periodic sweeps
/// respect the user's choice.  The override is cleared when the user
/// changes the series' monitor mode (fresh_apply=true), or automatically
/// (self-heal) when the mode's evaluation catches up to the user's value.
pub async fn toggle_episode_monitor(
    State(state): State<Arc<AppState>>,
    Path(episode_id): Path<String>,
    Json(payload): Json<jumbie_shared::types::EpisodeMonitorTogglePayload>,
) -> Result<StatusCode, AppError> {
    crate::validation::validate_id(&episode_id, "Episode ID").map_err(|e| {
        tracing::debug!("toggle_episode_monitor: invalid episode ID: {}", e);
        AppError::BadRequest(e.0)
    })?;
    tracing::debug!(
        "toggle_episode_monitor: episode_id={}, monitored={}",
        episode_id,
        payload.monitored
    );

    state
        .db
        .set_episode_monitor_override(&[episode_id], payload.monitored)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(StatusCode::OK)
}

/// Batch-set the monitored flag for the given episode IDs.
///
/// Direct DB write — sets `monitor_override=1` for each episode so sweeps
/// respect the user's choice.
pub async fn batch_monitor_episodes(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BatchMonitorEpisodesPayload>,
) -> Result<StatusCode, AppError> {
    if payload.ids.is_empty() {
        return Ok(StatusCode::OK);
    }

    state
        .db
        .set_episode_monitor_override(&payload.ids, payload.monitored)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(StatusCode::OK)
}
