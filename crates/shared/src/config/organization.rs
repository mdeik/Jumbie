use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// Kept for backward compatibility with older config files that defined rTorrent
// inline. Download-client settings now live in the downloader plugin's dynamic
// config (see PluginsConfig::downloader).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RTorrentSettings {
    pub url: String,
    pub download_path: PathBuf,
    // use_separate_paths and organizer_path work together:
    // When use_separate_paths is false, the downloader places files directly
    // into each series' folder. When true, the downloader uses a separate
    // watch directory and the organizer moves files from there. The organizer_path
    // points to that separate watch directory when applicable.
    #[serde(default)]
    pub use_separate_paths: bool,
    #[serde(default)]
    pub organizer_path: Option<PathBuf>,
    // Priority controls which files get processed first. Defaulting to 0 means
    // "normal priority" — negative values for lower priority, positive for higher.
    #[serde(default)]
    pub priority: i32,
}

/// A library destination root plus its per-root organization settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationRoot {
    pub path: PathBuf,
    /// When false, this root's subdirectories are not auto-listed in
    /// "Managed Folders & Monitoring". The root still appears in destination
    /// selectors (add series, batch move), and series registered in the library
    /// inside it still list — only untracked folders are hidden.
    #[serde(default = "crate::config::default_true")]
    pub include_subdirs_in_managed: bool,
}

impl DestinationRoot {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            include_subdirs_in_managed: true,
        }
    }
}

impl From<PathBuf> for DestinationRoot {
    fn from(path: PathBuf) -> Self {
        Self::new(path)
    }
}

impl From<&str> for DestinationRoot {
    fn from(path: &str) -> Self {
        Self::new(PathBuf::from(path))
    }
}

// File organization is the core competency of the application: the format strings,
// path rules, and naming conventions here define the whole media library layout, so
// they live in their own module rather than a monolithic Config struct.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OrganizationConfig {
    // Empty by default so users configure roots through the UI; the application
    // handles having no destination roots gracefully.
    #[serde(default = "default_destination_roots")]
    pub destination_roots: Vec<DestinationRoot>,
    #[serde(default = "default_collision_handling")]
    pub collision_handling: String,
    // Format strings use ${variable:format} syntax (familiar to Sonarr/Radarr
    // users; the :format suffix e.g. :02 is concise).
    #[serde(default = "default_season_folder_format")]
    pub season_folder_format: String,
    #[serde(default = "default_episode_file_format")]
    pub episode_file_format: String,
    // Absolute numbering gets its own formats because absolute episode numbers are
    // typically much larger (100+ vs 1-26). Both modes resolve through ${episode}:
    // the active numbering mode decides whether that is season-relative or absolute.
    #[serde(default = "default_season_folder_format_absolute")]
    pub season_folder_format_absolute: String,
    #[serde(default = "default_episode_file_format_absolute")]
    pub episode_file_format_absolute: String,
    // Search-key templates, one per numbering mode, mirroring the file formats:
    // the active mode decides which is used. Blank deletes the key entirely.
    #[serde(default = "default_search_format")]
    pub search_format: String,
    #[serde(default = "default_search_format_absolute")]
    pub search_format_absolute: String,
    // Defaults true: organizing names is the point of the application.
    #[serde(default = "crate::config::default_true")]
    pub rename_episodes: bool,
    #[serde(default)]
    pub auto_apply_renames: bool,
    /// How to handle filesystem-illegal characters in original filenames.
    /// Only applies when the original filename is being preserved (renaming
    /// disabled or auto-apply off). When renaming with the template, the
    /// template variables produce clean filenames.
    #[serde(default)]
    pub illegal_char_policy: InvalidCharPolicy,
    /// When true, allow characters that are valid on the backend's operating
    /// system but invalid on others (e.g. `?`, `*` on Linux). When false,
    /// enforce the strictest cross-platform safe set.
    #[serde(default)]
    pub allow_platform_specific_chars: bool,
    /// The suffix format to use when resolving filename collisions via rename.
    /// Only applies when `collision_handling` is `"rename"`.
    #[serde(default)]
    pub collision_rename_suffix: CollisionRenameSuffix,
    /// The part number format to use when injecting part suffixes into filenames
    /// (e.g. `-pt2`, ` (1)`, `-cd1`).
    #[serde(default)]
    pub part_number_format: PartNumberFormat,
}

// An enum (not a string) because the semantics matter and a raw string couldn't
// distinguish "unset" from "set to delete". Available for future filename
// sanitization; not yet used in OrganizationConfig.
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ColonReplacement {
    #[default]
    Delete,
    Dash,
    Underscore,
    Custom,
}

/// The suffix format to append when resolving filename collisions via rename.
///
/// These control how the uniquifying number looks when `collision_handling`
/// is set to `"rename"` and a file already exists at the destination.
/// Parsing accepts all formats, but this is what the backend generates.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CollisionRenameSuffix {
    /// Append `.001`, `.002` before the extension (e.g. `Episode.001.mkv`).
    #[default]
    DotNumeric,
    /// Append ` (1)`, ` (2)` before the extension (e.g. `Episode (1).mkv`).
    ParenNumeric,
    /// Append `_001`, `_002` before the extension (e.g. `Episode_001.mkv`).
    UnderscoreNumeric,
    /// Append `-1`, `-2` before the extension (e.g. `Episode-1.mkv`).
    DashNumeric,
}

/// The format to use when injecting part number suffixes into filenames.
///
/// When a file is detected as having a part number (e.g. CD1, pt2), the
/// system preserves the part number and re-injects it using this format
/// during renames. Parsing (detection) continues to accept all formats.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PartNumberFormat {
    /// `-pt{n}` (e.g. `Episode - pt2.mkv`).
    #[default]
    Pt,
    /// `-cd{n}` (e.g. `Episode - cd2.mkv`).
    Cd,
    /// `-part{n}` (e.g. `Episode - part2.mkv`).
    Part,
    /// `-disc{n}` (e.g. `Episode - disc2.mkv`).
    Disc,
    /// `-{n}` (e.g. `Episode - 2.mkv`).
    DashNumeric,
    /// ` ({n})` before the extension (e.g. `Episode (2).mkv`).
    ParenNumeric,
}

impl PartNumberFormat {
    /// Render a part number suffix using the configured format.
    /// Returns the suffix without the filename (e.g. `-pt2`, ` (1)`, `-cd1`).
    pub fn render(&self, part_number: u32) -> String {
        match self {
            PartNumberFormat::Pt => format!("-pt{}", part_number),
            PartNumberFormat::Cd => format!("-cd{}", part_number),
            PartNumberFormat::Part => format!("-part{}", part_number),
            PartNumberFormat::Disc => format!("-disc{}", part_number),
            PartNumberFormat::DashNumeric => format!("-{}", part_number),
            PartNumberFormat::ParenNumeric => format!(" ({})", part_number),
        }
    }
}

/// Policy for handling filesystem-illegal characters when preserving original
/// filenames (i.e. when renaming is disabled or auto-apply is off).
///
/// The options are ordered from most to least preservation of readability.
/// Unicode look-alikes preserve the visual intent without introducing actual
/// invalid characters. Underscore, hyphen, and space are safe substitution
/// characters. Remove strips the character entirely.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InvalidCharPolicy {
    /// Replace with Unicode look-alike characters (e.g. `?` → `?`, `:` → `:`).
    /// Control characters (\x00-\x1f) are always removed as they have no visual
    /// counterpart and can corrupt text processing.
    Unicode,
    /// Replace with underscore (`_`) — the safest and most compatible choice.
    #[default]
    Underscore,
    /// Replace with hyphen (`-`).
    Hyphen,
    /// Replace with space (` `).
    Space,
    /// Remove the character entirely.
    Remove,
}

impl OrganizationConfig {
    /// The primary destination root for relative path resolution.
    ///
    /// Returns the first configured root, or an empty path if none are set.
    /// SSoT for "which root do relative paths resolve against?".
    pub fn primary_root(&self) -> &Path {
        self.destination_roots
            .first()
            .map_or(Path::new(""), |root| root.path.as_path())
    }

    /// Check if a path is inside (or equal to) any configured destination root.
    ///
    /// Uses `Path::starts_with` which is component-aware — unlike string
    /// `starts_with`, it won't falsely match `/tv/My Show` against
    /// `/tv/My Showcase/S01`. This also handles non-UTF-8 paths natively.
    pub fn contains_path(&self, path: &Path) -> bool {
        self.destination_roots
            .iter()
            .any(|root| path.starts_with(&root.path))
    }
}

impl Default for OrganizationConfig {
    fn default() -> Self {
        Self {
            destination_roots: default_destination_roots(),
            collision_handling: default_collision_handling(),
            season_folder_format: default_season_folder_format(),
            episode_file_format: default_episode_file_format(),
            season_folder_format_absolute: default_season_folder_format_absolute(),
            episode_file_format_absolute: default_episode_file_format_absolute(),
            search_format: default_search_format(),
            search_format_absolute: default_search_format_absolute(),
            rename_episodes: true,
            auto_apply_renames: false,
            illegal_char_policy: InvalidCharPolicy::default(),
            allow_platform_specific_chars: false,
            collision_rename_suffix: CollisionRenameSuffix::default(),
            part_number_format: PartNumberFormat::default(),
        }
    }
}

fn default_destination_roots() -> Vec<DestinationRoot> {
    // Empty signals "not yet configured" rather than a hard-coded path that may
    // not exist on the user's system.
    vec![]
}

// Zero-padding (S01, S02, ...) is the most widely supported media-server format;
// unpadded "Season 1" would sort as "Season 1, Season 10, Season 2". `:auto2`
// keeps that two-digit minimum but grows past it for series with 100+ seasons.
fn default_season_folder_format() -> String {
    "S${season:auto2}".to_string()
}

// The Plex/Jellyfin/Kodi standard, so organized files are recognized immediately.
fn default_episode_file_format() -> String {
    "${series} - S${season:auto2}E${episode:auto2}?{ - ${title}}".to_string()
}

// Absolute-numbered series (long-running anime) have no named seasons, so the
// folder is shorter (just `S01`) — "Season 01" would be misleading. `:auto2`
// renders the canonical season 1 as `S01`.
fn default_season_folder_format_absolute() -> String {
    "S${season:auto2}".to_string()
}

// `:auto2` (not a fixed `:02`) so a season past episode 99 widens consistently
// (E005/E025/E105) instead of mixing widths, while short seasons keep the
// conventional two-digit padding (`E05`). In absolute mode the season is always
// the canonical 1, so `${season:auto2}` renders `S01`.
fn default_episode_file_format_absolute() -> String {
    "${series} - S${season:auto2}E${episode:auto2}?{ - ${title}}".to_string()
}

// The release convention (S01E05) is the most widely matched search key.
fn default_search_format() -> String {
    "S${season:02}E${episode:02}".to_string()
}

// Absolute-numbered series have no meaningful season, so the key is episode-only.
fn default_search_format_absolute() -> String {
    "E${episode:02}".to_string()
}

// "rename" is safer than "skip" or "overwrite": renaming the new file preserves
// both files, whereas overwrite could lose data and skip could orphan files.
fn default_collision_handling() -> String {
    "rename".to_string()
}

/// The organization collision strategies, parsed once from
/// [`OrganizationConfig::collision_handling`] so every caller — a colliding move and
/// a slot replacement alike — agrees on what each configured value means.
///
/// This is the SSoT for the string-to-strategy mapping; an unrecognised value falls
/// back to [`CollisionStrategy::Rename`] (the data-preserving default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionStrategy {
    /// Rename the incoming file to a free suffixed name — both files are preserved.
    Rename,
    /// Skip: the incoming file is not placed; the occupant keeps its slot.
    Skip,
    /// Overwrite: the occupant is replaced (its file is lost).
    Overwrite,
}

impl CollisionStrategy {
    /// Parse the configured strategy. Unknown values preserve data (`Rename`).
    pub fn from_config(config: &OrganizationConfig) -> Self {
        match config.collision_handling.as_str() {
            "overwrite" => Self::Overwrite,
            "skip" => Self::Skip,
            _ => Self::Rename,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_primary_root_returns_first_root() {
        let org = OrganizationConfig {
            destination_roots: vec!["/media/tv".into(), "/media/movies".into()],
            ..Default::default()
        };
        assert_eq!(org.primary_root(), Path::new("/media/tv"));
    }

    #[test]
    fn test_primary_root_empty_returns_empty_path() {
        let org = OrganizationConfig::default();
        assert_eq!(org.primary_root(), Path::new(""));
    }

    #[test]
    fn test_primary_root_single_root() {
        let org = OrganizationConfig {
            destination_roots: vec!["/data".into()],
            ..Default::default()
        };
        assert_eq!(org.primary_root(), Path::new("/data"));
    }

    #[test]
    fn test_contains_path_under_root() {
        let org = OrganizationConfig {
            destination_roots: vec!["/media/tv".into()],
            ..Default::default()
        };
        assert!(org.contains_path(Path::new("/media/tv/My Show/S01/ep.mkv")));
    }

    #[test]
    fn test_contains_path_exact_root_match() {
        let org = OrganizationConfig {
            destination_roots: vec!["/media/tv".into()],
            ..Default::default()
        };
        assert!(org.contains_path(Path::new("/media/tv")));
    }

    #[test]
    fn test_contains_path_not_under_root() {
        let org = OrganizationConfig {
            destination_roots: vec!["/media/tv".into()],
            ..Default::default()
        };
        assert!(!org.contains_path(Path::new("/media/movies/Show")));
    }

    #[test]
    fn test_contains_path_false_prefix_match() {
        // String starts_with would falsely match "/tv/My Show" against
        // "/tv/My Showcase". Path::starts_with must NOT.
        let org = OrganizationConfig {
            destination_roots: vec!["/media/tv/My Show".into()],
            ..Default::default()
        };
        assert!(!org.contains_path(Path::new("/media/tv/My Showcase/S01/ep.mkv")));
        assert!(org.contains_path(Path::new("/media/tv/My Show/S01/ep.mkv")));
    }

    #[test]
    fn test_contains_path_with_spaces_in_root() {
        let org = OrganizationConfig {
            destination_roots: vec!["/media/tv/My Shows".into()],
            ..Default::default()
        };
        assert!(org.contains_path(Path::new("/media/tv/My Shows/Show Name/S01/ep.mkv")));
        assert!(!org.contains_path(Path::new("/media/other/Path")));
    }

    #[test]
    fn test_contains_path_multiple_roots() {
        let org = OrganizationConfig {
            destination_roots: vec!["/media/tv".into(), "/media/movies".into()],
            ..Default::default()
        };
        assert!(org.contains_path(Path::new("/media/tv/Show/S01/ep.mkv")));
        assert!(org.contains_path(Path::new("/media/movies/Film.mkv")));
        assert!(!org.contains_path(Path::new("/media/music/Song.mp3")));
    }

    #[test]
    fn test_contains_path_empty_roots() {
        let org = OrganizationConfig::default();
        assert!(!org.contains_path(Path::new("/media/tv/Show")));
    }

    #[test]
    fn test_contains_path_root_with_trailing_slash_is_same_as_without() {
        // PathBuf normalizes trailing slashes, so both should work.
        let org = OrganizationConfig {
            destination_roots: vec!["/media/tv/".into()],
            ..Default::default()
        };
        assert!(org.contains_path(Path::new("/media/tv/My Show")));
    }

    // ── DestinationRoot ────────────────────────────────────────────────────

    #[test]
    fn test_destination_root_new_defaults_to_including_subdirs() {
        let root = DestinationRoot::new("/media/tv");
        assert_eq!(root.path, PathBuf::from("/media/tv"));
        assert!(root.include_subdirs_in_managed);
    }

    #[test]
    fn test_destination_root_deserializes_object_with_flag() {
        let json = serde_json::json!({
            "path": "/media/tv",
            "include_subdirs_in_managed": false
        });
        let root: DestinationRoot = serde_json::from_value(json).unwrap();
        assert_eq!(root.path, PathBuf::from("/media/tv"));
        assert!(!root.include_subdirs_in_managed);
    }

    #[test]
    fn test_destination_root_object_defaults_flag_when_missing() {
        let json = serde_json::json!({ "path": "/media/tv" });
        let root: DestinationRoot = serde_json::from_value(json).unwrap();
        assert!(root.include_subdirs_in_managed);
    }

    #[test]
    fn test_destination_root_serializes_object_form() {
        let mut root = DestinationRoot::new("/media/tv");
        root.include_subdirs_in_managed = false;
        let value = serde_json::to_value(&root).unwrap();
        assert_eq!(value["path"], "/media/tv");
        assert_eq!(value["include_subdirs_in_managed"], false);
    }

    // ── CollisionRenameSuffix ──────────────────────────────────────────────

    #[test]
    fn test_collision_rename_suffix_default_is_dot_numeric() {
        assert_eq!(
            CollisionRenameSuffix::default(),
            CollisionRenameSuffix::DotNumeric
        );
    }

    #[test]
    fn test_collision_rename_suffix_dot_numeric_format() {
        // Simulate what generate_unique_filename_with_check does for DotNumeric
        assert_eq!(
            format!("{}.{:03}.{}", "Episode", 1, "mkv"),
            "Episode.001.mkv"
        );
        assert_eq!(
            format!("{}.{:03}.{}", "Episode", 12, "mkv"),
            "Episode.012.mkv"
        );
        assert_eq!(
            format!("{}.{:03}.{}", "Episode", 999, "mkv"),
            "Episode.999.mkv"
        );
    }

    #[test]
    fn test_collision_rename_suffix_dash_numeric_format() {
        assert_eq!(format!("{}-{}.{}", "Episode", 1, "mkv"), "Episode-1.mkv");
        assert_eq!(format!("{}-{}.{}", "Episode", 12, "mkv"), "Episode-12.mkv");
    }

    #[test]
    fn test_collision_rename_suffix_underscore_numeric_format() {
        assert_eq!(
            format!("{}_{:03}.{}", "Episode", 1, "mkv"),
            "Episode_001.mkv"
        );
        assert_eq!(
            format!("{}_{:03}.{}", "Episode", 99, "mkv"),
            "Episode_099.mkv"
        );
    }

    #[test]
    fn test_collision_rename_suffix_paren_numeric_format() {
        assert_eq!(
            format!("{} ({}).{}", "Episode", 1, "mkv"),
            "Episode (1).mkv"
        );
        assert_eq!(
            format!("{} ({}).{}", "Episode", 10, "mkv"),
            "Episode (10).mkv"
        );
    }

    #[test]
    fn test_collision_rename_suffix_no_extension() {
        assert_eq!(format!("{}.{:03}", "Episode", 1), "Episode.001");
        assert_eq!(format!("{}-{}", "Episode", 1), "Episode-1");
        assert_eq!(format!("{}_{:03}", "Episode", 1), "Episode_001");
        assert_eq!(format!("{} ({})", "Episode", 1), "Episode (1)");
    }

    #[test]
    fn test_collision_rename_suffix_serde_roundtrip() {
        let variants = [
            CollisionRenameSuffix::DotNumeric,
            CollisionRenameSuffix::ParenNumeric,
            CollisionRenameSuffix::UnderscoreNumeric,
            CollisionRenameSuffix::DashNumeric,
        ];
        for v in &variants {
            let json = serde_json::to_string(v).unwrap();
            let deserialized: CollisionRenameSuffix = serde_json::from_str(&json).unwrap();
            assert_eq!(*v, deserialized, "Serde roundtrip failed for {:?}", v);
        }
    }

    // ── PartNumberFormat ───────────────────────────────────────────────────

    #[test]
    fn test_part_number_format_default_is_pt() {
        assert_eq!(PartNumberFormat::default(), PartNumberFormat::Pt);
    }

    #[test]
    fn test_part_number_format_render_pt() {
        assert_eq!(PartNumberFormat::Pt.render(1), "-pt1");
        assert_eq!(PartNumberFormat::Pt.render(2), "-pt2");
        assert_eq!(PartNumberFormat::Pt.render(10), "-pt10");
    }

    #[test]
    fn test_part_number_format_render_cd() {
        assert_eq!(PartNumberFormat::Cd.render(1), "-cd1");
        assert_eq!(PartNumberFormat::Cd.render(2), "-cd2");
    }

    #[test]
    fn test_part_number_format_render_part() {
        assert_eq!(PartNumberFormat::Part.render(1), "-part1");
        assert_eq!(PartNumberFormat::Part.render(3), "-part3");
    }

    #[test]
    fn test_part_number_format_render_disc() {
        assert_eq!(PartNumberFormat::Disc.render(1), "-disc1");
        assert_eq!(PartNumberFormat::Disc.render(4), "-disc4");
    }

    #[test]
    fn test_part_number_format_render_dash_numeric() {
        assert_eq!(PartNumberFormat::DashNumeric.render(1), "-1");
        assert_eq!(PartNumberFormat::DashNumeric.render(5), "-5");
    }

    #[test]
    fn test_part_number_format_render_paren_numeric() {
        assert_eq!(PartNumberFormat::ParenNumeric.render(1), " (1)");
        assert_eq!(PartNumberFormat::ParenNumeric.render(2), " (2)");
    }

    #[test]
    fn test_part_number_format_serde_roundtrip() {
        let variants = [
            PartNumberFormat::Pt,
            PartNumberFormat::Cd,
            PartNumberFormat::Part,
            PartNumberFormat::Disc,
            PartNumberFormat::DashNumeric,
            PartNumberFormat::ParenNumeric,
        ];
        for v in &variants {
            let json = serde_json::to_string(v).unwrap();
            let deserialized: PartNumberFormat = serde_json::from_str(&json).unwrap();
            assert_eq!(*v, deserialized, "Serde roundtrip failed for {:?}", v);
        }
    }

    #[test]
    fn test_organization_config_defaults_include_new_fields() {
        let org = OrganizationConfig::default();
        assert_eq!(
            org.collision_rename_suffix,
            CollisionRenameSuffix::DotNumeric
        );
        assert_eq!(org.part_number_format, PartNumberFormat::Pt);
    }

    #[test]
    fn test_organization_config_new_fields_serde_default_on_missing() {
        // Simulate an older config JSON that doesn't have the new fields
        let json = serde_json::json!({
            "destination_roots": [],
            "collision_handling": "skip"
        });
        let org: OrganizationConfig = serde_json::from_value(json).unwrap();
        assert_eq!(
            org.collision_rename_suffix,
            CollisionRenameSuffix::DotNumeric
        );
        assert_eq!(org.part_number_format, PartNumberFormat::Pt);
    }

    #[test]
    fn test_organization_config_new_fields_serde_explicit_values() {
        let json = serde_json::json!({
            "destination_roots": [],
            "collision_handling": "rename",
            "collision_rename_suffix": "paren_numeric",
            "part_number_format": "cd"
        });
        let org: OrganizationConfig = serde_json::from_value(json).unwrap();
        assert_eq!(
            org.collision_rename_suffix,
            CollisionRenameSuffix::ParenNumeric
        );
        assert_eq!(org.part_number_format, PartNumberFormat::Cd);
    }
}
