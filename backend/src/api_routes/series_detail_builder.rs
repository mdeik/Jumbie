use crate::datetime::{ReleaseDatesExt, compute_effective_date};
use crate::db::{EpisodeDetailRow, EpisodePartRow};
use futures::stream::{self, StreamExt};
use jumbie_shared::config::ui::ReleaseDateDisplayConfig;
use jumbie_shared::formatting::{LabelStyle, fmt_episode_id, fmt_season};
use jumbie_shared::types::{
    AuxiliaryFile, EpisodePartInfo, EpisodeStatus, EpisodeViewModel, MappingRule,
};
use std::collections::HashMap;

/// Resolve on-disk existence for every distinct main-file path in one batched pass:
/// direct episode paths plus multipart part paths. Deduplication matters for
/// multi-part / multi-episode files that share a path. Errors count as "does not
/// exist".
pub async fn resolve_file_existence(
    episodes: &[EpisodeDetailRow],
    parts_by_episode: &HashMap<String, Vec<EpisodePartRow>>,
) -> HashMap<String, bool> {
    let mut seen = std::collections::HashSet::new();
    let mut paths: Vec<String> = Vec::new();
    for row in episodes {
        if let Some(p) = row.file_path.as_deref().filter(|p| !p.is_empty())
            && seen.insert(p.to_string())
        {
            paths.push(p.to_string());
        }
    }
    for parts in parts_by_episode.values() {
        for part in parts {
            if seen.insert(part.file_path.clone()) {
                paths.push(part.file_path.clone());
            }
        }
    }
    resolve_paths_existence(paths).await
}

/// Resolve on-disk existence for an arbitrary set of paths in one batched pass
/// (deduplicated, bounded concurrency; errors count as "does not exist").
pub async fn resolve_paths_existence(paths: Vec<String>) -> HashMap<String, bool> {
    stream::iter(paths)
        .map(|path| async move {
            let exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
            (path, exists)
        })
        // Cap in-flight stats so a huge series can't flood the blocking pool.
        .buffered(64)
        .collect()
        .await
}

/// Maps raw database rows into `EpisodeViewModel`s, returning them alongside the
/// max episode number per season for gap calculation.
///
/// Standalone (not tied to the HTTP handler) so it is reusable and unit-testable
/// without Axum or a DB connection.
///
/// Status is derived from multiple inputs rather than stored literally: the
/// persistent statuses ("out_of_range", "organized", "downloaded") are used as-is
/// when the episode still has a file; otherwise it is derived from file/parts
/// presence, queue state, and air date. This lets the system self-heal when file
/// this lets the system self-heal when file
/// paths change, with no separate status-update query after file operations.
///
/// File existence is supplied precomputed by [`resolve_file_existence`] so this stays a
/// pure, I/O-free mapping (unit-testable without touching a disk).
pub fn map_db_rows_to_view_models(
    db_episodes: Vec<EpisodeDetailRow>,
    parts_by_episode: &HashMap<String, Vec<EpisodePartRow>>,
    aux_by_episode: &HashMap<String, Vec<AuxiliaryFile>>,
    episodes_in_queue: &std::collections::HashSet<String>,
    release_date_config: &ReleaseDateDisplayConfig,
    file_existence: &HashMap<String, bool>,
) -> (
    HashMap<(String, i32), EpisodeViewModel>,
    HashMap<String, i32>,
) {
    let mut episode_map = HashMap::new();
    let mut max_episodes = HashMap::new();

    for db_row in db_episodes {
        let ep_id = db_row.episode_id;
        let season_opt = db_row.season;
        let ep_num = db_row.episode;
        let status_opt = db_row.status;
        let path_opt = db_row.file_path;
        let size = db_row.size;
        let title = db_row.title;
        let quality_profile_id = db_row.quality_profile_id;
        let submitter = db_row.submitter;
        let release_title = db_row.release_title;
        let media_info_json = db_row.media_info;
        let fingerprint = db_row.quick_hash;
        let created_at = db_row.created_at;
        let file_acquired_at = db_row.file_acquired_at;
        let monitored = db_row.monitored;
        let dates = jumbie_shared::types::ReleaseDates {
            meta_date: db_row.meta_date,
            upload_date: db_row.upload_date,
            est_date: db_row.est_date,
        };
        let metadata_ids_str = db_row.metadata_ids;
        let description = db_row.description;
        let runtime = db_row.runtime;
        let image_url = db_row.image_url;
        let metadata_source = db_row.metadata_source.clone();

        // SSoT: the row's numbering_mode comes from the mode-scoped episode query.
        // Absolute rows resolve to ABSOLUTE_SEASON_NUM; a NULL season on a
        // normal-mode row is legacy data with no season recorded (default 1).
        let season =
            jumbie_shared::mapping::resolve_season_opt(season_opt, db_row.numbering_mode == 1)
                .unwrap_or(1);
        let season_fmt = fmt_season(season, LabelStyle::Short);

        // Parse stored media_info JSON (codec, resolution, etc.); absent or
        // unparseable leaves it empty and the frontend hides it gracefully.
        let media_info = media_info_json
            .as_ref()
            .and_then(|json| serde_json::from_str(json).ok());

        // Parts come from pre-joined episode_files data; an empty list is valid
        // (most episodes are single-file).
        let parts: Vec<EpisodePartInfo> = parts_by_episode
            .get(&ep_id)
            .map(|rows| {
                rows.iter()
                    .map(|r| EpisodePartInfo {
                        part_number: r.part_number as u32,
                        file_path: r.file_path.clone(),
                        size: r.size.unwrap_or(0) as u64,
                        fingerprint: r.fingerprint.clone(),
                        original_path: r.original_path.clone(),
                        media_info: r
                            .media_info
                            .as_ref()
                            .and_then(|json| serde_json::from_str(json).ok()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut status = status_opt.unwrap_or_else(|| EpisodeStatus::Unreleased.to_string());
        let path = path_opt.unwrap_or_default();
        let assigned = !path.is_empty() || !parts.is_empty();

        // Persistent statuses are stored explicitly by the organizing/download
        // pipeline; everything else is derived reactively from file presence, queue
        // state, and air date, so deleting a file outside the app shows "missing"
        // on the next load without an explicit sync.
        if !EpisodeStatus::is_persistent(
            &status
                .parse::<EpisodeStatus>()
                .unwrap_or(EpisodeStatus::Unreleased),
        ) || !assigned
        {
            if assigned {
                status = EpisodeStatus::Downloaded.to_string();
            } else if episodes_in_queue.contains(&ep_id) {
                status = EpisodeStatus::InQueue.to_string();
            } else {
                // SSoT: Use user's release date display preferences to determine
                // the effective date, matching the wanted page filter logic.
                let effective = compute_effective_date(
                    &dates,
                    &release_date_config.order,
                    release_date_config.metadata_enabled,
                    release_date_config.source_enabled,
                    release_date_config.estimated_enabled,
                );
                status = crate::datetime::derive_episode_status(
                    effective,
                    crate::datetime::UtcDateTime::now(),
                )
                .to_string();
            }
        }

        // SSoT: existence was resolved in one batched pass by `resolve_file_existence`
        // (direct paths + part paths).
        //
        // `assigned` is pure DB state (a main association exists).
        // `disk_present` is the disk state of the assigned main file(s): the direct
        // file for a single/multi-episode file, or every currently assigned part for a
        // multipart episode. Multipart completeness is unknown (no expected part count),
        // so only the parts actually assigned are checked. Auxiliary files never
        // contribute to either flag.
        let disk_present = if !path.is_empty() {
            file_existence.get(&path).copied().unwrap_or(false)
        } else if parts.is_empty() {
            false
        } else {
            parts
                .iter()
                .all(|p| file_existence.get(&p.file_path).copied().unwrap_or(false))
        };

        let vm = EpisodeViewModel {
            unique_id: ep_id.clone(),
            season: season_fmt.clone(),
            episode: ep_num,
            header: jumbie_shared::formatting::fmt_episode(ep_num, LabelStyle::Human),
            title: title.clone(),
            status,
            quality_profile_id,
            // A multipart episode's size is the sum of its parts; the single-file
            // `size` column describes the direct file only. Consistent for scanned
            // and downloaded multiparts (both populate `episode_files`).
            size: if parts.is_empty() {
                size as u64
            } else {
                parts.iter().map(|p| p.size).sum()
            },
            submitter,
            release_title,
            path: if !path.is_empty() { Some(path) } else { None },
            original_path: db_row.original_path,
            media_info,
            fingerprint,
            // Emit RFC 3339 UTC so the frontend can render these in the user's
            // browser timezone. Formatting them here via `format_local_date()`
            // would bake in the *server's* timezone, which is wrong for any
            // client not sharing it.
            created_at: if assigned {
                created_at.map(|d| crate::datetime::UtcDateTime::from_naive_utc(d).to_rfc3339_utc())
            } else {
                None
            },
            file_acquired_at: if assigned {
                file_acquired_at
                    .map(|d| crate::datetime::UtcDateTime::from_naive_utc(d).to_rfc3339_utc())
            } else {
                None
            },
            monitored,
            dates: dates.to_api(),
            metadata_ids: metadata_ids_str
                .and_then(|m| serde_json::from_str(&m).ok())
                .unwrap_or_default(),
            description,
            runtime,
            image_url,
            metadata_source,
            parts,
            auxiliary_files: aux_by_episode.get(&ep_id).cloned().unwrap_or_default(),
            show_only_downloaded: false,
            assigned,
            disk_present,
        };
        episode_map.insert((season_fmt.clone(), ep_num), vm);

        // Track the maximum episode number seen per season so the gap-filler
        // (fill_missing_episodes) knows what range to consider.
        let current_max = max_episodes.entry(season_fmt).or_insert(0);
        if ep_num > *current_max {
            *current_max = ep_num;
        }
    }

    (episode_map, max_episodes)
}

/// Fills gaps in the episode list based on configured season overrides and DB data.
///
/// For each season, looks up its `SeasonOverride`. With `cell_count = Some(n)`, it
/// renders slots 1..n (synthesizing "missing" placeholders, marking out-of-range
/// slots unmonitored) then appends DB episodes with files beyond slot n so
/// downloads past `cell_count` stay visible. With `cell_count = None`, all DB
/// episodes for the season are rendered as-is.
pub fn fill_missing_episodes(
    mapping: &MappingRule,
    normalized_seasons: &[String],
    episode_map: &HashMap<(String, i32), EpisodeViewModel>,
    global_absolute: bool,
) -> Vec<EpisodeViewModel> {
    let mut view_episodes = Vec::new();

    // SSoT: effective numbering mode (series tristate → global default).
    let absolute = mapping.settings.active_mode(global_absolute).is_absolute();

    for raw_season in normalized_seasons {
        let season_fmt = format!("S{}", raw_season);

        // SSoT: one season-override lookup (tolerates "2"/"02"/"S02" keys).
        let override_rule = mapping
            .settings
            .find_season_override(raw_season, global_absolute);

        let cell_count = override_rule.and_then(|o| o.cell_count);

        let configured_start = override_rule.and_then(|o| o.episode_start).unwrap_or(1);
        let configured_end_range = override_rule
            .and_then(|o| o.episode_end)
            .unwrap_or(i32::MAX);

        match cell_count {
            Some(n) if n > 0 => {
                for i in 1..=n {
                    let is_out_of_range = i < configured_start || i > configured_end_range;

                    if let Some(vm) = episode_map.get(&(season_fmt.clone(), i)) {
                        let mut vm_clone = vm.clone();
                        if is_out_of_range {
                            vm_clone.status = EpisodeStatus::OutOfRange.to_string();
                            vm_clone.monitored = false;
                        }
                        view_episodes.push(vm_clone);
                    } else {
                        // Synthesize a placeholder for the missing episode.
                        // Uses fmt_episode_id so the unique_id follows the same format
                        // as real DB rows — {series_id}_S{season:02}E{episode:02} in
                        // normal mode or {series_id}_ABS{episode:04} in absolute mode.
                        // A season label with no numeric equivalent cannot produce a
                        // correct ID, so skip the placeholder rather than guess one.
                        let placeholder_id = match fmt_episode_id(
                            raw_season,
                            i,
                            &mapping.series_id,
                            absolute,
                        ) {
                            Ok(id) => id,
                            Err(e) => {
                                tracing::debug!(
                                    "fill_missing_episodes: skipping placeholder for season {:?}: {}",
                                    raw_season,
                                    e
                                );
                                continue;
                            }
                        };
                        view_episodes.push(EpisodeViewModel {
                            unique_id: placeholder_id,
                            season: season_fmt.clone(),
                            episode: i,
                            header: jumbie_shared::formatting::fmt_episode(i, LabelStyle::Human),
                            title: None,
                            status: if is_out_of_range {
                                EpisodeStatus::OutOfRange.to_string()
                            } else {
                                EpisodeStatus::Missing.to_string()
                            },
                            quality_profile_id: None,
                            size: 0,
                            submitter: None,
                            release_title: None,
                            path: None,
                            original_path: None,
                            media_info: None,
                            fingerprint: None,
                            created_at: None,
                            file_acquired_at: None,
                            monitored: !is_out_of_range,
                            dates: jumbie_shared::types::ReleaseDates {
                                meta_date: None,
                                upload_date: None,
                                est_date: None,
                            },
                            metadata_ids: HashMap::new(),
                            metadata_source: None,
                            description: None,
                            runtime: None,
                            image_url: None,
                            parts: vec![],
                            auxiliary_files: Vec::new(),
                            show_only_downloaded: false,
                            assigned: false,
                            disk_present: false,
                        });
                    }
                }

                // Episodes with files outside 1..n are appended so downloads past
                // cell_count stay visible (dimmed via `show_only_downloaded`); those
                // without files stay hidden.
                let mut outside_with_files: Vec<EpisodeViewModel> = episode_map
                    .iter()
                    .filter(|((sf, ep), vm)| sf == &season_fmt && *ep > n && vm.path.is_some())
                    .map(|(_, vm)| {
                        let mut vm = vm.clone();
                        vm.show_only_downloaded = true;
                        vm
                    })
                    .collect();
                outside_with_files.sort_by_key(|vm| vm.episode);
                view_episodes.extend(outside_with_files);
            }
            _ => {
                let mut season_episodes: Vec<EpisodeViewModel> = episode_map
                    .iter()
                    .filter(|((sf, _), _)| sf == &season_fmt)
                    .map(|(_, vm)| vm.clone())
                    .collect();
                season_episodes.sort_by_key(|vm| vm.episode);
                view_episodes.extend(season_episodes);
            }
        }
    }

    view_episodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(episode_id: &str, episode: i32, size: i64) -> EpisodeDetailRow {
        EpisodeDetailRow {
            episode_id: episode_id.to_string(),
            season: Some(1),
            episode,
            status: Some("organized".to_string()),
            file_path: None,
            release_title: None,
            size,
            title: None,
            quality_profile_id: None,
            submitter: None,
            media_info: None,
            quick_hash: None,
            created_at: None,
            file_acquired_at: None,
            monitored: true,
            meta_date: None,
            upload_date: None,
            est_date: None,
            metadata_ids: None,
            description: None,
            runtime: None,
            image_url: None,
            download_id: None,
            score: None,
            numbering_mode: 0,
            metadata_source: None,
            series_id: "s1".to_string(),
            download_link: None,
            monitor_override: false,
            original_path: None,
        }
    }

    fn part(n: i32, size: i64) -> EpisodePartRow {
        EpisodePartRow {
            part_number: n,
            file_path: format!("/lib/pt{n}.mkv"),
            size: Some(size),
            fingerprint: None,
            media_info: None,
            original_path: None,
        }
    }

    /// A multipart episode's displayed size is the total of its parts, not the
    /// single-part size the join resolves.
    #[test]
    fn multipart_episode_size_is_the_sum_of_its_parts() {
        let mut parts = HashMap::new();
        parts.insert("s1_S01E01".to_string(), vec![part(1, 111), part(2, 222)]);
        let (map, _) = map_db_rows_to_view_models(
            vec![row("s1_S01E01", 1, 111)],
            &parts,
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &ReleaseDateDisplayConfig::default(),
            &HashMap::new(),
        );
        let vm = map.values().next().expect("episode mapped");
        assert_eq!(
            vm.size, 333,
            "multipart size must be the total of its parts"
        );
    }

    /// A single-file episode keeps the size the join resolved.
    #[test]
    fn single_file_episode_size_is_the_file_size() {
        let (map, _) = map_db_rows_to_view_models(
            vec![row("s1_S01E01", 1, 999)],
            &HashMap::new(),
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &ReleaseDateDisplayConfig::default(),
            &HashMap::new(),
        );
        let vm = map.values().next().expect("episode mapped");
        assert_eq!(vm.size, 999);
    }

    /// `assigned` is DB state; `disk_present` is disk state. A single-file episode is
    /// assigned whenever it has a path, but present only while that file exists.
    #[test]
    fn single_file_assigned_and_disk_present_follow_the_file() {
        let mut ep = row("s1_S01E01", 1, 0);
        ep.file_path = Some("/lib/ep.mkv".to_string());

        let mut present = HashMap::new();
        present.insert("/lib/ep.mkv".to_string(), true);
        let (map, _) = map_db_rows_to_view_models(
            vec![ep.clone()],
            &HashMap::new(),
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &ReleaseDateDisplayConfig::default(),
            &present,
        );
        let vm = map.values().next().unwrap();
        assert!(vm.assigned, "a direct file is an assignment");
        assert!(vm.disk_present, "the file exists on disk");

        let mut missing = HashMap::new();
        missing.insert("/lib/ep.mkv".to_string(), false);
        let (map, _) = map_db_rows_to_view_models(
            vec![ep],
            &HashMap::new(),
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &ReleaseDateDisplayConfig::default(),
            &missing,
        );
        let vm = map.values().next().unwrap();
        assert!(
            vm.assigned,
            "assigned is DB state and survives a missing file"
        );
        assert!(!vm.disk_present, "the file is gone from disk");
    }

    /// A multipart episode is assigned by any part, but present only when every
    /// assigned part exists.
    #[test]
    fn multipart_disk_present_requires_all_assigned_parts() {
        let mut parts = HashMap::new();
        parts.insert("s1_S01E01".to_string(), vec![part(1, 1), part(2, 1)]);
        let mut existence = HashMap::new();
        existence.insert("/lib/pt1.mkv".to_string(), true);
        existence.insert("/lib/pt2.mkv".to_string(), true);

        let (map, _) = map_db_rows_to_view_models(
            vec![row("s1_S01E01", 1, 0)],
            &parts,
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &ReleaseDateDisplayConfig::default(),
            &existence,
        );
        let vm = map.values().next().unwrap();
        assert!(vm.assigned, "a lone multipart part is an assignment");
        assert!(vm.disk_present, "all assigned parts exist");

        existence.insert("/lib/pt2.mkv".to_string(), false);
        let (map, _) = map_db_rows_to_view_models(
            vec![row("s1_S01E01", 1, 0)],
            &parts,
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &ReleaseDateDisplayConfig::default(),
            &existence,
        );
        let vm = map.values().next().unwrap();
        assert!(vm.assigned);
        assert!(!vm.disk_present, "one missing part clears disk_present");
    }

    /// An episode with no direct file and no parts is neither assigned nor present.
    #[test]
    fn unassigned_episode_is_not_assigned_and_not_present() {
        let (map, _) = map_db_rows_to_view_models(
            vec![row("s1_S01E01", 1, 0)],
            &HashMap::new(),
            &HashMap::new(),
            &std::collections::HashSet::new(),
            &ReleaseDateDisplayConfig::default(),
            &HashMap::new(),
        );
        let vm = map.values().next().unwrap();
        assert!(!vm.assigned);
        assert!(!vm.disk_present);
    }
}
