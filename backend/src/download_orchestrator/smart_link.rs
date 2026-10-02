// Orphan adoption runs BEFORE smart_link and targets files found in the download
// directory with no episode_id linked yet — e.g. a backend restart between download
// completion and linking (fingerprint state='complete' but episode_id NULL). Adoption
// re-parses the filename and attaches it to the correct episode.
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use anyhow::Result;
use tracing::{debug, info, trace, warn};

use crate::db::automatic_profiles::SubmitterScoreInput;
use crate::file_manager::file_ops::remove_empty_download_dir;
use crate::organizer::ContentOrganizer;
use jumbie_shared::types::EpisodeStatus;

use super::{RedundantHandling, redundant_file_handling};

/// Outcome of a permissive link attempt (series-scan or user-requested overspill).
enum PermissiveLink {
    /// The file was linked to an episode.
    Linked,
    /// The target episode already points at a different, live file. The incoming
    /// file is redundant and belongs in `unknown_files` (unmatched), where
    /// `unexpected_files_handling` decides whether to keep or delete it — the
    /// user intentionally downloaded this series-level release.
    AlreadyPresent,
    /// No episode identity could be derived; the caller should treat the file as
    /// unknown.
    Unresolvable,
    /// Several season overrides claim the file, so its identity is ambiguous.
    /// The caller keeps it for manual review instead of guessing or deleting.
    AmbiguousSeason,
}

/// Result of smart-linking a download's files to episodes.
#[derive(Default)]
pub(crate) struct SmartLinkOutcome {
    /// The download was assigned to episodes at queue time but no intended file
    /// could be placed.
    pub unplaced_assignment: bool,
    /// Files left in the download dir for manual review.
    pub kept_for_review: usize,
}

/// Inputs for [`ContentOrganizer::link_permissive_episode`]: the parsed file plus
/// the series/mapping context resolved once per download. Grouped into a struct
/// so the call sites stay readable.
struct PermissiveLinkRequest<'a> {
    file_path: &'a str,
    info: &'a jumbie_shared::mapping::types::EpisodeInfo,
    queue_item: &'a jumbie_shared::types::DownloadQueueItem,
    series_id: &'a str,
    mapping: Option<&'a jumbie_shared::types::MappingRule>,
    global_absolute: bool,
    season_str: &'a str,
    existing_episode_files: &'a mut HashMap<String, String>,
}

/// Inputs for [`ContentOrganizer::assign_download_file`].
struct AssignDownloadFileRequest<'a> {
    file_path: &'a str,
    parsed: Option<&'a jumbie_shared::mapping::EpisodeInfo>,
    size: Option<i64>,
    series_id: &'a str,
    season_str: &'a str,
    absolute: bool,
    queue_item: &'a jumbie_shared::types::DownloadQueueItem,
    episode_id: &'a str,
    local_episode: i32,
}

impl ContentOrganizer {
    pub(crate) fn adopt_orphans<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let orphans = self.db.get_orphan_files().await?;
            if orphans.is_empty() {
                return Ok(());
            }

            info!(
                "Found {} orphan files, attempting to adopt...",
                orphans.len()
            );

            let orphans: Vec<PathBuf> = orphans.into_iter().map(PathBuf::from).collect();

            // SSoT: effective numbering mode for episode-ID generation.
            let global_absolute = self.global_absolute_default().await;

            // Blocked files are cached per series so the loop does not re-query.
            let mut blocked_cache: std::collections::HashMap<
                String,
                Vec<crate::db::blocked_files::BlockedFile>,
            > = std::collections::HashMap::new();

            // Re-parse each filename for series_key + episode number, then look up the
            // mapping to compute the episode_id. Unparseable files stay orphaned and are
            // retried next cycle.
            for path in orphans {
                trace!("Processing standard orphan: '{}'", path.display());
                // A fingerprint can outlive its file (the client removed it); linking a
                // path that no longer exists would create a bogus episode.
                if !path.is_file() {
                    debug!("Skipping orphan {} — file no longer exists", path.display());
                    continue;
                }
                let orphan_path = path.to_string_lossy().to_string();
                if let Some(filename) = path.file_name().and_then(|n| n.to_str())
                    && let Some(info) =
                        crate::utils::parse_filename(filename, crate::utils::ParseContext::FileScan)
                    && let Some((_id, mapping)) = self
                        .db
                        .get_mapping_by_key(&info.series_key)
                        .await
                        .unwrap_or(None)
                {
                    // Discovery must not re-adopt a file the user deliberately
                    // unassigned (SSoT: `blocked_files`).
                    if !blocked_cache.contains_key(&mapping.series_id) {
                        let list = self
                            .db
                            .get_blocked_files(&mapping.series_id)
                            .await
                            .unwrap_or_default();
                        blocked_cache.insert(mapping.series_id.clone(), list);
                    }
                    if let Some(blocks) = blocked_cache.get(&mapping.series_id)
                        && !blocks.is_empty()
                    {
                        let size = std::fs::metadata(&path)
                            .map(|m| m.len() as i64)
                            .unwrap_or(0);
                        if crate::db::blocked_files::is_file_blocked(
                            &self.db,
                            &mapping.series_id,
                            blocks,
                            filename,
                            size,
                            &orphan_path,
                        )
                        .await
                        {
                            debug!("Skipping orphan {}: blocked (user-unassigned)", orphan_path);
                            continue;
                        }
                    }

                    // Download-side season resolution: alias titles, then (for
                    // offset seasons) the season-number alias, then the range.
                    let absolute = mapping.settings.active_mode(global_absolute).is_absolute();
                    let (season_str, episode_offset) =
                        match crate::source_processor::identification::resolve_release_season(
                            &mapping.settings,
                            info.seasons.first().copied(),
                            &info.series_key,
                            &info,
                            absolute,
                        ) {
                            crate::source_processor::identification::ResolvedReleaseSeason::Season {
                                season_key,
                                offset,
                            } => (season_key.to_string(), offset),
                            crate::source_processor::identification::ResolvedReleaseSeason::Ambiguous => {
                                // Season aliases matched ambiguously: route to review
                                // like every other unassignable file, so it isn't
                                // retried forever.
                                debug!(
                                    "Orphan {}: season aliases matched ambiguously — marking for review",
                                    orphan_path
                                );
                                super::mark_file_for_review(
                                    &self.db,
                                    &orphan_path,
                                    Some(mapping.series_id.as_str()),
                                    super::ReviewReason::Unmatched,
                                )
                                .await;
                                continue;
                            }
                            crate::source_processor::identification::ResolvedReleaseSeason::Unclaimed => {
                                // No override claims it: use the filename/folder season
                                // (never the season aliases, already tried above), with
                                // the absolute season as the fallback in absolute mode.
                                let fallback = absolute
                                    .then_some(jumbie_shared::mapping::ABSOLUTE_SEASON_NUM);
                                let (season_str, _) =
                                    crate::scanner::resolve_season_raw(&info, &path, fallback);
                                let offset = mapping
                                    .effective_episode_offset(&season_str, global_absolute);
                                (season_str, offset)
                            }
                        };
                    // No episode number parsed → leave the file orphaned for the
                    // next cycle rather than inventing episode 1.
                    let Some(&source_episode) = info.episodes.first() else {
                        debug!(
                            "Skipping orphan {} — no episode number parsed from filename",
                            orphan_path
                        );
                        continue;
                    };
                    // Parsed numbers are release/source-numbered; convert to the local
                    // number the episode ID is built from.
                    let episode_num = jumbie_shared::mapping::source_to_local_episode(
                        source_episode,
                        episode_offset,
                    );
                    let ep_id =
                        match mapping.get_episode_id(&season_str, episode_num, global_absolute) {
                            Ok(id) => id,
                            Err(e) => {
                                debug!("skipping non-numeric season: {e}");
                                continue;
                            }
                        };

                    // Ensure the target episode row exists so the association's
                    // foreign key holds (an orphan may name an episode not yet created).
                    let season_num =
                        jumbie_shared::mapping::resolve_season_num(&season_str, absolute)
                            .unwrap_or(jumbie_shared::mapping::DEFAULT_SEASON_NUM);
                    let _ = self
                        .db
                        .ensure_episode_row(
                            &ep_id,
                            &mapping.series_id,
                            season_num,
                            episode_num,
                            absolute as i32,
                        )
                        .await;

                    if let Err(e) = self.db.link_file_episode(&orphan_path, &ep_id).await {
                        warn!("Failed to link orphan {} to {}: {}", orphan_path, ep_id, e);
                    } else {
                        debug!("Adopted orphan {} -> {}", orphan_path, ep_id);
                    }
                }
            }

            Ok(())
        })
    }

    /// Enumerate all video files in a download content directory and link each to the correct episode.
    ///
    /// **Pack case** (`episode_end` is set on queue item, e.g. S01E01-04):
    /// The download may contain individual episode files (`S01E01.mkv`, `S01E02.mkv`, …).
    /// Each file is parsed and linked to its own `episode_id`. Episode DB records are created
    /// for any episodes in the range that don't yet have one.
    ///
    /// **Multipart case** (queue item is a single episode, file contains part suffix):
    /// Files like `S03E03 - part 1.mkv` are linked to the same `episode_id` AND registered in
    /// `episode_files` via `upsert_episode_part`.
    ///
    /// **Fallback**: anything that doesn't parse cleanly falls back to linking to the queue
    /// item's `episode_id`.
    // Torrents arrive in unpredictable layouts (samples, named episodes, parts), so
    // every file is parsed independently; anything failing all three cases falls to
    // unexpected_files_handling.
    pub(crate) fn smart_link_downloaded_files<'a>(
        &'a self,
        content_path: &'a str,
        queue_item: &'a jumbie_shared::types::DownloadQueueItem,
    ) -> Pin<Box<dyn Future<Output = Result<SmartLinkOutcome>> + Send + 'a>> {
        Box::pin(async move {
            let mut video_files: Vec<PathBuf> = Vec::new();
            let mut aux_files: Vec<PathBuf> = Vec::new();
            let mut unknown_files: Vec<PathBuf> = Vec::new();
            let mut unneeded_files: Vec<PathBuf> = Vec::new();
            let base = PathBuf::from(content_path);

            // Read config from the DB — self.config is TOML-only and never synced with
            // DB-managed sections.
            let general_cfg = self.db.get_general_config().await.unwrap_or_default();
            let auto_enabled = general_cfg.automatic_profiles.enabled;
            let auto_categories = general_cfg.automatic_profiles.categories;
            let unexpected_handling = general_cfg.unexpected_files_handling;

            // Normalize to the containing download directory — qBittorrent returns the
            // file path for single-file torrents but cleanup boundary logic needs the dir.
            let content_dir = if base.is_dir() {
                base.clone()
            } else {
                base.parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or(base.clone())
            };
            // Configured download roots bound empty-folder cleanup so the per-download
            // UUID staging folder (one level above the content dir) is culled too.
            let download_roots = self.download_roots().await;

            if base.is_file() {
                // Single-file torrent
                let ext = crate::utils::get_extended_extension(base.to_str().unwrap_or(""));
                let main_ext = ext.split('.').next_back().unwrap_or(&ext).to_lowercase();
                if jumbie_shared::media_format::is_video_ext(&main_ext) {
                    video_files.push(base.clone());
                    trace!("Single file: video file found: '{}'", base.display());
                } else if jumbie_shared::media_format::is_auxiliary_ext(&main_ext) {
                    aux_files.push(base.clone());
                    trace!("Single file: auxiliary file found: '{}'", base.display());
                } else {
                    unknown_files.push(base.clone());
                    trace!("Single file: unknown file found: '{}'", base.display());
                }
            } else {
                // Directory download — walk recursively
                let mut dirs = vec![base.clone()];
                while let Some(dir) = dirs.pop() {
                    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            let p = entry.path();
                            if p.is_dir() {
                                dirs.push(p);
                            } else {
                                let ext =
                                    crate::utils::get_extended_extension(p.to_str().unwrap_or(""));
                                let main_ext =
                                    ext.split('.').next_back().unwrap_or(&ext).to_lowercase();
                                if jumbie_shared::media_format::is_video_ext(&main_ext) {
                                    trace!("Walk: video file found: '{}'", p.display());
                                    video_files.push(p);
                                } else if jumbie_shared::media_format::is_auxiliary_ext(&main_ext) {
                                    trace!("Walk: auxiliary file found: '{}'", p.display());
                                    aux_files.push(p);
                                } else {
                                    // All non-video/non-subtitle files enter unknown_files.
                                    // IGNORED_UNKNOWN_EXTS filtering and file_patterns matching
                                    // are handled per-category during offense recording.
                                    trace!("Walk: unknown file found: '{}'", p.display());
                                    unknown_files.push(p);
                                }
                            }
                        }
                    }
                }
            }

            if video_files.is_empty() && (!aux_files.is_empty() || !unknown_files.is_empty()) {
                debug!(
                    "No video files found; {} auxiliary file(s) and {} unknown file(s) present",
                    aux_files.len(),
                    unknown_files.len()
                );
            }

            if video_files.is_empty() && aux_files.is_empty() {
                if !unknown_files.is_empty() {
                    // Payload contains ONLY unexpected files — delete the whole content dir/file
                    tracing::warn!(
                        "Download '{}' contains only unexpected files — discarding entire payload",
                        queue_item.media_name
                    );
                    let base_path = PathBuf::from(content_path);
                    if base_path.is_dir() {
                        let _ = tokio::fs::remove_dir_all(&base_path).await;
                        // Dir torrent: content dir removed, but the UUID staging
                        // folder (its parent) may now be empty — cull it too.
                        remove_empty_download_dir(&content_dir, &download_roots).await;
                    } else if base_path.is_file() {
                        let _ = tokio::fs::remove_file(&base_path).await;
                        // Single-file torrent: file removed, UUID folder still
                        // exists. Clean up UUID folder if now empty.
                        remove_empty_download_dir(&content_dir, &download_roots).await;
                    }
                    return Ok(SmartLinkOutcome::default());
                }
                // No video, no sub, no unknown — nothing to do (e.g. metadata-only download)
                self.db
                    .link_files_by_prefix(
                        content_path,
                        queue_item.episode_id.as_deref().unwrap_or(""),
                    )
                    .await?;
                return Ok(SmartLinkOutcome::default());
            }

            let season_str = queue_item
                .season
                .clone()
                .unwrap_or_else(|| "01".to_string());
            let series_title_for_insert = queue_item.series_title.clone();

            // SSoT: series_id is always set for new items; legacy items with none fall back
            // to "unknown".
            let series_id = if !queue_item.series_id.is_empty() {
                queue_item.series_id.clone()
            } else {
                "unknown".to_string()
            };

            // SSoT: force-keep and permissive fill apply only to a series-level release
            // the user explicitly picked (no episode context). An episode-level release
            // — manual or automatic — follows the configured keep/delete policy exactly
            // like an automatic search; only the searched episode's replacement is
            // manual-authoritative.
            let force_keep_review = queue_item.is_manual && queue_item.episode.is_none();

            // Prefer the DB-stored submitter (set at queue time); fall back to
            // title parsing for legacy items queued before submitter was tracked.
            let submitter: String = self
                .db
                .get_episode_submitter(queue_item.episode_id.as_deref().unwrap_or(""))
                .await
                .ok()
                .flatten()
                .or_else(|| crate::utils::extract_submitter(&queue_item.media_name))
                .unwrap_or_else(|| "Unknown".to_string());

            // SSoT: episode intentions are recorded at queue time by download_winner;
            // absent for legacy/manual downloads (fall back to heuristics).
            let intentions: Vec<jumbie_shared::types::EpisodeIntention> = queue_item
                .episode_intentions
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok())
                .unwrap_or_default();
            // Files are matched against the release-numbered key; `episode_num` stays
            // in local/DB space for the episode row writes. Keep and overspill are
            // indexed separately so a source number shared by both cannot cross-wire.
            let mut keep_source_counts: std::collections::HashMap<i32, usize> =
                std::collections::HashMap::new();
            for intention in intentions.iter().filter(|i| i.keep) {
                *keep_source_counts
                    .entry(intention.source_episode_num)
                    .or_default() += 1;
            }
            // A source number claimed by two keep-intentions (e.g. two target seasons)
            // is ambiguous by number alone, so it is left to the single-target and
            // permissive paths rather than guessed.
            let keep_by_source: std::collections::HashMap<
                i32,
                &jumbie_shared::types::EpisodeIntention,
            > = intentions
                .iter()
                .filter(|i| i.keep && keep_source_counts[&i.source_episode_num] == 1)
                .map(|i| (i.source_episode_num, i))
                .collect();
            let drop_by_source: std::collections::HashMap<
                i32,
                &jumbie_shared::types::EpisodeIntention,
            > = intentions
                .iter()
                .filter(|i| !i.keep)
                .map(|i| (i.source_episode_num, i))
                .collect();
            let keep_intention_ids: std::collections::HashSet<&str> = intentions
                .iter()
                .filter(|i| i.keep)
                .map(|i| i.episode_id.as_str())
                .collect();
            // A download assigned to exactly one episode is authoritative: its single
            // file (or the parts of that file) belongs there whatever the name parses to.
            let single_target: Option<&jumbie_shared::types::EpisodeIntention> =
                if keep_intention_ids.len() == 1 {
                    intentions.iter().find(|i| i.keep)
                } else {
                    None
                };
            let has_intentions = !intentions.is_empty();

            // SSoT: effective numbering mode for episode-ID generation. The mapping
            // may be absent for legacy "unknown" series, in which case the global
            // default decides.
            let global_absolute = self.global_absolute_default().await;
            let mapping = self.db.get_series_mapping(&series_id).await.ok().flatten();
            let absolute = mapping
                .as_ref()
                .map(|m| m.settings.active_mode(global_absolute).is_absolute())
                .unwrap_or(global_absolute);
            // Prefetch every episode of this series that already has a file so the
            // permissive path's "already present" guard needs no per-file query.
            let mut existing_episode_files = self
                .db
                .get_episode_file_paths_for_series(&series_id)
                .await
                .unwrap_or_default();

            // Parse once: the "every file is a part" check spans all files, so it
            // cannot be decided per iteration.
            let parsed_videos: Vec<(PathBuf, Option<jumbie_shared::mapping::EpisodeInfo>)> =
                video_files
                    .iter()
                    .map(|path| {
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        (
                            path.clone(),
                            crate::utils::parse_filename(
                                name,
                                crate::utils::ParseContext::FileScan,
                            ),
                        )
                    })
                    .collect();
            // Several files that are all parts of one video belong to a single episode.
            let all_video_files_are_parts = parsed_videos.len() > 1
                && parsed_videos
                    .iter()
                    .all(|(_, info)| info.as_ref().and_then(|i| i.part_number).is_some());

            // Intended video files that could not be placed. Resolved after the loop:
            // a download where nothing was linked is a matching failure to keep for
            // review, not unexpected content to delete.
            let mut unresolved_intended: Vec<PathBuf> = Vec::new();
            let mut intended_linked: usize = 0;

            for (file_path, parsed) in &parsed_videos {
                let file_path_str = file_path.to_string_lossy().to_string();
                let filename = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let size = tokio::fs::metadata(file_path)
                    .await
                    .ok()
                    .map(|m| m.len() as i64);

                // Overspill: a file is discarded only when it is not wanted by any keep
                // intention. A file whose first parsed episode is overspill but which also
                // covers a kept episode (a multi-episode file with no way to split) is
                // retained and matched below rather than dropped.
                let file_keeps_any = parsed.as_ref().is_some_and(|info| {
                    info.episodes
                        .iter()
                        .any(|ep| keep_by_source.contains_key(ep))
                });
                if !file_keeps_any
                    && parsed
                        .as_ref()
                        .and_then(|info| info.episodes.first())
                        .is_some_and(|ep| drop_by_source.contains_key(ep))
                {
                    trace!("Skipping unneeded intention file {}", filename);
                    unneeded_files.push(file_path.clone());
                    continue;
                }

                // Fingerprint with state="complete" — organize_completed expects it for
                // files that need moving to their final destination.
                let (hash_val, _hash, media_info) =
                    self.db.update_file_fingerprint(file_path, "complete").await;
                if let Some(info) = media_info {
                    if auto_enabled {
                        self.check_automatic_profile_rules(
                            &submitter,
                            filename,
                            &info,
                            &auto_categories,
                            Some(&hash_val),
                            true,
                        )
                        .await;
                    }

                    if let Ok(media_json) = serde_json::to_string(&info) {
                        let _ = self
                            .db
                            .record_media_scan(&submitter, filename, None, None, None, &media_json)
                            .await;
                    }
                }

                // A multi-episode file (one file covering several episodes) has no way
                // to split, so it is kept whole: assign it to every episode it covers,
                // replacing overlapping existing files. Files whose episodes are all
                // overspill were already dropped above. Targets come from the recorded
                // intentions (episode IDs are authoritative there), with the mapping as a
                // fallback for covered episodes that carry no intention.
                if file_keeps_any
                    && let Some(info) = parsed.as_ref()
                    && info.episodes.len() > 1
                    && let Ok(season) =
                        jumbie_shared::mapping::resolve_season_num(&season_str, absolute)
                {
                    // Coverage is authoritative for an indivisible file: this branch is
                    // only reached when the file covers a keep-intention (for an episode
                    // search, the searched episode), so the whole range is placed as a
                    // unit, replacing any overlapping files. A range that does NOT cover
                    // the searched episode never reaches here — it is handled by the
                    // permissive path, which fills only the free episodes it covers.
                    let targets: Vec<(String, i32)> = info
                        .episodes
                        .iter()
                        .filter_map(|&source_ep| {
                            if let Some(i) = keep_by_source
                                .get(&source_ep)
                                .copied()
                                .or_else(|| drop_by_source.get(&source_ep).copied())
                            {
                                return Some((i.episode_id.clone(), i.episode_num));
                            }
                            let m = mapping.as_ref()?;
                            // A mapping without a series id cannot build a valid episode id.
                            if m.series_id.is_empty() {
                                return None;
                            }
                            let offset = m.effective_episode_offset(&season_str, global_absolute);
                            let local =
                                jumbie_shared::mapping::source_to_local_episode(source_ep, offset);
                            m.get_episode_id(&season_str, local, absolute)
                                .ok()
                                .map(|id| (id, local))
                        })
                        .collect();
                    if self
                        .assign_multi_episode_file(
                            &file_path_str,
                            &targets,
                            &series_id,
                            &series_title_for_insert,
                            season,
                            i32::from(absolute),
                        )
                        .await
                    {
                        intended_linked += 1;
                        debug!(
                            "Linked multi-episode file {} covering {:?}",
                            filename, info.episodes
                        );
                    }
                    continue;
                }

                // Resolve the file to one episode. A file matching a keep-intention is
                // assigned there. The single-target fast path maps a *single-episode*
                // release onto the searched episode even when its parsed number/season
                // differs; a multi-episode (range) file is never collapsed onto a target
                // it does not cover — its coverage is authoritative and handled by the
                // branches above/below.
                let matched_intention = parsed.as_ref().and_then(|info| {
                    info.episodes
                        .iter()
                        .find_map(|ep| keep_by_source.get(ep).copied())
                });
                let single_target_file = single_target.filter(|_| {
                    (parsed_videos.len() == 1 || all_video_files_are_parts)
                        && parsed.as_ref().is_some_and(|info| info.episodes.len() == 1)
                });
                if let Some(intention) = matched_intention.or(single_target_file) {
                    if self
                        .assign_download_file(AssignDownloadFileRequest {
                            file_path: &file_path_str,
                            parsed: parsed.as_ref(),
                            size,
                            series_id: &series_id,
                            season_str: &season_str,
                            absolute,
                            queue_item,
                            episode_id: &intention.episode_id,
                            local_episode: intention.episode_num,
                        })
                        .await
                    {
                        intended_linked += 1;
                        let log_ref = jumbie_shared::types::EpisodeLogRef::new(
                            &series_title_for_insert,
                            &season_str,
                            intention.episode_num,
                            None,
                        );
                        debug!(
                            "Linked episode {}: {} -> {}",
                            log_ref, filename, intention.episode_id
                        );
                    }
                    continue;
                }

                // Multi-part file with no resolved episode is ambiguous.
                if parsed.as_ref().and_then(|info| info.part_number).is_some() {
                    if has_intentions {
                        unresolved_intended.push(file_path.clone());
                    } else {
                        unknown_files.push(file_path.clone());
                    }
                    continue;
                }

                // Unparseable file: link directly only when exactly one video file and a
                // valid episode_id exist; multiple unparseable files are ambiguous.
                if parsed.is_none() {
                    if parsed_videos.len() == 1
                        && let Some(ep_id) = queue_item.episode_id.as_deref()
                        && !ep_id.is_empty()
                    {
                        trace!(
                            "File '{}' unparseable — linking single-file download to episode_id {} directly",
                            filename, ep_id
                        );
                        let _ = self.db.link_file_episode(&file_path_str, ep_id).await;
                        continue;
                    }
                    if has_intentions {
                        unresolved_intended.push(file_path.clone());
                    } else {
                        unknown_files.push(file_path.clone());
                    }
                    continue;
                }
                let info = parsed.as_ref().unwrap();

                // SERIES-SCAN: episode is None means queued from series-level search.
                // No intentions exist — link every parsed file permissively.
                if queue_item.episode.is_none() {
                    if info.episodes.first().is_some_and(|ep| *ep > 0) {
                        match self
                            .link_permissive_episode(PermissiveLinkRequest {
                                file_path: &file_path_str,
                                info,
                                queue_item,
                                series_id: &series_id,
                                mapping: mapping.as_ref(),
                                global_absolute,
                                season_str: &season_str,
                                existing_episode_files: &mut existing_episode_files,
                            })
                            .await
                        {
                            PermissiveLink::Linked => continue,
                            PermissiveLink::AlreadyPresent => {
                                match redundant_file_handling(force_keep_review) {
                                    RedundantHandling::Unmatched => {
                                        unknown_files.push(file_path.clone())
                                    }
                                    RedundantHandling::Unneeded => {
                                        unneeded_files.push(file_path.clone())
                                    }
                                }
                                continue;
                            }
                            PermissiveLink::Unresolvable => {}
                            PermissiveLink::AmbiguousSeason => {
                                // Ambiguous season identity: keep for manual review rather
                                // than guessing or letting unexpected_files_handling delete it.
                                super::mark_file_for_review(
                                    &self.db,
                                    &file_path_str,
                                    Some(series_id.as_str()),
                                    super::ReviewReason::Unmatched,
                                )
                                .await;
                                continue;
                            }
                        }
                    }
                    unknown_files.push(file_path.clone());
                    continue;
                }

                // Parsed file matches no intention. It is classified after the loop:
                // placed-and-kept, or held for review only when nothing else linked.
                // Series-level releases are filled permissively in the branch above;
                // an episode-level release (manual or automatic) treats extras exactly
                // like an automatic search does.
                if has_intentions {
                    trace!(
                        "File '{}' parsed to ep {:?} but no matching intention",
                        filename,
                        info.episodes.first()
                    );
                    unresolved_intended.push(file_path.clone());
                    continue;
                }

                // Queue item has an episode_id but no intentions — link the parsed file
                // to the queue item's episode directly.
                if let Some(ep_id) = queue_item.episode_id.as_deref()
                    && !ep_id.is_empty()
                {
                    let _ = sqlx::query("UPDATE episodes SET status = ? WHERE episode_id = ?")
                        .bind(EpisodeStatus::Downloaded.as_str())
                        .bind(ep_id)
                        .execute(self.db.get_pool())
                        .await;
                    // A language sibling is reconciled (kept alongside, or promoted to
                    // the slot); any other file becomes the main file under the
                    // organization collision strategy.
                    let writes = matches!(
                        self.db
                            .reconcile_ingested_slot(ep_id, &file_path_str, None, None)
                            .await,
                        Ok(ingest) if ingest.outcome.caller_writes_slot()
                    );
                    if writes {
                        let _ = self
                            .db
                            .write_slot_displacing(ep_id, &file_path_str, None, None)
                            .await;
                    }
                    if let Err(e) = self.db.link_file_episode(&file_path_str, ep_id).await {
                        tracing::warn!(
                            "Legacy fallback: failed to link file {} -> {}: {}",
                            file_path_str,
                            ep_id,
                            e
                        );
                        unknown_files.push(file_path.clone());
                    } else {
                        info!(
                            "Legacy fallback: linked {} -> {} (no intentions)",
                            filename, ep_id
                        );
                        // Re-evaluate monitor status — file is now assigned to the
                        // episode, which changes has_file for Missing/Existing modes.
                        if let Ok(Some(sid)) = sqlx::query_scalar::<_, String>(
                            "SELECT series_id FROM episodes WHERE episode_id = ?",
                        )
                        .bind(ep_id)
                        .fetch_optional(self.db.get_pool())
                        .await
                        {
                            self.refresh_monitor_status_for_episode(&sid, ep_id).await;
                        }
                    }
                } else {
                    trace!(
                        "File '{}' has no intentions and no episode_id — treating as unknown",
                        filename
                    );
                    unknown_files.push(file_path.clone());
                }
            }

            for file_path in &aux_files {
                let file_path_str = file_path.to_string_lossy().to_string();
                let filename = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                let parsed =
                    crate::utils::parse_filename(filename, crate::utils::ParseContext::FileScan);

                // SERIES-SCAN: same permissive handling for subs.
                if queue_item.episode.is_none() {
                    if let Some(ref info) = parsed
                        && info.episodes.first().is_some_and(|ep| *ep > 0)
                    {
                        match self
                            .link_permissive_episode(PermissiveLinkRequest {
                                file_path: &file_path_str,
                                info,
                                queue_item,
                                series_id: &series_id,
                                mapping: mapping.as_ref(),
                                global_absolute,
                                season_str: &season_str,
                                existing_episode_files: &mut existing_episode_files,
                            })
                            .await
                        {
                            PermissiveLink::Linked => continue,
                            PermissiveLink::AlreadyPresent => {
                                match redundant_file_handling(force_keep_review) {
                                    RedundantHandling::Unmatched => {
                                        unknown_files.push(file_path.clone())
                                    }
                                    RedundantHandling::Unneeded => {
                                        unneeded_files.push(file_path.clone())
                                    }
                                }
                                continue;
                            }
                            PermissiveLink::Unresolvable => {}
                            PermissiveLink::AmbiguousSeason => {
                                // Ambiguous season identity: keep for manual review rather
                                // than guessing or letting unexpected_files_handling delete it.
                                super::mark_file_for_review(
                                    &self.db,
                                    &file_path_str,
                                    Some(series_id.as_str()),
                                    super::ReviewReason::Unmatched,
                                )
                                .await;
                                continue;
                            }
                        }
                    }
                    unknown_files.push(file_path.clone());
                    continue;
                }

                // Intention-based routing for subtitles.
                if has_intentions {
                    if let Some(intention) = parsed
                        .as_ref()
                        .and_then(|info| info.episodes.first())
                        .and_then(|ep| keep_by_source.get(ep))
                    {
                        if let Err(e) = self
                            .db
                            .link_file_episode(&file_path_str, &intention.episode_id)
                            .await
                        {
                            tracing::warn!(
                                "Failed to link subtitle {} -> {}: {}",
                                file_path_str,
                                intention.episode_id,
                                e
                            );
                        } else {
                            trace!("Linked subtitle {} -> {}", filename, intention.episode_id);
                        }
                        continue;
                    }
                    // keep=false → unneeded (same as video). Honour the
                    // unneeded_episodes_handling preference instead of deleting
                    // unconditionally.
                    if let Some(intention) = parsed
                        .as_ref()
                        .and_then(|info| info.episodes.first())
                        .and_then(|ep| drop_by_source.get(ep))
                    {
                        trace!(
                            "Unneeded subtitle for ep {} routed by preference: '{}'",
                            intention.episode_num,
                            file_path.display()
                        );
                        unneeded_files.push(file_path.clone());
                        continue;
                    }
                    // Assigned download, unmatched subtitle: let the post-loop guard
                    // decide, so a download that placed nothing keeps its subs too.
                    unresolved_intended.push(file_path.clone());
                    continue;
                }

                // No intentions but episode is known — skip (should not occur).
                trace!(
                    "Subtitle '{}' has no intentions and episode is Some — skipping",
                    filename
                );
                unknown_files.push(file_path.clone());
            }

            // A download assigned at queue time that linked nothing is a matching
            // failure, not unexpected payload: keep the intended files for review
            // instead of letting unexpected_files_handling delete them.
            let mut kept_for_review = 0usize;
            let unplaced_assignment = !keep_intention_ids.is_empty() && intended_linked == 0;
            if unplaced_assignment && !unresolved_intended.is_empty() {
                kept_for_review = unresolved_intended.len();
                tracing::warn!(
                    "Download '{}' was assigned {} episode(s) but no intended file matched — \
                     keeping {} file(s) for review",
                    queue_item.media_name,
                    keep_intention_ids.len(),
                    kept_for_review
                );
                for file in &unresolved_intended {
                    super::mark_file_for_review(
                        &self.db,
                        &file.to_string_lossy(),
                        Some(series_id.as_str()),
                        super::ReviewReason::Unmatched,
                    )
                    .await;
                }
            } else {
                unknown_files.append(&mut unresolved_intended);
            }

            // Handle Unknown/Unexpected Files
            if !unknown_files.is_empty() {
                // Offense recording (only when profiles are active).
                if auto_enabled {
                    let unexpected_cats: Vec<_> = auto_categories
                        .iter()
                        .filter(|(_, c)| {
                            matches!(
                                c.rule,
                                jumbie_shared::config::AutomaticProfileRule::UnexpectedFiles { .. }
                            )
                        })
                        .collect();

                    for (cat_name, cat) in unexpected_cats {
                        for file in &unknown_files {
                            let ext = file
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(|e| e.to_lowercase())
                                .unwrap_or_default();

                            if ext.is_empty() || !cat.rule.matches_unexpected_extension(&ext) {
                                continue;
                            }
                            let desc = format!(
                                "Included unknown file {} in download {}",
                                file.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                                queue_item.media_name
                            );
                            let (hash_val, _, _) =
                                self.db.update_file_fingerprint(file, "unknown").await;
                            let ext_str: Option<&str> =
                                if ext.is_empty() { None } else { Some(&ext) };
                            let _ = self
                                .db
                                .record_submitter_score(SubmitterScoreInput {
                                    submitter: &submitter,
                                    description: &desc,
                                    value: cat.modifier,
                                    category: cat_name,
                                    bound: cat.bound,
                                    source_identity: Some(&hash_val),
                                    extension: ext_str,
                                })
                                .await;
                            tracing::debug!("Recorded offense for {}: {}", submitter, desc);
                        }
                    }
                }

                // Persist extension counts for future rescoring regardless of profile
                // state — rescoring re-evaluates category changes even after the
                // original files are deleted.
                let mut ext_counts: std::collections::HashMap<String, i32> =
                    std::collections::HashMap::new();
                for f in &unknown_files {
                    if let Some(ext) = f
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_lowercase())
                        .filter(|e| !e.is_empty())
                    {
                        *ext_counts.entry(ext).or_insert(0) += 1;
                    }
                }
                if !ext_counts.is_empty() {
                    let counts: Vec<(String, i32)> = ext_counts.into_iter().collect();
                    let _ = self
                        .db
                        .record_unknown_file_counts(&submitter, &counts)
                        .await;
                }

                // "keep" (default): leave in the download dir, mark the fingerprint so
                //   adopt_orphans skips it, and surface it as unassigned in Manage Series
                //   Files for manual assignment. "delete": remove from disk entirely.
                // Only a series-level release the user picked treats every file as wanted;
                // an episode-level release follows the configured policy.
                let handling = if force_keep_review {
                    "keep"
                } else {
                    unexpected_handling.as_str()
                };
                if handling == "delete" {
                    for file in &unknown_files {
                        tracing::debug!(
                            "Deleting unexpected file (per config): '{}'",
                            file.display()
                        );
                        let _ = tokio::fs::remove_file(file).await;
                        // Drop the fingerprint too; the file is gone.
                        let _ = self.db.delete_fingerprint(&file.to_string_lossy()).await;
                    }
                    // Batch cleanup: cull the per-download staging folder once after
                    // all deletions.
                    remove_empty_download_dir(&content_dir, &download_roots).await;
                } else {
                    for file in &unknown_files {
                        let file_path_str = file.to_string_lossy().to_string();
                        // Record durably for review + mark the fingerprint so
                        // adopt_orphans skips it on subsequent cycles.
                        super::mark_file_for_review(
                            &self.db,
                            &file_path_str,
                            Some(series_id.as_str()),
                            super::ReviewReason::Unmatched,
                        )
                        .await;
                        tracing::debug!(
                            "Left unmatched file in download dir for manual assignment: '{}'",
                            file.display()
                        );
                    }
                }
            }

            // Handle Unneeded Files (configurable: keep or delete).
            // Only a series-level release the user picked treats every file as wanted.
            let unneeded_handling = if force_keep_review {
                "keep"
            } else {
                general_cfg.unneeded_episodes_handling.as_str()
            };
            if unneeded_handling == "keep" {
                for file in &unneeded_files {
                    let file_path_str = file.to_string_lossy().to_string();
                    super::mark_file_for_review(
                        &self.db,
                        &file_path_str,
                        Some(series_id.as_str()),
                        super::ReviewReason::Unneeded,
                    )
                    .await;
                    tracing::debug!(
                        "Left unneeded episode file in download dir for manual review: '{}'",
                        file.display()
                    );
                }
            } else {
                for file in &unneeded_files {
                    let file_path_str = file.to_string_lossy().to_string();
                    tracing::debug!("Deleting unneeded episode from pack: '{}'", file.display());
                    let _ = tokio::fs::remove_file(file).await;
                    // Drop the fingerprint too — the file is gone, and a stale row
                    // would otherwise linger until the stale-file cleanup sweep.
                    let _ = self.db.delete_fingerprint(&file_path_str).await;
                }
                // Batch cleanup: cull the per-download staging folder once after
                // all deletions.
                remove_empty_download_dir(&content_dir, &download_roots).await;
            }

            Ok(SmartLinkOutcome {
                unplaced_assignment,
                kept_for_review,
            })
        })
    }

    /// For user-initiated packs where the episode range is unknown or the pack
    /// contains files outside the intended range. Uses the mapping and the
    /// series's existing episode files, both prefetched by the caller, generates
    /// an episode_id, creates an episode row, and links the file.
    ///
    /// Returns [`PermissiveLink::AlreadyPresent`] when the target episode already
    /// points at a different, live file so the caller can treat the incoming file
    /// as redundant.
    /// Returns [`PermissiveLink::Unresolvable`] when the episode identity cannot
    /// be derived — including ambiguous season aliases — so the caller treats the
    /// file as unmatched.
    async fn link_permissive_episode(&self, req: PermissiveLinkRequest<'_>) -> PermissiveLink {
        let PermissiveLinkRequest {
            file_path,
            info,
            queue_item,
            series_id,
            mapping,
            global_absolute,
            season_str,
            existing_episode_files,
        } = req;
        let Some(mapping) = mapping else {
            return PermissiveLink::Unresolvable;
        };
        let absolute = mapping.settings.active_mode(global_absolute).is_absolute();

        // Season fallback for releases no override claims: absolute mode is
        // canonically ABSOLUTE_SEASON_NUM; normal mode uses the queue item's season
        // label and treats a non-numeric label as unassignable.
        let fallback_season = if absolute {
            Some(jumbie_shared::mapping::ABSOLUTE_SEASON_NUM)
        } else {
            match jumbie_shared::mapping::parse_season_num(season_str) {
                Some(s) => Some(s),
                None => {
                    debug!(
                        "skipping permissive link for {} — no season in filename or queue item",
                        file_path
                    );
                    return PermissiveLink::Unresolvable;
                }
            }
        };

        // Download-side season resolution (alias titles, then the season-number
        // alias for offset seasons), falling back to the scanner's local rule.
        let (season_fmt, episode_offset) =
            match crate::source_processor::identification::resolve_release_season(
                &mapping.settings,
                info.seasons.first().copied(),
                &info.series_key,
                info,
                absolute,
            ) {
                crate::source_processor::identification::ResolvedReleaseSeason::Season {
                    season_key,
                    offset,
                } => (season_key.to_string(), offset),
                crate::source_processor::identification::ResolvedReleaseSeason::Ambiguous => {
                    debug!(
                        "skipping permissive link for {} — season aliases matched ambiguously",
                        file_path
                    );
                    return PermissiveLink::AmbiguousSeason;
                }
                crate::source_processor::identification::ResolvedReleaseSeason::Unclaimed => {
                    let crate::scanner::ResolvedSeason::Season(season_fmt, _) =
                        crate::scanner::resolve_season_for_series_with_fallback(
                            info,
                            Path::new(file_path),
                            &mapping.settings,
                            absolute,
                            fallback_season,
                        )
                    else {
                        debug!(
                            "skipping permissive link for {} — season aliases matched ambiguously",
                            file_path
                        );
                        return PermissiveLink::AmbiguousSeason;
                    };
                    let offset = mapping.effective_episode_offset(&season_fmt, global_absolute);
                    (season_fmt, offset)
                }
            };
        let file_season: i32 = season_fmt
            .parse()
            .unwrap_or(jumbie_shared::mapping::DEFAULT_SEASON_NUM);

        // A file covers one episode (S01E05) or a range (S01E01-E03). Resolve every
        // covered episode up front so association is coverage-based and identical for
        // episode-level manual searches, series-level searches, and scans — those
        // paths differ only in how the target scope was established at queue time.
        //
        // This helper is only called for files with parsed episode numbers; if that
        // invariant is broken, skip the file rather than inventing episode 1.
        if info.episodes.is_empty() {
            debug!(
                "skipping permissive link for {} — no episode number parsed",
                file_path
            );
            return PermissiveLink::Unresolvable;
        }
        let mut targets: Vec<(String, i32)> = Vec::with_capacity(info.episodes.len());
        let mut seen = std::collections::HashSet::new();
        for &source_episode_num in &info.episodes {
            // Parsed numbers are release/source-numbered; convert to the local number
            // the episode ID is built from.
            let episode_num =
                jumbie_shared::mapping::source_to_local_episode(source_episode_num, episode_offset);
            let ep_id = match mapping.get_episode_id(&season_fmt, episode_num, global_absolute) {
                Ok(id) => id,
                Err(e) => {
                    debug!("skipping non-numeric season: {e}");
                    return PermissiveLink::Unresolvable;
                }
            };
            // A repeated number must not link the same episode twice.
            if seen.insert(ep_id.clone()) {
                targets.push((ep_id, episode_num));
            }
        }

        // Auxiliary sidecars attach to every covered episode but never become its
        // playable file, so they bypass the canonical-path logic below. Subtitles are
        // unlimited; the link helper replaces any existing nfo.
        if let Some(kind) = jumbie_shared::media_format::file_kind_for_path(Path::new(file_path))
            && kind.is_auxiliary()
        {
            for (ep_id, _) in &targets {
                if let Err(e) = self.db.link_file_episode_as(file_path, ep_id, kind).await {
                    warn!(
                        "Failed to link auxiliary file {} -> {}: {}",
                        file_path, ep_id, e
                    );
                    return PermissiveLink::Unresolvable;
                }
            }
            debug!("Permissive-linked auxiliary {} -> {:?}", file_path, targets);
            return PermissiveLink::Linked;
        }

        // Episode-level authority: associate the file with every covered episode that
        // has no live file of its own; a covered episode that already owns a different
        // file keeps it — this path never replaces. The searched episode is the only
        // one with replacement authority, and a file covering it is routed through the
        // target-aware multi/assign branches before reaching here.
        //
        // A language variant is a distinct artifact that attaches alongside the
        // primary, so it is never a conflict. The map is prefetched by the caller (one
        // query per series) plus any links made earlier in this run.
        let mut to_link: Vec<&(String, i32)> = Vec::with_capacity(targets.len());
        for target in &targets {
            let (ep_id, _) = target;
            let is_variant = self
                .db
                .has_variant_occupant(ep_id, file_path)
                .await
                .unwrap_or(false);
            let occupied = !is_variant
                && existing_episode_files
                    .get(ep_id)
                    .is_some_and(|existing| existing != file_path && Path::new(existing).exists());
            if occupied {
                debug!(
                    "Permissive file {} skips already-present episode {} (keeps its existing file)",
                    file_path, ep_id
                );
            } else {
                to_link.push(target);
            }
        }
        if to_link.is_empty() {
            // Every covered episode already owns a file: the incoming file is fully
            // redundant and the caller keeps it for review.
            return PermissiveLink::AlreadyPresent;
        }

        // Create each free covered episode (if needed) and link the file to all of them.
        for (ep_id, episode_num) in &to_link {
            let _ = self
                .db
                .insert_episode(crate::db::episodes::InsertEpisodeParams {
                    episode_id: ep_id,
                    series_id,
                    season: file_season,
                    episode: *episode_num,
                    file_path: Some(file_path),
                    title: None,
                    quality_profile_id: queue_item.quality_profile_id.as_deref(),
                    status: EpisodeStatus::Downloaded.as_str(),
                    meta_date: None,
                    est_date: None,
                    metadata_ids: &HashMap::new(),
                    description: None,
                    runtime: None,
                    image_url: None,
                    metadata_source: None,
                    numbering_mode: None,
                })
                .await;

            if let Err(e) = self.db.link_file_episode(file_path, ep_id).await {
                warn!(
                    "Failed to link permissive file {} -> {}: {}",
                    file_path, ep_id, e
                );
                return PermissiveLink::Unresolvable;
            }

            // Track the link so later files in the same run see this episode as
            // already present without another DB round-trip.
            existing_episode_files.insert(ep_id.clone(), file_path.to_string());
        }
        debug!("Permissive-linked {} -> {:?}", file_path, targets);
        PermissiveLink::Linked
    }

    /// Assign a single file that covers several episodes (a multi-episode file with no
    /// way to split) to every target episode, replacing any overlapping existing files.
    /// The whole target set is passed as `all_target_episode_ids` so the shared file
    /// never displaces its own targets (SSoT: `assign_file_to_episode`). Returns whether
    /// at least one episode was assigned.
    async fn assign_multi_episode_file(
        &self,
        file_path: &str,
        targets: &[(String, i32)],
        series_id: &str,
        series_title: &str,
        season: i32,
        numbering_mode: i32,
    ) -> bool {
        if targets.is_empty() {
            return false;
        }
        let target_ids: Vec<String> = targets.iter().map(|(id, _)| id.clone()).collect();

        let mut any = false;
        for (episode_id, local_ep) in targets {
            match self
                .db
                .ingest_file_to_episode(crate::db::episodes::assign::AssignFileToEpisodeParams {
                    episode_id,
                    series_id,
                    series_title,
                    season,
                    episode: *local_ep,
                    file_path,
                    episode_title: "",
                    all_target_episode_ids: Some(&target_ids),
                    numbering_mode,
                    only_unassigned: false,
                })
                .await
            {
                Ok(_) => any = true,
                Err(e) => warn!(
                    "Failed to assign multi-episode file {} -> {}: {}",
                    file_path, episode_id, e
                ),
            }
        }
        any
    }

    /// Link a downloaded file to `episode_id`, registering it as an episode part when
    /// the filename carries a part number. `local_episode` and `season_str` are used to
    /// upsert the episode row for a non-part file. Returns whether the link succeeded.
    async fn assign_download_file(&self, req: AssignDownloadFileRequest<'_>) -> bool {
        let AssignDownloadFileRequest {
            file_path: file_path_str,
            parsed,
            size,
            series_id,
            season_str,
            absolute,
            queue_item,
            episode_id,
            local_episode,
        } = req;
        if let Some(part_num) = parsed.and_then(|info| info.part_number) {
            // A language sibling is reconciled (kept alongside, or promoted into the
            // slot); otherwise the part occupies the slot under the organization
            // collision strategy.
            let writes = matches!(
                self.db
                    .reconcile_ingested_slot(episode_id, file_path_str, Some(part_num), size)
                    .await,
                Ok(ingest) if ingest.outcome.caller_writes_slot()
            );
            if writes
                && let Err(e) = self
                    .db
                    .write_slot_displacing(episode_id, file_path_str, Some(part_num), size)
                    .await
            {
                tracing::warn!(
                    "Failed to upsert episode part {}/{}: {}",
                    episode_id,
                    part_num,
                    e
                );
            }
        } else {
            // SSoT: absolute numbering is canonically ABSOLUTE_SEASON_NUM; a normal-mode
            // label with no numeric equivalent has no episode identity, so skip the row
            // write rather than guessing a season.
            let season = match jumbie_shared::mapping::resolve_season_num(season_str, absolute) {
                Ok(n) => n,
                Err(e) => {
                    warn!("Skipping episode row for {} — {}", file_path_str, e);
                    return false;
                }
            };
            let _ = self
                .db
                .insert_episode(crate::db::episodes::InsertEpisodeParams {
                    episode_id,
                    series_id,
                    season,
                    episode: local_episode,
                    file_path: Some(file_path_str),
                    title: None,
                    quality_profile_id: queue_item.quality_profile_id.as_deref(),
                    status: EpisodeStatus::Downloaded.as_str(),
                    meta_date: None,
                    est_date: None,
                    metadata_ids: &std::collections::HashMap::new(),
                    description: None,
                    runtime: None,
                    image_url: None,
                    metadata_source: None,
                    numbering_mode: None,
                })
                .await;
        }

        let linked = if parsed.and_then(|info| info.part_number).is_none() {
            // Non-part files keep an `episode_files` association, which
            // download-completion detection (`get_completed_files`) relies on for its
            // scanner-claimed race check.
            match self.db.link_file_episode(file_path_str, episode_id).await {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(
                        "Failed to link file {} -> {}: {}",
                        file_path_str,
                        episode_id,
                        e
                    );
                    false
                }
            }
        } else {
            // Multipart associations are registered by `upsert_episode_part` (the
            // SSoT); skipping the non-part link here avoids duplicating them as a
            // single file.
            true
        };
        // Invalidate cache — the episode row, its parts, or its linkage changed.
        linked
    }
}
