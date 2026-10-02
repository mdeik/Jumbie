use std::collections::HashMap;

use crate::api::AppState;
use crate::api::modifying_series::{try_lock_series, unlock_series};
use crate::db::EpisodeDetailRow;
use crate::db::episodes::assign::AssignFileToEpisodeParams;
use crate::error::AppError;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use std::sync::Arc;

use super::helpers::get_series_mapping_or_404;

pub fn calculate_rename_plan_hash(
    affected_episodes: usize,
    collision_count: usize,
    causes: &[String],
) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    affected_episodes.hash(&mut hasher);
    collision_count.hash(&mut hasher);
    for cause in causes {
        cause.hash(&mut hasher);
    }
    hasher.finish()
}

// Shared reorganization plan execution (SSoT for `reorganize_series` and
// `reorganize_all_core`).

/// Execute a single series' rename plan.
/// If `cancel` is signalled, the current file's move is finished (rename is atomic)
/// but no remaining files in the plan are started.
/// Returns `(success_count, fail_count)`.
async fn execute_reorganization_plan(
    state: &Arc<AppState>,
    organizer: &crate::organizer::ContentOrganizer,
    plan: &[crate::file_manager::PlannedMove],
) -> (usize, usize) {
    if plan.is_empty() {
        return (0, 0);
    }

    // Build the move list, skipping files currently being scanned. The plan was
    // built from a snapshot of active scan paths; re-check right before moving so a
    // file mid-hash/ffprobe isn't moved underneath the scanner (the next auto-apply
    // tick picks skipped files up).
    let mut moves: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    let mut move_plan: Vec<&crate::file_manager::PlannedMove> = Vec::new();
    let mut fail_count = 0usize;

    for (idx, planned) in plan.iter().enumerate() {
        // Shutdown: finish what has started but don't begin new moves.
        if crate::task::is_shutdown_requested(&state.shutdown_token, "reorganization plan") {
            break;
        }
        if state.scan_queue.contains(&planned.src).await {
            tracing::debug!(
                "[REORG] [{}/{}]: skipping '{}' — currently being scanned",
                idx + 1,
                plan.len(),
                planned.src.display()
            );
            fail_count += 1;
            continue;
        }
        if !planned.src.exists() {
            tracing::warn!("Skipping missing file: '{}'", planned.src.display());
            fail_count += 1;
            continue;
        }
        tracing::debug!(
            "[REORG] [{}/{}]: moving '{}' -> '{}'",
            idx + 1,
            plan.len(),
            planned.src.display(),
            planned.dst.display()
        );
        moves.push((planned.src.clone(), planned.dst.clone()));
        move_plan.push(planned);
    }

    // SSoT cycle-safe move: recycled sources are temp-renamed (with their
    // fingerprint rows) before the moves run.
    let results = crate::file_manager::move_files_cycle_safe(organizer, &moves).await;

    let mut success_count = 0usize;
    for (planned, result) in move_plan.iter().zip(results) {
        match result {
            Some(new_path) => {
                if handle_reorganization_success(state, planned, &new_path).await {
                    success_count += 1;
                } else {
                    tracing::error!("[REORG] DB update failed for '{}'", new_path.display());
                    fail_count += 1;
                }
            }
            None => fail_count += 1,
        }
    }

    (success_count, fail_count)
}

/// Record failure information for a series whose reorganization had errors.
/// Must be called from an async context (holds `.await` internally).
async fn record_reorganization_failures(
    state: &Arc<AppState>,
    id: &str,
    plan: &[crate::file_manager::PlannedMove],
    mapping: &jumbie_shared::types::MappingRule,
    fail_count: usize,
) {
    if fail_count == 0 {
        return;
    }

    let batch_sources: std::collections::HashSet<_> = plan.iter().map(|p| p.src.clone()).collect();
    let collision_count =
        crate::validation::collision::detect_collisions_in_plan(plan, &batch_sources);

    // SSoT: compute_plan_causes is the single source of truth for
    // rename-plan cause determination.
    let causes = crate::file_manager::compute_plan_causes(mapping, plan);

    // Hash-based failure tracking: the plan's properties (affected count,
    // collisions, causes) are hashed into `state.failed_renames`. A later rename
    // queue check shows "has_failed" while the hash matches; if the plan changes
    // the hash differs and the failure clears automatically, avoiding stored
    // failure metadata that could go stale.
    let plan_hash = calculate_rename_plan_hash(plan.len(), collision_count, &causes);
    let mut failed_map = state.failed_renames.write().await;
    failed_map.insert(id.to_string(), plan_hash);
}

pub async fn reorganize_series(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::ReorganizeSeriesPayload>,
) -> Result<StatusCode, AppError> {
    let organizer = state
        .organizer
        .as_ref()
        .ok_or_else(|| AppError::ServiceUnavailable("Organizer not available".to_string()))?;

    let (mapping, db_episodes) = {
        let mapping = get_series_mapping_or_404(&state, &id).await?;
        let db_episodes = state
            .db
            .get_rename_plan_episodes(&id, state.effective_absolute_numbering(&mapping).await)
            .await
            .map_err(|e: anyhow::Error| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
        (mapping, db_episodes)
    };

    tracing::debug!(
        "Reorganizing files for series {} ({}) with target absolute: {}",
        mapping.target_title,
        id,
        payload.target_absolute
    );

    if !try_lock_series(&state, &id).await {
        return Err(AppError::Conflict(
            "Series is currently being modified by another operation (e.g. batch move)".to_string(),
        ));
    }

    let episode_ids: Vec<String> = db_episodes.iter().map(|r| r.episode_id.clone()).collect();
    let episode_parts = state
        .db
        .get_parts_for_series(&episode_ids)
        .await
        .unwrap_or_default();

    // Proactive media info scan (SSoT: `proactively_scan_missing_media_info`).
    let mut skip_paths = state.scan_queue.active_paths_snapshot().await;
    {
        let mut single_map = std::collections::HashMap::new();
        single_map.insert(mapping.target_title.clone(), db_episodes.clone());
        skip_paths =
            crate::api::proactively_scan_missing_media_info(&state, &single_map, skip_paths).await;
    }

    // Build the full (src → dst) move plan, excluding files currently being
    // scanned (their media info may be incomplete). Use the cache when
    // target_absolute matches the mapping setting so `get_rename_queue` can reuse
    // this computation; a user override computes fresh (rare).
    let plan = {
        let config = state.cfg.read().await;
        let mapping_abs = crate::file_manager::should_use_absolute_numbering(&config, &mapping);
        let use_cache = payload.target_absolute == mapping_abs;
        let aux_files = crate::file_manager::auxiliary_paths_for_series(&state.db, &id).await;

        let plan_result = if use_cache {
            state
                .rename_plan_cache
                .get_or_compute_with_aux(crate::file_manager::RenamePlanInputs {
                    series_id: &id,
                    config: &config,
                    mapping: &mapping,
                    db_episodes: &db_episodes,
                    episode_parts: &episode_parts,
                    aux_files: &aux_files,
                    skip_paths: &skip_paths,
                })
                .await
        } else {
            crate::file_manager::compute_batch_plan_with_aux(
                &config,
                &mapping,
                &db_episodes,
                &episode_parts,
                &aux_files,
                payload.target_absolute,
                &skip_paths,
            )
            .map(std::sync::Arc::new)
        };

        match plan_result {
            Ok(p) => p,
            Err(crate::file_manager::RenamePlanError::DuplicateTarget(dst)) => {
                // DuplicateTarget means two episodes resolve to the same destination
                // (e.g. E01 and E03 both → "S01E01.mkv") — a permanent misconfiguration
                // that retrying cannot fix. Marked with the hash=0 sentinel so the queue
                // shows a failure until the user fixes the naming rules.
                tracing::error!(
                    "Rename plan for series {} ({}) has a duplicate target: '{}'",
                    mapping.target_title,
                    id,
                    dst.display()
                );
                let mut failed_map = state.failed_renames.write().await;
                failed_map.insert(id.clone(), 0); // mark as permanently failed
                unlock_series(&state, &id).await;
                return Err(AppError::Internal(anyhow::anyhow!(
                    "Rename plan has a duplicate target destination: '{}'",
                    dst.display()
                )));
            }
        }
    };

    if plan.is_empty() {
        tracing::info!(
            "Reorganization for series {} ({}): no files need moving",
            mapping.target_title,
            id
        );
        unlock_series(&state, &id).await;
        return Ok(StatusCode::OK);
    }

    state.processing_renames.write().await.insert(id.clone());

    let (success_count, fail_count) = execute_reorganization_plan(&state, organizer, &plan).await;

    state.processing_renames.write().await.remove(&id);

    if fail_count > 0 {
        record_reorganization_failures(&state, &id, &plan, &mapping, fail_count).await;

        // An error is returned even on partial failure: the frontend shows a
        // "succeeded" message on 200 OK, so silently-failed moves (cross-device,
        // permission denied) would give false confidence. AppError forces the
        // frontend to surface it, and the queue's has_failed flag gives details.
        let error_msg = format!(
            "Reorganization for series {} ({}) had failures: {} succeeded, {} failed",
            mapping.target_title, id, success_count, fail_count
        );
        tracing::error!("{}", error_msg);
        unlock_series(&state, &id).await;
        return Err(AppError::Internal(anyhow::anyhow!(error_msg)));
    } else {
        let mut failed_map = state.failed_renames.write().await;
        failed_map.remove(&id);
    }

    // Invalidate cache — file paths may have changed

    tracing::info!(
        "Reorganization for series {} ({}) complete. Success: {}, Fail: {}",
        mapping.target_title,
        id,
        success_count,
        fail_count
    );
    unlock_series(&state, &id).await;
    Ok(StatusCode::OK)
}

pub async fn reorganize_all_series(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, AppError> {
    tracing::info!("Reorganizing files for all series (Manual trigger)");
    reorganize_all_impl(&state)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(StatusCode::OK)
}

/// Async version: spawns the reorganization in the background and returns
/// a task_id immediately. The frontend polls the status endpoint to track
/// progress via a progress toast.
pub async fn reorganize_all_series_async(
    State(state): State<Arc<AppState>>,
) -> Result<Json<jumbie_shared::types::BatchMoveResponse>, AppError> {
    tracing::info!("Reorganizing all series (async, background spawn)");

    let series_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();
    let total = series_mappings.len();

    // SSoT: use ProgressTracker to start the task
    let (task_id, handle) = state
        .progress_tracker
        .start(crate::api::OperationType::Reorganize, total);

    let state_clone = state.clone();
    let task_id_clone = task_id.clone();

    tokio::spawn(async move {
        // Sub-spawn so panics are caught by tokio, not the outer task.
        // This ensures `handle` is always consumed (complete/cancel/fail),
        // which guarantees mark_finished() is called.
        // Clone before the inner spawn so state_clone/task_id_clone remain
        // available in the outer scope for reading final progress.
        let inner_state = state_clone.clone();
        let inner_task_id = task_id_clone.clone();
        let inner_handle = tokio::spawn(async move {
            let _ = reorganize_all_impl_inner(&inner_state, Some(inner_task_id)).await;
        });

        match inner_handle.await {
            Ok(_) => {
                // SSoT: complete the handle to set finished=true, finished_at.
                let final_progress = state_clone.progress_tracker.get(&task_id_clone);
                if let Some(p) = final_progress {
                    handle.complete(p.success_count, p.failed, p.errors.clone());
                } else {
                    handle.complete(0, 0, vec![]);
                }
            }
            Err(join_err) => {
                if join_err.is_panic() {
                    handle.fail("Reorganization task panicked".into());
                } else {
                    handle.cancel("Task was cancelled".into());
                }
            }
        }

        tracing::info!("Async reorganization {} completed", &task_id_clone);
    });

    Ok(Json(jumbie_shared::types::BatchMoveResponse {
        results: vec![],
        success_count: 0,
        failure_count: 0,
        task_id: Some(task_id),
    }))
}

/// GET /api/series/actions/reorganize_all/{task_id}/status
pub async fn reorganize_all_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Result<Json<crate::api::BatchMoveProgress>, AppError> {
    crate::validation::validate_not_empty(&task_id, "Task ID").map_err(|e| {
        tracing::debug!("reorganize_all_status: invalid task ID: {}", e);
        AppError::BadRequest(e.0)
    })?;
    match state.progress_tracker.get(&task_id) {
        Some(p) => Ok(Json(p)),
        None => Err(AppError::NotFound(
            "Reorganization task not found".to_string(),
        )),
    }
}

pub async fn reorganize_all_impl(state: &Arc<AppState>) -> Result<(), String> {
    reorganize_all_impl_inner(state, None).await
}

pub(crate) async fn reorganize_all_impl_inner(
    state: &Arc<AppState>,
    task_id: Option<String>,
) -> Result<(), String> {
    tracing::trace!("reorganize_all_impl starting");
    // SSoT: ReorgGuard always resets is_reorganizing on drop, even on panic.
    match crate::api::ReorgGuard::try_acquire(&state.is_reorganizing) {
        Some(guard) => {
            let result = reorganize_all_core(state, task_id).await;
            drop(guard); // explicitly release before returning
            result
        }
        None => {
            tracing::warn!("Reorganization already in progress. Skipping.");
            Ok(())
        }
    }
}

async fn reorganize_all_core(state: &Arc<AppState>, task_id: Option<String>) -> Result<(), String> {
    // Clear cached pending-rename flag — we're about to process everything,
    // so the cached state is stale until the next get_rename_queue call.
    *state.rename_queue_has_pending.write().await = false;

    let organizer = state
        .organizer
        .as_ref()
        .ok_or_else(|| "Organizer not available".to_string())?;

    let series_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();

    // Shared skip_paths (SSoT: `build_rename_queue_skip_paths`).
    let mut skip_paths = crate::api::build_rename_queue_skip_paths(state).await;

    let updater = task_id
        .as_ref()
        .and_then(|tid| state.progress_tracker.updater(tid));

    // SSoT: effective numbering-mode default, read once for the whole loop.
    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };
    let abs_ids: Vec<String> = series_mappings
        .iter()
        .filter(|(_, m)| m.settings.active_mode(global_absolute).is_absolute())
        .map(|(id, _)| id.clone())
        .collect();
    let norm_ids: Vec<String> = series_mappings
        .iter()
        .filter(|(_, m)| !m.settings.active_mode(global_absolute).is_absolute())
        .map(|(id, _)| id.clone())
        .collect();

    let mut episodes_by_id: HashMap<String, Vec<EpisodeDetailRow>> = HashMap::new();
    if !abs_ids.is_empty() {
        match state
            .db
            .get_rename_plan_episodes_batch(&abs_ids, true)
            .await
        {
            Ok(abs_batch) => {
                episodes_by_id.extend(abs_batch);
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to batch-fetch absolute-mode episodes for {} series: {}",
                    abs_ids.len(),
                    e
                );
            }
        }
    }
    if !norm_ids.is_empty() {
        match state
            .db
            .get_rename_plan_episodes_batch(&norm_ids, false)
            .await
        {
            Ok(norm_batch) => {
                episodes_by_id.extend(norm_batch);
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to batch-fetch normal-mode episodes for {} series: {}",
                    norm_ids.len(),
                    e
                );
            }
        }
    }

    let mut episode_to_series: HashMap<String, String> = HashMap::new();
    for (sid, eps) in &episodes_by_id {
        for ep in eps {
            episode_to_series.insert(ep.episode_id.clone(), sid.clone());
        }
    }
    let all_episode_ids: Vec<String> = episode_to_series.keys().cloned().collect();
    let all_parts = match state.db.get_parts_for_series(&all_episode_ids).await {
        Ok(parts) => parts,
        Err(e) => {
            tracing::warn!(
                "Failed to batch-fetch parts for {} episodes: {}",
                all_episode_ids.len(),
                e
            );
            Vec::new()
        }
    };

    let mut parts_by_series: HashMap<String, Vec<(String, crate::db::EpisodePartRow)>> =
        HashMap::new();
    for (ep_id, part_row) in &all_parts {
        if let Some(sid) = episode_to_series.get(ep_id) {
            parts_by_series
                .entry(sid.clone())
                .or_default()
                .push((ep_id.clone(), part_row.clone()));
        }
    }

    // Shutdown skips the scan-queue submissions and moves straight to executing
    // remaining plans (`execute_reorganization_plan` also checks shutdown between
    // files).
    if !crate::task::is_shutdown_requested(&state.shutdown_token, "series reorganization loop") {
        skip_paths =
            crate::api::proactively_scan_missing_media_info(state, &episodes_by_id, skip_paths)
                .await;
    }

    let mut total_success = 0;
    let mut total_fail = 0;
    let mut series_completed = 0usize;

    for (id, mapping) in &series_mappings {
        // Acquire per-series lock to prevent TOCTOU races.
        if !try_lock_series(state, id).await {
            tracing::debug!(
                "Skipping locked series {} ({}) during global reorganization",
                id,
                mapping.target_title
            );
            continue;
        }

        // Check global shutdown signal between series.
        if crate::task::is_shutdown_requested(&state.shutdown_token, "series reorganization loop") {
            unlock_series(state, id).await;
            break;
        }

        let Some(db_episodes) = episodes_by_id.get(id) else {
            unlock_series(state, id).await;
            tracing::trace!(
                "No episodes found for series {}, skipping",
                mapping.target_title
            );
            continue;
        };

        let episode_parts = parts_by_series.get(id).cloned().unwrap_or_default();

        let plan = {
            let config = state.cfg.read().await;
            let aux_files = crate::file_manager::auxiliary_paths_for_series(&state.db, id).await;
            match crate::file_manager::compute_batch_plan_with_aux(
                &config,
                mapping,
                db_episodes,
                &episode_parts,
                &aux_files,
                crate::file_manager::should_use_absolute_numbering(&config, mapping),
                &skip_paths,
            ) {
                Ok(p) => p,
                Err(crate::file_manager::RenamePlanError::DuplicateTarget(dst)) => {
                    unlock_series(state, id).await;
                    tracing::error!(
                        "Rename plan for series {} has duplicate target: '{}'",
                        mapping.target_title,
                        dst.display()
                    );
                    total_fail += 1;
                    continue;
                }
            }
        };

        series_completed += 1;

        if plan.is_empty() {
            unlock_series(state, id).await;
            if let Some(ref updater) = updater {
                updater.report(series_completed, total_success, total_fail);
            }
            continue;
        }

        state.processing_renames.write().await.insert(id.clone());
        let (success, fail) = execute_reorganization_plan(state, organizer, &plan).await;
        state.processing_renames.write().await.remove(id);

        total_success += success;
        total_fail += fail;

        if let Some(ref updater) = updater {
            updater.report(series_completed, total_success, total_fail);
        }

        if fail > 0 {
            record_reorganization_failures(state, id, &plan, mapping, fail).await;
        } else {
            let mut failed_map = state.failed_renames.write().await;
            failed_map.remove(id);
        }

        unlock_series(state, id).await;
    }

    if total_success == 0 && total_fail == 0 {
        tracing::debug!(
            "Global reorganization complete. Success: {}, Fail: {}",
            total_success,
            total_fail
        );
    } else {
        tracing::info!(
            "Global reorganization complete. Success: {}, Fail: {}",
            total_success,
            total_fail
        );
    }

    // The flag was cleared at the top of this function, but series that failed are
    // still in `failed_renames` and the sidebar indicator should reflect that.
    // Series skipped (locked, hidden, shutdown) have no failure record and reappear
    // on the next full queue poll, so clearing the flag for them is correct.
    let has_failures = !state.failed_renames.read().await.is_empty();
    *state.rename_queue_has_pending.write().await = has_failures;

    Ok(())
}

pub async fn handle_reorganization_success(
    state: &Arc<AppState>,
    planned: &crate::file_manager::PlannedMove,
    new_path: &std::path::Path,
) -> bool {
    let new_path_str = new_path.to_string_lossy().to_string();

    // Auxiliary sidecar move: `move_file_to_target` already migrated the
    // file_paths row, and its kind/episode link are preserved as-is.
    if planned.aux_kind.is_some() {
        return true;
    }

    if let Some(part_num) = planned.part_number {
        // Part-file move: re-point the episode's `episode_files` part association. A
        // multipart episode (CD1/CD2) shares one episode_id with distinct part
        // numbers, so a single main file would reflect only one part; `episode_files`
        // maps (episode_id, part_number) → file_path_id.
        let episode_id = match planned.covered_episodes.first() {
            Some(e) => e.episode_id.clone(),
            None => return false,
        };
        if let Err(e) = state
            .db
            .upsert_episode_part(&episode_id, part_num, &new_path_str, None)
            .await
        {
            tracing::error!(
                "Reorganized part file but failed to update episode_files for {}: {}",
                new_path_str,
                e
            );
            return false;
        }
        return true;
    }

    // Normal / multi-episode move: a file like "S01E01E02.mkv" covers two episodes,
    // so `assign_file_to_episode` sets the same file_path on all covered rows.
    let mut all_ok = true;
    for ep in &planned.covered_episodes {
        // Fetch numbering_mode from the existing row (it exists from cell generation
        // or metadata fetch at this point).
        let numbering_mode: i32 =
            sqlx::query_scalar("SELECT numbering_mode FROM episodes WHERE episode_id = ?")
                .bind(&ep.episode_id)
                .fetch_optional(state.db.get_pool())
                .await
                .unwrap_or(None)
                .unwrap_or(0);

        // SSoT: `numbering_mode == 1` is the absolute space, whose canonical
        // season is ABSOLUTE_SEASON_NUM; a normal-mode label must be numeric.
        // Skip this episode rather than writing a guessed season to the row.
        let season = match jumbie_shared::mapping::resolve_season_num(
            &planned.season_val,
            numbering_mode == 1,
        ) {
            Ok(season) => season,
            Err(e) => {
                tracing::warn!(
                    "Skipping season write for episode {} ({}): {}",
                    ep.episode_id,
                    new_path_str,
                    e
                );
                all_ok = false;
                continue;
            }
        };

        if let Err(e) = state
            .db
            .assign_file_to_episode(AssignFileToEpisodeParams {
                episode_id: &ep.episode_id,
                series_id: &planned.series_id,
                series_title: &planned.series_title,
                season,
                episode: ep.episode_num,
                file_path: &new_path_str,
                episode_title: &planned.episode_title,
                all_target_episode_ids: Some(
                    &planned
                        .covered_episodes
                        .iter()
                        .map(|e| e.episode_id.clone())
                        .collect::<Vec<_>>(),
                ),
                numbering_mode,
                only_unassigned: true,
            })
            .await
        {
            tracing::error!(
                "Reorganized file but failed to update DB for episode {} ({}): {}",
                ep.episode_id,
                new_path_str,
                e
            );
            all_ok = false;
        }
    }
    // Invalidate cache once after updating all covered episodes.

    // Fingerprint already migrated by `move_fingerprint_path`: `move_file_to_target`
    // (Phase 2 above) already moved the fingerprint row, preserving all metadata.
    // Re-fingerprinting here would re-hash the whole file and re-run ffprobe —
    // expensive and redundant for same-filesystem renames. For EXDEV copies the
    // background scanner refreshes stale identity fields (inode/device) next cycle.

    // Download queue cleanup (SSoT): reorganize is a form of organize, so any
    // 'Completed'/'Failed' queue item for a covered episode is stale and removed so
    // `organize_completed` won't return this file. Per-episode because one
    // PlannedMove may cover several episodes, each with its own queue item.
    for ep in &planned.covered_episodes {
        let _ = state.db.remove_completed_queue_item(&ep.episode_id).await;
    }

    all_ok
}
