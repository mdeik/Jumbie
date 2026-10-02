//! Organized Series — the "Manage Folders" view.
//!
//! Reconciles two sources of truth — the filesystem (directories on disk) and the
//! series mappings in DB — into a combined view showing which folders are tracked,
//! monitored, or exist only in one source.

use crate::api::modifying_series::{is_series_locked, try_lock_series, unlock_series};
use crate::api::{ActiveOperation, AppState, BatchMoveProgress};
use crate::error::AppError;
use crate::file_manager::move_series_directory;
use axum::Json;
use axum::extract::State;
use jumbie_shared::types::{CompletionStatus, MappingRule, PathOperation, SeriesSettings};
use std::collections::HashMap;
use std::sync::Arc;

/// Small delay before triggering a directory scan after unhiding or adding a
/// series to the library. Prevents rapid toggling from causing cascading scans
/// — if the user hides and unhides repeatedly, only the final state triggers a
/// scan.
const AUTO_SCAN_DELAY_MS: u64 = 500;

/// Cancel any queued or in-progress scan queue entries for a series' files.
/// Called when a series is hidden, removed from management, or deleted.
pub(crate) async fn cancel_series_scans(state: &Arc<AppState>, series_id: &str) {
    let mapping = match state.db.get_series_mapping(series_id).await {
        Ok(Some(m)) => m,
        _ => return,
    };

    let episodes = match state
        .db
        .get_series_episodes_details(
            series_id,
            state.effective_absolute_numbering(&mapping).await,
        )
        .await
    {
        Ok(eps) => eps,
        Err(_) => return,
    };

    let paths: Vec<std::path::PathBuf> = episodes
        .iter()
        .filter_map(|ep| ep.file_path.as_ref().map(std::path::PathBuf::from))
        .collect();

    if paths.is_empty() {
        return;
    }

    tracing::debug!(
        "Cancelling {} scan queue entries for series '{}'",
        paths.len(),
        mapping.target_title
    );
    state.scan_queue.cancel_paths(&paths).await;
}

/// Cancel scans, delete all series data and mapping, then clean up orphaned metadata.
/// Returns `true` if all DB operations succeeded.
///
/// This is the SSoT for "stop tracking a series" — used by batch_move, remove_series,
/// and hidden-series cleanup. If new cleanup steps are needed (e.g. a new DB table),
/// update here in one place.
pub(crate) async fn stop_series_tracking(state: &Arc<AppState>, series_id: &str) -> bool {
    cancel_series_scans(state, series_id).await;

    if let Err(e) = state.db.delete_series_data(series_id).await {
        tracing::error!("Failed to delete series data for {}: {}", series_id, e);
        return false;
    }

    if let Err(e) = state.db.delete_series_mapping(series_id).await {
        tracing::error!("Failed to delete series mapping for {}: {}", series_id, e);
        return false;
    }

    let _ = state.db.cleanup_orphaned_metadata().await;

    // Drop in-memory per-series state (failed-rename hashes, processing flags,
    // cached rename plans) so deleted series can't keep the sidebar indicator
    // lit or leak cache entries. SSoT: see prune_series_in_memory_state.
    crate::api::prune_series_in_memory_state(state, series_id).await;

    tracing::info!("Stopped tracking series {}", series_id);
    true
}

/// Background-scan a series directory after unhiding / adding to library.
/// A short debounce delay is applied before the scan to absorb rapid toggling
/// caused by accidental clicks or impatient double-taps on the toggle.
async fn auto_scan_series_path(state: &Arc<AppState>, series_id: &str) {
    let mapping = match state.db.get_series_mapping(series_id).await {
        Ok(Some(m)) => m,
        _ => {
            tracing::warn!(
                "auto_scan_series_path: series {} not found in DB, skipping scan",
                series_id
            );
            return;
        }
    };

    // SSoT: series path via `paths::mapping_path` (template + policy).
    let path_buf = crate::paths::mapping_path(&mapping, &state.org_config().await);

    if !path_buf.exists() || !path_buf.is_dir() {
        tracing::debug!(
            "auto_scan_series_path: path '{}' does not exist, skipping",
            path_buf.display()
        );
        return;
    }

    // Debounce delay so rapid toggling doesn't trigger cascading scans.
    tokio::time::sleep(std::time::Duration::from_millis(AUTO_SCAN_DELAY_MS)).await;

    // Re-check visibility AFTER the delay — if the user changed their mind,
    // the scan would be wasted I/O.
    if let Ok(Some(current)) = state.db.get_series_mapping(series_id).await {
        if current.hidden_in_library {
            tracing::debug!(
                "auto_scan_series_path: series {} ({}) was re-hidden during debounce, skipping scan",
                current.target_title,
                series_id
            );
            return;
        }
    } else {
        tracing::debug!(
            "auto_scan_series_path: series {} mapping removed during debounce, skipping scan",
            series_id
        );
        return;
    }

    tracing::info!(
        "Auto-scanning series directory after unhide/add: '{}'",
        path_buf.display()
    );

    // `scan_series_directory` (not `scan_directory`) is the SSoT here: the series is
    // already known, so it matches by SXXEXX patterns + season folders without
    // filename-derived series_key matching. It also updates last_known_dir_mtime.
    match crate::scanner::scan_series_directory(&path_buf, &mapping, state).await {
        Ok(count) => {
            if count > 0 {
                tracing::info!(
                    "Auto-scan found {} episodes in directory '{}'",
                    count,
                    path_buf.display()
                );
            }
        }
        Err(e) => {
            tracing::error!("Auto-scan failed for '{}': {}", path_buf.display(), e);
        }
    }
}

/// GET /api/system/organized-series
///
/// Lists every directory under all destination roots, annotated with tracking
/// status, monitor state, and library visibility.
pub async fn get_organized_series(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<jumbie_shared::types::OrganizedSeriesItem>>, AppError> {
    tracing::debug!("get_organized_series called");
    let config = state.cfg.read().await;
    let all_roots = config.organization.destination_roots.clone();
    // SSoT: effective numbering-mode default, needed to resolve each series'
    // active season-override map when deriving expected episode counts.
    let global_absolute = config.general.absolute_numbering;
    // Snapshot the org config: the path synthesis below sanitizes folder names per
    // the illegal-char policy so the listing matches what create/move/organize produce.
    let org_config = config.organization.clone();
    drop(config);

    let mut items = Vec::new();

    let all_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();

    // Per-season counts, fetched once for every series. Series-wide totals are
    // derived by summing the seasons so both views share one source of truth.
    let raw_season_counts: HashMap<String, Vec<jumbie_shared::types::SeasonEpisodeCount>> = state
        .db
        .get_series_season_completion_counts()
        .await
        .unwrap_or_default();

    // SSoT: overlay each season's configured `cell_count` on the expected count so a
    // season with a cell count shows the cells it is expected to hold (out-of-range
    // cells excluded) rather than the episodes currently in the DB. `organized` is
    // left untouched — real files always count, even outside the range.
    let season_counts: HashMap<String, Vec<jumbie_shared::types::SeasonEpisodeCount>> =
        raw_season_counts
            .into_iter()
            .map(|(series_id, seasons)| {
                let seasons = match all_mappings.get(&series_id) {
                    Some(mapping) => seasons
                        .into_iter()
                        .map(|s| jumbie_shared::types::SeasonEpisodeCount {
                            season: s.season,
                            organized: s.organized,
                            expected: mapping.settings.expected_episode_count(
                                &s.season.to_string(),
                                s.expected,
                                global_absolute,
                            ),
                        })
                        .collect(),
                    None => seasons,
                };
                (series_id, seasons)
            })
            .collect();

    let series_totals = |series_id: &str| -> (i32, i32) {
        season_counts
            .get(series_id)
            .map(|seasons| {
                seasons
                    .iter()
                    .fold((0, 0), |(o, e), s| (o + s.organized, e + s.expected))
            })
            .unwrap_or((0, 0))
    };

    // Keyed by absolute path, not series ID: the filesystem scan yields paths, and
    // O(1) path lookups avoid iterating all mappings per scanned directory. A mapping
    // with an empty/missing path could live under any root, so every root+title
    // candidate is generated and the scan matches against them; duplicate paths across
    // roots are resolved by the `remove()` below (first match wins).
    let mut tracked_paths = std::collections::HashMap::new();
    for (series_id, mapping) in &all_mappings {
        if let Some(p) = &mapping.settings.path {
            if p.is_empty() {
                // Empty path — try all roots to find where this series might live
                for root in &all_roots {
                    // SSoT: same policy as creation — the synthesized folder name
                    // matches what the organizer would create.
                    let path = root.path.join(crate::paths::sanitize_title(
                        &mapping.target_title,
                        &org_config,
                    ));
                    // Collision guard: an explicit-path mapping may already have
                    // inserted the same path (e.g. A: "/media/tv/My Show", B: path=None
                    // title "My Show" under "/media/tv"). First claim wins so the
                    // explicit mapping's identity is not overwritten.
                    tracked_paths.entry(path).or_insert_with(|| {
                        (
                            series_id.clone(),
                            mapping.settings.monitor_mode,
                            mapping.hidden_in_library,
                        )
                    });
                }
            } else {
                // Resolve templates so the path matches the real directory name (e.g.
                // "data/Actual Title", not "data/${series}") and the fallback below
                // shows a readable name. SSoT: ${series} is sanitized per the policy.
                let resolved_path =
                    crate::paths::resolve_template(p, &mapping.target_title, &org_config);
                // Normalize so it matches the lookup below (also normalized).
                let canon_key = crate::validation::normalize_path(&resolved_path);
                // Explicit paths are the definitive location and take priority; insert
                // only if no other mapping already claimed this path.
                tracked_paths.entry(canon_key).or_insert_with(|| {
                    (
                        series_id.clone(),
                        mapping.settings.monitor_mode,
                        mapping.hidden_in_library,
                    )
                });
            }
        } else {
            // No path set — try every root, assuming {root}/{target_title}.
            for root in &all_roots {
                // SSoT: same policy as creation — the synthesized folder name
                // matches what the organizer would create.
                let path = root.path.join(crate::paths::sanitize_title(
                    &mapping.target_title,
                    &org_config,
                ));
                // Auto-generated paths from path=None are lowest priority — only
                // insert if no explicit mapping already claimed this key.
                tracked_paths.entry(path).or_insert_with(|| {
                    (
                        series_id.clone(),
                        mapping.settings.monitor_mode,
                        mapping.hidden_in_library,
                    )
                });
            }
        }
    }

    // All roots are scanned, even with no tracked series there: users may have
    // manually created or copied folders, and hiding them would make the "add to
    // library" feature undiscoverable. Roots flagged to exclude subdirs still
    // contribute tracked series, but their untracked folders are skipped below.
    for root in &all_roots {
        if root.path.exists()
            && root.path.is_dir()
            && let Ok(entries) = std::fs::read_dir(&root.path)
        {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type()
                    && ft.is_dir()
                {
                    let path = entry.path();
                    let folder_name = entry.file_name().to_string_lossy().to_string();
                    let absolute_path = path.to_string_lossy().to_string();

                    // Normalize before lookup: `tracked_paths` keys were normalized,
                    // but read_dir returns raw paths that can differ in casing,
                    // separators, or junction resolution (Windows). Using the same
                    // helper keeps the lookup semantic, not byte-level.
                    let lookup_path = crate::validation::normalize_path(&path);
                    if let Some((series_id, monitor_mode, hidden)) = tracked_paths.get(&lookup_path)
                    {
                        let is_monitored =
                            monitor_mode.as_ref() != Some(&jumbie_shared::types::MonitorMode::None);
                        let (total_organized, total_expected) = series_totals(series_id);
                        items.push(jumbie_shared::types::OrganizedSeriesItem {
                            folder_name,
                            absolute_path,
                            is_tracked: true,
                            is_monitored,
                            monitor_mode: *monitor_mode,
                            series_id: Some(series_id.clone()),
                            hidden_in_library: *hidden,
                            selected: false,
                            completion_status: Some(CompletionStatus::compute(
                                total_expected,
                                total_organized,
                            )),
                            total_episodes_expected: total_expected,
                            total_episodes_organized: total_organized,
                            season_counts: season_counts
                                .get(series_id)
                                .cloned()
                                .unwrap_or_default(),
                            locked: is_series_locked(&state, series_id),
                        });
                        // Removed so it isn't processed twice: when a series could
                        // live under multiple roots (empty path), only the root where
                        // it was actually found gets an entry; the others remain and
                        // are handled by the "not found" loop below.
                        tracked_paths.remove(&lookup_path);
                    } else if root.include_subdirs_in_managed {
                        // Untracked — folder exists but no mapping references it.
                        // Skipped for roots whose subdirs are excluded from the
                        // managed listing; tracked series still appear via the
                        // branch above (and the second pass below).
                        items.push(jumbie_shared::types::OrganizedSeriesItem {
                            folder_name,
                            absolute_path,
                            is_tracked: false,
                            is_monitored: false,
                            monitor_mode: None,
                            series_id: None,
                            hidden_in_library: false,
                            selected: false,
                            completion_status: None,
                            total_episodes_expected: 0,
                            total_episodes_organized: 0,
                            season_counts: Vec::new(),
                            locked: false,
                        });
                    }
                }
            }
        }
    }

    // Second pass for tracked series whose directory does NOT exist on disk (yet):
    // a series tracked in DB but deleted or not-yet-created would otherwise be
    // invisible in Manage Folders, letting it never be cleaned up. Also catches
    // remaining candidate paths (empty path → multiple roots) for series that should
    // exist there but don't.
    for (path, (series_id, monitor_mode, hidden)) in tracked_paths {
        let folder_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let absolute_path = path.to_string_lossy().to_string();
        let is_monitored = monitor_mode.as_ref() != Some(&jumbie_shared::types::MonitorMode::None);

        let (total_organized, total_expected) = series_totals(&series_id);
        items.push(jumbie_shared::types::OrganizedSeriesItem {
            folder_name,
            absolute_path,
            is_tracked: true,
            is_monitored,
            monitor_mode,
            series_id: Some(series_id.clone()),
            hidden_in_library: hidden,
            selected: false,
            completion_status: Some(CompletionStatus::compute(total_expected, total_organized)),
            total_episodes_expected: total_expected,
            total_episodes_organized: total_organized,
            season_counts: season_counts.get(&series_id).cloned().unwrap_or_default(),
            locked: is_series_locked(&state, &series_id),
        });
    }

    items.sort_by(|a, b| {
        a.folder_name
            .to_lowercase()
            .cmp(&b.folder_name.to_lowercase())
    });

    Ok(Json(items))
}

/// POST /api/system/organized-series/:series_id/toggle-monitor
///
/// Updates the monitor mode for a single series.
pub async fn toggle_series_monitor(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(series_id): axum::extract::Path<String>,
    Json(payload): Json<jumbie_shared::types::ToggleMonitorPayload>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::validation::validate_id(&series_id, "Series ID").map_err(|e| {
        tracing::debug!("toggle_series_monitor: invalid series ID: {}", e);
        AppError::BadRequest(e.0)
    })?;
    tracing::debug!(
        "toggle_series_monitor called: series_id={}, mode={:?}",
        series_id,
        payload.monitor_mode
    );
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&series_id).await {
        mapping.settings.monitor_mode = Some(payload.monitor_mode);
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(format!("Failed to save: {}", e))))?;
        Ok(Json(serde_json::json!({"status": "ok"})))
    } else {
        Err(AppError::NotFound("Series not found".to_string()))
    }
}

/// POST /api/system/organized-series/:series_id/toggle-visibility
///
/// Hides or shows a series in the main library view.
///
/// Separate from toggle-monitor so a completed series can be hidden while staying
/// monitored for upgrades — one combined toggle would lose that flexibility.
pub async fn toggle_library_visibility(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(series_id): axum::extract::Path<String>,
    Json(payload): Json<jumbie_shared::types::ToggleVisibilityPayload>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::validation::validate_id(&series_id, "Series ID").map_err(|e| {
        tracing::debug!("toggle_library_visibility: invalid series ID: {}", e);
        AppError::BadRequest(e.0)
    })?;
    tracing::debug!(
        "toggle_library_visibility called: series_id={}, hidden={}",
        series_id,
        payload.hidden_in_library
    );
    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&series_id).await {
        let was_hidden = mapping.hidden_in_library;
        mapping.hidden_in_library = payload.hidden_in_library;
        state
            .db
            .upsert_series_mapping(&series_id, &mapping)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(format!("Failed to save: {}", e))))?;

        if was_hidden && !payload.hidden_in_library {
            // Unhidden — auto-scan (debounced) so new files appear without manual work.
            let state_clone = state.clone();
            let sid = series_id.clone();
            tokio::spawn(async move {
                auto_scan_series_path(&state_clone, &sid).await;
            });
        } else if !was_hidden && payload.hidden_in_library {
            // Hidden — cancel queued/in-progress scans: data is preserved for a later
            // unhide, but there's no point spending I/O on a series out of view.
            cancel_series_scans(&state, &series_id).await;
        }

        Ok(Json(serde_json::json!({"status": "ok"})))
    } else {
        Err(AppError::NotFound("Series not found".to_string()))
    }
}

/// POST /api/system/organized-series/batch-edit
///
/// Batch edit for the Manage Folders view. Supported operations:
///   - add_to_library: Track an untracked folder (creates a new series) or unhide a hidden one.
///   - remove_from_library: Mark a tracked series as hidden (soft delete).
///   - delete: Delete mapping + data AND files from disk.
pub async fn batch_edit_organized_series(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::BatchEditOrganizedSeriesPayload>,
) -> Result<Json<serde_json::Value>, AppError> {
    tracing::info!(
        "Batch edit organized series request: {} files, operation: {}",
        payload.paths.len(),
        payload.operation
    );

    let config = state.cfg.read().await;
    let all_roots: Vec<std::path::PathBuf> = config
        .organization
        .destination_roots
        .iter()
        .map(|r| r.path.clone())
        .collect();
    // Cloned because `SeriesPathIndex` sanitizes synthesized paths per the
    // illegal-char policy so the lookup matches the sanitized folders.
    let org_config = config.organization.clone();

    let all_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();

    // SSoT: precomputed path index (built once) — the same candidates the old
    // per-path scan produced, but O(1) per lookup. Shares the illegal-char
    // policy so sanitized synthesized folders match their mappings.
    let series_index = crate::file_manager::series_ops::SeriesPathIndex::build(
        &all_mappings,
        &all_roots,
        &org_config,
    );
    let find_series_id = |path_str: &str| series_index.get(path_str).map(str::to_string);

    let mut db_deletions: Vec<String> = Vec::new(); // series IDs to delete from DB
    let mut fs_deletions: Vec<String> = Vec::new(); // paths to delete from disk

    for path_str in &payload.paths {
        let series_id = find_series_id(path_str);

        match payload.operation.as_str() {
            "add_to_library" => {
                if let Some(id) = series_id {
                    // Unhide rather than re-create: the series may already have episodes,
                    // seasons, and metadata worth preserving.
                    tracing::debug!(
                        "batch_edit add_to_library: unhiding tracked series id={}",
                        id
                    );
                    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
                        mapping.hidden_in_library = false;
                        if let Err(e) = state.db.upsert_series_mapping(&id, &mapping).await {
                            tracing::error!(
                                "batch_edit add_to_library: failed to unhide {} ({}): {}",
                                mapping.target_title,
                                id,
                                e
                            );
                        } else {
                            // Auto-scan the directory (with debounce) so new files
                            // appear after unhiding.
                            let state_clone = state.clone();
                            let sid = id.clone();
                            tokio::spawn(async move {
                                auto_scan_series_path(&state_clone, &sid).await;
                            });
                        }
                    } else {
                        tracing::warn!(
                            "batch_edit add_to_library: series_id={} found by path but mapping not found in DB",
                            id
                        );
                    }
                } else {
                    // Untracked: create a fresh mapping (hidden_in_library=false so it
                    // appears in the library), showing it with no user preferences yet.
                    let safe_path = std::path::Path::new(path_str);
                    let path_exists = safe_path.exists();
                    // Create the directory if missing, matching create_series's
                    // validate_path → create_dir_all. Paths visible in the UI but absent
                    // on disk (or cleaned up) would otherwise be silently skipped, making
                    // batch operations appear to "miss" items that work individually.
                    if !path_exists && let Err(e) = tokio::fs::create_dir_all(safe_path).await {
                        tracing::error!(
                            "batch_edit add_to_library: failed to create directory '{}': {}",
                            path_str,
                            e
                        );
                    }

                    if path_exists || safe_path.exists() {
                        let series_name = safe_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .map(String::from)
                            .unwrap_or_else(|| path_str.clone());

                        let uuid = jumbie_shared::config::generate_uuid();
                        let series_key = series_name.to_lowercase().replace(' ', "_");

                        // No default quality/release profile: the folder has no user
                        // preference yet, and leaving it None ("None" in the edit form)
                        // avoids mismatching the UUID-keyed frontend select.
                        let mapping = MappingRule {
                            target_title: series_name.clone(),
                            name: series_key,
                            series_id: uuid.clone(),
                            quality_profile: None,
                            release_profile: None,
                            qb_category: Some(series_name.clone()),
                            settings: SeriesSettings {
                                path: Some(path_str.clone()),
                                ..Default::default()
                            },
                            ..Default::default()
                        };

                        if let Err(e) = state.db.upsert_series_mapping(&uuid, &mapping).await {
                            tracing::error!(
                                "batch_edit add_to_library: failed to create series for '{}': {}",
                                path_str,
                                e
                            );
                        } else {
                            tracing::info!(
                                "batch_edit add_to_library: created series '{}' with id={}",
                                series_name,
                                uuid
                            );
                            // Synchronous scan (no spawn, no debounce): the user clicked
                            // "Add to Library" and expects to wait, so total_size is written
                            // before the API returns and the first render shows correct sizes.
                            crate::scanner::scan_series_directory(
                                &std::path::PathBuf::from(&path_str),
                                &mapping,
                                &state,
                            )
                            .await
                            .unwrap_or_else(|e| {
                                tracing::warn!(
                                    "Initial scan failed for '{}' (non-fatal): {}",
                                    series_name,
                                    e
                                );
                                0
                            });
                        }
                    } else {
                        tracing::warn!(
                            "batch_edit add_to_library: path '{}' does not exist on disk — skipping",
                            path_str
                        );
                    }
                }
            }
            "remove_from_library" => {
                if let Some(id) = series_id {
                    // Cancel queued/in-progress scans before hiding — avoids
                    // wasting I/O on a series the user is removing from view.
                    cancel_series_scans(&state, &id).await;

                    // It's tracked, hide it
                    if let Ok(Some(mut mapping)) = state.db.get_series_mapping(&id).await {
                        mapping.hidden_in_library = true;
                        let _ = state.db.upsert_series_mapping(&id, &mapping).await;
                    }
                }
            }

            "delete" => {
                if let Some(id) = series_id {
                    if is_series_locked(&state, &id) {
                        tracing::warn!(
                            "Batch edit delete: series {} is currently being modified, skipping",
                            id
                        );
                        continue;
                    }

                    // Cancel scans first: files are about to be deleted, so an
                    // in-progress scan would fail anyway — this avoids the wasted I/O.
                    cancel_series_scans(&state, &id).await;

                    db_deletions.push(id.clone());
                }
                if payload.delete_files {
                    fs_deletions.push(path_str.clone());
                }
            }
            _ => {
                return Err(AppError::BadRequest(format!(
                    "Unknown operation: {}",
                    payload.operation
                )));
            }
        }
    }

    // The config read lock is no longer needed: DB and filesystem work can be slow,
    // and holding it would block any operation needing the config write lock.
    drop(config);

    // `delete_series_data` runs before `delete_series_mapping` because it needs the
    // mapping present to clean per-provider metadata caches. For a single-series
    // delete, `stop_series_tracking()` is the shared SSoT; this loop is its batched
    // equivalent (cancel_scans → delete_data → delete_mapping → cleanup).
    for id in &db_deletions {
        if let Err(e) = state.db.delete_series_data(id).await {
            tracing::error!("Failed to delete series data for {}: {}", id, e);
        }
    }
    for id in &db_deletions {
        let _ = state.db.delete_series_mapping(id).await;
    }
    if let Err(e) = state.db.cleanup_orphaned_metadata().await {
        tracing::warn!(
            "Failed to clean up orphaned metadata after deletions: {}",
            e
        );
    }

    // Filesystem deletions run in their own loop (file deletes can be slow, and a
    // failed FS delete must not skip the DB cleanup already done above).
    for path_str in fs_deletions {
        let path = std::path::Path::new(&path_str);
        if path.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                tracing::error!("Failed to delete directory {}: {}", path_str, e);
            } else {
                tracing::debug!("Deleted directory {}", path_str);
            }
        }
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

// Batch Move — preview + execute

/// POST /api/system/organized-series/batch-move-preview
///
/// Validates a batch move without making any changes.
/// Returns a (source → destination) mapping for every path so the
/// frontend can show a confirmation table.
pub async fn batch_move_series_preview(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::BatchMoveOrganizedSeriesPayload>,
) -> Result<Json<jumbie_shared::types::BatchMovePreviewResponse>, AppError> {
    tracing::info!(
        "Batch move preview: {} paths, target_root={}, operation={:?}",
        payload.paths.len(),
        payload.target_root,
        payload.file_operation
    );

    let config = state.cfg.read().await;
    let all_roots: Vec<std::path::PathBuf> = config
        .organization
        .destination_roots
        .iter()
        .map(|r| r.path.clone())
        .collect();
    // Collision resolution below needs the org policy fields, but the read lock must
    // be released before the DB fetch.
    let org_config = config.organization.clone();
    drop(config);

    let all_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();

    // SSoT: precomputed path index (built once per batch) so every path and
    // every collision-suffix candidate resolves in O(1) instead of re-scanning
    // all mappings per query.
    let series_index = crate::file_manager::series_ops::SeriesPathIndex::build(
        &all_mappings,
        &all_roots,
        &org_config,
    );

    let mut items = Vec::with_capacity(payload.paths.len());
    let mut has_collisions = false;
    // Track destinations claimed within this batch to detect intra-batch collisions
    let mut batch_destinations: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for path_str in &payload.paths {
        let (folder_name, mut destination_path) = resolve_destination_path(
            path_str,
            &payload.target_root,
            &payload.custom_paths,
            &org_config,
        )
        .unwrap_or_else(|| {
            let raw = path_str.clone();
            (raw.clone(), std::path::PathBuf::from(&raw))
        });

        // Look up series_id first
        let series_id = series_index.get(path_str).map(str::to_string);
        let is_tracked = series_id.is_some();

        // Validate that the target root exists when not using custom paths
        if !payload.custom_paths.contains_key(path_str) {
            let root = std::path::Path::new(&payload.target_root);
            if !root.exists() {
                return Err(AppError::BadRequest(format!(
                    "Target root '{}' does not exist on disk",
                    payload.target_root
                )));
            }
        }

        let dest_str = destination_path.to_string_lossy().to_string();

        // Check for collision: external (existing series) or internal (same batch)
        let collision = if !batch_destinations.insert(dest_str.clone()) {
            has_collisions = true;
            Some("Multiple selected series resolve to the same destination. ".to_string())
        } else if destination_path.exists() {
            let occupant_id = series_index.get(&dest_str).map(str::to_string);
            match (&occupant_id, &series_id) {
                (Some(occ_id), Some(sid)) if occ_id != sid => {
                    has_collisions = true;
                    Some("Destination already claimed by another series".to_string())
                }
                (Some(_), Some(_)) => {
                    // Destination is the series' own folder (same-path no-op) —
                    // leave as-is, matching the execution path.
                    None
                }
                (None, _) => {
                    // Orphan folder — derived destinations resolve per the org
                    // collision config (rename → suffixed path, skip → flagged);
                    // explicit custom overrides stay verbatim (historical flag).
                    if !payload.custom_paths.contains_key(path_str) {
                        match crate::paths::resolve_folder_collision(
                            &destination_path,
                            &org_config,
                            |p| series_index.get(&p.to_string_lossy()).is_some(),
                        ) {
                            Ok(resolved) => {
                                destination_path = resolved;
                                None
                            }
                            Err(msg) => {
                                has_collisions = true;
                                Some(msg)
                            }
                        }
                    } else {
                        has_collisions = true;
                        Some("Destination already exists and is not tracked".to_string())
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        items.push(jumbie_shared::types::BatchMovePreviewItem {
            source_path: path_str.clone(),
            destination_path: destination_path.to_string_lossy().to_string(),
            folder_name,
            series_id,
            is_tracked,
            collision,
        });
    }

    Ok(Json(jumbie_shared::types::BatchMovePreviewResponse {
        items,
        has_collisions,
    }))
}

/// Compute the destination path for a source path in a batch move.
/// Returns `None` if the source path has no filename component.
/// The folder name is sanitized via the shared illegal-char policy engine
/// (`sanitize_with_policy` — the SSoT also used for file renames), honoring
/// `illegal_char_policy` and `allow_platform_specific_chars`.
/// The `target_root_str` is trimmed of trailing slashes to prevent
/// "//" in the joined path.
fn resolve_destination_path(
    path_str: &str,
    target_root_str: &str,
    custom_paths: &std::collections::HashMap<String, String>,
    org: &jumbie_shared::config::organization::OrganizationConfig,
) -> Option<(String, std::path::PathBuf)> {
    let source_path = std::path::Path::new(path_str);
    let folder_name = source_path.file_name()?.to_string_lossy().to_string();
    // SSoT: same policy as creation/update — the folder name is a series title.
    let safe_name = crate::paths::sanitize_title(&folder_name, org);
    let destination_path = if let Some(custom) = custom_paths.get(path_str) {
        std::path::PathBuf::from(custom)
    } else {
        // Strip trailing slashes so "root//Name" doesn't occur
        let trimmed = target_root_str.trim_end_matches(['/', '\\']);
        std::path::Path::new(trimmed).join(&safe_name)
    };
    Some((folder_name, destination_path))
}

/// Update the batch progress map with current success/failure counts.
async fn update_batch_progress(
    state: &Arc<AppState>,
    task_id: &str,
    success_count: usize,
    failure_count: usize,
) {
    if let Some(updater) = state.progress_tracker.updater(task_id) {
        updater.report(success_count + failure_count, success_count, failure_count);
    }
}

/// Process a single path in a batch move operation.
/// Returns `Ok(())` on success, or `Err(error_message)` on failure.
///
/// Encapsulates the full per-path pipeline: resolve destination → lookup series →
/// check collisions → acquire lock → update DB → file operation → auto-scan → stop-tracking.
async fn process_batch_move_path(
    state: &Arc<AppState>,
    payload: &jumbie_shared::types::BatchMoveOrganizedSeriesPayload,
    path_str: &str,
    series_index: &crate::file_manager::series_ops::SeriesPathIndex,
) -> Result<(), String> {
    let source_path = std::path::Path::new(path_str);

    // Collision resolution needs the org policy fields; the read lock is released
    // before any async I/O.
    let org_config = {
        let config = state.cfg.read().await;
        config.organization.clone()
    };

    let (_folder_name, mut destination_path) = match resolve_destination_path(
        path_str,
        &payload.target_root,
        &payload.custom_paths,
        &org_config,
    ) {
        Some(result) => result,
        None => {
            return Err(format!(
                "Could not resolve destination path for '{}'",
                path_str
            ));
        }
    };

    let dest_str = destination_path.to_string_lossy().to_string();

    let series_id = match series_index.get(path_str) {
        Some(id) => id.to_string(),
        None => return Err(format!("No series mapping found for '{}'", path_str)),
    };

    // Series-to-series claims are always rejected. A destination that exists and is
    // the series' OWN folder (same-path no-op / Delete) is left untouched; an
    // UNCLAIMED (orphan) folder is resolved per the org collision config for derived
    // destinations (rename/skip/overwrite), while explicit overrides are honored verbatim.
    if destination_path.exists() {
        let occupant_id = series_index.get(&dest_str);
        if let Some(ref occ_id) = occupant_id
            && &series_id != occ_id
        {
            return Err(format!(
                "Destination '{}' is already claimed by another series",
                destination_path.display()
            ));
        }
        if occupant_id.is_none() && !payload.custom_paths.contains_key(path_str) {
            destination_path =
                crate::paths::resolve_folder_collision(&destination_path, &org_config, |p| {
                    series_index.get(&p.to_string_lossy()).is_some()
                })?;
        }
    }

    // Re-derive after collision resolution so the stored mapping matches the
    // folder that will actually be used.
    let dest_str = destination_path.to_string_lossy().to_string();

    if !try_lock_series(state, &series_id).await {
        return Err(format!(
            "Series {} is currently being modified by another operation (e.g. batch move)",
            series_id
        ));
    }

    let mut mapping = match state.db.get_series_mapping(&series_id).await {
        Ok(Some(m)) => m,
        _ => {
            unlock_series(state, &series_id).await;
            return Err(format!("Series mapping not found for '{}'", series_id));
        }
    };
    let old_resolved = crate::paths::mapping_path(&mapping, &org_config);

    // Skip if source and destination are effectively the same path.
    // Delete always executes — the user explicitly wants to remove the source
    // even if the DB path doesn't change.
    if old_resolved == destination_path
        && !payload.stop_tracking
        && payload.file_operation != PathOperation::Delete
    {
        unlock_series(state, &series_id).await;
        return Ok(());
    }

    // The DB path is updated BEFORE the files move so the mapping never points at a
    // location the files have left. Delete always executes even if the DB path is
    // unchanged.
    mapping.settings.path = Some(dest_str.clone());
    if let Err(e) = state.db.upsert_series_mapping(&series_id, &mapping).await {
        unlock_series(state, &series_id).await;
        return Err(format!("Failed to update database path: {}", e));
    }

    // Invalidate rename plan cache — series path changed

    // Migrate tracked paths from the old directory to the new one. Move/Copy rewrite
    // them in place (`file_paths.id` is stable, so the `episode_files` associations
    // follow); Delete and leave-in-place clear them, mirroring the single-series path
    // update. Runs before the physical operation so the mapping never references files
    // at their old location.
    if !old_resolved.as_os_str().is_empty() && old_resolved != destination_path {
        let outcome = match payload.file_operation {
            PathOperation::Move | PathOperation::Copy => {
                state
                    .db
                    .update_series_paths(&old_resolved, &destination_path)
                    .await
            }
            PathOperation::Delete | PathOperation::DoNothing => {
                let cleared = state.db.clear_series_paths(&old_resolved).await;
                // Cleared associations can change `has_file`, so re-evaluate monitoring.
                crate::source_processor::reapply_monitor_for_series(
                    &state.db,
                    &series_id,
                    false,
                    state.effective_absolute_numbering(&mapping).await,
                )
                .await;
                cleared
            }
        };
        if let Err(e) = outcome {
            tracing::warn!(
                "Failed to update tracked paths for series {} ({}): {}",
                mapping.target_title,
                series_id,
                e
            );
        }
    }

    // Invalidate rename plan cache — episode paths changed

    // Step 2: execute the file operation (delegated to the SSoT).
    let should_scan = match &payload.file_operation {
        PathOperation::Move | PathOperation::Copy => true,
        PathOperation::Delete | PathOperation::DoNothing => false,
    };

    let effective_source = if old_resolved.exists() {
        old_resolved.clone()
    } else {
        source_path.to_path_buf()
    };

    if let Err(e) = move_series_directory(
        state,
        &effective_source,
        &destination_path,
        payload.file_operation.clone(),
        &state.shutdown_token,
    )
    .await
    {
        unlock_series(state, &series_id).await;
        return Err(format!("File operation failed: {}", e.message()));
    }

    // Step 3: auto-scan if files were placed at the destination.
    if should_scan && destination_path.exists() && !payload.stop_tracking {
        let sc = state.clone();
        let sid = series_id.clone();
        let dest = destination_path.clone();
        tokio::spawn(async move {
            if let Ok(Some(m)) = sc.db.get_series_mapping(&sid).await {
                let _ = crate::scanner::scan_series_directory(&dest, &m, &sc).await;
            }
        });
    }

    // Step 4: stop tracking if requested.
    if payload.stop_tracking {
        let ok = stop_series_tracking(state, &series_id).await;
        unlock_series(state, &series_id).await;
        if ok {
            Ok(())
        } else {
            Err("Failed to stop tracking series".to_string())
        }
    } else {
        unlock_series(state, &series_id).await;
        Ok(())
    }
}

/// POST /api/system/organized-series/batch-move
///
/// Executes a batch move. The server always appends the series folder name
/// to `target_root` (e.g. target_root=/media/tv + series "My Show" →
/// /media/tv/My Show). This prevents API callers from accidentally dumping
/// all series into a single flat directory. Use `custom_paths` for
/// per-series overrides that ignore both the root and the folder-name append.
pub async fn batch_move_series(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::BatchMoveOrganizedSeriesPayload>,
) -> Result<Json<jumbie_shared::types::BatchMoveResponse>, AppError> {
    tracing::info!(
        "Batch move: {} paths, target_root={}, operation={:?}",
        payload.paths.len(),
        payload.target_root,
        payload.file_operation
    );

    // Pre-flight: reject intra-batch collisions. If two series would end up at the
    // same destination (after sanitization) the batch is rejected outright — HashMap
    // iteration makes their arrival order non-deterministic, so it can't be resolved.
    {
        let org_config = {
            let config = state.cfg.read().await;
            config.organization.clone()
        };
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for path_str in &payload.paths {
            if let Some(custom) = payload.custom_paths.get(path_str) {
                if custom.trim().is_empty() {
                    return Err(AppError::BadRequest(format!(
                        "Custom path for '{}' is empty",
                        path_str
                    )));
                }
                // Custom paths bypass the sanitization and root-append,
                // so use the raw custom path for dedup.
                if !seen.insert(custom.clone()) {
                    return Err(AppError::BadRequest(format!(
                        "Batch move rejected: multiple series resolve to the same destination '{}'. ",
                        custom
                    )));
                }
            } else if let Some((_name, dest)) = resolve_destination_path(
                path_str,
                &payload.target_root,
                &payload.custom_paths,
                &org_config,
            ) {
                let dest_str = dest.to_string_lossy().to_string();
                if !seen.insert(dest_str.clone()) {
                    return Err(AppError::BadRequest(format!(
                        "Batch move rejected: multiple series resolve to the same destination '{}'. ",
                        dest_str
                    )));
                }
            }
        }
    }

    // SSoT: starting the batch through the ProgressTracker.
    let total = payload.paths.len();
    let (task_id, handle) = state
        .progress_tracker
        .start(crate::api::OperationType::BatchMove, total);

    let state_clone = state.clone();
    let payload_clone = payload;
    let task_id_clone = task_id.clone();

    tokio::spawn(async move {
        let config = state_clone.cfg.read().await;
        let all_roots: Vec<std::path::PathBuf> = config
            .organization
            .destination_roots
            .iter()
            .map(|r| r.path.clone())
            .collect();
        // The index and collision resolution need the org policy fields; the read
        // lock is released before any async I/O.
        let org_config = config.organization.clone();
        drop(config);

        let all_mappings = state_clone
            .db
            .get_all_series_mappings()
            .await
            .unwrap_or_default();

        // SSoT: precomputed path index (built once for the whole batch) — the
        // per-path and per-collision-candidate lookups are O(1).
        let series_index = crate::file_manager::series_ops::SeriesPathIndex::build(
            &all_mappings,
            &all_roots,
            &org_config,
        );

        let mut success_count = 0usize;
        let mut failure_count = 0usize;
        let mut errors: Vec<String> = Vec::new();

        for path_str in &payload_clone.paths {
            match process_batch_move_path(&state_clone, &payload_clone, path_str, &series_index)
                .await
            {
                Ok(_) => {
                    success_count += 1;
                }
                Err(err_msg) => {
                    failure_count += 1;
                    if errors.len() < 20 {
                        errors.push(err_msg);
                    }
                }
            }

            update_batch_progress(&state_clone, &task_id_clone, success_count, failure_count).await;
        }

        // SSoT: complete the handle to set finished=true, finished_at, and
        // normalise completed = total.
        handle.complete(success_count, failure_count, errors);

        tracing::info!(
            "Batch move {} completed: {} success, {} failures",
            task_id_clone,
            success_count,
            failure_count,
        );
    });

    Ok(Json(jumbie_shared::types::BatchMoveResponse {
        results: vec![],
        success_count: 0,
        failure_count: 0,
        task_id: Some(task_id),
    }))
}

/// GET /api/system/organized-series/batch-move/{task_id}/status
///
/// Returns the current progress of a batch-move operation.
pub async fn batch_move_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Result<Json<BatchMoveProgress>, AppError> {
    crate::validation::validate_not_empty(&task_id, "Task ID").map_err(|e| {
        tracing::debug!("batch_move_status: invalid task ID: {}", e);
        AppError::BadRequest(e.0)
    })?;
    match state.progress_tracker.get(&task_id) {
        Some(p) => Ok(Json(p)),
        None => Err(AppError::NotFound("Batch move task not found".to_string())),
    }
}

// Active Operations — visibility for multi-user scenarios

/// Exposes currently in-progress operations so other users/sessions can see
/// what's happening and understand why some series are locked.
///
/// Returns a list of active batch moves and a list of currently-locked series IDs.
pub async fn get_active_operations(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let active_ops: Vec<ActiveOperation> =
        state.progress_tracker.all_active(std::time::Instant::now());

    let locked_series: Vec<String> = {
        let ms = state.modifying_series.read().await;
        ms.iter().cloned().collect()
    };

    Json(serde_json::json!({
        "batch_moves": active_ops,
        "locked_series": locked_series,
    }))
}
