use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use jumbie_shared::config::Config;
use jumbie_shared::config::organization::CollisionRenameSuffix;
use jumbie_shared::mapping::resolve_season_opt;
use jumbie_shared::types::MappingRule;

use crate::db::EpisodePartRow;
use crate::file_manager::ResolveMode;
use crate::file_manager::path::PathBuildVars;
use crate::organizer::ContentOrganizer;

/// Error type for rename plan computation.
///
/// Only `DuplicateTarget` is a real error: missing sources, unparseable filenames,
/// and template-render failures are skipped (bubbling up would abort the batch for a
/// transient filesystem state). Two sources mapping to one destination is
/// contradictory regardless of filesystem state.
#[derive(Debug)]
pub enum RenamePlanError {
    /// Two different source files would be renamed to the same destination.
    DuplicateTarget(PathBuf),
}

/// Pre-computed context for naming templates.
///
/// `max_season` is the highest season number in the series; it sets the width of
/// `${season:auto}`. `max_episode_by_season` holds the highest episode number of
/// each season, which sets `${episode:auto}` for episodes in that season. The
/// active numbering mode decides whether episode numbers are season-relative or
/// absolute (absolute has a single season).
pub struct MappingContext {
    pub max_season: i32,
    pub max_episode_by_season: HashMap<i32, i32>,
}

impl MappingContext {
    /// Highest episode number in `season` (0 when the season is unknown).
    pub fn max_episode_for(&self, season: i32) -> i32 {
        self.max_episode_by_season
            .get(&season)
            .copied()
            .unwrap_or(0)
    }
}

pub fn get_mapping_context(
    db_episodes: &[crate::db::EpisodeDetailRow],
    absolute: bool,
) -> MappingContext {
    let mut max_episode_by_season: HashMap<i32, i32> = HashMap::new();
    let mut max_season = 0;
    for row in db_episodes {
        // SSoT: seasons are canonicalised to the active mode, so absolute rows
        // (whatever their stored season) group into the single canonical season.
        let season = resolve_season_opt(row.season, absolute).unwrap_or(1);
        max_season = max_season.max(season);
        let max = max_episode_by_season.entry(season).or_insert(0);
        *max = (*max).max(row.episode);
    }
    MappingContext {
        max_season,
        max_episode_by_season,
    }
}

/// A single episode entry inside a [`PlannedMove`].
/// Using a vec of these instead of lossy `episode_val`/`episode_end_val` fields.
#[derive(Debug, Clone)]
pub struct EpisodeSummary {
    pub episode_id: String,
    pub episode_num: i32,
}

/// A planned move: source → destination, with the DB metadata needed to update records.
///
/// * `covered_episodes`: all logical episode rows that share `src`. Len > 1 means multi-episode.
/// * `part_number`: `Some(n)` for episode-part files; `None` for normal/multi-episode files.
#[derive(Clone)]
pub struct PlannedMove {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub covered_episodes: Vec<EpisodeSummary>,
    pub series_title: String,
    pub series_id: String,
    pub season_val: String,
    pub episode_title: String,
    pub part_number: Option<u32>,
    /// Set for an auxiliary sidecar move (subtitle/nfo); `None` for a video.
    pub aux_kind: Option<jumbie_shared::media_format::FileKind>,
}

/// One auxiliary sidecar (subtitle/nfo) to include in a rename plan.
#[derive(Debug, Clone, Hash)]
pub struct AuxiliaryPath {
    pub path: String,
    pub kind: jumbie_shared::media_format::FileKind,
    pub episode_id: String,
}

impl PlannedMove {
    /// Convenience: the first (lowest) episode number covered by this move.
    pub fn first_episode_num(&self) -> i32 {
        self.covered_episodes
            .first()
            .map(|e| e.episode_num)
            .unwrap_or(0)
    }
    /// Convenience: the last episode number covered (same as first for single-episode).
    pub fn last_episode_num(&self) -> i32 {
        self.covered_episodes
            .last()
            .map(|e| e.episode_num)
            .unwrap_or(0)
    }
}

/// Build a human-readable episode range string for use in filename templates.
/// Returns raw numbers — padding is applied by the template's own format spec
/// (`${episode:02}`, `${episode:auto}`), the single source of padding truth.
/// Examples: `[5]` → `"5"`, `[1,2,3]` → `"1-3"`, `[1,2,4]` → `"1-2+4"`
fn format_episode_range(episodes: &[i32]) -> String {
    if episodes.is_empty() {
        return String::new();
    }
    jumbie_shared::formatting::consecutive_to_ranges(episodes)
        .iter()
        .map(|(s, e)| {
            if s == e {
                s.to_string()
            } else {
                format!("{}-{}", s, e)
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Returns `true` if the filename already contains a part/cd indicator
/// (e.g. `-cd1`, `-pt2`, `part 3`, `_part-1`). Used to avoid double-injecting `-ptN`.
pub(crate) fn filename_has_part_indicator(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    for prefix in &["cd", "pt", "part", "part-", "part_", "part "] {
        if let Some(idx) = lower.find(prefix) {
            let after = &lower[idx + prefix.len()..];
            if after
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

/// Fetch a series' auxiliary sidecars (subtitles / nfo) as rename-plan inputs.
pub async fn auxiliary_paths_for_series(
    db: &crate::db::DbManager,
    series_id: &str,
) -> Vec<AuxiliaryPath> {
    db.get_auxiliary_files_for_series(series_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .flat_map(|(episode_id, files)| {
            files.into_iter().map(move |a| AuxiliaryPath {
                path: a.path,
                kind: a.kind,
                episode_id: episode_id.clone(),
            })
        })
        .collect()
}

/// The trailing part of a sidecar filename after its episode stem (e.g. `.en.srt`),
/// or just the extension when the sidecar does not share the video's stem.
///
/// Any collision counter the sidecar accumulated is stripped first, so the plan
/// targets the episode's clean name and the move removes the counter instead of
/// nesting another one. Only `variant` (the configured collision suffix) is
/// stripped, so a legitimate `(2019)` in a release name is preserved.
/// The trailing part of a sidecar filename after the episode's expected base stem
/// (e.g. `.en.srt`), or just the extension when the sidecar does not share the
/// episode's name.
///
/// Slicing against the expected stem (the template's base) rather than the current
/// video path keeps this independent of the video's own collision state, and it
/// naturally carries language tags and other qualifiers (`.en.srt`, `.forced.srt`).
/// A collision counter the sidecar accumulated is stripped first, so the plan
/// targets the episode's clean name and the move removes the counter instead of
/// nesting another one. Only `variant` (the configured collision suffix) is
/// stripped, so a legitimate `(2019)` in a release name is preserved.
fn auxiliary_suffix(src: &Path, expected_stem: &str, variant: CollisionRenameSuffix) -> String {
    let normalized = crate::paths::strip_collision_suffixes_of(src, variant);
    let name = normalized
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if let Some(rest) = name.strip_prefix(expected_stem)
        && rest.starts_with('.')
    {
        return rest.to_string();
    }
    match normalized.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!(".{ext}"),
        None => String::new(),
    }
}

/// Compute the full set of (src → dst) moves for a series without touching the filesystem.
pub fn compute_batch_plan(
    config: &Config,
    _series_id: &str,
    mapping: &MappingRule,
    db_episodes: &[crate::db::EpisodeDetailRow],
    episode_parts: &[(String, EpisodePartRow)],
    target_absolute: bool,
    skip_paths: &std::collections::HashSet<std::path::PathBuf>,
) -> std::result::Result<Vec<PlannedMove>, RenamePlanError> {
    compute_batch_plan_with_aux(
        config,
        mapping,
        db_episodes,
        episode_parts,
        &[],
        target_absolute,
        skip_paths,
    )
}

/// Like [`compute_batch_plan`], but also plans moves for the given auxiliary
/// sidecars so subtitles/nfo follow their episode's name.
pub fn compute_batch_plan_with_aux(
    config: &Config,
    mapping: &MappingRule,
    db_episodes: &[crate::db::EpisodeDetailRow],
    episode_parts: &[(String, EpisodePartRow)],
    aux_files: &[AuxiliaryPath],
    target_absolute: bool,
    skip_paths: &std::collections::HashSet<std::path::PathBuf>,
) -> std::result::Result<Vec<PlannedMove>, RenamePlanError> {
    tracing::debug!(
        "Computing rename plan for '{}' — {} episodes, {} parts",
        mapping.target_title,
        db_episodes.len(),
        episode_parts.len(),
    );

    let mut plan: Vec<PlannedMove> = Vec::new();

    let context = get_mapping_context(db_episodes, target_absolute);

    let mut temp_mapping = mapping.clone();
    temp_mapping.settings.absolute_numbering = Some(target_absolute);

    // Padding is per-season: `${episode:auto}` sizes to the highest episode in
    // the season the file belongs to. Absolute mode has a single season.
    let pad_options_for = |season: i32| crate::utils::TemplatePadOptions {
        max_season: context.max_season as u32,
        max_episode: context.max_episode_for(season) as u32,
        max_length: 0,
    };

    // Pre-pass: canonicalize cache & media_info parse-once
    let mut canonical_cache: HashMap<PathBuf, PathBuf> = HashMap::new();

    let get_canonical = |cache: &mut HashMap<PathBuf, PathBuf>, p: &Path| -> PathBuf {
        cache
            .entry(p.to_path_buf())
            .or_insert_with(|| crate::validation::normalize_path(p))
            .clone()
    };

    // Parse media_info JSON only when the active filename format actually
    // references media-info variables — otherwise parsing every episode's JSON
    // on every poll is pure CPU waste. SSoT: resolve_active_episode_format +
    // format_needs_media_info (also used by organize.rs). Rows whose format
    // needs media info keep their parsed values; others render empty vars,
    // identical to a file whose media info hasn't been scanned yet.
    let media_info_cache: HashMap<&str, Option<jumbie_shared::types::MediaInfo>> =
        if crate::file_manager::format_needs_media_info(
            crate::file_manager::resolve_active_episode_format(config, mapping),
        ) {
            db_episodes
                .iter()
                .map(|row| {
                    let mi = row.media_info.as_deref().and_then(|json| {
                        serde_json::from_str::<jumbie_shared::types::MediaInfo>(json).ok()
                    });
                    (row.episode_id.as_str(), mi)
                })
                .collect()
        } else {
            HashMap::new()
        };

    // Pass 1: group db_episodes by file_path (multiepisode detection)
    let mut by_path: std::collections::BTreeMap<String, Vec<&crate::db::EpisodeDetailRow>> =
        std::collections::BTreeMap::new();
    for row in db_episodes {
        if let Some(fp) = &row.file_path {
            by_path.entry(fp.clone()).or_default().push(row);
        }
    }

    for (file_path_str, mut rows) in by_path {
        let src = PathBuf::from(&file_path_str);
        if !src.exists() {
            continue;
        }
        if skip_paths.contains(&src) {
            continue;
        }

        if src.file_name().and_then(|n| n.to_str()).is_none() {
            continue;
        }

        rows.sort_by_key(|r| r.episode);
        let first = rows[0];
        // SSoT: target_absolute is the effective mode for this plan. A NULL season
        // on a normal-mode row is legacy data with no season recorded, defaulted to 1.
        let season_str = resolve_season_opt(first.season, target_absolute).unwrap_or(1);
        let season_num = season_str;

        let ep_nums: Vec<i32> = rows.iter().map(|r| r.episode).collect();
        let episode_var = format_episode_range(&ep_nums);
        let pad_options = pad_options_for(season_num);

        let episode_title = first.title.clone().unwrap_or_default();

        let row_media_info = media_info_cache
            .get(first.episode_id.as_str())
            .and_then(|mi| mi.as_ref());

        let mut dst = match ContentOrganizer::build_target_path_with_episode_var(&PathBuildVars {
            config,
            mapping: &temp_mapping,
            season_num,
            episode_num: first.episode,
            episode_var: &episode_var,
            source_path: &src,
            episode_title: first.title.as_deref(),
            episode_quality: None,
            episode_submitter: first.submitter.as_deref(),
            release_date: first.meta_date,
            created_at: first.created_at,
            pad_options: &pad_options,
            part_number: None,
            media_info: row_media_info,
        }) {
            Ok(p) => p,
            Err(_) => continue,
        };

        dst = crate::file_manager::resolve_episode_target_path(
            &src,
            &dst,
            config,
            mapping,
            ResolveMode::Preview,
        );

        let src_canon = get_canonical(&mut canonical_cache, &src);
        let dst_canon = get_canonical(&mut canonical_cache, &dst);
        if src_canon == dst_canon {
            continue;
        }

        let covered_episodes = rows
            .iter()
            .map(|r| EpisodeSummary {
                episode_id: r.episode_id.clone(),
                episode_num: r.episode,
            })
            .collect();

        plan.push(PlannedMove {
            src,
            dst,
            covered_episodes,
            series_title: mapping.target_title.clone(),
            series_id: mapping.series_id.clone(),
            season_val: season_str.to_string(),
            episode_title,
            part_number: None,
            aux_kind: None,
        });
    }

    // Pass 2: multipart associations (multifile/part detection)
    let ep_lookup: HashMap<&str, &crate::db::EpisodeDetailRow> = db_episodes
        .iter()
        .map(|r| (r.episode_id.as_str(), r))
        .collect();

    let mut parts_by_ep: std::collections::BTreeMap<&str, Vec<&EpisodePartRow>> =
        std::collections::BTreeMap::new();
    for (ep_id, part_row) in episode_parts {
        parts_by_ep
            .entry(ep_id.as_str())
            .or_default()
            .push(part_row);
    }

    for (ep_id, mut parts) in parts_by_ep {
        parts.sort_by_key(|p| p.part_number);
        let needs_reindex = parts
            .iter()
            .enumerate()
            .any(|(i, p)| p.part_number as usize != i + 1);
        let indexed_parts: Vec<(u32, &EpisodePartRow)> = if needs_reindex {
            let mut sortable = parts.clone();
            sortable.sort_by(|a, b| a.file_path.cmp(&b.file_path));
            sortable
                .into_iter()
                .enumerate()
                .map(|(i, p)| ((i + 1) as u32, p))
                .collect()
        } else {
            parts.iter().map(|p| (p.part_number as u32, *p)).collect()
        };

        let parent_row = match ep_lookup.get(ep_id) {
            Some(r) => r,
            None => continue,
        };
        // SSoT: same effective mode as above; NULL season on a normal-mode row is
        // legacy data with no season recorded, defaulted to 1.
        let season_str = resolve_season_opt(parent_row.season, target_absolute).unwrap_or(1);
        let season_num = season_str;
        let episode_var = parent_row.episode.to_string();
        let pad_options = pad_options_for(season_num);

        for (part_num, part_row) in indexed_parts {
            let src = PathBuf::from(&part_row.file_path);
            if !src.exists() {
                continue;
            }
            if skip_paths.contains(&src) {
                continue;
            }
            if src.file_name().and_then(|n| n.to_str()).is_none() {
                continue;
            }

            let row_media_info = media_info_cache.get(ep_id).and_then(|mi| mi.as_ref());

            let mut dst =
                match ContentOrganizer::build_target_path_with_episode_var(&PathBuildVars {
                    config,
                    mapping: &temp_mapping,
                    season_num,
                    episode_num: parent_row.episode,
                    episode_var: &episode_var,
                    source_path: &src,
                    episode_title: parent_row.title.as_deref(),
                    episode_quality: None,
                    episode_submitter: parent_row.submitter.as_deref(),
                    release_date: parent_row.meta_date,
                    created_at: parent_row.created_at,
                    pad_options: &pad_options,
                    part_number: Some(part_num),
                    media_info: row_media_info,
                }) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

            dst = crate::file_manager::resolve_episode_target_path(
                &src,
                &dst,
                config,
                mapping,
                ResolveMode::Preview,
            );

            let src_canon = get_canonical(&mut canonical_cache, &src);
            let dst_canon = get_canonical(&mut canonical_cache, &dst);
            if src_canon == dst_canon {
                continue;
            }

            let episode_title = parent_row.title.clone().unwrap_or_default();

            plan.push(PlannedMove {
                src,
                dst,
                covered_episodes: vec![EpisodeSummary {
                    episode_id: ep_id.to_string(),
                    episode_num: parent_row.episode,
                }],
                series_title: mapping.target_title.clone(),
                series_id: mapping.series_id.clone(),
                season_val: season_str.to_string(),
                episode_title,
                part_number: Some(part_num),
                aux_kind: None,
            });
        }
    }

    // Pass 3: auxiliary sidecars (subtitles / nfo). Each targets its episode's
    // name with the sidecar's own suffix preserved (e.g. `.en.srt`).
    let eps_by_id: HashMap<&str, &crate::db::EpisodeDetailRow> = db_episodes
        .iter()
        .map(|r| (r.episode_id.as_str(), r))
        .collect();
    for aux in aux_files {
        let Some(row) = eps_by_id.get(aux.episode_id.as_str()).copied() else {
            continue;
        };
        let src = PathBuf::from(&aux.path);
        if !src.exists() || skip_paths.contains(&src) {
            continue;
        }
        let season_num = resolve_season_opt(row.season, target_absolute).unwrap_or(1);
        let episode_var = row.episode.to_string();
        let pad_options = pad_options_for(season_num);
        let row_media_info = media_info_cache
            .get(row.episode_id.as_str())
            .and_then(|mi| mi.as_ref());
        let base = match ContentOrganizer::build_target_path_with_episode_var(&PathBuildVars {
            config,
            mapping: &temp_mapping,
            season_num,
            episode_num: row.episode,
            episode_var: &episode_var,
            source_path: &src,
            episode_title: row.title.as_deref(),
            episode_quality: None,
            episode_submitter: row.submitter.as_deref(),
            release_date: row.meta_date,
            created_at: row.created_at,
            pad_options: &pad_options,
            part_number: None,
            media_info: row_media_info,
        }) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let base = crate::file_manager::resolve_episode_target_path(
            &src,
            &base,
            config,
            mapping,
            ResolveMode::Preview,
        );
        let Some(stem) = base.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let suffix = auxiliary_suffix(&src, stem, config.organization.collision_rename_suffix);
        let dst = base.with_file_name(format!("{stem}{suffix}"));
        let src_canon = get_canonical(&mut canonical_cache, &src);
        let dst_canon = get_canonical(&mut canonical_cache, &dst);
        if src_canon == dst_canon {
            continue;
        }
        plan.push(PlannedMove {
            src,
            dst,
            covered_episodes: vec![EpisodeSummary {
                episode_id: row.episode_id.clone(),
                episode_num: row.episode,
            }],
            series_title: mapping.target_title.clone(),
            series_id: mapping.series_id.clone(),
            season_val: season_num.to_string(),
            episode_title: row.title.clone().unwrap_or_default(),
            part_number: None,
            aux_kind: Some(aux.kind),
        });
    }

    // Post-pass: duplicate destination check
    let mut seen_dsts: HashSet<PathBuf> = HashSet::new();
    for m in &plan {
        if !seen_dsts.insert(m.dst.clone()) {
            return Err(RenamePlanError::DuplicateTarget(m.dst.clone()));
        }
    }

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::auxiliary_suffix;
    use jumbie_shared::config::organization::CollisionRenameSuffix;
    use std::path::Path;

    #[test]
    fn auxiliary_suffix_preserves_language_tag() {
        assert_eq!(
            auxiliary_suffix(
                Path::new("/media/Show - S01E01.en.srt"),
                "Show - S01E01",
                CollisionRenameSuffix::DotNumeric,
            ),
            ".en.srt"
        );
    }

    #[test]
    fn auxiliary_suffix_preserves_other_qualifiers() {
        // The suffix is whatever trails the expected base, not only a language tag.
        assert_eq!(
            auxiliary_suffix(
                Path::new("/media/Show - S01E01.forced.srt"),
                "Show - S01E01",
                CollisionRenameSuffix::DotNumeric,
            ),
            ".forced.srt"
        );
    }

    #[test]
    fn auxiliary_suffix_strips_collision_counter() {
        // A sidecar left suffixed by collision resolution keeps only its real
        // suffix, so the plan targets the episode's clean name and the move
        // removes the counter instead of nesting another one.
        assert_eq!(
            auxiliary_suffix(
                Path::new("/media/Show - S01E01.001.nfo"),
                "Show - S01E01",
                CollisionRenameSuffix::DotNumeric,
            ),
            ".nfo"
        );
        assert_eq!(
            auxiliary_suffix(
                Path::new("/media/Show - S01E01.001.001.001.nfo"),
                "Show - S01E01",
                CollisionRenameSuffix::DotNumeric,
            ),
            ".nfo"
        );
    }

    #[test]
    fn auxiliary_suffix_strips_counter_before_language_tag() {
        assert_eq!(
            auxiliary_suffix(
                Path::new("/media/Show - S01E01.001.en.srt"),
                "Show - S01E01",
                CollisionRenameSuffix::DotNumeric,
            ),
            ".en.srt"
        );
        // Legacy order (counter after the tag) also normalizes.
        assert_eq!(
            auxiliary_suffix(
                Path::new("/media/Show - S01E01.en.001.srt"),
                "Show - S01E01",
                CollisionRenameSuffix::DotNumeric,
            ),
            ".en.srt"
        );
    }

    #[test]
    fn auxiliary_suffix_falls_back_when_base_does_not_match() {
        // A release-named sidecar that does not share the episode's base keeps its
        // extension rather than inventing a suffix.
        assert_eq!(
            auxiliary_suffix(
                Path::new("/media/Other.Release-en.srt"),
                "Show - S01E01",
                CollisionRenameSuffix::DotNumeric,
            ),
            ".srt"
        );
    }
}
