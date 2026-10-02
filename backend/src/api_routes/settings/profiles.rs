use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use jumbie_shared::{
    scoring::ReleaseProfile,
    types::{AutomaticProfile, AutomaticProfileRecord, Quality, QualityProfile},
};

use crate::api::AppState;
use crate::error::{AppError, IntoApiResponse};

pub async fn get_quality_definitions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<std::collections::HashMap<String, Quality>>, AppError> {
    tracing::debug!("get_quality_definitions called");
    state
        .db
        .get_all_quality_definitions()
        .await
        .into_json_response()
}

pub async fn put_quality_definitions(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<std::collections::HashMap<String, Quality>>,
) -> Result<StatusCode, AppError> {
    tracing::debug!(
        "put_quality_definitions called: {} qualities",
        payload.len()
    );
    for (id, quality) in &payload {
        quality
            .validate()
            .map_err(|e| AppError::BadRequest(format!("Quality '{}': {}", id, e)))?;
    }
    state
        .db
        .save_all_quality_definitions(&payload)
        .await
        .into_status_response()
}

pub async fn get_quality_profiles(
    State(state): State<Arc<AppState>>,
) -> Result<Json<std::collections::HashMap<String, QualityProfile>>, AppError> {
    tracing::debug!("get_quality_profiles called");
    state
        .db
        .get_all_quality_profiles()
        .await
        .into_json_response()
}

pub async fn put_quality_profiles(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<std::collections::HashMap<String, QualityProfile>>,
) -> Result<StatusCode, AppError> {
    tracing::debug!("put_quality_profiles called: {} profiles", payload.len());
    for profile in payload.values() {
        profile.validate().map_err(AppError::BadRequest)?;
    }
    state
        .db
        .save_all_quality_profiles(&payload)
        .await
        .into_status_response()
}

pub async fn get_release_profiles(
    State(state): State<Arc<AppState>>,
) -> Result<Json<std::collections::HashMap<String, ReleaseProfile>>, AppError> {
    tracing::debug!("get_release_profiles called");
    state
        .db
        .get_all_release_profiles()
        .await
        .into_json_response()
}

pub async fn put_release_profiles(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<std::collections::HashMap<String, ReleaseProfile>>,
) -> Result<StatusCode, AppError> {
    tracing::debug!("put_release_profiles called: {} profiles", payload.len());
    for profile in payload.values() {
        profile.validate().map_err(AppError::BadRequest)?;
    }
    state
        .db
        .save_all_release_profiles(&payload)
        .await
        .into_status_response()?;

    // Retroactive rescore: after saving profile changes, recalculate scores for
    // all series referencing any saved profile. Uses the indexed
    // `release_profile` column on `series_mappings` for a direct SQL lookup.
    //
    // Deliberately does NOT diff old-vs-new profiles — rescoring is idempotent
    // (same inputs + same profile = same score), so over-rescoring is safe and
    // simpler than comparing profile contents.
    let profile_uuids: Vec<&str> = payload.keys().map(|s| s.as_str()).collect();

    if profile_uuids.is_empty() {
        return Ok(StatusCode::OK);
    }

    let pool = state.db.get_pool();
    let placeholders = crate::db::sql_in_placeholders(profile_uuids.len());
    let sql = format!(
        "SELECT id FROM series_mappings WHERE release_profile IN ({})",
        placeholders
    );
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for uuid in &profile_uuids {
        q = q.bind(uuid);
    }
    let series_ids: Vec<String> = q.fetch_all(pool).await.unwrap_or_default();

    if !series_ids.is_empty() {
        tracing::info!(
            "Rescoring {} series after release profile update",
            series_ids.len()
        );
        if let Err(e) = state.db.rescore_episodes_for_profiles(&series_ids).await {
            tracing::error!("Rescore after profile update failed: {}", e);
        }
    }

    Ok(StatusCode::OK)
}

pub async fn get_automatic_profiles(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AutomaticProfile>>, AppError> {
    tracing::debug!("get_automatic_profiles called");
    state.db.get_automatic_profiles().await.into_json_response()
}

pub async fn delete_automatic_profile(
    State(state): State<Arc<AppState>>,
    Path(submitter): Path<String>,
) -> Result<StatusCode, AppError> {
    tracing::debug!("delete_automatic_profile called: submitter={}", submitter);
    crate::validation::validate_not_empty(&submitter, "Submitter").map_err(|e| {
        tracing::debug!("delete_automatic_profile: invalid submitter: {}", e);
        AppError::BadRequest(e.0)
    })?;
    crate::validation::validate_max_length(&submitter, "Submitter", 255).map_err(|e| {
        tracing::debug!("delete_automatic_profile: submitter too long: {}", e);
        AppError::BadRequest(e.0)
    })?;
    state
        .db
        .delete_automatic_profile(&submitter)
        .await
        .into_status_response()
}

pub async fn get_automatic_profile_records(
    State(state): State<Arc<AppState>>,
    Path(submitter): Path<String>,
) -> Result<Json<Vec<AutomaticProfileRecord>>, AppError> {
    tracing::debug!(
        "get_automatic_profile_records called: submitter={}",
        submitter
    );
    state
        .db
        .get_automatic_profile_records(&submitter)
        .await
        .into_json_response()
}

pub async fn recalculate_automatic_scores(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, AppError> {
    tracing::info!("Recalculating automatic scores for all series");
    // Cancel any pending debounced recalculation from config saves
    if let Some(token) = state.recalc_cancel.lock().await.take() {
        token.cancel();
    }
    if let Some(organizer) = &state.organizer {
        organizer
            .reapply_automatic_profiles_to_library()
            .await
            .map_err(AppError::Internal)?;
        Ok(StatusCode::OK)
    } else {
        Err(AppError::Internal(anyhow::anyhow!(
            "Organizer not available"
        )))
    }
}
