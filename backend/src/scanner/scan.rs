use crate::api::AppState;
use crate::models::activity::ActivityEvent;
use crate::utils;
use crate::utils::ParseContext;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tracing::trace;
use walkdir::WalkDir;

/// Scan a directory for video files, fingerprint them, and register episode records.
///
/// # Arguments
///
/// * `path` - The directory path to scan (recursive, up to 5 levels deep).
/// * `state` - Application state providing DB access and configuration.
/// * `trust_existing` - When `true`, any file that already has a fingerprint entry in the DB
///   (checked via `get_fingerprint_meta`) is skipped **without** re-verifying inode/mtime/size.
///   Files without an existing fingerprint are still fully processed (fingerprinted + media info
///   scanned). Use this after an `update_series_paths` call has already migrated all fingerprint
///   data (including quick_hash and media_info) to the new path, so the scan only needs to
///   discover genuinely new files.
///   When `false` (the default), the traditional inode/mtime/size comparison is used to detect
///   whether the file has changed since the last scan.
///
/// Hash-first helper: record the content fingerprint for a discovered video
/// through the scan queue (bounded concurrency, never all at once).
pub(crate) async fn hash_first(state: &Arc<AppState>, path: &Path) {
    let db = state.db.clone();
    let owned = path.to_path_buf();
    state
        .scan_queue
        .submit_await(path.to_path_buf(), move || async move {
            db.record_hash_only(&owned, "organized").await;
        })
        .await;
}

/// Hash-first helper: record the content fingerprint for a discovered video before
/// any assignment. Returns whether the file changed since the last scan; unchanged
/// files are not re-hashed so rescans stay cheap.
///
/// The path→fingerprint mapping is only trustworthy while the file's identity
/// (inode, device, size, mtime) is unchanged, so a matching row short-circuits the
/// re-hash; any mismatch re-hashes and rewrites the row.
pub(crate) async fn ensure_hashed(
    state: &Arc<AppState>,
    path: &Path,
    trust_existing: bool,
) -> bool {
    // SSoT for path→fingerprint validity is `stored_fingerprint_if_unchanged`
    // (inode/device/size/mtime). `trust_existing` skips the identity check and
    // treats any existing row as valid.
    let changed = if trust_existing {
        state
            .db
            .get_fingerprint_meta(&path.to_string_lossy())
            .await
            .map(|meta| meta.is_none())
            .unwrap_or(true)
    } else {
        state
            .db
            .stored_fingerprint_if_unchanged(path, false)
            .await
            .is_none()
    };

    if changed {
        hash_first(state, path).await;
    }
    changed
}

/// Resolve the episode cell a sidecar names, from its parsed filename and the
/// owning series' mapping. `None` when the target cannot be represented (decimal
/// episode, ambiguous season aliases, non-numeric season) — such sidecars surface
/// as unassigned for manual handling.
///
/// Pure resolution: it only computes the cell id, never touches the DB, so callers
/// can resolve during a walk and attach once every video has created its cell.
pub(crate) fn resolve_auxiliary_cell(
    info: &jumbie_shared::mapping::EpisodeInfo,
    path: &Path,
    mapping: &jumbie_shared::types::MappingRule,
    global_absolute: bool,
) -> Option<String> {
    if info.has_decimal_episode {
        return None;
    }

    let absolute = mapping.settings.active_mode(global_absolute).is_absolute();
    let season_str =
        match crate::scanner::resolve_season_for_series(info, path, &mapping.settings, absolute) {
            crate::scanner::ResolvedSeason::Season(season, _) => season,
            crate::scanner::ResolvedSeason::Unneeded => return None,
        };
    let &episode_num = info.episodes.first()?;
    mapping
        .get_episode_id(&season_str, episode_num, global_absolute)
        .ok()
}

/// Attach a sidecar (subtitle/nfo) to an episode cell that already exists.
///
/// The scanner never creates a cell for a sidecar: a cell exists because a
/// video/part file, a user-defined cell, or a provider episode put a row in
/// `episodes`. A sidecar whose cell does not exist is left unlinked — it surfaces
/// as unassigned, and the user may materialize the cell (and attach the sidecar)
/// through manual assignment.
///
/// Shared by `scan_directory` and `import_scan_for_series`, which stash sidecars
/// and flush them after the walk so a sidecar scanned before its video still
/// attaches in a single pass.
pub(crate) async fn attach_auxiliary_to_cell(
    state: &Arc<AppState>,
    path: &Path,
    kind: jumbie_shared::media_format::FileKind,
    episode_id: &str,
) -> bool {
    if !state.db.episode_exists(episode_id).await.unwrap_or(false) {
        trace!(
            "Left auxiliary file '{}' unlinked: episode cell '{}' does not exist",
            path.display(),
            episode_id
        );
        return false;
    }

    // Ensure a `file_paths` row exists so the link has a target.
    hash_first(state, path).await;
    let path_str = path.to_string_lossy().to_string();
    if let Err(e) = state
        .db
        .link_file_episode_as(&path_str, episode_id, kind)
        .await
    {
        tracing::debug!(
            "Failed to link auxiliary file {} -> {}: {}",
            path_str,
            episode_id,
            e
        );
        return false;
    }
    trace!("Linked auxiliary {} -> {}", path_str, episode_id);
    true
}

/// Resolve the episode cell a sidecar at `path` names, looking up its series by
/// the filename-derived series key. `None` when no series matches or the target
/// cannot be resolved.
pub(crate) async fn resolve_auxiliary_cell_for_path(
    state: &Arc<AppState>,
    path: &Path,
) -> Option<String> {
    let filename_os = path.file_name()?;
    let filename = filename_os.to_string_lossy();
    let info = utils::parse_filename(&filename, ParseContext::FileScan)?;
    let series_key = utils::generate_series_key(&filename);
    let (_id, mapping) = state
        .db
        .get_mapping_by_key(&series_key)
        .await
        .ok()
        .flatten()?;
    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };
    resolve_auxiliary_cell(&info, path, &mapping, global_absolute)
}

pub fn scan_directory<'a>(
    path: &'a Path,
    state: &'a Arc<AppState>,
    trust_existing: bool,
) -> Pin<Box<dyn Future<Output = Result<Vec<String>, anyhow::Error>> + Send + 'a>> {
    Box::pin(async move {
        let mut found_series = Vec::new();

        // Offload the synchronous directory walk to the blocking thread pool so the
        // async runtime isn't stalled by the file system crawl (WalkDir + std::fs::metadata).
        let walk_entries: Vec<_> = tokio::task::spawn_blocking({
            let path = path.to_path_buf();
            move || {
                WalkDir::new(&path)
                    .max_depth(5) // safety: cap traversal depth
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .collect::<Vec<_>>()
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Scanner: spawn_blocking for directory walk failed: {}", e))?;

        let mut suppressed_cache: std::collections::HashMap<
            (String, i32),
            std::collections::HashSet<i32>,
        > = std::collections::HashMap::new();

        // Blocked files are cached per series so a walk does not re-query them
        // for every file. SSoT: `blocked_files`.
        let mut blocked_cache: std::collections::HashMap<
            String,
            Vec<crate::db::blocked_files::BlockedFile>,
        > = std::collections::HashMap::new();

        // Sidecars discovered during the walk are stashed and attached after every
        // video has been processed, so a sidecar scanned before its video still
        // finds the cell its video created (single pass). A sidecar whose cell never
        // materializes stays unlinked for manual assignment.
        let mut pending_auxiliary: Vec<(
            std::path::PathBuf,
            jumbie_shared::media_format::FileKind,
            String,
        )> = Vec::new();

        for entry in &walk_entries {
            let path = entry.path();
            let path_str = path.to_string_lossy();
            trace!("Evaluating file: {}", path_str);

            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                if jumbie_shared::media_format::is_video_ext(ext_str.as_str()) {
                    // Hash-first: fingerprint every video found (matched or not)
                    // before any assignment, so origin and auto-assign key on content.
                    let changed_early = ensure_hashed(state, path, trust_existing).await;
                    if let Some(filename_os) = path.file_name() {
                        let filename_str = filename_os.to_string_lossy();
                        let info =
                            match utils::parse_filename(&filename_str, ParseContext::FileScan) {
                                Some(i) => i,
                                None => {
                                    trace!("Skipped {}: unable to parse filename", filename_str);
                                    continue;
                                }
                            };

                        // Decimal episode guard
                        // Episode numbers with fractional components (e.g. S01E1.5)
                        // cannot be represented in the system. Skip auto-assignment;
                        // the file will appear unassigned in "Manage Series Files"
                        // for manual handling. Users can override via custom RSS patterns.
                        if info.has_decimal_episode {
                            trace!(
                                "Skipped '{}': decimal episode number in '{}'",
                                path.display(),
                                filename_str
                            );
                            continue;
                        }

                        let submitter = utils::extract_submitter(&filename_str);
                        let series_key = utils::generate_series_key(&filename_str);

                        if let Some((_id, mapping)) = state
                            .db
                            .get_mapping_by_key(&series_key)
                            .await
                            .ok()
                            .flatten()
                        {
                            let path_str = path.to_string_lossy().to_string();
                            // SSoT: series tristate (None = "use global") falls back to the
                            // global default; resolve it once for all mode-aware decisions.
                            let global_absolute = {
                                let c = state.cfg.read().await;
                                c.general.absolute_numbering
                            };
                            let absolute =
                                mapping.settings.active_mode(global_absolute).is_absolute();
                            // Resolve season with season-alias awareness now that the
                            // series (and its season overrides) are known.
                            let (season_str, _was_explicit) =
                                match crate::scanner::resolve_season_for_series(
                                    &info,
                                    path,
                                    &mapping.settings,
                                    absolute,
                                ) {
                                    crate::scanner::ResolvedSeason::Season(s, e) => (s, e),
                                    crate::scanner::ResolvedSeason::Unneeded => {
                                        trace!(
                                            "Skipped {}: season aliases matched ambiguously",
                                            filename_str
                                        );
                                        continue;
                                    }
                                };

                            // A suppressed season is a deliberate user deletion: a
                            // rescan must not resurrect it. The set is cached per
                            // (series, mode) so it is not re-queried for every file.
                            let numbering_mode =
                                mapping.settings.active_mode(global_absolute).is_absolute() as i32;
                            let cache_key = (mapping.series_id.clone(), numbering_mode);
                            if !suppressed_cache.contains_key(&cache_key) {
                                let set: std::collections::HashSet<i32> = state
                                    .db
                                    .get_suppressed_seasons(&mapping.series_id, numbering_mode)
                                    .await
                                    .unwrap_or_default()
                                    .into_iter()
                                    .collect();
                                suppressed_cache.insert(cache_key.clone(), set);
                            }
                            if let Some(set) = suppressed_cache.get(&cache_key)
                                && season_str
                                    .trim()
                                    .parse::<i32>()
                                    .map(|n| set.contains(&n))
                                    .unwrap_or(false)
                            {
                                trace!(
                                    "Skipped {}: season {} is suppressed for series {}",
                                    path_str, season_str, mapping.series_id
                                );
                                continue;
                            }

                            // Never re-adopt a file the user deliberately
                            // unassigned ("blocked"). Cached per series; the content
                            // hash is only fetched when a block carries one.
                            if !blocked_cache.contains_key(&mapping.series_id) {
                                let list = state
                                    .db
                                    .get_blocked_files(&mapping.series_id)
                                    .await
                                    .unwrap_or_default();
                                blocked_cache.insert(mapping.series_id.clone(), list);
                            }
                            if let Some(blocks) = blocked_cache.get(&mapping.series_id)
                                && !blocks.is_empty()
                            {
                                let file_size =
                                    std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
                                if crate::db::blocked_files::is_file_blocked(
                                    &state.db,
                                    &mapping.series_id,
                                    blocks,
                                    &filename_str,
                                    file_size,
                                    &path_str,
                                )
                                .await
                                {
                                    trace!(
                                        "Skipped {}: blocked (user-unassigned) for series {}",
                                        path_str, mapping.series_id
                                    );
                                    continue;
                                }
                            }
                            // Change detection was performed by `ensure_hashed` (hash-first).
                            let is_changed = changed_early;

                            if is_changed {
                                // A part file (e.g. E01-part-1.mkv, E01-cd2.mkv)
                                // is a fragment of one logical episode. Registration
                                // (parent row + multipart association
                                // + background fingerprinting) is the shared SSoT
                                // `scanner::parts::register_part_file`, also used by
                                // `import_scan_for_series`.
                                if let Some(part_num) = info.part_number {
                                    let episode_num = info.episodes.first().copied().unwrap_or(1);
                                    if let Err(e) = crate::scanner::parts::register_part_file(
                                        state,
                                        &mapping,
                                        &season_str,
                                        episode_num,
                                        global_absolute,
                                        path,
                                        part_num,
                                    )
                                    .await
                                    {
                                        tracing::debug!("skipping part file {}: {}", path_str, e);
                                        continue;
                                    }
                                } else {
                                    // NORMAL / RANGE FILE
                                    // A multi-episode range (e.g. E01-E03) packs
                                    // several episodes into one physical container.
                                    // We insert one row per episode, all sharing the
                                    // same file_path.  This lets the library view
                                    // each logical episode as separately watchable
                                    // and trackable, even though they all resolve to
                                    // the same file on disk.  The player seeks to
                                    // the correct chapter/timestamp based on the
                                    // episode number metadata.
                                    let ep_start = info.episodes.first().copied().unwrap_or(1);
                                    let ep_end = info.episodes.last().copied().unwrap_or(ep_start);

                                    // Look up existing submitter from DB so that
                                    // re-scans (e.g. after a rename) preserve the
                                    // original release group instead of re-deriving
                                    // it from the new filename.
                                    let _first_ep_id = match mapping.get_episode_id(
                                        &season_str,
                                        ep_start,
                                        global_absolute,
                                    ) {
                                        Ok(id) => id,
                                        Err(e) => {
                                            tracing::debug!("skipping non-numeric season: {e}");
                                            continue;
                                        }
                                    };
                                    for ep in ep_start..=ep_end {
                                        // When the file on disk has changed (manual replacement),
                                        // clear any stale download metadata from the episode.
                                        let clr_id = match mapping.get_episode_id(
                                            &season_str,
                                            ep,
                                            global_absolute,
                                        ) {
                                            Ok(id) => id,
                                            Err(e) => {
                                                tracing::debug!("skipping non-numeric season: {e}");
                                                continue;
                                            }
                                        };
                                        let _ = sqlx::query(
                                            "UPDATE episodes SET download_link = NULL WHERE episode_id = ?"
                                        )
                                        .bind(&clr_id)
                                        .execute(state.db.get_pool())
                                        .await;
                                        let episode_id = match mapping.get_episode_id(
                                            &season_str,
                                            ep,
                                            global_absolute,
                                        ) {
                                            Ok(id) => id,
                                            Err(e) => {
                                                tracing::debug!("skipping non-numeric season: {e}");
                                                continue;
                                            }
                                        };
                                        trace!("Inserting episode row: {}", episode_id);

                                        let numbering_mode = mapping
                                            .settings
                                            .active_mode(global_absolute)
                                            .is_absolute()
                                            as i32;
                                        let _ = state
                                            .db
                                            .insert_episode(
                                                crate::db::episodes::InsertEpisodeParams {
                                                    episode_id: &episode_id,
                                                    series_id: &mapping.series_id,
                                                    season: season_str.parse::<i32>().unwrap_or(1),
                                                    episode: ep,
                                                    file_path: Some(&path_str),
                                                    title: Some(""),
                                                    quality_profile_id: None,
                                                    status: "organized",
                                                    meta_date: None,
                                                    est_date: None,
                                                    metadata_ids: &std::collections::HashMap::new(),
                                                    description: None,
                                                    runtime: None,
                                                    image_url: None,
                                                    metadata_source: None,
                                                    numbering_mode: Some(numbering_mode),
                                                },
                                            )
                                            .await;
                                    }

                                    // Same rationale as the part-file branch: keep hashing and
                                    // profile checks off the scanner's hot path.
                                    let state_ref = state.clone();
                                    let path_for_queue = path.to_path_buf();
                                    let path_clone = path_for_queue.clone();
                                    let ep_start = info.episodes.first().copied().unwrap_or(1);
                                    let season_bg = season_str.clone();
                                    let mapping_bg = mapping.clone();
                                    let submitter_bg = submitter.clone();
                                    let path_str_bg = path_str.clone();
                                    let global_absolute_bg = global_absolute;
                                    state.scan_queue.submit(path_for_queue.clone(), move || {
                                    let state_ref = state_ref.clone();
                                    let path_clone = path_clone.clone();
                                    let ep_start = ep_start;
                                    let season_bg = season_bg.clone();
                                    let mapping_bg = mapping_bg.clone();
                                    let submitter_bg = submitter_bg.clone();
                                    let path_str_bg = path_str_bg.clone();
                                    let global_absolute_bg = global_absolute_bg;
                                    let ep_end_bg = ep_end;
                                    async move {
                                    // Skip fingerprinting if this file's identity
                                    // (inode, device, mtime, size) matches the DB —
                                    // it hasn't changed since the last scan.
                                    if state_ref
                                        .db
                                        .has_existing_fingerprint(&path_str_bg)
                                        .await
                                        .unwrap_or(false)
                                    {
                                        trace!(
                                            "Skipping fingerprint for unchanged file: '{}'",
                                            path_clone.display(),
                                        );
                                    } else {
                                    let (hash_val, media_info) = state_ref
                                        .db
                                        .scan_media_info_only(&path_clone)
                                        .await;

                                    if let (Some(info), Some(organizer)) =
                                        (media_info.as_ref(), &state_ref.organizer)
                                    {
                                        let cfg = state_ref.cfg.read().await;
                                        if let Some(submitter) = submitter_bg.as_deref() {
                                            let filename = path_clone
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .unwrap_or("");
                                            if cfg.general.automatic_profiles.enabled {
                                                organizer
                                                    .check_automatic_profile_rules(
                                                        submitter,
                                                        filename,
                                                        info,
                                                        &cfg.general.automatic_profiles.categories,
                                                        Some(&hash_val),
                                                        true,
                                                    )
                                                    .await;
                                            }

                                            // Persist the scan so recalculation
                                            // works without the original file.
                                            if let Ok(media_json) =
                                                serde_json::to_string(info)
                                            {
                                                let _ = state_ref
                                                    .db
                                                    .record_media_scan(
                                                        submitter,
                                                        filename,
                                                        Some(&mapping_bg.series_id),
                                                        season_bg
                                                            .parse::<i32>()
                                                            .ok(),
                                                        Some(ep_start),
                                                        &media_json,
                                                    )
                                                    .await;
                                            }
                                        }
                                    }

                                    // Write submitter to the normalized release_info table
                                    // so it survives organize (JOINed through quick_hash)
                                    // and is shared by all copies of the same content.
                                    if let Some(ref rg) = submitter_bg {
                                        let _ = state_ref
                                            .db
                                            .set_release_info(
                                                &hash_val,
                                                None,
                                                None,
                                                Some(rg),
                                            )
                                            .await;
                                    }

                                    match mapping_bg.get_episode_id(
                                        &season_bg,
                                        ep_start,
                                        global_absolute_bg,
                                    ) {
                                        Ok(first_ep_id) => {
                                            let _ = sqlx::query(
                                                "INSERT INTO file_event_log (event_type, source_path, destination_path, status) VALUES ('analyze', ?, ?, 'completed')"
                                            )
                                            .bind(&path_str_bg)
                                            .bind(&first_ep_id)
                                            .execute(state_ref.db.get_pool())
                                            .await;
                                        }
                                        Err(e) => {
                                            tracing::debug!("skipping non-numeric season: {e}");
                                        }
                                    }
                                let _ = state_ref.db.record_activity(ActivityEvent {
                                    event_type: jumbie_shared::types::ActivityType::Import,
                                    series_title: mapping_bg.target_title.clone(),
                                    season: Some(season_bg.clone()),
                                    episode: Some(ep_start),
                                    episode_end: if ep_end_bg > ep_start { Some(ep_end_bg) } else { None },
                                    title: None,
                                    details: Some(path_str_bg.clone()),
                                    status: "Success".to_string(),
                                }).await;
                                    }
                                }
                                }).await;
                                }
                            }

                            trace!("File matched known series: {}", mapping.target_title);
                            if !found_series.contains(&mapping.target_title) {
                                found_series.push(mapping.target_title.clone());
                            }
                        }
                    }
                } else if let Some(kind) = jumbie_shared::media_format::file_kind_for_path(path)
                    && kind.is_auxiliary()
                {
                    // Resolve now, attach after the walk: the video that creates the
                    // cell may be visited later in this same walk.
                    if let Some(episode_id) = resolve_auxiliary_cell_for_path(state, path).await {
                        pending_auxiliary.push((path.to_path_buf(), kind, episode_id));
                    }
                } else {
                    trace!("Skipped {}: not a video extension", ext_str);
                }
            }
        }

        // Attach the stashed sidecars now that every video has had the chance to
        // create its cell. A sidecar with no cell stays unlinked (unassigned).
        for (aux_path, kind, episode_id) in pending_auxiliary {
            attach_auxiliary_to_cell(state, &aux_path, kind, &episode_id).await;
        }

        // Record each discovered series' current dir mtimes so the periodic scanner can
        // skip unchanged dirs. Each series' path is resolved individually: scan_directory
        // may run on a destination root holding several series, whose mtime won't match
        // any single series directory.
        if !found_series.is_empty() {
            let all_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();
            for (series_id, mapping) in &all_mappings {
                if found_series.contains(&mapping.target_title)
                    && let Some(ref p) = mapping.settings.path
                {
                    let series_path = std::path::Path::new(p);
                    if series_path.exists() {
                        let mtimes = crate::scanner::collect_dir_mtimes(series_path, 5);
                        if let Some(mut latest) =
                            state.db.get_series_mapping(series_id).await.ok().flatten()
                        {
                            latest.settings.last_known_dir_mtimes = mtimes;
                            let _ = state.db.upsert_series_mapping(series_id, &latest).await;
                        }
                    }
                }
            }
        }

        Ok(found_series)
    })
}
