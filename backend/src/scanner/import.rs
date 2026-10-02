use crate::api::AppState;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// Language-variant base of a path's filename (`jumbie_shared::parsing` SSoT).
fn file_variant_base(path: &Path) -> String {
    jumbie_shared::parsing::variant_base(path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
}

/// Whether a path's filename carries a trailing language tag.
fn file_has_language_tag(path: &Path) -> bool {
    jumbie_shared::parsing::has_language_tag(
        path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    )
}

/// Scan a newly imported series directory and create episode records.
///
/// Unlike [`scan_directory`] (which relies on filename-derived series keys
/// matching an existing mapping), this function is used **during import**
/// when the mapping has just been created.  It:
///
/// 1. Walks all video files in `path` (recursively).
/// 2. Parses each filename for SXXEXX patterns to extract season+episode.
/// 3. If the filename has no explicit season, tries to infer it from the
///    parent folder name ("Season 1", "S01", etc.), falling back to season 1.
/// 4. Detects conflicts — if two *non-part* files resolve to the same
///    `(season, episode)` tuple. When a conflict involves files where one
///    has an explicit season (from filename) and another has an inferred
///    season (from folder / fallback), the explicit-season file is kept
///    and the inferred one is left unassigned. If all conflicting files
///    have the same explicitness, all are left unassigned.
/// 5. Returns whether all files were found directly in the root (flat).
///
/// # Conflict rules
///
/// - Part files (`-part-1`, `-cd2`, etc.) are grouped under the parent
///   episode ID and never count as duplicates.
/// - Files that differ only by a trailing language tag (`.en`, `.jpn`) are not
///   a conflict: the file with no language tag is the primary and the tagged
///   ones attach alongside it as `linked`.
/// - If a non-part file collides with another non-part file for the same
///   season+episode, and one has an explicit season while the other has
///   an inferred/folder/fallback season, the explicit one wins.
/// - If both have explicit or both have inferred seasons, both are skipped
///   (unassigned) so the user can manually resolve the ambiguity later.
/// - If a non-part file collides with a part-file group, the part files
///   remain (they share a parent episode); the non-part file is skipped.
pub async fn import_scan_for_series(
    path: &PathBuf,
    mapping: &jumbie_shared::types::MappingRule,
    state: &std::sync::Arc<crate::api::AppState>,
) -> Result<(usize, Vec<String>), anyhow::Error> {
    use std::collections::HashMap;
    use tracing::{debug, trace};

    // SSoT: series tristate (None = "use global") falls back to the global
    // default, so resolve the global absolute-numbering default once up-front
    // for all mode-aware decisions in this function.
    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };

    // A suppressed season is a deliberate user deletion. A filesystem rescan must
    // not resurrect its episode rows, otherwise the season reappears in raw counts
    // until the next metadata sync. Its files still count toward disk usage.
    // Cleared only by an explicit restore / match. SSoT: `suppressed_seasons`.
    let numbering_mode = mapping.settings.active_mode(global_absolute).is_absolute() as i32;
    let suppressed_seasons: std::collections::HashSet<i32> = state
        .db
        .get_suppressed_seasons(&mapping.series_id, numbering_mode)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let is_suppressed = |season: &str| -> bool {
        season
            .trim()
            .parse::<i32>()
            .map(|n| suppressed_seasons.contains(&n))
            .unwrap_or(false)
    };

    // Files the user deliberately unassigned ("blocked") must not be re-adopted
    // by a rescan. Loaded once per series scan. SSoT: `blocked_files`.
    let blocked_files = state
        .db
        .get_blocked_files(&mapping.series_id)
        .await
        .unwrap_or_default();

    // Phase 1: walk the directory once, parsing every video file and grouping by
    // (season, episode) so conflicts are detected before writing anything. Per key,
    // `files` holds non-part files (expected 0 or 1) and `part_files` the part files;
    // `files.len() > 1` means a conflict.

    // The bool is whether the season was explicit (from filename) or inferred (from
    // folder/fallback), used to prefer explicit-season files during conflict resolution.
    #[derive(Default)]
    struct EpisodeSlot {
        files: Vec<(
            std::path::PathBuf,
            jumbie_shared::mapping::EpisodeInfo,
            bool,
        )>,
        part_files: Vec<(
            std::path::PathBuf,
            jumbie_shared::mapping::EpisodeInfo,
            bool,
        )>,
    }

    // Materialized `series_mappings.total_size`, accumulated during the walk so the UI
    // can read it without waiting for async file_paths. Tallied AFTER the
    // mode-aware filter so it counts only files valid for this series. entry.metadata()
    // reuses cached stat data from the walk, avoiding an extra syscall.
    let mut total_size: i64 = 0;

    let mut slots: HashMap<(String, i32), EpisodeSlot> = HashMap::new();

    // Sidecars discovered during the walk are stashed and attached after phase 2
    // (which is what creates the video episode cells), so a sidecar scanned before
    // its video attaches in the same pass. A sidecar whose cell never materializes
    // stays unlinked for manual assignment.
    let mut pending_auxiliary: Vec<(
        std::path::PathBuf,
        jumbie_shared::media_format::FileKind,
        String,
    )> = Vec::new();

    for entry in walkdir::WalkDir::new(path)
        .max_depth(5) // safety: cap traversal depth to prevent runaway filesystem crawl
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let entry_path = entry.path().to_path_buf();

        if let Some(ext) = entry_path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if jumbie_shared::media_format::is_video_ext(ext_str.as_str()) {
                let filename = match entry_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                let info = match crate::utils::parse_filename(
                    &filename,
                    crate::utils::ParseContext::FileScan,
                ) {
                    Some(i) => i,
                    None => {
                        trace!("Skipped {}: unable to parse filename", filename);
                        continue;
                    }
                };

                // Decimal episode guard
                // Episode numbers with fractional components (e.g. S01E1.5)
                // cannot be represented in the system. Skip auto-assignment
                // so the file appears unassigned for manual handling.
                if info.has_decimal_episode {
                    trace!(
                        "Skipped {}: decimal episode number in '{}'",
                        entry_path.display(),
                        filename
                    );
                    continue;
                }

                // Mode-aware filter
                // When a series uses normal mode, only accept files parsed with
                // a season number (S01E01-style patterns). When using absolute
                // mode, only accept files parsed without a season number
                // (Episode 01-style patterns). Season packs are always accepted
                // regardless of mode since they belong to a specific season.
                //
                // A season-alias match resolves the season, so it counts as an
                // explicit season for this filter (alias resolution is disabled
                // in absolute mode, which has exactly one season).
                let absolute_numbering =
                    mapping.settings.active_mode(global_absolute).is_absolute();

                let (season_str, was_explicit) = match crate::scanner::resolve_season_for_series(
                    &info,
                    &entry_path,
                    &mapping.settings,
                    absolute_numbering,
                ) {
                    crate::scanner::ResolvedSeason::Season(s, e) => (s, e),
                    crate::scanner::ResolvedSeason::Unneeded => {
                        trace!("Skipped {}: season aliases matched ambiguously", filename);
                        continue;
                    }
                };

                if !info.is_season_pack {
                    match (was_explicit, absolute_numbering) {
                        (true, true) => {
                            trace!("Skipped {}: normal pattern in absolute mode", filename);
                            continue;
                        }
                        (false, false) => {
                            trace!("Skipped {}: absolute pattern in normal mode", filename);
                            continue;
                        }
                        _ => {}
                    }
                }

                // Count the file toward the materialized disk-usage total BEFORE
                // the suppression / block filters: a suppressed season's or a
                // user-unassigned file's bytes still occupy disk. Only files that
                // fail the mode check above are excluded from this total.
                total_size += entry.metadata().map(|m| m.len() as i64).unwrap_or(0);

                // Never re-create a season the user deleted.
                if is_suppressed(&season_str) {
                    trace!(
                        "Skipped {}: season {} is suppressed for series {}",
                        filename, season_str, mapping.series_id
                    );
                    continue;
                }

                // Never re-adopt a file the user deliberately unassigned.
                if !blocked_files.is_empty() {
                    let file_size = entry.metadata().map(|m| m.len() as i64).unwrap_or(0);
                    if crate::db::blocked_files::is_file_blocked(
                        &state.db,
                        &mapping.series_id,
                        &blocked_files,
                        &filename,
                        file_size,
                        &entry_path.to_string_lossy(),
                    )
                    .await
                    {
                        trace!(
                            "Skipped {}: blocked (user-unassigned) for series {}",
                            filename, mapping.series_id
                        );
                        continue;
                    }
                }

                let key = (season_str, info.episodes.first().copied().unwrap_or(1));

                if info.part_number.is_some() {
                    slots
                        .entry(key)
                        .or_default()
                        .part_files
                        .push((entry_path, info, was_explicit));
                } else {
                    slots
                        .entry(key)
                        .or_default()
                        .files
                        .push((entry_path, info, was_explicit));
                }
            } else if let Some(kind) = jumbie_shared::media_format::file_kind_for_path(&entry_path)
                && kind.is_auxiliary()
            {
                // Auxiliary sidecars (subtitle/nfo) are not episodes themselves;
                // they attach to the episode their filename names. Resolved here and
                // attached after phase 2, since the video that creates the cell may be
                // visited later in the walk. Never creates a cell of its own.
                let Some(filename) = entry_path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let Some(info) =
                    crate::utils::parse_filename(filename, crate::utils::ParseContext::FileScan)
                else {
                    continue;
                };
                if let Some(episode_id) = crate::scanner::scan::resolve_auxiliary_cell(
                    &info,
                    &entry_path,
                    mapping,
                    global_absolute,
                ) {
                    pending_auxiliary.push((entry_path.clone(), kind, episode_id));
                }
            }
        }
    }

    // Phase 2: resolve conflicts and insert. Per slot: 0 files → skip; 1 file + parts
    // → insert episode + part rows; 2+ files → keep the single explicit-season file
    // (skip inferred), or if explicitness ties, CONFLICT and skip all. Part files with
    // no non-part parent are skipped (nothing to attach them to).

    let mut inserted = 0usize;
    let mut conflicts = Vec::new();

    // Collect keys first so we can modify slots during iteration
    let slot_keys: Vec<(String, i32)> = slots.keys().cloned().collect();

    for (season_str, episode_num) in &slot_keys {
        let (mut file_count, has_parts) = {
            let slot = &slots[&(season_str.clone(), *episode_num)];
            (slot.files.len(), !slot.part_files.is_empty())
        };

        if file_count == 0 && !has_parts {
            continue;
        }

        // Language variants of one artifact are companions, not competing files: the
        // file with no language tag is the primary and the rest attach alongside it.
        // Collapsing them to the primary here keeps the conflict logic below focused on
        // genuinely different files.
        let mut variants: Vec<PathBuf> = Vec::new();
        if file_count > 1 {
            let coalesce = {
                let slot = &slots[&(season_str.clone(), *episode_num)];
                let bases: Vec<String> = slot
                    .files
                    .iter()
                    .map(|(p, _, _)| file_variant_base(p))
                    .collect();
                !bases[0].is_empty() && bases.iter().all(|b| *b == bases[0])
            };
            if coalesce {
                let slot = slots.get_mut(&(season_str.clone(), *episode_num)).unwrap();
                let primary_idx = slot
                    .files
                    .iter()
                    .position(|(p, _, _)| !file_has_language_tag(p))
                    .unwrap_or(0);
                let primary = slot.files.remove(primary_idx);
                variants = slot.files.drain(..).map(|(p, _, _)| p).collect();
                slot.files.push(primary);
                file_count = 1;
            }
        }

        // Whether this slot is a real conflict or resolves by preferring the
        // explicit-season file over inferred ones.
        let skip_slot = if file_count > 1 {
            let slot = &slots[&(season_str.clone(), *episode_num)];
            let explicit_count = slot.files.iter().filter(|(_, _, e)| *e).count();

            if explicit_count == 1 && slot.files.len() > 1 {
                // One explicit season, others inferred — keep explicit only.
                let explicit_file = slot.files.iter().find(|(_, _, e)| *e).cloned().unwrap();
                let inferred_files: Vec<_> = slot.files.iter().filter(|(_, _, e)| !*e).collect();

                // Report inferred files as conflicts for user review.
                for (inf_path, _, _) in &inferred_files {
                    let desc = format!(
                        "S{}E{}: skipped inferred-season file '{}' in favor of explicit-season file '{}'",
                        season_str,
                        episode_num,
                        inf_path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                        explicit_file
                            .0
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("?"),
                    );
                    debug!("{}", desc);
                    conflicts.push(desc);
                }

                // Replace slot with only the explicit file.
                let part_files = std::mem::take(
                    &mut slots
                        .get_mut(&(season_str.clone(), *episode_num))
                        .unwrap()
                        .part_files,
                );
                slots.insert(
                    (season_str.clone(), *episode_num),
                    EpisodeSlot {
                        files: vec![explicit_file],
                        part_files,
                    },
                );
                false // don't skip — process normally below
            } else {
                // Real conflict — skip entirely.
                let desc = format!(
                    "S{}E{}: {} conflicting files ({})",
                    season_str,
                    episode_num,
                    file_count,
                    slot.files
                        .iter()
                        .filter_map(|(p, _, _)| p.file_name().and_then(|n| n.to_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                debug!("{}", desc);
                conflicts.push(desc);
                true
            }
        } else {
            false // single file or part files only — no conflict
        };

        if skip_slot {
            // Hash-first: still fingerprint conflicting (unassigned) files so their
            // origin is recorded even though no episode claims them. Gated on the
            // file's identity so unchanged files are not re-hashed every scan.
            if let Some(slot) = slots.get(&(season_str.clone(), *episode_num)) {
                for (p, _, _) in &slot.files {
                    crate::scanner::scan::ensure_hashed(state, p, false).await;
                }
            }
            continue;
        }

        let slot = &slots[&(season_str.clone(), *episode_num)];
        let file_count = slot.files.len();

        if file_count == 1 {
            // Normal single file (or range) → insert episode row
            let (file_path, info, _was_explicit) = &slot.files[0];
            let path_str = file_path.to_string_lossy().to_string();
            let ep_start = info.episodes.first().copied().unwrap_or(1);
            let ep_end = info.episodes.last().copied().unwrap_or(ep_start);

            // Hash-first: record the content fingerprint before assignment. Gated
            // on the file's identity (inode/device/size/mtime) so unchanged files
            // are not re-hashed on every scan — a mismatched row is rewritten.
            crate::scanner::scan::ensure_hashed(state, file_path, false).await;

            // The file's date is a content-level property (`release_info`, keyed by
            // fingerprint): record the mtime as a fallback. `organize.rs` overwrites
            // it with the authoritative source-feed date for downloaded content.
            let mtime = std::fs::metadata(&path_str)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| {
                    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
                    chrono::DateTime::from_timestamp(secs, 0).map(|dt| dt.naive_utc())
                });
            let _ = state
                .db
                .set_release_upload_date_fallback_by_path(&path_str, mtime)
                .await;

            for ep in ep_start..=ep_end {
                let episode_id = mapping.get_episode_id(season_str, ep, global_absolute)?;
                trace!("Inserting episode row (import): {}", episode_id);

                let numbering_mode =
                    mapping.settings.active_mode(global_absolute).is_absolute() as i32;

                let _ = state
                    .db
                    .insert_episode(crate::db::episodes::InsertEpisodeParams {
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
                    })
                    .await?;
            }

            // Skip fingerprinting if this file hasn't changed since the last
            // scan (inode, device, mtime, size all match).  Re-hashing every
            // file on every scan cycle would keep the scan queue permanently
            // populated, causing the UI to perpetually show "X episode(s)
            // scanning" and wasting I/O on unchanged files.
            if state
                .db
                .has_existing_fingerprint(&path_str)
                .await
                .unwrap_or(false)
            {
                trace!(
                    "Skipping fingerprint for unchanged file: '{}'",
                    file_path.display()
                );
            } else {
                // Spawn background hashing via scan queue
                let state_ref = state.clone();
                let path_for_queue = file_path.clone();
                let path_bg = path_for_queue.clone();
                let mapping_bg = mapping.clone();
                let season_bg = season_str.clone();
                let ep_start = info.episodes.first().copied().unwrap_or(1);
                let ep_end_val = ep_end;
                state
                    .scan_queue
                    .submit(path_for_queue.clone(), move || {
                        let state_ref = state_ref.clone();
                        let path_bg = path_bg.clone();
                        let _mapping_bg = mapping_bg.clone();
                        let _season_bg = season_bg.clone();
                        let _ep_start = ep_start;
                        let _ep_end_val = ep_end_val;
                        async move {
                            let _ = state_ref.db.scan_media_info_only(&path_bg).await;
                        }
                    })
                    .await;
            }

            // Count actual episode rows inserted (handles multi-episode ranges)
            let ep_first = info.episodes.first().copied().unwrap_or(1);
            inserted += (ep_end - ep_first + 1) as usize;
        }

        // Attach the slot's language variants alongside the primary just inserted.
        if !variants.is_empty() {
            let (ep_start, ep_end) = slots[&(season_str.clone(), *episode_num)]
                .files
                .first()
                .map(|(_, info, _)| {
                    let start = info.episodes.first().copied().unwrap_or(1);
                    (start, info.episodes.last().copied().unwrap_or(start))
                })
                .unwrap_or((1, 1));
            for variant in &variants {
                crate::scanner::scan::ensure_hashed(state, variant, false).await;
                let variant_path = variant.to_string_lossy().to_string();
                for ep in ep_start..=ep_end {
                    if let Ok(episode_id) = mapping.get_episode_id(season_str, ep, global_absolute)
                    {
                        let _ = state
                            .db
                            .associate_linked_file(&episode_id, &variant_path)
                            .await;
                    }
                }
            }
        }

        // Part files: registered whenever they exist — including a parts-only
        // episode with no non-part sibling. SSoT: `scanner::parts::register_part_file`
        // (also used by `scan_directory`), which creates the parent row with
        // file_path = NULL, upserts the part row, and queues fingerprinting so the
        // two scan paths agree.
        if has_parts {
            for (part_path, part_info, _was_explicit) in &slot.part_files {
                let part_num = part_info.part_number.unwrap_or(1);
                if let Err(e) = crate::scanner::parts::register_part_file(
                    state,
                    mapping,
                    season_str,
                    *episode_num,
                    global_absolute,
                    part_path,
                    part_num,
                )
                .await
                {
                    tracing::debug!("skipping part file {}: {}", part_path.display(), e);
                    continue;
                }
            }
            // Preserve the historical count semantics: each part counts as one.
            inserted += slot.part_files.len();
        }
    }

    if !conflicts.is_empty() {
        debug!(
            "Import scan for '{}' found {} conflict(s): {:?}",
            mapping.target_title,
            conflicts.len(),
            conflicts
        );
    }

    // Attach the stashed sidecars after phase 2 has created every video episode
    // cell. A sidecar whose cell does not exist stays unlinked (unassigned).
    for (aux_path, kind, episode_id) in pending_auxiliary {
        crate::scanner::scan::attach_auxiliary_to_cell(state, &aux_path, kind, &episode_id).await;
    }

    // Write the accumulated size to series_mappings.total_size so the library view can
    // read it without waiting for async file_paths. The SSoT write for stat queries.
    if let Err(e) = state
        .db
        .set_series_total_size(&mapping.series_id, total_size)
        .await
    {
        tracing::error!(
            "Failed to persist total_size for '{}' ({}): {}",
            mapping.target_title,
            mapping.series_id,
            e
        );
    }

    Ok((inserted, conflicts))
}

/// Scan a specific series directory, identifying episodes by SXXEXX patterns and
/// season folder structure. All video files in the directory are assumed to belong
/// to this series — no series_key matching is performed.
///
/// This is the **SSoT** for scanning a known series directory. Use this instead of
/// [`scan_directory`] when you know which series the files belong to (e.g., after
/// unhiding a series, refreshing a series, or during periodic background scanning
/// of individual series directories).
///
/// [`scan_directory`] is retained for download-directory scanning where multiple
/// series' files may be mixed and series_key matching is required to disambiguate.
///
/// # What it does
/// 1. Calls [`import_scan_for_series`] to walk the directory, parse SXXEXX patterns
///    from filenames, and upsert episode rows for the given series.
/// 2. Updates the series' `last_known_dir_mtime` so the periodic scanner can skip
///    unchanged directories on subsequent cycles.
///
/// # Returns
/// The number of episode rows inserted.
pub async fn scan_series_directory(
    path: &PathBuf,
    mapping: &jumbie_shared::types::MappingRule,
    state: &Arc<AppState>,
) -> Result<usize, anyhow::Error> {
    let (count, _conflicts) = import_scan_for_series(path, mapping, state).await?;

    // Update last_known_dir_mtimes so the periodic background scanner can skip
    // unchanged directories on subsequent cycles.
    // Uses max_depth = 5 to match the scanner's WalkDir depth.
    update_last_known_mtimes(state, &mapping.series_id, path, 5).await;

    Ok(count)
}

/// Update a series' `last_known_dir_mtimes` after a scan, so the periodic scanner
/// can skip unchanged directories on subsequent cycles.
/// Stores mtimes for the root dir and every subdirectory (up to match_depth).
async fn update_last_known_mtimes(
    state: &Arc<AppState>,
    series_id: &str,
    path: &Path,
    match_depth: usize,
) {
    let mtimes = crate::scanner::collect_dir_mtimes(path, match_depth);
    if let Some(mut latest) = state.db.get_series_mapping(series_id).await.ok().flatten() {
        latest.settings.last_known_dir_mtimes = mtimes;
        let _ = state.db.upsert_series_mapping(series_id, &latest).await;
    }
}
