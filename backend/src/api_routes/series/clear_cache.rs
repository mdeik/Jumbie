use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
};

use crate::api_routes::series::helpers::get_series_mapping_or_404;
use crate::error::AppError;

/// DELETE /api/series/{id}/metadata-cache
///
/// Clears locally stored metadata snapshots for this series.
/// Query params:
///   - provider (optional): if provided, only that provider's cache is cleared;
///     if omitted, all providers' caches for this series are cleared.
///
/// Returns: 204 No Content
pub async fn clear_metadata_cache(
    State(state): State<Arc<crate::api::AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<StatusCode, AppError> {
    let mapping = get_series_mapping_or_404(&state, &id).await?;

    // instance id → plugin_id (backend TYPE id) map, so each provider's cache
    // rows — keyed on (metadata_id, plugin_id, instance_id) — are deleted exactly.
    let instance_plugins = crate::utils::metadata::instance_plugin_id_map(&state.db).await;

    let provider_filter = params.get("provider");

    let mut delete_failures: Vec<String> = Vec::new();

    for (instance_id, metadata_id) in &mapping.settings.metadata_ids {
        if let Some(filter) = provider_filter
            && instance_id != filter
        {
            continue;
        }

        let plugin_id = instance_plugins
            .get(instance_id)
            .cloned()
            .unwrap_or_else(|| instance_id.clone());

        if let Err(e) = state
            .db
            .delete_metadata_episodes_cache_for_provider(metadata_id, &plugin_id, instance_id)
            .await
        {
            delete_failures.push(format!(
                "metadata_episodes_cache for {} ({}): {}",
                metadata_id, plugin_id, e
            ));
        }

        if let Err(e) = state
            .db
            .delete_metadata_season_cache_for_provider(metadata_id, &plugin_id, instance_id)
            .await
        {
            delete_failures.push(format!(
                "metadata_season_cache for {} ({}): {}",
                metadata_id, plugin_id, e
            ));
        }

        if let Err(e) = state
            .db
            .delete_metadata_series_cache_for_provider(metadata_id, &plugin_id, instance_id)
            .await
        {
            delete_failures.push(format!(
                "metadata_series_cache for {} ({}): {}",
                metadata_id, plugin_id, e
            ));
        }

        if let Err(e) = state
            .db
            .delete_metadata_fetch_log_for_provider(metadata_id, &plugin_id, instance_id)
            .await
        {
            delete_failures.push(format!(
                "metadata_fetch_log for {} ({}): {}",
                metadata_id, plugin_id, e
            ));
        }
    }

    if !delete_failures.is_empty() {
        tracing::warn!(
            "Failed to delete metadata cache for series {} ({}): {}",
            mapping.target_title,
            id,
            delete_failures.join("; ")
        );
    }

    // Reset `metadata_last_synced_at` so the next sync repopulates the cache:
    // a full clear removes all entries; a per-provider clear only removes that
    // provider's, leaving other providers' timestamps intact.
    let mut updated_mapping = mapping.clone();
    if let Some(provider_filter) = provider_filter {
        updated_mapping
            .settings
            .metadata_last_synced_at
            .retain(|k, _| k != provider_filter);
    } else {
        updated_mapping.settings.metadata_last_synced_at.clear();
    }
    if updated_mapping.settings.metadata_last_synced_at != mapping.settings.metadata_last_synced_at
        && let Err(e) = state.db.upsert_series_mapping(&id, &updated_mapping).await
    {
        tracing::warn!(
            "Failed to reset metadata_last_synced_at for series {} ({}): {}",
            mapping.target_title,
            id,
            e
        );
    }

    if let Err(e) = state.db.cleanup_orphaned_metadata().await {
        tracing::warn!("Failed to clean up orphaned metadata: {}", e);
    }

    Ok(StatusCode::NO_CONTENT)
}
