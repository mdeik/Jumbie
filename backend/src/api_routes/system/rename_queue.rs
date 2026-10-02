//! Rename Queue — preview planned file renames before they execute.
//!
//! The "dry run" is separate from the actual reorganize action so users can see
//! what WILL happen first. The same `compute_batch_plan` drives both this preview
//! and the real reorganization, keeping them consistent.

use crate::api::AppState;
use crate::error::AppError;
use axum::{Json, extract::State};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Singleton rename-queue notifier instance.
///
/// A global static so firing-cooldown and last-fired state persist across ALL
/// invocations of `get_rename_queue`, regardless of which HTTP worker handles the
/// request; a local would be recreated each call and lose that state.
static RENAME_QUEUE_NOTIFIER: std::sync::LazyLock<
    crate::api_routes::system::rename_queue_notifier::RenameQueueNotifier,
> = std::sync::LazyLock::new(|| {
    crate::api_routes::system::rename_queue_notifier::RenameQueueNotifier::new(10_000)
});

/// GET /api/system/rename-queue
///
/// Iterates ALL series, computes the reorganize plan for each, and returns which
/// series need renames, how many files are affected, and whether any failed.
pub async fn get_rename_queue(
    State(state): State<Arc<AppState>>,
) -> Json<jumbie_shared::types::RenameQueueResponse> {
    tracing::trace!("get_rename_queue called");
    let mut items = Vec::new();
    let mut total_affected: usize = 0;

    // Build shared skip_paths (scan_queue + pending-organization exclusions).
    // SSoT: See build_rename_queue_skip_paths in api/mod.rs.
    let mut skip_paths = crate::api::build_rename_queue_skip_paths(&state).await;

    let config = state.cfg.read().await;
    let mappings = state.db.get_all_series_mappings().await.unwrap_or_default();

    // All episodes are fetched in one query (split by numbering mode because the
    // batch query filters on mode), then all parts in another — O(1) round-trips
    // regardless of series count.
    let abs_ids: Vec<String> = mappings
        .iter()
        .filter(|(_, m)| {
            m.settings
                .active_mode(config.general.absolute_numbering)
                .is_absolute()
        })
        .map(|(id, _)| id.clone())
        .collect();
    let norm_ids: Vec<String> = mappings
        .iter()
        .filter(|(_, m)| {
            !m.settings
                .active_mode(config.general.absolute_numbering)
                .is_absolute()
        })
        .map(|(id, _)| id.clone())
        .collect();

    let mut episodes_by_id: HashMap<String, Vec<crate::db::EpisodeDetailRow>> = HashMap::new();
    if !abs_ids.is_empty()
        && let Ok(abs_batch) = state
            .db
            .get_rename_plan_episodes_batch(&abs_ids, true)
            .await
    {
        episodes_by_id.extend(abs_batch);
    }
    if !norm_ids.is_empty()
        && let Ok(norm_batch) = state
            .db
            .get_rename_plan_episodes_batch(&norm_ids, false)
            .await
    {
        episodes_by_id.extend(norm_batch);
    }

    // Build episode_id -> series_id reverse lookup for parts grouping.
    let mut episode_to_series: HashMap<String, String> = HashMap::new();
    for (sid, eps) in &episodes_by_id {
        for ep in eps {
            episode_to_series.insert(ep.episode_id.clone(), sid.clone());
        }
    }

    // Collect ALL episode IDs across all series for a single parts query.
    let all_episode_ids: Vec<String> = episode_to_series.keys().cloned().collect();

    let all_parts = state
        .db
        .get_parts_for_series(&all_episode_ids)
        .await
        .unwrap_or_default();

    // Group parts by series_id: compute_batch_plan expects a slice scoped to one
    // series, so pre-grouping avoids per-series filtering in the loop.
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

    // Proactive media info scan (SSoT: `proactively_scan_missing_media_info`):
    // files with a path but no media info are submitted so the rename plan never
    // uses empty template variables; permanently-failed files are excluded.
    skip_paths =
        crate::api::proactively_scan_missing_media_info(&state, &episodes_by_id, skip_paths).await;

    // Snapshot the shared state locks once, then defer all mutations to a single
    // write batch after the loop — avoids repeated lock contention per iteration.
    let failed_map_snapshot: HashMap<String, u64> = state.failed_renames.read().await.clone();
    let processing_set: std::collections::HashSet<String> =
        state.processing_renames.read().await.clone();

    let mut failed_removals: Vec<String> = Vec::new();
    let mut failed_insertions: Vec<(String, u64)> = Vec::new();
    let mut processing_removals: Vec<String> = Vec::new();

    for (series_id, mapping) in &mappings {
        // Hidden series are soft-deleted from the library: their episodes must not
        // appear (the user can't access them, plans waste CPU/IO, and notifications
        // would spam for invisible series). Unhiding triggers a scan that
        // regenerates the entry if needed.
        if mapping.hidden_in_library {
            failed_removals.push(series_id.clone());
            processing_removals.push(series_id.clone());
            continue;
        }

        let series_title = mapping.target_title.clone();

        // Borrow the pre-fetched batch rows instead of cloning: the map is
        // immutable for the loop and `compute_batch_plan` only needs a slice.
        let db_episodes: &[crate::db::EpisodeDetailRow] = match episodes_by_id.get(series_id) {
            Some(eps) => eps.as_slice(),
            None => {
                // No batch rows (e.g. DB inconsistency) means any stored failure
                // hash is stale; clear it to avoid a false "has_failed" indicator.
                failed_removals.push(series_id.clone());
                continue;
            }
        };

        let episode_parts = parts_by_series.remove(series_id).unwrap_or_default();

        // Proactive metadata enrichment (SSoT: `proactively_fetch_missing_episode_metadata`):
        // Episodes with an empty/NULL title that aren't user-protected trigger a
        // background metadata fetch (cooldown-gated) so the next poll shows complete
        // filenames. Spawned — never blocks the response.
        crate::api::proactively_fetch_missing_episode_metadata(
            &state,
            series_id,
            db_episodes,
            mapping,
            config.general.metadata_fetch_cooldown_minutes,
        )
        .await;

        let aux_files = crate::file_manager::auxiliary_paths_for_series(&state.db, series_id).await;
        let plan = match state
            .rename_plan_cache
            .get_or_compute_with_aux(crate::file_manager::RenamePlanInputs {
                series_id,
                config: &config,
                mapping,
                db_episodes,
                episode_parts: &episode_parts,
                aux_files: &aux_files,
                skip_paths: &skip_paths,
            })
            .await
        {
            Ok(p) => p,
            Err(crate::file_manager::RenamePlanError::DuplicateTarget(dst)) => {
                tracing::warn!(
                    "Rename plan for series {} ({}) has duplicate target '{}'; marking failed",
                    series_title,
                    series_id,
                    dst.display()
                );
                failed_insertions.push((series_id.clone(), 0));
                continue;
            }
        };

        let affected_episodes = plan.len();

        if affected_episodes > 0 {
            total_affected += affected_episodes;

            let batch_sources: std::collections::HashSet<_> =
                plan.iter().map(|p| p.src.clone()).collect();

            let collision_count =
                crate::validation::collision::detect_collisions_in_plan(&plan, &batch_sources);

            // SSoT: compute_plan_causes is the single source of truth for
            // rename-plan cause determination.
            let causes = crate::file_manager::compute_plan_causes(mapping, &plan);

            let current_hash = crate::api_routes::series::calculate_rename_plan_hash(
                affected_episodes,
                collision_count,
                &causes,
            );

            // A stored failure hash only stays relevant while the plan is unchanged:
            // a matching hash means nothing was fixed yet, a different hash means the
            // user edited format settings etc. and the failure is stale.
            let has_failed = match failed_map_snapshot.get(series_id) {
                Some(&stored_hash) if stored_hash == current_hash => true,
                Some(_) => {
                    failed_removals.push(series_id.clone());
                    false
                }
                None => false,
            };

            let processing = processing_set.contains(series_id);

            items.push(jumbie_shared::types::RenameQueueItem {
                series_id: series_id.clone(),
                series_title,
                affected_episodes,
                collision_count,
                causes,
                absolute_numbering: mapping.settings.absolute_numbering.unwrap_or(false),
                has_failed,
                processing,
            });
        } else {
            // Series with no pending renames — clear any stale failure flags
            failed_removals.push(series_id.clone());

            // Also clear processing flag for non-pending series to prevent stuck states
            processing_removals.push(series_id.clone());
        }
    }

    // Apply all deferred mutations, acquiring each lock exactly once.
    {
        let mut failed_map = state.failed_renames.write().await;
        for id in &failed_removals {
            failed_map.remove(id);
        }
        for (id, hash) in &failed_insertions {
            failed_map.insert(id.clone(), *hash);
        }
    }
    {
        let mut processing = state.processing_renames.write().await;
        for id in &processing_removals {
            processing.remove(id);
        }
    }

    let has_failed_renames = items.iter().any(|i| i.has_failed);

    *state.rename_queue_has_pending.write().await = total_affected > 0;

    // The notifier owns startup seeding, per-series change detection, the
    // 10-second cooldown/debounce, and auto-apply suppression.
    // SSoT: `RenameQueueNotifier` is the only place that decides whether a
    // notification fires.
    RENAME_QUEUE_NOTIFIER.evaluate(&items, &state).await;

    Json(jumbie_shared::types::RenameQueueResponse {
        items,
        total_affected_episodes: total_affected,
        has_failed_renames,
    })
}

/// Resolve a potential collision by appending a disambiguating suffix.
///
/// Shared by `get_rename_queue_detail` and the rename executor so the preview and
/// the execution agree on the path that will actually be used.
pub fn simulate_resolve_collision(
    target: &Path,
    claimed_dsts: &mut std::collections::HashSet<PathBuf>,
    src_paths: &std::collections::HashSet<PathBuf>,
    suffix: jumbie_shared::config::organization::CollisionRenameSuffix,
) -> PathBuf {
    // Two collision levels: (1) two planned moves targeting the same path
    // (claimed_dsts), and (2) a planned move targeting a file already on disk that
    // isn't part of this batch (src_paths). A source file in this batch is NOT a
    // collision — it will be moved away.
    let new_path = crate::paths::next_free_suffixed_path(target, suffix, true, |p| {
        claimed_dsts.contains(p) || (p.exists() && !src_paths.contains(p))
    })
    .unwrap_or_else(|| target.to_path_buf());

    claimed_dsts.insert(new_path.clone());
    new_path
}

/// GET /api/system/rename-queue/:series_id
///
/// Returns the detailed rename plan for a single series, including:
///   - Every planned rename (original → expected path)
///   - Collision mode and how each collision is resolved
///   - Directories that will be emptied (candidates for deletion)
///   - New directories that will be created
pub async fn get_rename_queue_detail(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(series_id): axum::extract::Path<String>,
) -> Result<Json<jumbie_shared::types::RenameQueueItemDetail>, AppError> {
    crate::validation::validate_id(&series_id, "Series ID").map_err(|e| {
        tracing::debug!("get_rename_queue_detail: invalid series ID: {}", e);
        AppError::BadRequest(e.0)
    })?;
    tracing::debug!("get_rename_queue_detail called: series_id={}", series_id);
    let config = state.cfg.read().await;

    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
        .ok_or_else(|| AppError::NotFound(format!("Series {} not found", series_id)))?;

    let series_title = mapping.target_title.clone();

    let db_episodes = state
        .db
        .get_rename_plan_episodes(
            &series_id,
            mapping
                .settings
                .active_mode(config.general.absolute_numbering)
                .is_absolute(),
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;

    let episode_ids: Vec<String> = db_episodes.iter().map(|r| r.episode_id.clone()).collect();
    let episode_parts = state
        .db
        .get_parts_for_series(&episode_ids)
        .await
        .unwrap_or_default();

    // Build shared skip_paths (scan_queue + pending-organization exclusions).
    // SSoT: See build_rename_queue_skip_paths in api/mod.rs.
    let mut skip_paths = crate::api::build_rename_queue_skip_paths(&state).await;

    // Proactive media info scan (see `get_rename_queue`).
    {
        let mut single_map = std::collections::HashMap::new();
        single_map.insert(series_title.clone(), db_episodes.clone());
        skip_paths =
            crate::api::proactively_scan_missing_media_info(&state, &single_map, skip_paths).await;
    }

    let aux_files = crate::file_manager::auxiliary_paths_for_series(&state.db, &series_id).await;
    let plan = match state
        .rename_plan_cache
        .get_or_compute_with_aux(crate::file_manager::RenamePlanInputs {
            series_id: &series_id,
            config: &config,
            mapping: &mapping,
            db_episodes: &db_episodes,
            episode_parts: &episode_parts,
            aux_files: &aux_files,
            skip_paths: &skip_paths,
        })
        .await
    {
        Ok(p) => p,
        Err(crate::file_manager::RenamePlanError::DuplicateTarget(dst)) => {
            return Err(AppError::Internal(anyhow::anyhow!(
                "Rename plan has a duplicate target destination: '{}'",
                dst.display()
            )));
        }
    };

    let mut renames = Vec::with_capacity(plan.len());
    let src_paths: std::collections::HashSet<PathBuf> =
        plan.iter().map(|m| m.src.clone()).collect();

    let mut src_dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    let mut new_dst_dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();

    // Collision diagnosis is delegated to `diagnose_collision` (file_ops.rs), the
    // SSoT shared with execution (move_file_to_target), so preview and execution
    // never drift:
    //   - UnassignedOccupant → no collision flag (execution resolves silently)
    //   - AssignedOccupant   → has_collision=true (user needs to know)
    //   - batch duplicates   → handled by claimed_dsts below
    //   - TempOccupant       → no flag (batch sorts itself out)
    let mut claimed_dsts: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    for m in plan.iter() {
        let mut expected_path = m.dst.clone();
        let mut has_collision = false;

        let is_batch_duplicate = !claimed_dsts.insert(m.dst.clone());

        if is_batch_duplicate {
            has_collision = true;
            match config.organization.collision_handling.as_str() {
                "skip" => {
                    expected_path = m.src.clone();
                }
                "overwrite" => {
                    // Keep target as-is — execution will clobber
                }
                _ => {
                    expected_path = simulate_resolve_collision(
                        &m.dst,
                        &mut claimed_dsts,
                        &src_paths,
                        config.organization.collision_rename_suffix,
                    );
                }
            }
        } else {
            // Check for on-disk collision via the shared SSoT function
            match crate::file_manager::diagnose_collision(
                &m.src,
                &m.dst,
                Some(&src_paths),
                &state.db,
            )
            .await
            {
                crate::file_manager::CollisionKind::UnassignedOccupant => {
                    // Execution will silently move the occupant aside — no flag needed
                }
                crate::file_manager::CollisionKind::AssignedOccupant => {
                    has_collision = true;
                    match config.organization.collision_handling.as_str() {
                        "skip" => {
                            expected_path = m.src.clone();
                        }
                        _ => {
                            // Keep original dst, flag via has_collision
                        }
                    }
                }
                _ => {}
            }
        }

        renames.push(jumbie_shared::types::RenameDetail {
            original: m.src.to_string_lossy().to_string(),
            expected: expected_path.to_string_lossy().to_string(),
            has_collision,
            part_number: m.part_number,
            aux_kind: m.aux_kind,
        });

        if let Some(src_dir) = m.src.parent() {
            src_dirs.insert(src_dir.to_path_buf());
        }

        if let Some(dst_dir) = m.dst.parent()
            && !tokio::fs::try_exists(dst_dir).await.unwrap_or(false)
        {
            new_dst_dirs.insert(dst_dir.to_path_buf());
        }
    }

    // Directories that will be emptied by the planned renames. Empty directories
    // clutter the filesystem, so they are reported for the UI to warn about or the
    // reorganizer to clean up.
    let mut emptied_src_dirs: Vec<String> = Vec::new();

    for src_dir in &src_dirs {
        let is_dir = match tokio::fs::metadata(src_dir).await {
            Ok(m) => m.is_dir(),
            Err(_) => continue,
        };
        if !is_dir {
            continue;
        }

        let mut remaining_files = 0usize;
        let mut read_dir = match tokio::fs::read_dir(src_dir).await {
            Ok(rd) => rd,
            Err(_) => continue,
        };

        loop {
            let entry = match read_dir.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(_) => continue,
            };
            let path = entry.path();
            let is_file = match tokio::fs::metadata(&path).await {
                Ok(m) => m.is_file(),
                Err(_) => continue,
            };
            if !is_file {
                continue;
            }
            // Only video files count: a directory that still has .srt files but no
            // .mkv files is effectively empty for our purposes and can be cleaned up.
            let is_media = path
                .extension()
                .and_then(|x| x.to_str())
                .map(jumbie_shared::media_format::is_video_ext)
                .unwrap_or(false);
            if !is_media {
                continue;
            }
            if !src_paths.contains(&path) {
                remaining_files += 1;
            }
        }

        if remaining_files == 0 {
            emptied_src_dirs.push(src_dir.to_string_lossy().to_string());
        }
    }
    emptied_src_dirs.sort();

    let folder_creations: Vec<String> = new_dst_dirs
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    Ok(Json(jumbie_shared::types::RenameQueueItemDetail {
        series_id,
        series_title,
        renames,
        collision_mode: config.organization.collision_handling.clone(),
        folder_creations,
        folder_deletions: emptied_src_dirs,
    }))
}
