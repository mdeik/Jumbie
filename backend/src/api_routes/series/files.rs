use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use jumbie_shared::formatting::LabelStyle;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::api::AppState;
use crate::api::modifying_series::{try_lock_series, unlock_series};
use crate::db::episodes::assign::AssignFileToEpisodeParams;
use crate::models::activity::ActivityEvent;
use crate::utils::path_utils::PathExt;
use jumbie_shared::types::ActivityType::*;
use jumbie_shared::types::EpisodeStatus;
use walkdir::WalkDir;

/// Shared type alias for inline async handlers that return a status-code result.
type HandlerResult = Pin<Box<dyn Future<Output = Result<StatusCode, (StatusCode, String)>> + Send>>;

/// Returns all video files found on disk for a series, annotated with their
/// current episode assignment (if any).
///
/// Deliberately a "scan disk" operation rather than a DB query, so unassigned
/// files are shown alongside assigned ones for a complete inventory: assignment
/// state is built into a path→episode map from the DB, then files are discovered by
/// filesystem walk and annotated where they match.
///
/// Stale DB references (files assigned in DB but gone from disk) are NOT cleaned
/// here — a background Stale File Cleanup task handles that on an interval.
pub async fn get_series_files(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<jumbie_shared::types::SeriesFileViewModel>>, (StatusCode, String)> {
    tracing::debug!("get_series_files called: id={}", id);
    let mapping = state
        .db
        .get_series_mapping(&id)
        .await
        .map_err(|e| {
            tracing::error!("get_series_files DB error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| {
            tracing::debug!("get_series_files: series {} not found", id);
            (StatusCode::NOT_FOUND, format!("Series {} not found", id))
        })?;

    let mut files_view = Vec::new();

    // Build a lookup table of normalized path → episode assignment so symlink and
    // separator differences still match WalkDir results. The original stored path is
    // kept alongside it for display.
    let db_episodes: Vec<crate::db::EpisodeDetailRow> = state
        .db
        .get_series_episodes_details(&id, state.effective_absolute_numbering(&mapping).await)
        .await
        .map_err(|e: anyhow::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    struct PathMappingInfo {
        episode_id: String,
        season: Option<String>,
        episodes: Vec<i32>,
        parts: Vec<i32>,
        /// The DB path exactly as stored, reported to the UI.
        path: PathBuf,
    }

    let mut path_map: HashMap<PathBuf, PathMappingInfo> = HashMap::new();

    let episode_ids: Vec<String> = db_episodes.iter().map(|e| e.episode_id.clone()).collect();
    let parts_for_series = state
        .db
        .get_parts_for_series(&episode_ids)
        .await
        .unwrap_or_default();

    for db_row in &db_episodes {
        if let Some(ref p) = db_row.file_path {
            let p_buf = PathBuf::from(p);
            let key = crate::validation::normalize_path(&p_buf);
            let info = path_map.entry(key).or_insert_with(|| PathMappingInfo {
                episode_id: db_row.episode_id.clone(),
                season: db_row.season.map(|s| s.to_string()),
                episodes: Vec::new(),
                parts: Vec::new(),
                path: p_buf.clone(),
            });
            if !info.episodes.contains(&db_row.episode) {
                info.episodes.push(db_row.episode);
            }
        }
    }

    for (ep_id, part_row) in parts_for_series {
        if let Some(db_row) = db_episodes.iter().find(|e| e.episode_id == ep_id) {
            let p_buf = PathBuf::from(&part_row.file_path);
            let key = crate::validation::normalize_path(&p_buf);
            let info = path_map.entry(key).or_insert_with(|| PathMappingInfo {
                episode_id: ep_id.clone(),
                season: db_row.season.map(|s| s.to_string()),
                episodes: Vec::new(),
                parts: Vec::new(),
                path: p_buf.clone(),
            });
            if !info.episodes.contains(&db_row.episode) {
                info.episodes.push(db_row.episode);
            }
            if !info.parts.contains(&part_row.part_number) {
                info.parts.push(part_row.part_number);
            }
        }
    }

    // Linked (non-primary) video attachments — language/version variants kept
    // alongside an episode's main file — count as assigned so the file list shows
    // them against their episode rather than as unassigned.
    for (ep_id, path) in state
        .db
        .get_linked_files_for_series(&id)
        .await
        .unwrap_or_default()
    {
        if let Some(db_row) = db_episodes.iter().find(|e| e.episode_id == ep_id) {
            let p_buf = PathBuf::from(&path);
            let key = crate::validation::normalize_path(&p_buf);
            let info = path_map.entry(key).or_insert_with(|| PathMappingInfo {
                episode_id: ep_id.clone(),
                season: db_row.season.map(|s| s.to_string()),
                episodes: Vec::new(),
                parts: Vec::new(),
                path: p_buf.clone(),
            });
            if !info.episodes.contains(&db_row.episode) {
                info.episodes.push(db_row.episode);
            }
        }
    }

    for info in path_map.values_mut() {
        info.episodes.sort_unstable();
        info.parts.sort_unstable();
    }

    // Helper shared between on-disk files and missing entries so that the
    // assigned_header formatting (S##E##, multi-episode ranges, part suffixes)
    // is defined in exactly one place (SSoT).
    let build_header = |info: &PathMappingInfo| -> String {
        let s_str = info
            .season
            .as_deref()
            .and_then(|v| v.parse::<u32>().ok())
            .map(|n| jumbie_shared::formatting::fmt_season(n as i32, LabelStyle::Short))
            .unwrap_or("?".to_string());

        let ep_str = if info.episodes.len() > 1 {
            let first = info.episodes.first().unwrap();
            let last = info.episodes.last().unwrap();
            let mut is_contiguous = true;
            for i in 1..info.episodes.len() {
                if info.episodes[i] != info.episodes[i - 1] + 1 {
                    is_contiguous = false;
                    break;
                }
            }
            if is_contiguous {
                format!(
                    "{}-{}",
                    jumbie_shared::formatting::fmt_episode(*first, LabelStyle::Short),
                    jumbie_shared::formatting::fmt_episode(*last, LabelStyle::Short)
                )
            } else {
                info.episodes
                    .iter()
                    .map(|e| jumbie_shared::formatting::fmt_episode(*e, LabelStyle::Short))
                    .collect::<Vec<_>>()
                    .join(",")
            }
        } else if let Some(first) = info.episodes.first() {
            jumbie_shared::formatting::fmt_episode(*first, LabelStyle::Short)
        } else {
            "E??".to_string()
        };

        let part_str = if !info.parts.is_empty() {
            format!(
                " Part {}",
                info.parts
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        } else {
            String::new()
        };

        format!("{}{}{}", s_str, ep_str, part_str)
    };

    // Anonymous helper closure to avoid duplicating the WalkDir → filter → assign loop
    // across both the series directory and the download directory below.
    let process_dir =
        |root: &std::path::Path, files_vec: &mut Vec<jumbie_shared::types::SeriesFileViewModel>| {
            for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if let Some(kind) = jumbie_shared::media_format::file_kind_for_ext(&ext_str) {
                        // Video assignment comes from `path_map`; auxiliary sidecars
                        // are resolved from the file_paths links below.
                        let mut assigned_id = None;
                        let mut assigned_header = None;
                        if kind == jumbie_shared::media_format::FileKind::Video {
                            let check_path = crate::validation::normalize_path(path);
                            if let Some(info) = path_map.get(&check_path) {
                                assigned_id = Some(info.episode_id.clone());
                                assigned_header = Some(build_header(info));
                            }
                        }

                        files_vec.push(jumbie_shared::types::SeriesFileViewModel {
                            path: path.to_string_lossy().to_string(),
                            filename: path.file_name_string(),
                            size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                            assigned_id,
                            assigned_header,
                            kind,
                        });
                    }
                }
            }
        };

    // SSoT: series path via `paths::mapping_path` (template + policy).
    let series_path = crate::paths::mapping_path(&mapping, &state.org_config().await);
    if series_path.exists() {
        process_dir(&series_path, &mut files_view);
    }

    // 2. Assigned files from their stored DB paths. The episode/part `file_path`
    //    is the SSoT: a file still in the download directory (linked but not yet
    //    organized) appears here, and a moved or deleted file is reflected
    //    automatically without walking any download directory.
    for (key, info) in &path_map {
        let is_video = key
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .is_some_and(|e| jumbie_shared::media_format::is_video_ext(e.as_str()));
        if !is_video {
            continue;
        }
        let Ok(meta) = std::fs::metadata(key) else {
            continue; // gone from disk — not shown
        };
        if !meta.is_file() {
            continue;
        }
        files_view.push(jumbie_shared::types::SeriesFileViewModel {
            // Report the stored path verbatim: canonicalizing only for the match
            // above rewrites separators/case on Windows, which would not match the
            // path shown to (and assigned by) the user.
            path: info.path.to_string_lossy().to_string(),
            filename: info.path.file_name_string(),
            size: meta.len(),
            assigned_id: Some(info.episode_id.clone()),
            assigned_header: Some(build_header(info)),
            kind: jumbie_shared::media_format::FileKind::Video,
        });
    }

    // 2b. Auxiliary sidecars (subtitles / nfo) attached to episodes. Their real
    //     assignment is the `file_paths` link, not the episode's playable path.
    for (episode_id, aux_files) in state
        .db
        .get_auxiliary_files_for_series(&id)
        .await
        .unwrap_or_default()
    {
        let header = db_episodes
            .iter()
            .find(|e| e.episode_id == episode_id)
            .map(|e| {
                build_header(&PathMappingInfo {
                    episode_id: episode_id.clone(),
                    season: e.season.map(|s| s.to_string()),
                    episodes: vec![e.episode],
                    parts: Vec::new(),
                    path: PathBuf::new(),
                })
            });
        for aux in aux_files {
            let path = PathBuf::from(&aux.path);
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            files_view.push(jumbie_shared::types::SeriesFileViewModel {
                filename: path.file_name_string(),
                size: meta.len(),
                assigned_id: Some(episode_id.clone()),
                assigned_header: header.clone(),
                path: aux.path,
                kind: aux.kind,
            });
        }
    }

    // 3. Files kept for review (kept unneeded / unmatched leftovers) for THIS
    //    series, from the durable `unmatched_files` table. Video files only,
    //    matching the series-directory pass above; a path that no longer exists
    //    is skipped (the table prunes it after a grace period).
    for path_str in state
        .db
        .get_unmatched_files_for_series(&id)
        .await
        .unwrap_or_default()
    {
        let path = PathBuf::from(&path_str);
        let Some(kind) = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .and_then(|e| jumbie_shared::media_format::file_kind_for_ext(&e))
        else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            continue; // tracked path gone from disk — not shown
        };
        if !meta.is_file() {
            continue;
        }
        files_view.push(jumbie_shared::types::SeriesFileViewModel {
            filename: path.file_name_string(),
            size: meta.len(),
            assigned_id: None,
            assigned_header: None,
            path: path_str,
            kind,
        });
    }

    // Deduplicate: an unassigned fingerprint may point at a file the series-dir
    // walk already listed. Prefer the entry that carries an assignment.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut deduped: Vec<jumbie_shared::types::SeriesFileViewModel> =
        Vec::with_capacity(files_view.len());
    for entry in files_view {
        match seen.get(&entry.path) {
            Some(&idx) => {
                if deduped[idx].assigned_id.is_none() && entry.assigned_id.is_some() {
                    deduped[idx] = entry;
                }
            }
            None => {
                seen.insert(entry.path.clone(), deduped.len());
                deduped.push(entry);
            }
        }
    }

    tracing::debug!(
        "get_series_files completed for {} ({}): {} files found",
        mapping.target_title,
        id,
        deduped.len()
    );

    // Deterministic base order by natural filename; the user's selected column
    // sort is applied by the UI. No kind/type grouping.
    deduped.sort_by(|a, b| jumbie_shared::formatting::natural_cmp(&a.filename, &b.filename));
    Ok(Json(deduped))
}

/// Whether `path` is an auxiliary sidecar (subtitle/nfo) rather than a playable
/// video. Auxiliary files are link-only: they attach to an episode but never
/// become its playable file and must not run through the video organize/rename
/// pipeline (which would overwrite `episode_files`).
fn is_auxiliary_path(path: &std::path::Path) -> bool {
    jumbie_shared::media_format::is_auxiliary_path(path)
}

/// Single-file manual assignment, called from the UI when a user drags a file onto
/// an episode slot and clicks "Assign".
///
/// Design notes:
///   • Single-digit episode strings ("5") and ranges ("1-3") are accepted; extracting
///     the range here rather than in the UI makes the API the authority on range syntax.
///   • Part detection is filename-based: if both files carry a part suffix (-pt1,
///     -cd2), they route to a multipart association instead of overwriting the main file.
///   • Fingerprinting is spawned in the background so the API returns 200 before
///     hashing completes (hashing multi-GB files takes seconds).
pub fn assign_series_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::AssignFilePayload>,
) -> HandlerResult {
    Box::pin(async move {
        tracing::info!(
            "Manual assignment request for series {}: '{}' -> S{}E{}",
            id,
            payload.path,
            payload.season,
            payload.episode
        );

        // Path traversal guard: reject any path containing ".." regardless of OS.
        // This is a belt-and-suspenders check on top of whatever the OS would do,
        // because we're about to pass the path to the DB and filesystem.
        //
        // Path length check: some filesystems (e.g. ext4) have a 255-byte limit
        // on individual filename components, so extremely long paths can't exist.
        if payload.path.len() > 4096 {
            tracing::warn!(
                "assign_series_file: path exceeds 4096 bytes (length={}): '{}'",
                payload.path.len(),
                &payload.path[..std::cmp::min(payload.path.len(), 200)]
            );
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Invalid path: length exceeds maximum ({} bytes)",
                    payload.path.len()
                ),
            ));
        }
        // Path traversal guard: use the shared SSoT function so traversal rules
        // are defined in exactly one place (validation::contains_path_traversal).
        if crate::validation::contains_path_traversal(&payload.path) {
            tracing::warn!(
                "assign_series_file: path traversal detected in path (length={}): '{}'",
                payload.path.len(),
                &payload.path[..std::cmp::min(payload.path.len(), 200)]
            );
            return Err((
                StatusCode::BAD_REQUEST,
                "Invalid path: directory traversal is not allowed".to_string(),
            ));
        }

        let src_path = PathBuf::from(&payload.path);
        if !src_path.exists() {
            tracing::debug!(
                "assign_series_file: source file not found: '{}'",
                payload.path
            );
            return Err((StatusCode::NOT_FOUND, "Source file not found".to_string()));
        }

        // Auxiliary sidecars (subtitle/nfo) are link-only. Ensure a `file_paths`
        // row exists first so the auxiliary link has a target
        // (`link_file_episode_as` only UPDATEs an existing row), then skip the
        // video organize/rename pipeline below — running a sidecar through it would
        // move the nfo/subtitle to the episode's video path and overwrite
        // `episode_files`, unlinking the episode's actual video.
        let is_auxiliary = is_auxiliary_path(&src_path);
        if is_auxiliary {
            crate::scanner::scan::hash_first(&state, &src_path).await;
        }

        crate::validation::validate_season_number(&payload.season).map_err(|e| {
            tracing::debug!(
                "assign_series_file: invalid season '{}': {}",
                payload.season,
                e
            );
            (StatusCode::BAD_REQUEST, format!("Invalid season: {}", e))
        })?;

        // Resolve the season number once. The generated episode IDs and the
        // stored `season` column must be derived from this exact value so they
        // can never disagree.
        let season_num =
            jumbie_shared::mapping::parse_season_num(&payload.season).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid season number: {}", payload.season),
                )
            })?;

        // Parse episode range: "5" or "1-3"
        let (ep_start, ep_end) = {
            let s = payload.episode.trim();
            if let Some((a, b)) = s.split_once('-') {
                let start = a.trim().parse::<i32>().map_err(|_| {
                    tracing::debug!("assign_series_file: invalid episode range start: {}", s);
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Invalid episode range: {}", s),
                    )
                })?;
                let end = b.trim().parse::<i32>().map_err(|_| {
                    tracing::debug!("assign_series_file: invalid episode range end: {}", s);
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Invalid episode range: {}", s),
                    )
                })?;
                if end < start {
                    tracing::debug!(
                        "assign_series_file: episode range end < start: {}-{}",
                        start,
                        end
                    );
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "Episode range end must be >= start".to_string(),
                    ));
                }
                crate::validation::validate_episode_number(start).map_err(|e| {
                    tracing::debug!(
                        "assign_series_file: invalid episode number {}: {}",
                        start,
                        e
                    );
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Invalid episode number: {}", e),
                    )
                })?;
                crate::validation::validate_episode_number(end).map_err(|e| {
                    tracing::debug!("assign_series_file: invalid episode number {}: {}", end, e);
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Invalid episode number: {}", e),
                    )
                })?;
                (start, end)
            } else {
                let ep = s.parse::<i32>().map_err(|_| {
                    tracing::debug!("assign_series_file: invalid episode number: {}", s);
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Invalid episode number: {}", s),
                    )
                })?;
                crate::validation::validate_episode_number(ep).map_err(|e| {
                    tracing::debug!("assign_series_file: invalid episode number {}: {}", ep, e);
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Invalid episode number: {}", e),
                    )
                })?;
                (ep, ep)
            }
        };

        // Detect part suffix in the incoming filename (e.g. -pt2, -cd1, -part1)
        let filename_str = src_path.file_name_string();
        let incoming_part = crate::utils::detect_part_number(&filename_str);
        let (series_title, series_id, numbering_mode, episode_ids) = {
            let mapping = state
                .db
                .get_series_mapping(&id)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .ok_or((StatusCode::NOT_FOUND, "Series not found".to_string()))?;
            let title = mapping.target_title.clone();
            let sid = mapping.series_id.clone();
            let nm = state.effective_absolute_numbering(&mapping).await as i32;
            let global_absolute = {
                let c = state.cfg.read().await;
                c.general.absolute_numbering
            };
            let ids: Vec<(i32, String)> = (ep_start..=ep_end)
                .map(|ep| {
                    mapping
                        .get_episode_id(&season_num.to_string(), ep, global_absolute)
                        .map(|id| (ep, id))
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            (title, sid, nm, ids)
        };

        if !try_lock_series(&state, &series_id).await {
            return Err((
                StatusCode::CONFLICT,
                "Series is currently being modified by another operation (e.g. batch move or reorganize)"
                    .to_string(),
            ));
        }

        let original_path_str = src_path.to_string_lossy().to_string();
        let target_episode_ids: Vec<String> = episode_ids
            .iter()
            .map(|(_, id): &(i32, String)| id.clone())
            .collect();

        for (episode_num, episode_id) in &episode_ids {
            // The episode's current primary file (None when unassigned).
            let existing_path: Option<String> = state
                .db
                .get_episode_file_path(episode_id)
                .await
                .unwrap_or(None)
                .filter(|p| !p.is_empty());

            // If both the existing and incoming files are parts (e.g. -cd1 / -cd2),
            // route them to multipart associations rather than overwriting the episode's
            // single file.
            let existing_part = existing_path.as_deref().and_then(|existing| {
                crate::utils::detect_part_number(&PathBuf::from(existing).file_name_string())
            });

            if let (Some(part_num), Some(_)) = (incoming_part, existing_part) {
                // A part must be fingerprinted so its content (media_info, release_info)
                // resolves through the details join — the multipart association itself
                // lives in `episode_files`.
                crate::scanner::scan::ensure_hashed(&state, &src_path, false).await;

                let incoming_size = std::fs::metadata(&src_path).map(|m| m.len() as i64).ok();

                // One transaction: ensure the parent row, disown any episode that
                // already held this part file (SSoT collision), and register the
                // incoming part (migrating any implicit primary into a part).
                match state
                    .db
                    .register_multipart_assignment(crate::db::ownership::MultipartAssignment {
                        episode_id,
                        series_id: &series_id,
                        season: season_num,
                        episode: *episode_num,
                        numbering_mode,
                        file_path: &original_path_str,
                        part_number: part_num,
                        size: incoming_size,
                        target_episode_ids: Some(target_episode_ids.as_slice()),
                    })
                    .await
                {
                    Ok(displaced) => {
                        for old_id in &displaced {
                            crate::source_processor::reapply_monitor_for_episode(
                                &state.db,
                                &series_id,
                                old_id,
                                false,
                                {
                                    let c = state.cfg.read().await;
                                    c.general.absolute_numbering
                                },
                            )
                            .await;
                        }
                    }
                    Err(e) => {
                        unlock_series(&state, &series_id).await;
                        return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
                    }
                }
            } else {
                // Normal single-file assignment
                // Use empty title when no real episode title is available from metadata
                // so the `{title}` template variable resolves to nothing instead of "Episode X".
                let episode_title = String::new();
                match state
                    .db
                    .assign_file_to_episode(AssignFileToEpisodeParams {
                        episode_id,
                        series_id: &series_id,
                        series_title: &series_title,
                        season: season_num,
                        episode: *episode_num,
                        file_path: &original_path_str,
                        episode_title: &episode_title,
                        all_target_episode_ids: Some(&target_episode_ids),
                        numbering_mode,
                        only_unassigned: false,
                    })
                    .await
                {
                    Ok(old_ids) => {
                        // Re-evaluate episodes that lost their file to this reassign.
                        for old_id in &old_ids {
                            crate::source_processor::reapply_monitor_for_episode(
                                &state.db,
                                &series_id,
                                old_id,
                                false,
                                {
                                    let c = state.cfg.read().await;
                                    c.general.absolute_numbering
                                },
                            )
                            .await;
                        }
                    }
                    Err(e) => {
                        unlock_series(&state, &series_id).await;
                        return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
                    }
                }
            }
        }

        // A language/version variant attaches alongside the episode's primary file; it
        // must not run the organize/rename pipeline, which would move it onto the
        // primary's canonical path and clobber it. An assignment the collision strategy
        // declined (nothing attached) must not be organized either.
        let mut attach_as_variant = false;
        let mut attached = false;
        for (_, episode_id) in &episode_ids {
            if state
                .db
                .has_variant_occupant(episode_id, &original_path_str)
                .await
                .unwrap_or(false)
            {
                attach_as_variant = true;
            }
            if state
                .db
                .episode_holds_any_path(episode_id, &original_path_str)
                .await
                .unwrap_or(false)
            {
                attached = true;
            }
        }

        // After the DB assignment the file is moved to its correct destination.
        // Renaming (filename template) is controlled by `should_rename_episodes` AND
        // `auto_apply_renames` — see `resolve_episode_target_path` in policy.rs (SSoT).
        // Part files (-pt2, -cd1) are handled by the rename queue instead, which
        // re-points the `episode_files` part associations during organization.
        let organized_path: Option<PathBuf> = if !is_auxiliary
            && attached
            && !attach_as_variant
            && incoming_part.is_none()
            && let Some(organizer) = &state.organizer
        {
            // Skip if the file is currently being scanned (TOCTOU guard)
            if !state.scan_queue.contains(&src_path).await {
                let mut last_result: Option<PathBuf> = None;
                for (episode_num, _) in &episode_ids {
                    match organizer
                        .organize_single_assigned_file(
                            &src_path,
                            &series_id,
                            &payload.season,
                            *episode_num,
                        )
                        .await
                    {
                        Ok(Some(p)) => last_result = Some(p),
                        Err(e) => {
                            tracing::warn!(
                                "Failed to organize assigned file {} (S{}E{}): {}. \
                                 The file can be organized via the rename queue.",
                                src_path.display(),
                                payload.season,
                                episode_num,
                                e
                            );
                        }
                        _ => {}
                    }
                }
                last_result
            } else {
                tracing::debug!(
                    "Skipping organization for '{}' — currently being scanned",
                    src_path.display()
                );
                None
            }
        } else {
            None
        };

        // The path the file lives at now (organized dir, or original if unorganized).
        let current_path = organized_path.as_ref().unwrap_or(&src_path);
        let current_path_str = current_path.to_string_lossy().to_string();

        // Run AFTER organization so the scan_queue task operates on the final path.
        // The background task hashes if needed and, once media info is available,
        // renames the file if the episode template uses media-info variables
        // ({width}, {codec}, etc.).
        let already_fingerprinted = state
            .db
            .has_existing_fingerprint(&current_path_str)
            .await
            .unwrap_or(false);

        if !is_auxiliary && !already_fingerprinted {
            let state_ref = state.clone();
            let scan_path = current_path.clone();
            let ep_ids_clone: Vec<String> = episode_ids
                .iter()
                .map(|(_, id): &(i32, String)| id.clone())
                .collect();
            let path_str_clone = current_path_str.clone();
            let incoming_part_bg = incoming_part;
            let series_id_bg = series_id.clone();
            let season_bg = payload.season.clone();
            let episode_num_bg = episode_ids.first().map(|(num, _)| *num).unwrap_or(0);
            let scan_queue = state_ref.scan_queue.clone();
            scan_queue.submit(scan_path.clone(), move || {
            let state_ref = state_ref.clone();
            let scan_path = scan_path.clone();
            let ep_ids_clone = ep_ids_clone.clone();
            let path_str_clone = path_str_clone.clone();
            let incoming_part_bg = incoming_part_bg;
            let series_id_bg = series_id_bg.clone();
            let season_bg = season_bg.clone();
            let episode_num_bg = episode_num_bg;
            async move {
                let (hash_val, hash, media_info) = state_ref
                    .db
                                        .update_file_fingerprint(&scan_path, EpisodeStatus::Organized.as_str())
                                        .await;
                                    if !hash.is_empty() {
                                        for ep_id in &ep_ids_clone {
                        if let Some(part_num) = incoming_part_bg {
                            let _ = state_ref
                                .db
                                .update_episode_part_fingerprint(ep_id, part_num, &hash_val)
                                .await;
                        }
                    }

                    let first_ep_id = ep_ids_clone.first().cloned().unwrap_or_default();
                    let _ = sqlx::query("INSERT INTO file_event_log (event_type, source_path, destination_path, status) VALUES ('analyze', ?, ?, 'completed')")
                        .bind(&path_str_clone)
                        .bind(&first_ep_id)
                        .execute(state_ref.db.get_pool())
                        .await;

                    let _ = state_ref
                        .db
                        .record_activity(ActivityEvent {
                            event_type: Analyze,
                            series_title: String::new(),
                            season: None,
                            episode: None,
                            episode_end: None,
                            title: None,
                            details: Some(path_str_clone.clone()),
                            status: "Success".to_string(),
                        })
                        .await;

                        // Post-fingerprint rename: if the filename template uses media-info
                        // variables, rename now that the metadata is available (cheap —
                        // template resolution + a filesystem rename).
                        if incoming_part_bg.is_none()
                            && let Some(mi) = &media_info
                            && let Some(organizer) = &state_ref.organizer
                        {
                            let _ = organizer
                                .maybe_rename_with_media_info(
                                    &scan_path,
                                    &series_id_bg,
                                    &season_bg,
                                    episode_num_bg,
                                    mi,
                                )
                                .await;
                        }
                }
            }
        }).await;
        }

        // Re-evaluate monitor status for each newly assigned episode.
        // Assigning a file changes has_file, which affects Missing/Existing modes.
        for (_, ep_id) in &episode_ids {
            crate::source_processor::reapply_monitor_for_episode(
                &state.db,
                &series_id,
                ep_id,
                false,
                {
                    let c = state.cfg.read().await;
                    c.general.absolute_numbering
                },
            )
            .await;
        }

        // Assigned files are no longer kept for review.
        let _ = state
            .db
            .delete_unmatched_files(std::slice::from_ref(&payload.path))
            .await;
        unlock_series(&state, &series_id).await;
        tracing::debug!(
            "assign_series_file completed for series {} ({}): {} episodes assigned",
            series_title,
            id,
            episode_ids.len()
        );
        Ok(StatusCode::OK)
    })
}

/// Batch-deletes files on disk and clears the corresponding DB references.
///
/// Best-effort: a file that can't be found or a failed DB update is logged and the
/// rest continue — with the filesystem and DB out of sync, deleting what we can is
/// preferable to aborting mid-batch.
pub async fn delete_series_files(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::BatchDeletePayload>,
) -> Result<StatusCode, (StatusCode, String)> {
    tracing::info!(
        "Batch delete request for series {}: {} files",
        id,
        payload.paths.len()
    );

    if !try_lock_series(&state, &id).await {
        return Err((
            StatusCode::CONFLICT,
            "Series is currently being modified by another operation (e.g. batch move)".to_string(),
        ));
    }

    // Deleting an episode's last video must take its attached sidecars with it —
    // otherwise subtitles/nfo are orphaned on disk. The DB is the SSoT for what is
    // attached, so for every episode left with no main file by this request, add
    // its remaining files (sidecars, linked variants) to the delete set. An episode
    // that keeps another main file — e.g. one part of a multipart episode — is left
    // untouched beyond the requested paths.
    let mut paths = payload.paths.clone();
    for attached in state
        .db
        .attached_files_of_emptied_episodes(&payload.paths)
        .await
        .unwrap_or_default()
    {
        if !paths.contains(&attached) {
            paths.push(attached);
        }
    }

    // Cancel any queued/in-progress scans for these files — they will be
    // deleted from disk, so scanning them would be wasted I/O.
    let scan_paths: Vec<std::path::PathBuf> = paths.iter().map(std::path::PathBuf::from).collect();
    state.scan_queue.cancel_paths(&scan_paths).await;
    // Keep the paths for the review-record cleanup after the loop consumes them.
    let deleted_paths = paths.clone();

    for path_str in paths {
        // Check for shutdown between file deletions.
        // If signalled, stop early — remaining files will still have DB
        // references, which is acceptable (the ongoing delete is best-effort).
        if crate::task::is_shutdown_requested(&state.shutdown_token, "delete_series_files") {
            break;
        }

        let path = PathBuf::from(&path_str);
        if path.exists() {
            // Log delete event with episode info for the activity feed. Resolved
            // through `episode_files` too so a part file is attributed.
            let ep_record = sqlx::query_as::<_, (String, i32, i32, String)>(
                "SELECT e.episode_id, e.season, e.episode, COALESCE(sm.target_title, '')
                 FROM episodes e
                 JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main'
                 JOIN file_paths fp ON fp.id = ef.file_path_id
                 LEFT JOIN series_mappings sm ON e.series_id = sm.id
                 WHERE fp.file_path = ?
                 LIMIT 1",
            )
            .bind(&path_str)
            .fetch_optional(state.db.get_pool())
            .await
            .unwrap_or(None);

            if let Some((ref ep_id, season, episode, ref series_title)) = ep_record {
                let _ = sqlx::query("INSERT INTO file_event_log (event_type, source_path, destination_path, status) VALUES ('delete', ?, ?, 'completed')")
                    .bind(&path_str)
                    .bind(ep_id)
                    .execute(state.db.get_pool())
                    .await;

                let _ = state
                    .db
                    .record_activity(ActivityEvent {
                        event_type: Delete,
                        series_title: series_title.clone(),
                        season: Some(season.to_string()),
                        episode: Some(episode),
                        episode_end: None,
                        title: None,
                        details: Some(path_str.clone()),
                        status: "Success".to_string(),
                    })
                    .await;
            } else {
                let _ = sqlx::query("INSERT INTO file_event_log (event_type, source_path, status) VALUES ('delete', ?, 'completed')")
                        .bind(&path_str)
                        .execute(state.db.get_pool())
                        .await;

                let _ = state
                    .db
                    .record_activity(ActivityEvent {
                        event_type: Delete,
                        series_title: String::new(),
                        season: None,
                        episode: None,
                        episode_end: None,
                        title: None,
                        details: Some(path_str.clone()),
                        status: "Success".to_string(),
                    })
                    .await;
            }

            // File is gone from disk — disown it via the SSoT: clears every
            // covering episode (both modes), removes part rows, severs the
            // file_paths link.
            let affected = match state.db.disown_path(&path_str).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::warn!("Failed to update DB for deleted file {}: {}", path_str, e);
                    Vec::new()
                }
            };

            // Re-evaluate monitor status for every affected episode; the file going
            // missing changes has_file. Episode-scoped refresh respects soft-overrides.
            for ep_id in &affected {
                crate::source_processor::reapply_monitor_for_episode(
                    &state.db,
                    &id,
                    ep_id,
                    false,
                    {
                        let c = state.cfg.read().await;
                        c.general.absolute_numbering
                    },
                )
                .await;
            }

            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::error!("Failed to delete file {}: {}", path_str, e);
            } else {
                tracing::debug!("Deleted file: {}", path_str);

                // Dead-reference cleanup: drop the fingerprint so the deleted file
                // isn't resurrected by orphan adoption or a later scan, and cull the
                // download folder if this was its last file. Library folders keep
                // their structure — only download organizer roots are culled.
                let _ = state.db.delete_fingerprint(&path_str).await;
                if let Some(downloader_lock) = &state.downloader {
                    let downloader = downloader_lock.read().await;
                    let roots = downloader.get_download_roots().await;
                    drop(downloader);
                    // Cull this file's per-download staging folder if this was its
                    // last file. Library folders keep their structure — the helper
                    // only acts inside a configured download root.
                    crate::file_manager::file_ops::remove_empty_download_dir(&path, &roots).await;
                }
            }
        } else {
            tracing::warn!("File to delete not found: {}", path_str);
        }
    }

    // Deleted files are no longer kept for review.
    let _ = state.db.delete_unmatched_files(&deleted_paths).await;

    unlock_series(&state, &id).await;
    tracing::debug!("delete_series_files completed for series {}", id);
    Ok(StatusCode::OK)
}

/// Clears `file_path` for episodes whose file no longer exists on disk.
/// Returns the number of episodes that were cleared.
///
/// POST /api/series/{id}/clear-missing-files
pub async fn clear_not_found_files(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    tracing::info!("clear_not_found_files called for series {}", id);

    if !try_lock_series(&state, &id).await {
        return Err((
            StatusCode::CONFLICT,
            "Series is currently being modified by another operation (e.g. batch move)".to_string(),
        ));
    }

    let (affected_ids, count) = state
        .db
        .clear_not_found_episode_files(&id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to clear missing episode files for {}: {}", id, e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    // Re-evaluate monitor status for each affected episode.
    // Files going missing changes has_file, which affects Missing/Existing modes.
    // Episode-scoped refresh respects user soft-overrides.
    for ep_id in &affected_ids {
        crate::source_processor::reapply_monitor_for_episode(&state.db, &id, ep_id, false, {
            let c = state.cfg.read().await;
            c.general.absolute_numbering
        })
        .await;
    }

    unlock_series(&state, &id).await;
    tracing::info!(
        "Cleared {} missing episode file(s) for series {} ({} re-evaluated)",
        count,
        id,
        affected_ids.len(),
    );
    Ok(Json(serde_json::json!({"cleared": count})))
}

/// Same loop as `delete_series_files` but without the actual `fs::remove_file` call:
/// the file stays on disk but is disowned from its episode, so the user can re-assign
/// it elsewhere or delete it later.
pub async fn unassign_series_files(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::BatchDeletePayload>,
) -> Result<StatusCode, (StatusCode, String)> {
    tracing::info!(
        "Batch unassign request for series {}: {} files",
        id,
        payload.paths.len()
    );

    if !try_lock_series(&state, &id).await {
        return Err((
            StatusCode::CONFLICT,
            "Series is currently being modified by another operation (e.g. batch move)".to_string(),
        ));
    }

    // Cancel queued/in-progress scans for these files before unassigning.
    let scan_paths: Vec<std::path::PathBuf> =
        payload.paths.iter().map(std::path::PathBuf::from).collect();
    state.scan_queue.cancel_paths(&scan_paths).await;

    for path_str in payload.paths {
        let path = PathBuf::from(&path_str);

        // Owner info for the activity feed, resolved through `episode_files` too so
        // a part file is attributed. Read BEFORE disown_path clears the association.
        let owner = sqlx::query_as::<_, (String, i32, i32, String)>(
            "SELECT e.episode_id, e.season, e.episode, COALESCE(sm.target_title, '')
             FROM episodes e
             JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main'
             JOIN file_paths fp ON fp.id = ef.file_path_id
             LEFT JOIN series_mappings sm ON e.series_id = sm.id
             WHERE fp.file_path = ?
             LIMIT 1",
        )
        .bind(&path_str)
        .fetch_optional(state.db.get_pool())
        .await
        .unwrap_or(None);

        // SSoT disown: drops every association and clears status/dates for each
        // covering episode (multi-episode files) and each multipart part,
        // including auxiliary sidecars.
        let affected = match state.db.disown_path(&path_str).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!("Failed to disown unassigned file {}: {}", path_str, e);
                Vec::new()
            }
        };

        if path.exists() {
            // File still on disk → record the event journal + activity entry so the
            // audit trail names the episode(s) it was detached from.
            match owner {
                Some((ref ep_id, season, episode, ref series_title)) => {
                    let _ = sqlx::query("INSERT INTO file_event_log (event_type, source_path, destination_path, status) VALUES ('unassign', ?, ?, 'completed')")
                        .bind(&path_str)
                        .bind(ep_id)
                        .execute(state.db.get_pool())
                        .await;

                    let _ = state
                        .db
                        .record_activity(ActivityEvent {
                            event_type: Unassign,
                            series_title: series_title.clone(),
                            season: Some(season.to_string()),
                            episode: Some(episode),
                            episode_end: None,
                            title: None,
                            details: Some(path_str.clone()),
                            status: "Success".to_string(),
                        })
                        .await;
                }
                None => {
                    // No episode claims this path — log for audit only.
                    let _ = sqlx::query("INSERT INTO file_event_log (event_type, source_path, status) VALUES ('unassign', ?, 'completed')")
                        .bind(&path_str)
                        .execute(state.db.get_pool())
                        .await;

                    let _ = state
                        .db
                        .record_activity(ActivityEvent {
                            event_type: Unassign,
                            series_title: String::new(),
                            season: None,
                            episode: None,
                            episode_end: None,
                            title: None,
                            details: Some(path_str.clone()),
                            status: "Success".to_string(),
                        })
                        .await;
                }
            }
        } else {
            tracing::warn!("Unassign requested for missing file: {}", path_str);
        }

        // Re-evaluate monitor status for every episode that lost the file;
        // unassigning changes has_file. Episode-scoped refresh respects
        // user soft-overrides.
        for ep_id in &affected {
            crate::source_processor::reapply_monitor_for_episode(&state.db, &id, ep_id, false, {
                let c = state.cfg.read().await;
                c.general.absolute_numbering
            })
            .await;
        }

        // Record a durable block so a rescan cannot re-adopt the file the user
        // judged wrong. The file stays on disk (unassign, not delete).
        // SSoT: `blocked_files`. Cleared by an explicit (re)assignment or the
        // stale-entry sweep.
        if path.exists()
            && let Some((size, hash)) = state.db.path_file_identity(&path_str).await
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            let _ = state.db.block_file(&id, name, size, hash.as_deref()).await;
        }
    }

    unlock_series(&state, &id).await;
    tracing::debug!("unassign_series_files completed for series {}", id);
    Ok(StatusCode::OK)
}

/// Bulk sequential assignment: the user provides N files and a starting
/// (season, episode) coordinate; file[i] is assigned to (start_season,
/// start_episode + i), with automatic season rollover when the current season's
/// episode count is known from metadata.
///
/// Rollover uses `metadata_season_cache` (the provider's official per-season count):
/// on the last episode of season S the next file starts at S+01E01, so a multi-season
/// batch import doesn't need manual season switching in the UI.
pub fn batch_assign_series_files(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<jumbie_shared::types::BatchAssignPayload>,
) -> HandlerResult {
    Box::pin(async move {
        tracing::info!(
            "Batch assign request for series {}: {} files starting at S{}E{}",
            id,
            payload.paths.len(),
            payload.start_season,
            payload.start_episode
        );

        let (target_title, series_id, numbering_mode, start_episode_ids) = {
            let mapping = state
                .db
                .get_series_mapping(&id)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .ok_or((StatusCode::NOT_FOUND, "Series not found".to_string()))?;

            let active_mode = if state.effective_absolute_numbering(&mapping).await {
                jumbie_shared::types::NumberingMode::Absolute
            } else {
                jumbie_shared::types::NumberingMode::Normal
            };
            let metadata_season_counts = {
                let mut count_map: HashMap<i32, i32> = HashMap::new();
                let ordering_mode = if active_mode.is_absolute() {
                    "absolute"
                } else {
                    "normal"
                };

                // Try each provider in PRIORITY order until one has stored metadata.
                let instance_plugins =
                    crate::utils::metadata::instance_plugin_id_map(&state.db).await;
                let ordered_instances = state
                    .plugin_manager
                    .read()
                    .await
                    .ordered_metadata_providers();
                for provider in crate::utils::metadata::ordered_series_providers(
                    &mapping,
                    &ordered_instances,
                    &instance_plugins,
                ) {
                    let rows = state
                        .db
                        .get_metadata_season_cache(
                            &provider.metadata_id,
                            &provider.plugin_id,
                            &provider.instance_id,
                            ordering_mode,
                        )
                        .await
                        .unwrap_or_default();

                    if !rows.is_empty() {
                        for (season_str, count) in rows {
                            if let Some(s_num) =
                                jumbie_shared::mapping::parse_season_num(&season_str)
                            {
                                count_map.insert(s_num, count);
                            }
                        }
                        break;
                    }
                }

                // Augment with actual existing episode numbers to handle cases where
                // a user has episodes beyond the official metadata count.
                if let Ok(db_episodes) = state
                    .db
                    .get_series_episodes_details(&id, active_mode.is_absolute())
                    .await
                {
                    for ep in db_episodes {
                        if let Some(s) = ep.season {
                            let entry = count_map.entry(s).or_insert(0);
                            if ep.episode > *entry {
                                *entry = ep.episode;
                            }
                        }
                    }
                }

                count_map
            };

            let title = mapping.target_title.clone();
            let sid = mapping.series_id.clone();
            let nm = active_mode.is_absolute() as i32;
            let global_absolute = {
                let c = state.cfg.read().await;
                c.general.absolute_numbering
            };

            crate::validation::validate_season_number(&payload.start_season).map_err(|e| {
                tracing::debug!(
                    "batch_assign_series_files: invalid start_season '{}': {}",
                    payload.start_season,
                    e
                );
                (StatusCode::BAD_REQUEST, format!("Invalid season: {}", e))
            })?;
            crate::validation::validate_episode_number(payload.start_episode).map_err(|e| {
                tracing::debug!(
                    "batch_assign_series_files: invalid start_episode {}: {}",
                    payload.start_episode,
                    e
                );
                (StatusCode::BAD_REQUEST, format!("Invalid episode: {}", e))
            })?;

            let mut current_season = jumbie_shared::mapping::parse_season_num(
                &payload.start_season,
            )
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid season number: {}", payload.start_season),
                )
            })?;
            let mut current_episode = payload.start_episode;
            // If they start past the last episode of the season, rollover is disabled for this sequence
            let rollover_enabled = metadata_season_counts
                .get(&current_season)
                .map(|&max| payload.start_episode <= max)
                .unwrap_or(false);

            let ids: Vec<(i32, String, String, Option<u32>)> = payload
                .paths
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    if i > 0 && !payload.is_multipart {
                        let mut rolled_over = false;
                        if rollover_enabled
                            && let Some(&max_ep) = metadata_season_counts.get(&current_season)
                            && current_episode >= max_ep
                        {
                            let next_season = current_season + 1;
                            if metadata_season_counts.contains_key(&next_season) {
                                current_season = next_season;
                                current_episode = 1;
                                rolled_over = true;
                            }
                        }
                        if !rolled_over {
                            current_episode += 1;
                        }
                    }

                    let part = if payload.is_multipart {
                        Some((i + 1) as u32)
                    } else {
                        None
                    };
                    let season_str = current_season.to_string();
                    mapping
                        .get_episode_id(&season_str, current_episode, global_absolute)
                        .map(|episode_id| (current_episode, season_str, episode_id, part))
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            (title, sid, nm, ids)
        };

        if !try_lock_series(&state, &series_id).await {
            return Err((
                StatusCode::CONFLICT,
                "Series is currently being modified by another operation (e.g. batch move)"
                    .to_string(),
            ));
        }

        // Files whose (valid) assignment should be organized/fingerprinted after
        // ALL DB assignments are applied — see phase 2/3 below.
        let mut to_organize: Vec<(PathBuf, String, i32, String, Option<u32>)> = Vec::new();

        // Phase 1: persist all assignments.
        for ((episode, season, episode_id, part_number), path_str) in
            start_episode_ids.iter().zip(payload.paths.iter())
        {
            let src = PathBuf::from(path_str);
            if !src.exists() {
                continue;
            }

            // Auxiliary sidecars (subtitle/nfo) are link-only: ensure a `file_paths`
            // row exists so the link has a target, then skip the video organize /
            // rename pipeline (see `assign_series_file`).
            let is_auxiliary = is_auxiliary_path(&src);
            if is_auxiliary {
                crate::scanner::scan::hash_first(&state, &src).await;
            }

            let original_path_str = src.to_string_lossy().to_string();
            // Use empty title when no real episode title is available from metadata
            // so the `{title}` template variable resolves to nothing instead of "Episode X".
            let episode_title = String::new();

            if let Some(part_num) = *part_number {
                // Fingerprint first so the part has a `file_paths` row (content hash /
                // media info). The multipart association itself lives in `episode_files`.
                crate::scanner::scan::ensure_hashed(&state, &src, false).await;
                let file_size = std::fs::metadata(&src).map(|m| m.len() as i64).ok();
                let season_num = season.parse::<i32>().unwrap_or(1);

                // One transaction: ensure the parent row, disown any episode that
                // already held this part file (SSoT collision), register the part.
                match state
                    .db
                    .register_multipart_assignment(crate::db::ownership::MultipartAssignment {
                        episode_id,
                        series_id: &series_id,
                        season: season_num,
                        episode: *episode,
                        numbering_mode,
                        file_path: &original_path_str,
                        part_number: part_num,
                        size: file_size,
                        target_episode_ids: None,
                    })
                    .await
                {
                    Ok(displaced) => {
                        for old_id in &displaced {
                            crate::source_processor::reapply_monitor_for_episode(
                                &state.db,
                                &series_id,
                                old_id,
                                false,
                                {
                                    let c = state.cfg.read().await;
                                    c.general.absolute_numbering
                                },
                            )
                            .await;
                        }
                    }
                    Err(e) => {
                        unlock_series(&state, &series_id).await;
                        return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
                    }
                }
            } else {
                let old_ids = state
                    .db
                    .assign_file_to_episode(AssignFileToEpisodeParams {
                        episode_id,
                        series_id: &series_id,
                        series_title: &target_title,
                        season: jumbie_shared::mapping::parse_season_num(season).ok_or_else(
                            || {
                                (
                                    StatusCode::BAD_REQUEST,
                                    format!("Invalid season number: {}", season),
                                )
                            },
                        )?,
                        episode: *episode,
                        file_path: &original_path_str,
                        episode_title: &episode_title,
                        all_target_episode_ids: None,
                        numbering_mode,
                        only_unassigned: false,
                    })
                    .await
                    .map_err(|e| {
                        tracing::error!("DB assign_file_to_episode failed in batch assign: {}", e);
                        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                    })?;
                // Re-evaluate episodes that lost their file to this reassign.
                for old_id in &old_ids {
                    crate::source_processor::reapply_monitor_for_episode(
                        &state.db,
                        &series_id,
                        old_id,
                        false,
                        {
                            let c = state.cfg.read().await;
                            c.general.absolute_numbering
                        },
                    )
                    .await;
                }
            }

            // Organization (rename/move) is deferred to phase 2 so every DB
            // assignment is applied against the still-stable original paths:
            // renaming a file here would recycle a path that a later item in this
            // batch still uses as its source, wiping that later assignment.
            //
            // A language/version variant attaches alongside its primary file and is
            // not the episode's canonical file, so it is never organized onto the
            // template path. An assignment the collision strategy declined (nothing
            // attached) is not organized either.
            if !is_auxiliary
                && src.exists()
                && !state
                    .db
                    .has_variant_occupant(episode_id, &original_path_str)
                    .await
                    .unwrap_or(false)
                && state
                    .db
                    .episode_holds_any_path(episode_id, &original_path_str)
                    .await
                    .unwrap_or(false)
            {
                to_organize.push((
                    src,
                    season.clone(),
                    *episode,
                    episode_id.clone(),
                    *part_number,
                ));
            }
        }

        // Phase 2: organize the assigned files. Every destination is resolved up
        // front and files are moved cycle-safely: a source that is itself a target
        // of another move in this batch is temp-renamed first, so a rotation
        // (S00E17→S00E18, S00E18→S00E19, …) neither clobbers a file nor leaves a
        // spurious collision suffix. `batch_sources` tells the move engine that an
        // in-batch occupant is temporary, not a permanent conflict.
        //
        // Skipped for multipart batches: their files are registered as episode
        // parts and organized by the rename queue instead.
        let mut final_paths: HashMap<PathBuf, PathBuf> = HashMap::new();
        if !payload.is_multipart
            && let Some(organizer) = &state.organizer
        {
            // Resolve destinations up front (skipping files currently being scanned),
            // then move the whole batch through the SSoT cycle-safe helper:
            // `move_files_cycle_safe` temp-renames recycled sources (with their
            // fingerprint rows) and moves each file into a freed destination with
            // `batch_sources`, so a rotation (S00E17→S00E18, S00E18→S00E19, …)
            // neither clobbers a file nor leaves a spurious collision suffix.
            let mut planned: Vec<(PathBuf, PathBuf, String)> = Vec::new(); // (src, dst, episode_id)
            for (src, season, episode, episode_id, _part) in &to_organize {
                if state.scan_queue.contains(src).await {
                    tracing::debug!(
                        "Skipping organize for '{}' — currently being scanned",
                        src.display()
                    );
                    continue;
                }
                match organizer
                    .resolve_assigned_target_path(src, &series_id, season, *episode)
                    .await
                {
                    Ok(dst) => planned.push((src.clone(), dst, episode_id.clone())),
                    Err(e) => {
                        tracing::warn!("Failed to resolve target for '{}': {}", src.display(), e)
                    }
                }
            }

            let moves: Vec<(PathBuf, PathBuf)> = planned
                .iter()
                .map(|(src, dst, _)| (src.clone(), dst.clone()))
                .collect();
            let results = crate::file_manager::move_files_cycle_safe(organizer, &moves).await;
            for ((src, _dst, episode_id), result) in planned.iter().zip(results) {
                if let Some(final_path) = result {
                    let _ = state
                        .db
                        .update_file_path(episode_id, &final_path.to_string_lossy())
                        .await;
                    final_paths.insert(src.clone(), final_path);
                }
            }
        }

        // Phase 3: background fingerprint every assigned file on its final path
        // (the moved path, or the original when organization was skipped).
        for (src, season, episode, episode_id, part_number) in &to_organize {
            let final_path = final_paths.get(src).cloned().unwrap_or_else(|| src.clone());
            let scan_path_str = final_path.to_string_lossy().to_string();
            if state
                .db
                .has_existing_fingerprint(&scan_path_str)
                .await
                .unwrap_or(false)
            {
                continue;
            }

            let state_ref = state.clone();
            let scan_path = final_path.clone();
            let ep_id_clone = episode_id.clone();
            let path_str_clone = scan_path_str.clone();
            let part_num_bg = *part_number;
            let series_id_bg = series_id.clone();
            let season_bg = season.clone();
            let episode_bg = *episode;
            state.scan_queue.submit(scan_path.clone(), move || {
                let state_ref = state_ref.clone();
                async move {
                    let (hash_val, hash, media_info) = state_ref
                        .db
                        .update_file_fingerprint(&scan_path, EpisodeStatus::Organized.as_str())
                        .await;
                    if !hash.is_empty() {
                        if let Some(part_num) = part_num_bg {
                            let _ = state_ref
                                .db
                                .update_episode_part_fingerprint(&ep_id_clone, part_num, &hash_val)
                                .await;
                        }

                        let _ = sqlx::query("INSERT INTO file_event_log (event_type, source_path, destination_path, status) VALUES ('analyze', ?, ?, 'completed')")
                            .bind(&path_str_clone)
                            .bind(&ep_id_clone)
                            .execute(state_ref.db.get_pool())
                            .await;

                        let _ = state_ref
                            .db
                            .record_activity(ActivityEvent {
                                event_type: Analyze,
                                series_title: String::new(),
                                season: None,
                                episode: None,
                                episode_end: None,
                                title: None,
                                details: Some(path_str_clone.clone()),
                                status: "Success".to_string(),
                            })
                            .await;

                        if part_num_bg.is_none()
                            && let Some(mi) = &media_info
                            && let Some(organizer) = &state_ref.organizer
                        {
                            let _ = organizer
                                .maybe_rename_with_media_info(
                                    &scan_path,
                                    &series_id_bg,
                                    &season_bg,
                                    episode_bg,
                                    mi,
                                )
                                .await;
                        }
                    }
                }
            })
            .await;
        }

        // Re-evaluate monitor status for each newly assigned episode.
        // Batch assigning files changes has_file, which affects Missing/Existing modes.
        for (_, _, ep_id, _) in &start_episode_ids {
            crate::source_processor::reapply_monitor_for_episode(
                &state.db,
                &series_id,
                ep_id,
                false,
                {
                    let c = state.cfg.read().await;
                    c.general.absolute_numbering
                },
            )
            .await;
        }

        // Assigned files are no longer kept for review.
        let _ = state.db.delete_unmatched_files(&payload.paths).await;
        unlock_series(&state, &series_id).await;
        tracing::debug!(
            "batch_assign_series_files completed for series {} ({}): {} files assigned",
            target_title,
            id,
            payload.paths.len()
        );
        Ok(StatusCode::OK)
    })
}
