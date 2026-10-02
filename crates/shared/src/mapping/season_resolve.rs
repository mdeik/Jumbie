//! Effective-season resolution for a parsed file — the SSoT shared by the
//! backend scanner (`scan`/`import`/`smart_link`) and the frontend
//! "Manage Series Files" auto-assign flow, so a file's season can never be
//! resolved two different ways.
//!
//! Resolution order:
//!
//! 1. **Season aliases** win (normal numbering mode only) — a title matching a
//!    season alias supplies the season and counts as explicit, so mode-aware
//!    callers accept the file.
//! 2. The **filename's** declared season (e.g. `S02E05`) — explicit.
//! 3. A **season folder** in the path (e.g. `.../Season 2/file.mkv`) — inferred,
//!    not explicit.
//! 4. The caller's **fallback** season, else [`DEFAULT_SEASON_NUM`].
//!
//! A season-vs-season alias conflict that the parsed season number cannot break
//! yields [`ResolvedSeason::Unneeded`]: the caller must leave the file
//! unassigned rather than guess.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::season_alias::{SeasonAliasDecision, resolve_season_from_aliases};
use super::types::{DEFAULT_SEASON_NUM, EpisodeInfo, SeriesSettings};

// Season-from-folder inference patterns (compiled once at module level): when a
// filename lacks an explicit season, scan parent directory names for patterns
// like "Season 1" or "S01".
static SEASON_FOLDER_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)^season[\s_]*?(\d+)$").unwrap(),
        Regex::new(r"^(?i)S(\d+)$").unwrap(),
    ]
});

/// Infer the season number from a file's parent directory structure.
///
/// Walks up the directory tree looking for folders named like "Season 1",
/// "Season_02", "S01", etc. Returns the parsed season number on first match.
/// This is the SSoT for path-based season inference — both the scanner and
/// smart_link use this function rather than duplicating the logic.
pub fn infer_season_from_path(file_path: &Path) -> Option<i32> {
    let parent = file_path.parent()?;
    for component in parent.ancestors() {
        if let Some(dirname) = component.file_name().and_then(|n| n.to_str()) {
            for re in SEASON_FOLDER_PATTERNS.iter() {
                if let Some(caps) = re.captures(dirname)
                    && let Ok(num) = caps[1].parse::<i32>()
                {
                    return Some(num);
                }
            }
        }
    }
    None
}

/// Outcome of [`resolve_season_for_series`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSeason {
    /// The resolved season and whether it was explicit (in the filename, or
    /// supplied by a season alias).
    Season(String, bool),
    /// Season aliases matched ambiguously and no season number disambiguated —
    /// the caller must leave the file unassigned (treat it as unneeded).
    Unneeded,
}

/// Resolve the effective season for a parsed file, honoring season aliases.
///
/// A season-alias match supplies the season and is reported as explicit so
/// mode-aware callers accept the file. A season-vs-season alias conflict that
/// the parsed season number cannot resolve yields [`ResolvedSeason::Unneeded`].
/// Otherwise the normal filename/folder/default logic applies (default = season
/// 1, or `fallback_season` when supplied).
///
/// Alias resolution is only applied in normal numbering mode: absolute mode has
/// exactly one season ([`super::types::ABSOLUTE_SEASON_NUM`]).
pub fn resolve_season_for_series(
    info: &EpisodeInfo,
    file_path: &Path,
    settings: &SeriesSettings,
    absolute: bool,
) -> ResolvedSeason {
    resolve_season_for_series_with_fallback(info, file_path, settings, absolute, None)
}

/// Like [`resolve_season_for_series`], but with a caller-supplied season used only
/// when neither the filename nor the folder names a season (e.g. a download
/// queue item's season label). `None` falls back to season 1.
pub fn resolve_season_for_series_with_fallback(
    info: &EpisodeInfo,
    file_path: &Path,
    settings: &SeriesSettings,
    absolute: bool,
    fallback_season: Option<i32>,
) -> ResolvedSeason {
    if !absolute {
        match resolve_season_from_aliases(
            &info.series_key,
            settings,
            absolute,
            info.seasons.first().copied(),
        ) {
            SeasonAliasDecision::UseSeason(season) => {
                return ResolvedSeason::Season(format!("{:02}", season), true);
            }
            SeasonAliasDecision::Unneeded => return ResolvedSeason::Unneeded,
            SeasonAliasDecision::NoSignal => {}
        }
    }
    let (season, was_explicit) = resolve_season_raw(info, file_path, fallback_season);
    ResolvedSeason::Season(season, was_explicit)
}

/// Shared core: filename season → folder season → fallback (`None` → season 1).
///
/// Returns `(season_str, was_explicit)` where:
/// - `was_explicit = true`  → season came from the filename itself (e.g. "S01E05")
/// - `was_explicit = false` → season was inferred from folder name or defaulted
pub fn resolve_season_raw(
    info: &EpisodeInfo,
    file_path: &Path,
    fallback_season: Option<i32>,
) -> (String, bool) {
    if let Some(&s) = info.seasons.first() {
        (format!("{:02}", s), true)
    } else if let Some(s) = infer_season_from_path(file_path) {
        (format!("{:02}", s), false)
    } else {
        (
            format!("{:02}", fallback_season.unwrap_or(DEFAULT_SEASON_NUM)),
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{SeasonOverride, SeriesSettings};

    fn info(series_key: &str, seasons: Vec<i32>) -> EpisodeInfo {
        EpisodeInfo {
            raw_title: String::new(),
            series_key: series_key.to_string(),
            file_ext: String::new(),
            submitter: None,
            resolution: None,
            version: 1,
            part_number: None,
            is_season_pack: false,
            is_complete_pack: false,
            seasons,
            episodes: vec![1],
            has_decimal_episode: false,
        }
    }

    fn season_alias(season: &str, alias: &str) -> SeasonOverride {
        SeasonOverride {
            season: season.to_string(),
            aliases: vec![alias.to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn alias_supplies_season_and_counts_as_explicit() {
        let mut settings = SeriesSettings::default();
        settings
            .season
            .insert("0".to_string(), season_alias("0", "Whisker Mini"));
        let path = Path::new("/dl/Whisker Mini Anime - 01.mkv");
        match resolve_season_for_series(&info("Whisker Mini Anime", vec![]), path, &settings, false) {
            ResolvedSeason::Season(season, explicit) => {
                assert_eq!(season, "00");
                assert!(explicit);
            }
            ResolvedSeason::Unneeded => panic!("expected the alias to win"),
        }
    }

    #[test]
    fn aliases_are_ignored_in_absolute_mode() {
        let mut settings = SeriesSettings::default();
        settings
            .season_absolute
            .insert("1".to_string(), season_alias("1", "Whisker Mini"));
        let path = Path::new("/dl/Whisker Mini Anime - 01.mkv");
        match resolve_season_for_series(
            &info("Whisker Mini Anime", vec![]),
            path,
            &settings,
            true,
        ) {
            ResolvedSeason::Season(season, _) => assert_eq!(season, "01"),
            ResolvedSeason::Unneeded => panic!("absolute mode must not be unneeded"),
        }
    }

    #[test]
    fn folder_season_is_used_when_filename_omits_it() {
        let settings = SeriesSettings::default();
        let path = Path::new("/media/Series Name/Season 2/file.mkv");
        match resolve_season_for_series(&info("", vec![]), path, &settings, false) {
            ResolvedSeason::Season(season, explicit) => {
                assert_eq!(season, "02");
                assert!(!explicit, "folder season is inferred, not explicit");
            }
            ResolvedSeason::Unneeded => panic!("expected folder inference"),
        }
    }
}
