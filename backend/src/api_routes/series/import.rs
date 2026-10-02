use crate::api::AppState;
use crate::error::AppError;
use axum::{Json, extract::State, http::StatusCode};
use jumbie_shared::types::{MappingRule, SeriesSettings};
use std::sync::Arc;

/// Check that `path` is not the Unix filesystem root `/`, which would scan the
/// entire filesystem.
///
/// Only Unix `/` is blocked: series may have custom `settings.path` values
/// outside any configured destination root, and on Windows each drive root
/// (e.g. `C:\`, `M:\`) is an independent volume — a valid media-library
/// location, not the whole system.
fn path_is_not_root(path: &std::path::Path) -> bool {
    let canonical = crate::validation::normalize_path(path);
    // On Unix `/` is its own parent; drive roots share this property but are
    // valid on Windows, so the check is Unix-only.
    #[cfg(unix)]
    {
        canonical.parent() != Some(&canonical)
    }
    #[cfg(not(unix))]
    {
        // No single filesystem root to guard against off Unix.
        let _ = canonical;
        true
    }
}

pub async fn preview_import(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::PreviewSeriesRequest>,
) -> Result<Json<Vec<jumbie_shared::types::PreviewSeriesItem>>, AppError> {
    tracing::debug!(
        "preview_import called: path={}, is_bulk={}",
        payload.path,
        payload.is_bulk
    );

    let p = std::path::PathBuf::from(&payload.path);

    // Shared validator for traversal (".."), double-slash, and length checks.
    // The permission check is skipped (preview is read-only), and `allowed_root`
    // is the path's own parent so it need not lie under a destination root.
    let allowed_root = p.parent().unwrap_or(&p).to_path_buf();
    let validated = crate::validation::validate_path(&payload.path, &allowed_root);
    if let Err(e) = validated {
        return Err(AppError::BadRequest(format!("Invalid path: {}", e)));
    }

    if !p.exists() || !p.is_dir() {
        return Err(AppError::BadRequest(format!(
            "Path {} does not exist or is not a directory",
            payload.path
        )));
    }

    if !path_is_not_root(&p) {
        return Err(AppError::BadRequest(format!(
            "Path '{}' is a filesystem root — scanning here would traverse \
             the entire filesystem",
            payload.path
        )));
    }

    // All existing paths are passed at once so the scanner does O(1) lookups
    // instead of O(N) DB queries per discovered subdirectory.
    let existing_paths: Vec<String> = state
        .db
        .get_all_series_mappings()
        .await
        .unwrap_or_default()
        .values()
        .filter_map(|m| m.settings.path.clone())
        .collect();

    match crate::scanner::quick_preview_directory(&p, payload.is_bulk, &existing_paths).await {
        Ok(items) => Ok(Json(items)),
        Err(e) => Err(AppError::Internal(anyhow::anyhow!(format!(
            "Failed to preview directory: {}",
            e
        ),))),
    }
}

pub async fn bulk_create_series(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::ConfirmSeriesImportRequest>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Received request to create {} series",
        payload.items.iter().filter(|i| i.selected).count()
    );

    // Mappings and org policy are fetched once for the whole batch: the collision
    // checker needs both, and per-item fetches would be N+1 queries / N lock acquires.
    let all_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();
    let org_config = state.org_config().await;

    struct ImportEntry {
        series_id: String,
        path: String,
    }

    let mut new_entries = Vec::new();

    for item in payload.items {
        if !item.selected {
            continue;
        }

        let series_name = item.final_title.clone();
        let path_str = item.path.clone();

        // Checked here rather than at preview time because the user may change
        // final_title between preview and confirm, and because mappings created
        // since the preview must also be considered.
        if let Err(e) = crate::validation::check_series_path_not_taken(
            std::path::Path::new(&path_str),
            None,
            &all_mappings,
            &org_config,
        ) {
            tracing::error!(
                "Path collision in bulk_create_series for '{}': {}",
                series_name,
                e
            );
            continue;
        }

        // Best-effort mkdir even if the directory likely exists: the user may
        // have edited final_title to a new path, and the filesystem should match
        // the DB record. A failure (e.g. permissions) skips just this item.
        let safe_path = std::path::Path::new(&path_str);
        if !path_is_not_root(safe_path) {
            tracing::error!("Path '{}' is a filesystem root — skipping", path_str);
            continue;
        }

        if !safe_path.exists()
            && let Err(e) = tokio::fs::create_dir_all(&safe_path).await
        {
            tracing::error!("Failed to create directory {}: {}", path_str, e);
            continue;
        }

        let uuid = jumbie_shared::config::generate_uuid();
        let series_key = series_name.to_lowercase().replace(' ', "_");

        // Auto-enable flatten_season_folders when all files are in the root
        // directory (no season subfolder structure).  This is determined during
        // the preview phase and stored in `all_files_in_root`.
        let flatten = item.all_files_in_root;

        let mapping = MappingRule {
            target_title: series_name.clone(),
            name: series_key,
            series_id: uuid.clone(),
            quality_profile: payload.quality_profile.clone(),
            release_profile: payload.release_profile.clone(),
            qb_category: Some(series_name.clone()),
            settings: SeriesSettings {
                path: Some(path_str.to_string()),
                monitor_mode: payload.monitor_mode,
                flatten_season_folders: Some(flatten),
                ..Default::default()
            },
            ..Default::default()
        };

        {
            // Re-fetch inside the loop to catch intra-batch path collisions: the
            // upfront snapshot would miss two items in this batch sharing a path,
            // since the first inserts its mapping only as the loop runs.
            let all = state.db.get_all_series_mappings().await.unwrap_or_default();
            let exists = all
                .values()
                .any(|m| m.settings.path.as_deref() == Some(path_str.as_str()));
            if !exists {
                if let Err(e) = state.db.upsert_series_mapping(&uuid, &mapping).await {
                    tracing::error!("Failed to save mapping for {}: {}", series_name, e);
                    continue;
                }
                new_entries.push(ImportEntry {
                    series_id: uuid.clone(),
                    path: path_str.clone(),
                });
            }
        }
    }

    if !new_entries.is_empty() && payload.scan_for_existing {
        // `import_scan_for_series` (not `scan_directory`) matches episodes by
        // directory path + SXXEXX patterns rather than filename-derived series
        // keys, and leaves duplicate (season, episode) tuples unassigned for the
        // user to resolve. The mapping is re-fetched so DB-applied defaults are
        // seen rather than the in-memory struct just upserted.
        for entry in new_entries {
            let safe_path = std::path::PathBuf::from(&entry.path);
            if let Some(mapping) = state
                .db
                .get_series_mapping(&entry.series_id)
                .await
                .ok()
                .flatten()
            {
                match crate::scanner::import_scan_for_series(&safe_path, &mapping, &state).await {
                    Ok((count, conflicts)) => {
                        tracing::debug!(
                            "Import scan for {}: inserted {} episodes, {} conflict(s)",
                            mapping.target_title,
                            count,
                            conflicts.len()
                        );
                        if !conflicts.is_empty() {
                            tracing::debug!(
                                "Import conflicts for '{}': {:?}",
                                mapping.target_title,
                                conflicts
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("Import scan failed for {}: {}", mapping.target_title, e);
                    }
                }
            }

            if payload.monitor_mode.is_some() {
                let _ =
                    crate::api_routes::series::apply_monitor_mode(&state, &entry.series_id).await;
            }
        }
    }

    tracing::debug!("bulk_create_series completed");
    Ok(StatusCode::OK)
}
