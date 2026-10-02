use crate::api::AppState;
use crate::api::modifying_series::{is_series_locked, try_lock_series, unlock_series};
use crate::error::AppError;
use crate::models::activity::{ActivityEvent, ActivityType};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use jumbie_shared::types::{MappingRule, SeasonOverride, SeriesSettings};
use std::sync::Arc;

use super::helpers::get_series_mapping_or_404;

/// SSoT: total budget for the add-series metadata sync loop (`create_series`).
///
/// Bounds the whole loop so the server always answers before the frontend's 120s
/// client timeout (N providers, or a busy concurrency-1 metadata queue, could
/// otherwise exceed it). Each fetch is additionally bounded by
/// [`crate::metadata_queue::DEFAULT_JOB_TIMEOUT`].
const SYNC_DEADLINE: std::time::Duration = std::time::Duration::from_secs(100);

pub async fn create_series(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::CreateSeriesRequest>,
) -> Result<(StatusCode, Json<String>), AppError> {
    tracing::info!("Received request to register series: '{}'", payload.path);

    // The read lock is released before any async I/O (holding it across awaits
    // contends with the write lock); only what we need is cloned, via `org_config()`.
    let config = state.cfg.read().await;
    // SSoT: config.organization.primary_root
    let primary_root = config.organization.primary_root().to_path_buf();
    let org_config = config.organization.clone();
    drop(config);

    if let Some(ref name) = payload.series_name {
        crate::validation::validate_title(name).map_err(|e| {
            tracing::error!("Invalid series_name: {}", e);
            AppError::BadRequest(e.to_string())
        })?;
    }

    if let Some(ref profile) = payload.quality_profile {
        crate::validation::validate_quality_profile(profile).map_err(|e| {
            tracing::error!("Invalid quality_profile: {}", e);
            AppError::BadRequest(e.to_string())
        })?;
    }

    // Release profiles share the structural schema (name + flags) with quality
    // profiles, so the same validator applies.
    if let Some(ref profile) = payload.release_profile {
        crate::validation::validate_quality_profile(profile).map_err(|e| {
            tracing::error!("Invalid release_profile: {}", e);
            AppError::BadRequest(e.to_string())
        })?;
    }

    // Series-level settings (aliases, absolute numbering, search flags, format
    // overrides, rename/flatten) are accepted at creation so the add call can set
    // the same options as the edit page. Convert to the storage struct once (SSoT:
    // `From<CreateSeriesSettings> for SeriesSettings`) and validate up front
    // (aliases, regex patterns) so an invalid value fails before any side effects.
    let series_settings: SeriesSettings = payload.settings.into();
    series_settings.validate().map_err(AppError::BadRequest)?;

    // SSoT for path resolution: validate_path joins relative paths with the first
    // destination root, canonicalizes where possible, and checks permissions. The
    // returned `safe_path` is authoritative for all subsequent operations.
    // SSoT: config.organization.primary_root
    let safe_path =
        crate::validation::validate_path(&payload.path, &primary_root).map_err(|e| {
            tracing::error!("Invalid path in create_series: {}", e);
            AppError::BadRequest(format!("Invalid path: {}", e))
        })?;

    // SSoT (shared with update_series and the validate-path preview): sanitize the
    // folder name per the illegal-char policy, reclaim hidden-series paths, reject
    // visible-series claims, then resolve folder collisions per the config. Explicit
    // custom paths send resolve_collisions = false and are honored verbatim.
    //
    // `safe_path` (resolved, possibly canonicalized) is used rather than the raw
    // `payload.path`, so relative paths and canonicalization compare consistently.
    let effective_path = super::helpers::resolve_series_folder_path(
        &state,
        &safe_path,
        &org_config,
        None,
        payload.resolve_collisions,
    )
    .await
    .map_err(|e| {
        tracing::error!("Path collision in create_series: {}", e);
        match e {
            // A visible series owns the path — include its id so the frontend
            // can link the user to the existing series (timeout-then-re-add).
            super::helpers::FolderResolveError::ClaimedBySeries(claim) => {
                AppError::BadRequestWithSeries {
                    message: claim.message,
                    series_id: claim.series_id,
                }
            }
            super::helpers::FolderResolveError::Other(msg) => AppError::BadRequest(msg),
        }
    })?;

    // Directory is created before the DB write: if the DB write fails we merely have
    // a harmless orphan directory, whereas DB-first would risk a mapping pointing at
    // a non-existent path. `effective_path` may differ from `safe_path` when collision
    // handling renamed the folder.
    if !effective_path.exists()
        && let Err(e) = tokio::fs::create_dir_all(&effective_path).await
    {
        tracing::error!(
            "Failed to create directory {}: {}",
            effective_path.display(),
            e
        );
        return Err(AppError::Internal(anyhow::anyhow!(format!(
            "Failed to create directory: {}",
            e
        ),)));
    }

    // Prefer the user-supplied `series_name` (a dedicated title input); fall back to
    // the path filename only when the caller omits it (e.g. automated imports).
    let series_name = if let Some(ref name) = payload.series_name {
        name.clone()
    } else {
        effective_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(String::from)
            .unwrap_or_else(|| payload.path.clone())
    };

    let uuid = jumbie_shared::config::generate_uuid();

    // Non-empty metadata IDs are collected before `payload.metadata_ids` is moved
    // into the mapping; the same list drives the auto-sync loop.
    // `search_missing_on_add` only has meaning when at least one ID is present
    // (the UI also disables the checkbox; this guards direct API callers).
    let sync_metadata_ids: Vec<(String, String)> = payload
        .metadata_ids
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    // Only non-empty IDs are stored — an empty-value key is useless in settings.
    let metadata_ids_map: std::collections::HashMap<String, String> =
        sync_metadata_ids.iter().cloned().collect();
    let search_on_add = payload.search_missing_on_add && !sync_metadata_ids.is_empty();

    // First available quality/release profiles, rather than hardcoded names: names
    // are user-defined, so a hardcoded "Default" could break if renamed.
    let default_quality = state.db.get_default_quality_profile().await;
    let default_release = state.db.get_default_release_profile().await;

    {
        let series_key = series_name.to_lowercase().replace(' ', "_");

        // Complete the create-time settings with the add-specific inputs. Aliases
        // are deduplicated on save (same as `update_series`) so manual entry can't
        // persist duplicates, and blank lines are dropped.
        let mut settings = series_settings;
        {
            let mut seen = std::collections::HashSet::new();
            settings
                .aliases
                .retain(|a| !a.is_empty() && seen.insert(a.clone()));
        }
        settings.path = Some(effective_path.to_string_lossy().to_string());
        settings.monitor_mode = payload.monitor_mode;
        settings.metadata_ids = metadata_ids_map;

        let mapping = MappingRule {
            target_title: series_name.clone(),
            name: series_key.clone(),
            series_id: uuid.clone(),
            quality_profile: Some(payload.quality_profile.unwrap_or(default_quality)),
            release_profile: Some(payload.release_profile.unwrap_or(default_release)),
            qb_category: Some(series_name.clone()),
            // Stores the resolved `effective_path` (from validate_path / collision
            // resolution), not the raw `payload.path`, so relative paths and
            // canonicalization compare consistently and match the on-disk folder.
            settings,
            ..Default::default()
        };

        state
            .db
            .upsert_series_mapping(&uuid, &mapping)
            .await
            .map_err(|e| {
                tracing::error!("Failed to save mapping to DB: {}", e);
                AppError::Internal(anyhow::anyhow!(format!("Failed to save mapping: {}", e)))
            })?;
    }

    // Scanning defaults on: files already on disk must be discovered immediately so
    // the library shows correct episode counts. `scan_series_directory` (not
    // `scan_directory`) is used because the series is already known here — it matches
    // by SXXEXX patterns + season folders rather than filename-derived series keys.
    if payload.scan_for_existing.unwrap_or(true) {
        let mapping = match state.db.get_series_mapping(&uuid).await {
            Ok(Some(m)) => m,
            _ => {
                // The mapping must exist immediately after creation.
                tracing::error!(
                    "create_series: mapping for {} ({}) not found immediately after creation",
                    series_name,
                    uuid
                );
                return Err(AppError::Internal(anyhow::anyhow!(
                    "Mapping disappeared after creation"
                )));
            }
        };
        match crate::scanner::scan_series_directory(&effective_path, &mapping, &state).await {
            Ok(count) => {
                tracing::info!("Found {} existing episodes during registration", count);

                // Monitor mode is applied AFTER the scan (not before) so the scan
                // doesn't trigger download-search logic for not-yet-registered
                // episodes. `MonitorMode::All` is the default and needs no filtering.
                let mode = payload
                    .monitor_mode
                    .unwrap_or(jumbie_shared::types::DEFAULT_MONITOR_MODE);
                if mode != jumbie_shared::types::MonitorMode::All
                    && let Err(e) =
                        crate::api_routes::series::apply_monitor_mode(&state, &uuid).await
                {
                    tracing::error!("Failed to apply monitor mode after creation: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("Scan series directory failed (non-fatal): {}", e);
            }
        }
    }

    // Auto-sync metadata for each provider that has an ID. SSoT: the fetch and
    // rename-queue wake-up live in `sync_series_metadata` (shared with the edit-page sync).
    // Non-fatal: the series is already registered, so a provider failure must not roll
    // it back — failures are logged and recorded as activity events.
    let sync_deadline = tokio::time::Instant::now() + SYNC_DEADLINE;
    let mut any_sync_succeeded = false;
    for (provider, meta_id) in &sync_metadata_ids {
        let outcome = tokio::time::timeout_at(
            sync_deadline,
            crate::api_routes::series::metadata::sync_series_metadata(
                &state,
                &uuid,
                Some(meta_id.clone()),
            ),
        )
        .await;
        match outcome {
            Ok(Ok(_)) => {
                any_sync_succeeded = true;
            }
            Ok(Err(e)) => {
                record_sync_failure(&state, provider, &series_name, e.message()).await;
            }
            Err(_elapsed) => {
                record_sync_failure(
                    &state,
                    provider,
                    &series_name,
                    format!("sync deadline ({SYNC_DEADLINE:?}) exceeded"),
                )
                .await;
                // Deadline reached — stop attempting remaining providers; the
                // series is already registered with what succeeded so far.
                break;
            }
        }
    }

    // Search for monitored+missing episodes in the SAME request, after all syncs
    // above: the episode snapshot is taken after metadata exists and monitor status
    // was reapplied, so it can never observe pre-fetch state. Only monitored,
    // missing, and RELEASED episodes qualify (`get_monitored_missing_for_series`).
    if search_on_add && any_sync_succeeded {
        if let Some(organizer) = &state.organizer {
            match state
                .search_queue
                .submit_monitored_missing(&uuid, state.db.clone(), organizer)
                .await
            {
                Ok((seasons, episodes)) => {
                    tracing::info!(
                        "create_series: search-on-add for {} ({}) submitted {} seasons / {} episodes",
                        series_name,
                        uuid,
                        seasons,
                        episodes
                    );
                    if episodes == 0 {
                        tracing::debug!(
                            "create_series: search-on-add for {} ({}) found 0 monitored+missing+released episodes (monitor_mode={:?})",
                            series_name,
                            uuid,
                            payload.monitor_mode
                        );
                        // Surfaced through the activity log so the user can see why
                        // nothing was searched. This is INFORMATIONAL (0 episodes is
                        // not a failure), so it must not carry an error status.
                        let _ = state
                            .db
                            .record_activity(ActivityEvent {
                                event_type: ActivityType::Metadata,
                                series_title: series_name.clone(),
                                season: None,
                                episode: None,
                                episode_end: None,
                                title: None,
                                details: Some(format!(
                                    "Search-on-add found 0 monitored+missing+released episodes (monitor_mode={:?})",
                                    payload.monitor_mode
                                )),
                                status: "Info".to_string(),
                            })
                            .await;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "create_series: search-on-add for {} ({}) failed: {}",
                        series_name,
                        uuid,
                        e.message()
                    );
                }
            }
        } else {
            tracing::warn!("create_series: organizer not available, skipping search-on-add");
        }
    }

    Ok((StatusCode::CREATED, Json(uuid)))
}

/// Record the durable side of a season deletion. Normal numbering tombstones the
/// season so a metadata sync does not recreate it. Absolute numbering has one
/// canonical season, so deleting it is a reset: clear any stale tombstone instead.
async fn apply_season_deletion(
    state: &Arc<AppState>,
    series_id: &str,
    season_num: i32,
    numbering_mode: i32,
) {
    let result = if numbering_mode == 1 {
        state
            .db
            .unsuppress_season(series_id, season_num, numbering_mode)
            .await
    } else {
        state
            .db
            .suppress_season(series_id, season_num, numbering_mode)
            .await
    };
    if let Err(e) = result {
        tracing::error!(
            "Failed to update suppression for series {} season {}: {}",
            series_id,
            season_num,
            e
        );
    }
}

pub async fn delete_season(
    State(state): State<Arc<AppState>>,
    Path((id, season)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Received request to delete season {} for series {}",
        season,
        id
    );

    // Validate season number before DB lookup
    let season_num = crate::validation::validate_season_number(&season).map_err(|e| {
        tracing::error!("Invalid season number: {}", e);
        AppError::BadRequest(e.0)
    })?;

    let mapping = get_series_mapping_or_404(&state, &id).await?;
    let series_title = &mapping.target_title;
    let numbering_mode = state.effective_absolute_numbering(&mapping).await as i32;

    if !try_lock_series(&state, &id).await {
        return Err(AppError::Conflict(
            "Series is currently being modified by another operation (e.g. batch move)".to_string(),
        ));
    }

    if let Err(e) = state
        .db
        .delete_season_data(&id, season_num, numbering_mode)
        .await
    {
        unlock_series(&state, &id).await;
        return Err(AppError::Internal(anyhow::anyhow!(e.to_string())));
    }

    // Durable suppression: the season stays deleted across metadata resync.
    // Clearing it is an explicit restore/match (see `restore_season_metadata`).
    apply_season_deletion(&state, &id, season_num, numbering_mode).await;

    tracing::info!(
        "Deleted season {} DB data for series {}",
        season,
        series_title
    );

    if let Err(e) = crate::release_estimator::trigger_estimation_for_series(&state.db, &id).await {
        tracing::warn!(
            "Failed to run release date estimator for series {} ({}): {}",
            series_title,
            id,
            e
        );
    }

    unlock_series(&state, &id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Delete episode data (metadata, media scans) for a single season.
///
/// Like the series-level `delete_episode_data` but scoped to one season.
/// Resets `metadata_last_synced_at` so the background refresh loop re-fetches
/// metadata on its next tick for this season.
pub async fn delete_season_episode_data(
    State(state): State<Arc<AppState>>,
    Path((id, season)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Deleting episode data for season {} of series {}",
        season,
        id
    );

    let season_num = crate::validation::validate_season_number(&season).map_err(|e| {
        tracing::error!("Invalid season number: {}", e);
        AppError::BadRequest(e.0)
    })?;

    let mapping = get_series_mapping_or_404(&state, &id).await?;
    let numbering_mode = state.effective_absolute_numbering(&mapping).await as i32;

    if !try_lock_series(&state, &id).await {
        return Err(AppError::Conflict(
            "Series is currently being modified by another operation (e.g. batch move)".to_string(),
        ));
    }

    if let Err(e) = state
        .db
        .delete_season_data(&id, season_num, numbering_mode)
        .await
    {
        unlock_series(&state, &id).await;
        return Err(AppError::Internal(anyhow::anyhow!(
            "Failed to delete season episode data: {}",
            e
        )));
    }

    // Durable suppression: the season stays deleted across metadata resync.
    apply_season_deletion(&state, &id, season_num, numbering_mode).await;

    // Reset all `metadata_last_synced_at` entries so the background refresh
    // loop re-fetches metadata on its next tick instead of thinking everything
    // is up-to-date.
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
        mapping.settings.metadata_last_synced_at.clear();
        if let Err(e) = state.db.upsert_series_mapping(&id, &mapping).await {
            tracing::error!(
                "Failed to reset metadata_last_synced_at after deleting season episode data: {}",
                e
            );
        }
    }

    if let Err(e) = crate::release_estimator::trigger_estimation_for_series(&state.db, &id).await {
        tracing::warn!(
            "Failed to run release date estimator for series {} ({}): {}",
            mapping.target_title,
            id,
            e
        );
    }

    unlock_series(&state, &id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Reset all overrides for a single season to defaults.
///
/// Normal numbering removes the SeasonOverride entry. Absolute numbering has one
/// canonical season, so its override is reset in place and the season is kept.
/// Episode data for the season is kept in both cases.
pub async fn reset_season_configuration(
    State(state): State<Arc<AppState>>,
    Path((id, season)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Resetting configuration for season {} of series {}",
        season,
        id
    );

    crate::validation::validate_season_number(&season).map_err(|e| {
        tracing::error!("Invalid season number: {}", e);
        AppError::BadRequest(e.0)
    })?;

    let mut mapping = get_series_mapping_or_404(&state, &id).await?;

    // Reset the season override for the active numbering mode.
    {
        let config = state.cfg.read().await;
        let global_absolute = config.general.absolute_numbering;
        let absolute = mapping.settings.active_mode(global_absolute).is_absolute();
        let overrides = mapping.settings.season_for_active_mode_mut(global_absolute);
        if absolute {
            if let Some(o) = overrides.get_mut(&season) {
                let key = o.season.clone();
                *o = jumbie_shared::types::SeasonOverride {
                    season: key,
                    ..Default::default()
                };
            }
        } else {
            overrides.remove(&season);
        }
    }

    state
        .db
        .upsert_series_mapping(&id, &mapping)
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!(
                "Failed to reset season configuration: {}",
                e
            ))
        })?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_series(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::RemoveSeriesPayload>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Received request to remove series: {} (config={}, data={}, files={})",
        id,
        payload.delete_configurations,
        payload.delete_episode_data,
        payload.delete_episodes
    );
    tracing::debug!(
        "remove_series called: series_id={}, delete_config={}, delete_data={}, delete_files={}",
        id,
        payload.delete_configurations,
        payload.delete_episode_data,
        payload.delete_episodes
    );

    let delete_data = payload.delete_episodes || payload.delete_episode_data;

    // CASE 1: neither flag — just hide the series, preserving all data.
    if !payload.delete_configurations && !delete_data {
        if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
            mapping.hidden_in_library = true;
            let _ = state.db.upsert_series_mapping(&id, &mapping).await;
            tracing::info!(
                "Series {} ({}) hidden (no data deleted)",
                mapping.target_title,
                id
            );
            return Ok(StatusCode::NO_CONTENT);
        }
        tracing::warn!("Series {} not found for hide", id);
        return Err(AppError::NotFound(format!("Series {} not found", id)));
    }

    // CASE 2: config only (no data/files) — reset settings + hide.
    if payload.delete_configurations && !delete_data {
        let mut mapping = get_series_mapping_or_404(&state, &id).await?;

        // Resets all configuration fields to defaults, preserving target_title,
        // name, series_id, and all episode data.
        mapping.release_profile = None;
        mapping.qb_category = None;
        mapping.filters = None;
        mapping.scoring = None;
        mapping.quality_profile = None;
        mapping.settings = jumbie_shared::types::SeriesSettings::default();
        mapping.hidden_in_library = true;

        state
            .db
            .upsert_series_mapping(&id, &mapping)
            .await
            .map_err(|e| {
                AppError::Internal(anyhow::anyhow!("Failed to update series mapping: {}", e))
            })?;

        // Clean up orphaned metadata IDs that were in the cleared settings.
        if let Err(e) = state.db.cleanup_orphaned_metadata().await {
            tracing::warn!("Failed to clean up orphaned metadata: {}", e);
        }

        tracing::info!(
            "Series {} ({}) config reset and hidden (data preserved)",
            mapping.target_title,
            id
        );
        return Ok(StatusCode::NO_CONTENT);
    }

    // CASE 3 & 4: delete data/files, optionally delete config.
    let mapping = get_series_mapping_or_404(&state, &id).await?;
    let series_title = mapping.target_title.clone();
    let series_path = mapping.settings.path.clone();

    // Full nuke (delete data AND config). SSoT: `stop_series_tracking` handles
    // cancel_scans, delete_data, delete_mapping, and cleanup_orphaned_metadata.

    if !try_lock_series(&state, &id).await {
        return Err(AppError::Conflict(
            "Series is currently being modified by another operation (e.g. batch move)".to_string(),
        ));
    }

    if delete_data && payload.delete_configurations {
        if !crate::api_routes::system::organized_series::stop_series_tracking(&state, &id).await {
            unlock_series(&state, &id).await;
            return Err(AppError::Internal(anyhow::anyhow!(
                "Failed to delete series {}",
                id
            )));
        }
    } else {
        // Cancel scan queue entries before any deletion.
        crate::api_routes::system::organized_series::cancel_series_scans(&state, &id).await;

        if delete_data {
            state.db.delete_series_data(&id).await.map_err(|e| {
                AppError::Internal(anyhow::anyhow!("Failed to delete series data: {}", e))
            })?;
        }

        if payload.delete_configurations {
            state.db.delete_series_mapping(&id).await.map_err(|e| {
                AppError::Internal(anyhow::anyhow!("Failed to delete series mapping: {}", e))
            })?;

            if let Err(e) = state.db.cleanup_orphaned_metadata().await {
                tracing::warn!("Failed to clean up orphaned metadata: {}", e);
            }
        }
    }

    if payload.delete_episodes {
        // SSoT for the series path: resolve ${series} with the illegal-char policy so
        // the resolved path matches the folder that was actually created.
        let org_config = state.org_config().await;
        let resolved_path =
            series_path.map(|p| crate::paths::resolve_template(&p, &series_title, &org_config));

        if let Some(ref dir) = resolved_path {
            if dir.exists() {
                tracing::debug!("Deleting episode files from disk: {}", dir.display());
                if let Err(e) = tokio::fs::remove_dir_all(dir).await {
                    tracing::error!("Failed to delete series directory {}: {}", dir.display(), e);
                }
            } else {
                tracing::warn!(
                    "Episode directory not found, skipping disk delete: {}",
                    dir.display()
                );
            }
        }
    }

    // When deleting data but NOT config, the series is hidden: the identity (title,
    // id, mapping) is preserved but it leaves the library until re-scanned.
    if !payload.delete_configurations
        && delete_data
        && let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await
    {
        mapping.hidden_in_library = true;
        let _ = state.db.upsert_series_mapping(&id, &mapping).await;
    }

    // Prune in-memory per-series state once data/config is gone: stale failed-rename
    // hashes would keep the sidebar indicator lit and stale rename-plan cache entries
    // would leak. (The full-nuke branch already prunes via stop_series_tracking.)
    if delete_data || payload.delete_configurations {
        crate::api::prune_series_in_memory_state(&state, &id).await;
    }

    unlock_series(&state, &id).await;
    tracing::info!("Successfully removed series {} ({})", series_title, id);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn batch_edit_series(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::BatchEditSeriesPayload>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Received batch edit request for {} series",
        payload.series_ids.len()
    );

    // Validate payload: at least one series ID must be provided
    if payload.series_ids.is_empty() {
        return Err(AppError::BadRequest(
            "At least one series ID must be provided".to_string(),
        ));
    }

    // Track which series had release profile changes for retroactive rescore
    let mut rescore_ids: Vec<String> = Vec::new();

    // Batch edit runs in two phases: phase 1 persists the new monitor_mode to the
    // mapping (so it survives restarts), then phase 2 applies it per-episode. Merged,
    // a crash mid-phase-2 would leave episodes in the wrong state while the mapping
    // (the source of truth) says otherwise.
    for id in &payload.series_ids {
        if let Ok(Some(mut mapping)) = state.db.get_series_mapping(id).await {
            let mut changed = false;
            if let Some(ref qp) = payload.quality_profile {
                crate::validation::validate_quality_profile(qp).map_err(|e| {
                    tracing::error!("Invalid quality_profile in batch edit: {}", e);
                    AppError::BadRequest(e.0)
                })?;
                if qp.is_empty() {
                    mapping.quality_profile = None;
                } else {
                    mapping.quality_profile = Some(qp.clone());
                }
                changed = true;
            }
            let profile_changed = if let Some(ref rp) = payload.release_profile {
                crate::validation::validate_quality_profile(rp).map_err(|e| {
                    tracing::error!("Invalid release_profile in batch edit: {}", e);
                    AppError::BadRequest(e.0)
                })?;
                let old = mapping.release_profile.clone();
                if rp.is_empty() {
                    mapping.release_profile = None;
                } else {
                    mapping.release_profile = Some(rp.clone());
                }
                changed = true;
                old.as_deref() != Some(rp) || (old.is_some() != rp.is_empty())
            } else {
                false
            };
            if let Some(mode) = payload.monitor_mode {
                mapping.settings.monitor_mode = Some(mode);
                changed = true;
            }
            if profile_changed {
                rescore_ids.push(id.clone());
            }
            if changed && let Err(e) = state.db.upsert_series_mapping(id, &mapping).await {
                // Non-fatal: one series failing must not block the rest of the batch;
                // the caller still gets a 200, so log and continue.
                tracing::error!(
                    "Failed to upsert mapping in batch edit for {} ({}): {}",
                    mapping.target_title,
                    id,
                    e
                );
            }
        }
    }

    // Applying monitor mode via the batch call collapses the serial per-series
    // round-trips (~4N) into ~4 total regardless of batch size.
    if let Some(mode) = payload.monitor_mode {
        crate::api_routes::series::batch_apply_monitor_mode(
            &state,
            &payload.series_ids,
            mode,
            true, // fresh_apply: user-initiated mode change
        )
        .await;
    }

    // Retroactive rescore for series whose release profile changed.
    if !rescore_ids.is_empty() {
        let db = state.db.clone();
        tokio::spawn(async move {
            if let Err(e) = db.rescore_episodes_for_profiles(&rescore_ids).await {
                tracing::error!("Rescore after batch edit failed: {}", e);
            }
        });
    }

    Ok(StatusCode::OK)
}

pub async fn batch_remove_series(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::BatchRemoveSeriesPayload>,
) -> Result<StatusCode, AppError> {
    tracing::info!(
        "Received request to batch remove {} series (config={}, data={}, files={})",
        payload.series_ids.len(),
        payload.delete_configurations,
        payload.delete_episode_data,
        payload.delete_episodes
    );

    let delete_data = payload.delete_episodes || payload.delete_episode_data;

    // Validate payload: at least one series ID must be provided
    if payload.series_ids.is_empty() {
        return Err(AppError::BadRequest(
            "At least one series ID must be provided".to_string(),
        ));
    }

    // `paths::mapping_path` (delete_episodes below) needs the org policy; fetching it
    // per series would be N lock acquisitions.
    let org_config = if payload.delete_episodes {
        Some(state.org_config().await)
    } else {
        None
    };

    for id in &payload.series_ids {
        if is_series_locked(&state, id) {
            tracing::warn!(
                "Batch remove: series {} is currently being modified by another operation, skipping",
                id
            );
            continue;
        }

        // CASE 1: neither flag — just hide the series.
        if !payload.delete_configurations && !delete_data {
            if let Ok(Some(mut mapping)) = state.db.get_series_mapping(id).await {
                mapping.hidden_in_library = true;
                let _ = state.db.upsert_series_mapping(id, &mapping).await;
            }
            continue;
        }

        // CASE 2: config only — reset settings + hide.
        if payload.delete_configurations && !delete_data {
            if let Ok(Some(mut mapping)) = state.db.get_series_mapping(id).await {
                mapping.release_profile = None;
                mapping.qb_category = None;
                mapping.filters = None;
                mapping.scoring = None;
                mapping.quality_profile = None;
                mapping.settings = jumbie_shared::types::SeriesSettings::default();
                mapping.hidden_in_library = true;
                let _ = state.db.upsert_series_mapping(id, &mapping).await;
            }
            continue;
        }

        // CASE 3 & 4: delete data/files, optionally delete config.
        // Cancel scan queue entries before any deletion.
        crate::api_routes::system::organized_series::cancel_series_scans(&state, id).await;

        if let Ok(Some(mapping)) = state.db.get_series_mapping(id).await {
            let series_title = mapping.target_title.clone();

            // Deleted when either `delete_episode_data` (data-only) or
            // `delete_episodes` (data + files) is set.
            if delete_data && let Err(e) = state.db.delete_series_data(id).await {
                tracing::error!("Failed to delete series data for {}: {}", series_title, e);
            }

            if payload.delete_episodes
                && let Some(ref org_config) = org_config
            {
                // SSoT: series path via `paths::mapping_path` (template resolution +
                // illegal-char sanitization).
                let resolved_path = Some(crate::paths::mapping_path(&mapping, org_config));

                if let Some(ref dir) = resolved_path {
                    if dir.exists() {
                        tracing::debug!("Deleting episode files from disk: {}", dir.display());
                        if let Err(e) = tokio::fs::remove_dir_all(dir).await {
                            tracing::error!(
                                "Failed to delete series directory {}: {}",
                                dir.display(),
                                e
                            );
                        }
                    } else {
                        tracing::warn!(
                            "Episode directory not found, skipping disk delete: {}",
                            dir.display()
                        );
                    }
                }
            }

            // When deleting data but NOT config, the series is hidden (identity kept).
            if !payload.delete_configurations
                && delete_data
                && let Ok(Some(mut m)) = state.db.get_series_mapping(id).await
            {
                m.hidden_in_library = true;
                let _ = state.db.upsert_series_mapping(id, &m).await;
            }

            if payload.delete_configurations
                && let Err(e) = state.db.delete_series_mapping(id).await
            {
                tracing::error!(
                    "Failed to delete mapping for {} ({}): {}",
                    series_title,
                    id,
                    e
                );
            }

            if delete_data || payload.delete_configurations {
                crate::api::prune_series_in_memory_state(&state, id).await;
            }
        }
    }

    if payload.delete_configurations {
        // Called once after the loop: `cleanup_orphaned_metadata` scans all remaining
        // mappings, so doing it per-series would re-read the full table N times.
        if let Err(e) = state.db.cleanup_orphaned_metadata().await {
            tracing::warn!(
                "Failed to clean up orphaned metadata after batch removal: {}",
                e
            );
        }
    }

    Ok(StatusCode::OK)
}

/// Delete all episode data (DB rows) for a series without hiding or removing it.
///
/// This is a lighter operation than full removal — it clears episode/stats
/// data so the scanner will re-import from scratch, but preserves the series
/// mapping and all configuration (profiles, filters, scoring, etc.).
pub async fn delete_episode_data(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    tracing::info!("Deleting episode data for series {}", id);
    let _ = get_series_mapping_or_404(&state, &id).await?.target_title;

    if !try_lock_series(&state, &id).await {
        return Err(AppError::Conflict(
            "Series is currently being modified by another operation (e.g. batch move)".to_string(),
        ));
    }

    if let Err(e) = state.db.delete_series_data(&id).await {
        unlock_series(&state, &id).await;
        return Err(AppError::Internal(anyhow::anyhow!(
            "Failed to delete episode data: {}",
            e
        )));
    }

    // Reset all `metadata_last_synced_at` entries so the background refresh
    // loop re-fetches metadata on its next tick instead of thinking everything
    // is up-to-date.
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
        mapping.settings.metadata_last_synced_at.clear();
        if let Err(e) = state.db.upsert_series_mapping(&id, &mapping).await {
            tracing::error!(
                "Failed to reset metadata_last_synced_at after deleting episode data: {}",
                e
            );
        }
    }

    unlock_series(&state, &id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Reset all configuration for a series back to defaults without hiding it.
///
/// Preserves: target_title, name, series_id, path, and all episode data.
/// Resets: release_profile, qb_category, filters, scoring, episode_start,
/// episode_end, quality_profile, settings.
pub async fn reset_configuration(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    tracing::info!("Resetting configuration for series {}", id);
    let mut mapping = get_series_mapping_or_404(&state, &id).await?;
    mapping.release_profile = None;
    mapping.qb_category = None;
    mapping.filters = None;
    mapping.scoring = None;
    mapping.quality_profile = None;
    mapping.settings = jumbie_shared::types::SeriesSettings::default();
    mapping.settings.monitor_mode = Some(jumbie_shared::types::DEFAULT_MONITOR_MODE);
    state
        .db
        .upsert_series_mapping(&id, &mapping)
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("Failed to update series mapping: {}", e))
        })?;

    if let Err(e) = crate::api_routes::series::apply_monitor_mode(&state, &id).await {
        tracing::error!("Failed to apply monitor mode after reset: {}", e);
    }

    if let Err(e) = state.db.cleanup_orphaned_metadata().await {
        tracing::warn!("Failed to clean up orphaned metadata: {}", e);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Batch-upsert seasons for a series.
///
/// Accepts a minimal payload (seasons + episode_count + absolute_numbering flag)
/// and upserts each season into the series mapping. Reuses the existing
/// `upsert_series_mapping` DB write path — no dual-write concern.
///
/// Each season number is validated via the shared `validate_season_number`.
/// Episode count must be >= 1.
pub async fn batch_upsert_seasons(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::BatchUpsertSeasonsRequest>,
) -> Result<Json<Vec<String>>, AppError> {
    tracing::info!(
        "Batch-upserting {} season(s) for series {}",
        payload.seasons.len(),
        id
    );

    if payload.seasons.is_empty() {
        return Err(AppError::BadRequest("No seasons provided.".to_string()));
    }

    if payload.episode_count < 1 {
        return Err(AppError::BadRequest(
            "Episode count must be at least 1.".to_string(),
        ));
    }

    for &s in &payload.seasons {
        crate::validation::validate_season_number(&s.to_string()).map_err(|e| {
            tracing::error!("Invalid season number {}: {}", s, e);
            AppError::BadRequest(e.0)
        })?;
    }

    let mut mapping = get_series_mapping_or_404(&state, &id).await?;

    let target_map = if payload.absolute_numbering {
        &mut mapping.settings.season_absolute
    } else {
        &mut mapping.settings.season
    };

    for &s in &payload.seasons {
        let season_key = s.to_string();
        if let Some(existing) = target_map.get_mut(&season_key) {
            existing.cell_count = Some(payload.episode_count);
        } else {
            target_map.insert(
                season_key.clone(),
                SeasonOverride {
                    season: season_key,
                    episode_start: None,
                    episode_end: None,
                    cell_count: Some(payload.episode_count),
                    episode_offset: None,
                    alias_season_number: None,
                    search_format: None,
                    aliases: Vec::new(),
                    reg_patterns: Vec::new(),
                },
            );
        }
    }

    state
        .db
        .upsert_series_mapping(&id, &mapping)
        .await
        .map_err(|e| {
            tracing::error!(
                "Failed to save mapping for series {} ({}) after season upsert: {}",
                mapping.target_title,
                id,
                e
            );
            AppError::Internal(anyhow::anyhow!("Failed to save mapping: {}", e))
        })?;

    let keys: Vec<String> = payload.seasons.iter().map(|s| s.to_string()).collect();
    tracing::info!(
        "Successfully upserted {} season(s) for series {} ({})",
        keys.len(),
        mapping.target_title,
        id
    );
    Ok(Json(keys))
}

/// SSoT: record a per-provider metadata sync failure for the add-series flow
/// (log warning + `Error` activity event).  Used by `create_series` for both
/// provider errors and the sync-deadline expiry, so the log line and activity
/// row stay in one place.
async fn record_sync_failure(
    state: &Arc<AppState>,
    provider: &str,
    series_title: &str,
    reason: String,
) {
    tracing::warn!(
        "create_series: metadata sync failed for series '{}' provider {}: {}",
        series_title,
        provider,
        reason
    );
    let _ = state
        .db
        .record_activity(ActivityEvent {
            event_type: ActivityType::Metadata,
            series_title: series_title.to_string(),
            season: None,
            episode: None,
            episode_end: None,
            title: None,
            details: Some(format!("Sync failed ({provider}): {reason}")),
            status: "Error".to_string(),
        })
        .await;
}
