use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::api::AppState;

/// Returns a map of series_id → count of active scan paths for that series.
///
/// Only series whose stored path is a prefix (component-wise) of one or more
/// active scan paths are included in the result. Series without a configured
/// path are skipped (no title-based matching).
pub async fn get_media_info_scan_counts(
    State(state): State<Arc<AppState>>,
) -> Json<HashMap<String, usize>> {
    tracing::debug!("get_media_info_scan_counts called");

    let active_paths = state.scan_queue.active_paths_snapshot().await;

    if active_paths.is_empty() {
        tracing::debug!("get_media_info_scan_counts: no active scan paths");
        return Json(HashMap::new());
    }

    let mappings = state.db.get_all_series_mappings().await.unwrap_or_default();

    let mut counts: HashMap<String, usize> = HashMap::new();

    for (series_id, mapping) in &mappings {
        let series_path = match &mapping.settings.path {
            Some(p) if !p.is_empty() => Path::new(p),
            _ => continue,
        };

        // Component-aware matching: `Path::starts_with` won't falsely match
        // "/tv/My Show" against "/tv/My Showcase" the way String::starts_with
        // would.
        let count = active_paths
            .iter()
            .filter(|active_path| Path::new(active_path).starts_with(series_path))
            .count();

        if count > 0 {
            counts.insert(series_id.clone(), count);
        }
    }

    tracing::debug!(
        "get_media_info_scan_counts completed: {} series with active scans",
        counts.len()
    );

    Json(counts)
}

/// Returns the count of active scan paths for a single series.
///
/// A lightweight alternative to `get_media_info_scan_counts` when the frontend
/// only needs to display progress for one series (e.g. the edit-series page).
pub async fn get_media_info_scan_count_for_series(
    State(state): State<Arc<AppState>>,
    AxumPath(series_id): AxumPath<String>,
) -> Json<HashMap<String, usize>> {
    tracing::debug!("get_media_info_scan_count_for_series({}) called", series_id);

    let active_paths = state.scan_queue.active_paths_snapshot().await;

    if active_paths.is_empty() {
        tracing::debug!("get_media_info_scan_count_for_series: no active scan paths");
        return Json(HashMap::new());
    }

    let mapping = state.db.get_series_mapping(&series_id).await;

    let mapping = match mapping {
        Ok(Some(m)) => m,
        Ok(None) => {
            tracing::debug!(
                "get_media_info_scan_count_for_series: series {} not found",
                series_id
            );
            return Json(HashMap::new());
        }
        Err(e) => {
            tracing::error!(
                "get_media_info_scan_count_for_series: DB error for series {}: {:?}",
                series_id,
                e
            );
            return Json(HashMap::new());
        }
    };

    let series_path = match &mapping.settings.path {
        Some(p) if !p.is_empty() => Path::new(p),
        _ => {
            tracing::debug!(
                "get_media_info_scan_count_for_series: series {} ({}) has no path configured",
                mapping.target_title,
                series_id
            );
            return Json(HashMap::new());
        }
    };

    // See get_media_info_scan_counts for why component-aware `Path::starts_with`
    // is used instead of String prefix matching.
    let count = active_paths
        .iter()
        .filter(|active_path| Path::new(active_path).starts_with(series_path))
        .count();

    let mut result = HashMap::new();
    if count > 0 {
        result.insert(series_id, count);
    }

    tracing::debug!(
        "get_media_info_scan_count_for_series completed: count={}",
        count
    );

    Json(result)
}
