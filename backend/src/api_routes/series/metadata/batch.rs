use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;

use crate::api::AppState;
use crate::error::AppError;

use super::fetch::fetch_metadata_for_series;

/// Payload for the batch metadata fetch endpoint.
#[derive(Debug, Deserialize)]
pub struct BatchFetchMetadataPayload {
    /// Series IDs to fetch metadata for.
    pub series_ids: Vec<String>,
}

/// `POST /api/series/batch-fetch-metadata`
///
/// Fetches metadata for multiple series at once, skipping any without a
/// configured metadata ID. Returns immediately after queuing — results are
/// logged, not returned. Requires an active metadata provider.
pub async fn batch_fetch_metadata(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BatchFetchMetadataPayload>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Batch metadata fetch requested for {} series",
        payload.series_ids.len()
    );

    let absolute_cap = jumbie_shared::plugin::Capability::MetadataProviderAbsolute;
    let normal_cap = jumbie_shared::plugin::Capability::MetadataProviderNormal;
    let pm = state.plugin_manager.read().await;
    let has_normal = pm.first_plugin_by_capability(normal_cap).is_some();
    let has_absolute = pm.first_plugin_by_capability(absolute_cap).is_some();
    drop(pm);

    if !has_normal && !has_absolute {
        return Err(AppError::BadRequest(
            "No active metadata provider found".to_string(),
        ));
    }

    for series_id in &payload.series_ids {
        let mapping = state.db.get_series_mapping(series_id).await.ok().flatten();
        match mapping {
            Some(m) => {
                let has_metadata_id = m.settings.metadata_ids.values().any(|v| !v.is_empty());
                if has_metadata_id {
                    tracing::debug!(
                        "Batch metadata fetch: queuing series {} ({})",
                        series_id,
                        m.target_title
                    );
                    // Submitted to the MetadataQueue (concurrency=1, P0 priority) so
                    // provider rate limits are respected and duplicate submissions
                    // for the same series are deduplicated.
                    let state_clone = state.clone();
                    let sid = series_id.clone();
                    state
                        .metadata_queue
                        .submit(
                            sid.clone(),
                            crate::metadata_queue::Priority::Bulk,
                            move || {
                                let state = state_clone.clone();
                                let sid = sid.clone();
                                async move {
                                    if let Err(e) =
                                        fetch_metadata_for_series(&state, &sid, None).await
                                    {
                                        tracing::warn!(
                                            "Batch metadata fetch failed for series {}: {:?}",
                                            sid,
                                            e
                                        );
                                    }
                                    state.rename_queue_trigger.notify_one();
                                }
                            },
                        )
                        .await;
                } else {
                    tracing::debug!(
                        "Batch metadata fetch: skipping series {} ({}) — no metadata ID configured",
                        series_id,
                        m.target_title
                    );
                }
            }
            None => {
                tracing::debug!(
                    "Batch metadata fetch: series {} not found, skipping",
                    series_id
                );
            }
        }
    }

    Ok(StatusCode::OK)
}
