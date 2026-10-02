//! Shared data types for series mapping and episode information.
//!
//! SSoT: any code representing a season override, series settings, monitor mode,
//! or parsed episode info must use these types rather than redefining them.

use regex::{Regex, RegexSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::validation::{
    MAX_SEASON_PATTERNS, MAX_SERIES_PATTERNS, validate_aliases, validate_patterns,
};

/// The numbering scheme used for episode identification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumberingMode {
    Normal,
    Absolute,
}

impl NumberingMode {
    pub fn is_absolute(self) -> bool {
        matches!(self, NumberingMode::Absolute)
    }
}

/// Monitoring behaviour for episode discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MonitorMode {
    All,
    Future,
    Missing,
    Existing,
    Pilot,
    FirstSeason,
    Specials,
    None,
}

/// The default monitor mode when a series is first created or reset.
pub const DEFAULT_MONITOR_MODE: MonitorMode = MonitorMode::None;

impl MonitorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MonitorMode::All => "All",
            MonitorMode::Future => "Future",
            MonitorMode::Missing => "Missing",
            MonitorMode::Existing => "Existing",
            MonitorMode::Pilot => "Pilot",
            MonitorMode::FirstSeason => "FirstSeason",
            MonitorMode::Specials => "Specials",
            MonitorMode::None => "None",
        }
    }

    pub fn help_text(self) -> &'static str {
        match self {
            MonitorMode::All => "Monitor all episodes except specials",
            MonitorMode::Future => {
                "Monitor episodes that have not aired yet and download them once they air"
            }
            MonitorMode::Missing => "Monitor episodes that do not have files or have not aired yet",
            MonitorMode::Existing => "Monitor episodes that are downloaded or organized",
            MonitorMode::Pilot => "Only monitor the first episode of the first season",
            MonitorMode::FirstSeason => "Monitor all episodes of the first season",
            MonitorMode::Specials => "Monitor all special episodes",
            MonitorMode::None => "No episodes will be monitored",
        }
    }
}

/// Pre-compiled regex patterns for efficient title matching.
///
/// Filter-only patterns (no named capture groups) are combined into a single
/// alternation so one match attempt covers them all, with cost O(N) in input
/// length regardless of pattern count. Extraction patterns (with `episode`/`season`
/// named groups) stay individual because the regex crate disallows duplicate named
/// groups across alternation branches; when several exist, a `RegexSet` pre-screen
/// names the matching ones so `captures` runs only where it can succeed.
#[derive(Debug, Clone)]
pub struct CompiledPatterns {
    /// Combined regex for generic (unscoped) filter-only patterns.
    /// Patterns with no named capture groups, joined via `(?:p1)|(?:p2)|...`.
    pub(crate) generic_filter: Option<Regex>,
    /// Individual compiled regexes for generic extraction patterns.
    /// Patterns with `(?P<episode>...)` or `(?P<season>...)` named groups.
    pub(crate) generic_extraction: Vec<Regex>,
    /// Pre-screen for `generic_extraction`, index-aligned with it.
    pub(crate) generic_extraction_screen: Option<RegexSet>,
    /// Per-source patterns: `instance_id → group`.
    pub(crate) per_source: HashMap<String, SourcePatterns>,
}

/// Extraction patterns below this count are cheaper to try directly: a screen
/// adds a scan on top of the `captures` calls it can save.
const EXTRACTION_SCREEN_MIN: usize = 3;

/// Compiled patterns for one source scope.
#[derive(Debug, Clone)]
pub(crate) struct SourcePatterns {
    pub(crate) filter: Option<Regex>,
    pub(crate) extraction: Vec<Regex>,
    pub(crate) extraction_screen: Option<RegexSet>,
}

impl CompiledPatterns {
    /// Compile a set of (possibly source-scoped) pattern strings into
    /// cached regexes for efficient matching.
    ///
    /// Invalid regex patterns are silently skipped (same behaviour as
    /// the original per-pattern loop).
    pub fn compile(patterns: &[String]) -> Self {
        let mut generic_filter = Vec::new();
        let mut generic_extraction = Vec::new();
        let mut per_source: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();

        for pattern in patterns {
            // Blank entries are not patterns (`has_non_empty` ignores them);
            // compiling `""` would match every title, turning a blank line into a
            // catch-all.
            if pattern.trim().is_empty() {
                continue;
            }
            let parsed = super::alias::parse_source_pattern(pattern);
            let is_extraction =
                parsed.pattern.contains("(?P<episode>") || parsed.pattern.contains("(?P<season>");

            match parsed.source_id {
                Some(id) => {
                    let entry = per_source.entry(id).or_default();
                    if is_extraction {
                        entry.1.push(parsed.pattern);
                    } else {
                        entry.0.push(format!("(?:{})", parsed.pattern));
                    }
                }
                None => {
                    if is_extraction {
                        generic_extraction.push(parsed.pattern);
                    } else {
                        generic_filter.push(format!("(?:{})", parsed.pattern));
                    }
                }
            }
        }

        let (generic_extraction, generic_extraction_screen) =
            compile_extraction(&generic_extraction);
        let per_source = per_source
            .into_iter()
            .map(|(id, (filter, extraction))| {
                let (extraction, extraction_screen) = compile_extraction(&extraction);
                (
                    id,
                    SourcePatterns {
                        filter: join_alternation(&filter),
                        extraction,
                        extraction_screen,
                    },
                )
            })
            .collect();

        Self {
            generic_filter: join_alternation(&generic_filter),
            generic_extraction,
            generic_extraction_screen,
            per_source,
        }
    }

    /// Returns true if no patterns are compiled (empty input).
    pub fn is_empty(&self) -> bool {
        self.generic_filter.is_none()
            && self.generic_extraction.is_empty()
            && self.per_source.is_empty()
    }
}

/// Join already-wrapped filter alternatives into one regex.
fn join_alternation(parts: &[String]) -> Option<Regex> {
    if parts.is_empty() {
        return None;
    }
    Regex::new(&parts.join("|")).ok()
}

/// Compile extraction patterns, dropping invalid ones, and build a pre-screen
/// index-aligned with the survivors (so a screen hit maps to the same `captures`
/// candidate).
fn compile_extraction(patterns: &[String]) -> (Vec<Regex>, Option<RegexSet>) {
    let mut compiled = Vec::new();
    let mut stripped = Vec::new();
    for pattern in patterns {
        if let Ok(re) = Regex::new(pattern) {
            stripped.push(super::alias::strip_named_groups(pattern));
            compiled.push(re);
        }
    }
    let screen = if compiled.len() >= EXTRACTION_SCREEN_MIN {
        RegexSet::new(&stripped).ok()
    } else {
        None
    };
    (compiled, screen)
}

// Episode-numbering space conversions (SSoT). Two spaces exist: LOCAL (DB rows,
// episode IDs, UI cells, target paths — the filesystem and DB always use this) and
// SOURCE (the numbering releases use in titles). `episode_offset` is the only bridge
// (`source = local + offset`) and is used solely for searching/parsing, never for
// what is written to the DB or disk. All conversions route through these helpers.

/// Parse a season number from any label form occurring across the app — `"2"`,
/// `"02"`, `"S02"`, `"s2"` — tolerating surrounding whitespace. Returns `None` for
/// non-numeric seasons (e.g. named specials).
///
/// SSoT: every season-string → number conversion goes through this; a bare
/// `str::parse` treats `"S02"` as a failure.
pub fn parse_season_num(season: &str) -> Option<i32> {
    season
        .trim()
        .trim_start_matches(['S', 's'])
        .parse::<i32>()
        .ok()
}

/// A season label that is not a number.
///
/// Returned instead of silently substituting a season, which would generate episode
/// IDs and write rows under a season the caller never asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonParseError {
    /// The offending label, verbatim.
    pub label: String,
}

impl SeasonParseError {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl std::fmt::Display for SeasonParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "season label {:?} is not a number (expected e.g. \"2\", \"02\", \"S02\")",
            self.label
        )
    }
}

impl std::error::Error for SeasonParseError {}

/// Canonical season number for absolute numbering.
///
/// Absolute episode IDs carry no season, but the absolute space is keyed as season 1
/// everywhere else (metadata caches, `season_absolute` overrides, the UI's single
/// season tab, `S01` target paths), so in absolute mode the season is 1 *by
/// definition*, never a guess. Use [`resolve_season_num`] rather than hard-coding it.
///
/// Declared separately from [`DEFAULT_SEASON_NUM`]: they answer different questions
/// and only happen to share the value.
pub const ABSOLUTE_SEASON_NUM: i32 = 1;

/// Season assumed when a submission names no season.
///
/// Submitters routinely omit the season for single-season shows, so a release title
/// with no season number is treated as **season 1** at the parse boundary (both the
/// built-in parser and custom regex patterns).
pub const DEFAULT_SEASON_NUM: i32 = 1;

/// Resolve the season number an episode belongs to, given its label and mode.
///
/// * `absolute` → [`ABSOLUTE_SEASON_NUM`]; the label is not consulted because
///   absolute numbering has one season and its IDs are season-agnostic.
/// * normal → the label must be numeric (`"2"`, `"02"`, `"S02"`), else
///   [`SeasonParseError`] — there is no correct default.
///
/// ```
/// # use jumbie_shared::mapping::resolve_season_num;
/// assert_eq!(resolve_season_num("S02", false).unwrap(), 2);
/// assert!(resolve_season_num("SP", false).is_err());
/// assert_eq!(resolve_season_num("", true).unwrap(), 1);
/// ```
pub fn resolve_season_num(season: &str, absolute: bool) -> Result<i32, SeasonParseError> {
    if absolute {
        return Ok(ABSOLUTE_SEASON_NUM);
    }
    parse_season_num(season).ok_or_else(|| SeasonParseError::new(season))
}

/// Resolve the season for a value already stored as a number (e.g. a DB
/// column, or a parsed release).
///
/// Absolute mode is canonically [`ABSOLUTE_SEASON_NUM`]; in normal mode `None`
/// stays `None` — an unknown season is reported as unknown, never invented.
pub fn resolve_season_opt(season: Option<i32>, absolute: bool) -> Option<i32> {
    if absolute {
        Some(ABSOLUTE_SEASON_NUM)
    } else {
        season
    }
}

/// Local (DB) → source (release-title) episode number: `source = local + offset`.
/// Inverse of [`source_to_local_episode`].
pub fn local_to_source_episode(local_episode: i32, offset: i32) -> i32 {
    local_episode + offset
}

/// Source (release-title) → local (DB) episode number: `local = source - offset`.
/// Inverse of [`local_to_source_episode`].
pub fn source_to_local_episode(source_episode: i32, offset: i32) -> i32 {
    source_episode - offset
}

/// Season-level mapping overrides for a series.
///
/// `Default` is a blank override (no fields configured, empty `season`). It is
/// used as the seed/reset value for form state so that "no saved override" and
/// "saved override" reduce to a single code path. Callers that persist an
/// override set `season` explicitly.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SeasonOverride {
    pub season: String,
    pub episode_start: Option<i32>,
    pub episode_end: Option<i32>,
    /// How many episode cells to render in the UI for this season.
    /// `None` means: show only DB episodes (no placeholders).
    /// When set, renders slots 1..cell_count with placeholders for missing entries.
    #[serde(default)]
    pub cell_count: Option<i32>,
    pub episode_offset: Option<i32>,
    pub alias_season_number: Option<u32>,
    /// Season-level search-format override (`${season}`/`${episode}` template).
    /// `None` inherits the series/global template; `Some("")` is a deliberate
    /// blank, so the query carries no season/episode key.
    #[serde(default)]
    pub search_format: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub reg_patterns: Vec<String>,
}

impl SeasonOverride {
    /// Default episode start when no `episode_start` is configured.
    /// Episode numbering conventionally starts at 1.
    pub const DEFAULT_EPISODE_START: i32 = 1;

    /// Default episode end when no `episode_end` is configured.
    /// Unbounded — allows any episode number up to the max.
    pub const DEFAULT_EPISODE_END: i32 = i32::MAX;

    /// Default episode offset when no `episode_offset` is configured.
    /// Zero means source episode numbers map directly to local episode numbers.
    pub const DEFAULT_EPISODE_OFFSET: i32 = 0;

    /// The effective episode offset, defaulting to 0 when unset.
    pub fn offset(&self) -> i32 {
        self.episode_offset.unwrap_or(Self::DEFAULT_EPISODE_OFFSET)
    }

    /// Map a local (DB) episode number to the source number releases use.
    /// SSoT: delegates to [`local_to_source_episode`].
    pub fn source_episode(&self, local_episode: i32) -> i32 {
        local_to_source_episode(local_episode, self.offset())
    }

    /// Map a source (release-title) episode number back to the local (DB) number.
    /// SSoT: delegates to [`source_to_local_episode`].
    pub fn local_episode(&self, source_episode: i32) -> i32 {
        source_to_local_episode(source_episode, self.offset())
    }

    /// Check if a source episode number (from filename/RSS) falls within this
    /// override's range **after** applying the episode offset.
    ///
    /// Formula: `local_ep = source_ep - offset`  then  `local_ep in [start, end]`
    pub fn contains_source_episode(&self, source_episode: i32) -> bool {
        self.contains_local_episode(self.local_episode(source_episode))
    }

    /// Resolve the effective season number to use when building search queries:
    /// when `alias_season_number` is set, source plugins search that season instead
    /// of the real one.
    ///
    /// SSoT: all auto-search code paths call this instead of inlining the alias check.
    pub fn effective_search_season(&self, real_season: i32) -> i32 {
        self.alias_season_number
            .map(|n| n as i32)
            .unwrap_or(real_season)
    }

    /// Check if a local (DB) episode number falls within this override's range
    /// **without** applying any offset.  Used by `should_monitor_episode` where
    /// the episode number is already in the local (DB) numbering space.
    pub fn contains_local_episode(&self, local_episode: i32) -> bool {
        let start = self.episode_start.unwrap_or(Self::DEFAULT_EPISODE_START);
        let end = self.episode_end.unwrap_or(Self::DEFAULT_EPISODE_END);
        local_episode >= start && local_episode <= end
    }

    /// Number of in-range episode cells this override expects for its season.
    ///
    /// Returns `None` when no positive `cell_count` is configured (the caller
    /// falls back to the number of episodes actually present). Cells in
    /// `1..=cell_count` that fall outside `[episode_start, episode_end]` are
    /// excluded — they are the out-of-range slots `fill_missing_episodes` marks
    /// unmonitored, so they must not inflate the expected total.
    pub fn in_range_cell_count(&self) -> Option<i32> {
        let cell_count = self.cell_count.filter(|&n| n > 0)?;
        let start = self
            .episode_start
            .unwrap_or(Self::DEFAULT_EPISODE_START)
            .max(1);
        let end = self
            .episode_end
            .unwrap_or(Self::DEFAULT_EPISODE_END)
            .min(cell_count);
        Some((end - start + 1).max(0))
    }

    /// Iterate over non-empty aliases, filtering blank lines.
    /// SSoT: delegates to `non_empty_strs` in `mapping::validation`.
    pub fn search_aliases(&self) -> impl Iterator<Item = &str> {
        crate::mapping::validation::non_empty_strs(&self.aliases)
    }

    /// Whether any non-empty alias exists (blank-line-safe).
    /// SSoT: delegates to `has_non_empty` in `mapping::validation`.
    pub fn has_search_aliases(&self) -> bool {
        crate::mapping::validation::has_non_empty(&self.aliases)
    }

    /// Iterate over non-empty regex patterns, filtering blank lines.
    /// SSoT: delegates to `non_empty_strs` in `mapping::validation`.
    pub fn search_patterns(&self) -> impl Iterator<Item = &str> {
        crate::mapping::validation::non_empty_strs(&self.reg_patterns)
    }

    /// Whether any non-empty pattern exists (blank-line-safe).
    /// SSoT: delegates to `has_non_empty` in `mapping::validation`.
    pub fn has_search_patterns(&self) -> bool {
        crate::mapping::validation::has_non_empty(&self.reg_patterns)
    }

    pub fn validate(&self) -> Result<(), String> {
        crate::validation::validate_season_number(&self.season).map_err(|e| e.to_string())?;

        if let (Some(start), Some(end)) = (self.episode_start, self.episode_end) {
            crate::validation::validate_episode_number(start).map_err(|e| e.to_string())?;
            crate::validation::validate_episode_number(end).map_err(|e| e.to_string())?;
            if end < start {
                return Err("episode_end must be >= episode_start".to_string());
            }
        } else if let Some(start) = self.episode_start {
            crate::validation::validate_episode_number(start).map_err(|e| e.to_string())?;
        } else if let Some(end) = self.episode_end {
            crate::validation::validate_episode_number(end).map_err(|e| e.to_string())?;
        }

        if let Some(offset) = self.episode_offset {
            crate::validation::validate_episode_offset(offset).map_err(|e| e.to_string())?;
        }

        super::validation::validate_search_format(
            self.search_format.as_deref(),
            &format!("Season {}", self.season),
        )?;

        validate_aliases(&self.aliases, &format!("Season {}", self.season))?;
        validate_patterns(
            &self.reg_patterns,
            MAX_SEASON_PATTERNS,
            &format!("Season {}", self.season),
        )?;

        Ok(())
    }
}

/// Per-series settings for a tracked series.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SeriesSettings {
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub absolute_numbering: Option<bool>,
    #[serde(default)]
    pub season: HashMap<String, SeasonOverride>,
    /// Season overrides for absolute numbering mode.
    /// Completely independent from `season` — same season keys can exist
    /// in both maps with different values, enabling per-mode cell counts,
    /// episode ranges, and aliases.
    #[serde(default)]
    pub season_absolute: HashMap<String, SeasonOverride>,
    #[serde(default)]
    pub reg_patterns: Vec<String>,
    #[serde(default)]
    pub season_folder_format: Option<String>,
    #[serde(default)]
    pub episode_file_format: Option<String>,
    #[serde(default)]
    pub season_folder_format_absolute: Option<String>,
    #[serde(default)]
    pub episode_file_format_absolute: Option<String>,
    #[serde(default)]
    pub flatten_season_folders: Option<bool>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub monitor_mode: Option<MonitorMode>,
    #[serde(default)]
    pub metadata_ids: HashMap<String, String>,
    /// Per-provider last synced timestamps.
    /// Key: instance id (same key as `metadata_ids`).
    /// Value: ISO-8601 timestamp of the last successful metadata sync for that provider/ID pair.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata_last_synced_at: HashMap<String, String>,
    /// Per-series override for episode renaming.
    /// `None` = inherit from global config; `Some(true)` = rename; `Some(false)` = keep original filename.
    #[serde(default)]
    pub rename_episodes: Option<bool>,
    /// Per-series search-format override for normal numbering.
    /// `None` inherits the global default; `Some("")` is a deliberate blank.
    #[serde(default)]
    pub search_format: Option<String>,
    /// Per-series search-format override for absolute numbering.
    #[serde(default)]
    pub search_format_absolute: Option<String>,
    /// Directory mtimes (in fractional seconds) at the time of the last scan.
    /// Keyed by relative path from series root: `"."` for the root dir itself,
    /// `"Season 1"` for immediate subdirectories, `"Season 1/Disc 1"` for deeper.
    /// Compared against fresh mtimes to decide whether the series directory needs
    /// a new scan.  An empty map means "never scanned" — the periodic scanner
    /// will treat it as a first-time scan and establish the baseline.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub last_known_dir_mtimes: HashMap<String, f64>,
}

/// Form fields consumed by [`SeriesSettings::from_form`]. Bundled into a struct
/// so the constructor stays readable as more per-series settings are added.
pub struct SeriesSettingsForm {
    /// Each format field is `None` to inherit the global default; the caller
    /// resolves its own inherit/override state (there is no group flag).
    pub season_folder_format: Option<String>,
    pub episode_file_format: Option<String>,
    pub season_folder_format_absolute: Option<String>,
    pub episode_file_format_absolute: Option<String>,
    pub flatten_season_folders: Option<bool>,
    pub absolute_numbering: Option<bool>,
    pub rename_episodes: Option<bool>,
    pub search_format: Option<String>,
    pub search_format_absolute: Option<String>,
    pub metadata_ids: HashMap<String, String>,
}

impl SeriesSettings {
    /// Construct [`SeriesSettings`] from form data.
    ///
    /// Each format field is `Option`: `None` = inherit the server's global
    /// default, `Some(_)` = an explicit override. Fields that vary per call site
    /// (`aliases`, `reg_patterns`, `season`, `season_absolute`, `path`) are left
    /// empty — fill them via struct update syntax:
    ///
    /// ```ignore
    /// SeriesSettings {
    ///     aliases: vec!["foo"],
    ///     reg_patterns: vec!["bar"],
    ///     season: my_season_map,
    ///     season_absolute: my_absolute_map,
    ///     path: Some("/tv/My Series"),
    ///     ..SeriesSettings::from_form(SeriesSettingsForm {
    ///         season_folder_format: form.season_folder_format,
    ///         episode_file_format: form.episode_file_format,
    ///         season_folder_format_absolute: form.season_folder_format_absolute,
    ///         episode_file_format_absolute: form.episode_file_format_absolute,
    ///         flatten_season_folders: form.flatten_season_folders,
    ///         absolute_numbering: form.absolute_numbering,
    ///         rename_episodes: form.rename_episodes,
    ///         search_format: form.search_format,
    ///         search_format_absolute: form.search_format_absolute,
    ///         metadata_ids: form.metadata_ids,
    ///     })
    /// }
    /// ```
    ///
    /// SSoT: every simple per-series setting is set here, so a new call site can only
    /// omit the "varies per call site" fields above.
    pub fn from_form(form: SeriesSettingsForm) -> Self {
        let SeriesSettingsForm {
            season_folder_format,
            episode_file_format,
            season_folder_format_absolute,
            episode_file_format_absolute,
            flatten_season_folders,
            absolute_numbering,
            rename_episodes,
            search_format,
            search_format_absolute,
            metadata_ids,
        } = form;
        Self {
            aliases: Vec::new(),
            reg_patterns: Vec::new(),
            season: HashMap::new(),
            season_absolute: HashMap::new(),
            season_folder_format,
            episode_file_format,
            flatten_season_folders,
            absolute_numbering,
            season_folder_format_absolute,
            episode_file_format_absolute,
            rename_episodes,
            search_format,
            search_format_absolute,
            monitor_mode: None,
            metadata_ids,
            metadata_last_synced_at: HashMap::new(),
            path: None,
            last_known_dir_mtimes: HashMap::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_aliases(&self.aliases, "Series")?;
        validate_patterns(&self.reg_patterns, MAX_SERIES_PATTERNS, "Series")?;
        super::validation::validate_search_format(self.search_format.as_deref(), "Series")?;
        super::validation::validate_search_format(
            self.search_format_absolute.as_deref(),
            "Series (absolute)",
        )?;

        for (season_key, override_rule) in &self.season {
            override_rule
                .validate()
                .map_err(|e| format!("Season {}: {}", season_key, e))?;
        }
        for (season_key, override_rule) in &self.season_absolute {
            override_rule
                .validate()
                .map_err(|e| format!("Season {}: {}", season_key, e))?;
        }

        Ok(())
    }

    /// Resolve the active numbering mode: the series tristate (`Some(true)` =
    /// absolute, `Some(false)` = normal) falls back to the global default when `None`.
    ///
    /// SSoT for the effective mode — every call site must pass
    /// `config.general.absolute_numbering` as the fallback so format, DB-query,
    /// scanner, and metadata decisions can't diverge.
    pub fn active_mode(&self, global_absolute_default: bool) -> NumberingMode {
        if self.absolute_numbering.unwrap_or(global_absolute_default) {
            NumberingMode::Absolute
        } else {
            NumberingMode::Normal
        }
    }

    /// [`active_mode`](Self::active_mode) as a bool.
    ///
    /// Use this instead of reading the `absolute_numbering` tristate directly: a bare
    /// `unwrap_or(false)` turns `None` ("inherit global") into normal mode even when
    /// the global default is absolute, making a no-op update look like a mode switch.
    pub fn effective_absolute_numbering(&self, global_absolute_default: bool) -> bool {
        self.active_mode(global_absolute_default).is_absolute()
    }

    /// Returns a reference to the season overrides for the given numbering mode.
    pub fn season_for_mode(&self, mode: NumberingMode) -> &HashMap<String, SeasonOverride> {
        match mode {
            NumberingMode::Normal => &self.season,
            NumberingMode::Absolute => &self.season_absolute,
        }
    }

    /// Returns a mutable reference to the season overrides for the given numbering mode.
    pub fn season_for_mode_mut(
        &mut self,
        mode: NumberingMode,
    ) -> &mut HashMap<String, SeasonOverride> {
        match mode {
            NumberingMode::Normal => &mut self.season,
            NumberingMode::Absolute => &mut self.season_absolute,
        }
    }

    /// Convenience: returns the season map for the active mode (series tristate
    /// with global-default fallback — see [`SeriesSettings::active_mode`]).
    pub fn season_for_active_mode(
        &self,
        global_absolute_default: bool,
    ) -> &HashMap<String, SeasonOverride> {
        self.season_for_mode(self.active_mode(global_absolute_default))
    }

    /// Find the season override for `season`, tolerating the key formats that occur
    /// across the app: `"2"` (normalised — what the season UI stores), `"02"`
    /// (zero-padded), `"S02"` (`fmt_season` output).
    ///
    /// A plain `HashMap::get` on the raw string would silently miss the override for
    /// the other forms, so season-scoped settings (alias season number, omit-season,
    /// aliases, patterns, ranges) would be ignored.
    ///
    /// SSoT: every season-override lookup should go through this instead of
    /// `season_for_active_mode(..).get(..)` directly.
    pub fn find_season_override(
        &self,
        season: &str,
        global_absolute_default: bool,
    ) -> Option<&SeasonOverride> {
        let overrides = self.season_for_active_mode(global_absolute_default);
        if let Some(override_rule) = overrides.get(season) {
            return Some(override_rule);
        }
        // Fall back to a numeric comparison so "2", "02" and "S02" all match.
        let target = parse_season_num(season)?;
        overrides
            .values()
            .find(|o| parse_season_num(&o.season) == Some(target))
    }

    /// Expected episode count for a season — the SSoT for the "total"/"expected"
    /// numbers shown in the series library and Manage Folders.
    ///
    /// When the season configures a `cell_count`, that many in-range cells are
    /// expected (out-of-range slots excluded, see
    /// [`SeasonOverride::in_range_cell_count`]). Otherwise the season is expected
    /// to hold exactly the episodes it currently has (`present_count`).
    ///
    /// Downloaded/organized counts are intentionally *not* derived here: those
    /// always reflect the files on disk, even outside the configured range.
    pub fn expected_episode_count(
        &self,
        season: &str,
        present_count: i32,
        global_absolute_default: bool,
    ) -> i32 {
        self.find_season_override(season, global_absolute_default)
            .and_then(SeasonOverride::in_range_cell_count)
            .unwrap_or(present_count)
    }

    /// Convenience: returns the mutable season map for the active mode (series
    /// tristate with global-default fallback — see [`SeriesSettings::active_mode`]).
    pub fn season_for_active_mode_mut(
        &mut self,
        global_absolute_default: bool,
    ) -> &mut HashMap<String, SeasonOverride> {
        let mode = self.active_mode(global_absolute_default);
        self.season_for_mode_mut(mode)
    }

    /// Iterate over non-empty aliases, filtering blank lines.
    /// SSoT: delegates to `non_empty_strs` in `mapping::validation`.
    pub fn search_aliases(&self) -> impl Iterator<Item = &str> {
        crate::mapping::validation::non_empty_strs(&self.aliases)
    }

    /// Whether any non-empty alias exists (blank-line-safe).
    /// SSoT: delegates to `has_non_empty` in `mapping::validation`.
    pub fn has_search_aliases(&self) -> bool {
        crate::mapping::validation::has_non_empty(&self.aliases)
    }

    /// Iterate over non-empty regex patterns, filtering blank lines.
    /// SSoT: delegates to `non_empty_strs` in `mapping::validation`.
    pub fn search_patterns(&self) -> impl Iterator<Item = &str> {
        crate::mapping::validation::non_empty_strs(&self.reg_patterns)
    }

    /// Whether any non-empty pattern exists (blank-line-safe).
    /// SSoT: delegates to `has_non_empty` in `mapping::validation`.
    pub fn has_search_patterns(&self) -> bool {
        crate::mapping::validation::has_non_empty(&self.reg_patterns)
    }

    /// Series-level search template for the given numbering mode, falling back to
    /// `global` when the series has no override for that mode.
    pub fn effective_search_format<'a>(&'a self, global: &'a str, absolute: bool) -> &'a str {
        if absolute {
            self.search_format_absolute.as_deref().unwrap_or(global)
        } else {
            self.search_format.as_deref().unwrap_or(global)
        }
    }

    /// Iterate every non-empty alias across the series and **both** season-override
    /// maps (normal + absolute).
    ///
    /// SSoT for "what titles can this series be recognised by": release
    /// identification and file scanning use this rather than `search_aliases()` so
    /// season aliases (alternate titles for a season) participate too.
    pub fn all_aliases(&self) -> impl Iterator<Item = &str> {
        self.search_aliases()
            .chain(self.season.values().flat_map(|o| o.search_aliases()))
            .chain(
                self.season_absolute
                    .values()
                    .flat_map(|o| o.search_aliases()),
            )
    }
}

/// Resolve the effective search-format template, layered season → series → global.
///
/// `Some("")` at any layer is a deliberate blank (the query carries no
/// season/episode key); `None` inherits the layer above. The absolute mode picks
/// the `search_format_absolute` chain. SSoT for every search-key lookup.
pub fn resolve_search_format<'a>(
    series: &'a SeriesSettings,
    season_override: Option<&'a SeasonOverride>,
    global: &'a str,
    absolute: bool,
) -> &'a str {
    season_override
        .and_then(|o| o.search_format.as_deref())
        .unwrap_or_else(|| series.effective_search_format(global, absolute))
}

/// Parsed metadata about a media file's episode content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeInfo {
    pub raw_title: String,
    pub series_key: String,
    pub file_ext: String,
    pub submitter: Option<String>,
    pub resolution: Option<String>,
    #[serde(default = "default_version")]
    pub version: i32,
    /// For multi-part files (e.g. `-part-1`, `-cd2`), the 1-based part number.
    /// `None` for normal single-file episodes.
    #[serde(default)]
    pub part_number: Option<u32>,
    #[serde(default)]
    pub is_season_pack: bool,
    /// Set when the title contains "Complete" and no season was specified.
    /// Signals that this release covers every episode of the series (all seasons).
    #[serde(default)]
    pub is_complete_pack: bool,
    /// Explicit list of seasons (e.g. "S01-S02" or "Season 1-2").
    /// Empty for episode-only files.
    #[serde(default)]
    pub seasons: Vec<i32>,
    /// Explicit list of episode numbers for non-contiguous episode patterns
    /// like "e03, e06". When non-empty, these are the intended episodes.
    /// For contiguous ranges, this contains all episodes (e.g. [1, 2, 3]).
    #[serde(default)]
    pub episodes: Vec<i32>,
    /// Set when the parsed episode number has a decimal component (e.g. `S01E1.5`).
    ///
    /// Episode numbers with fractional components cannot be represented in the
    /// system — all episode numbers are integers. Releases with this flag set
    /// are blocked from auto-download and auto-assignment. The file still appears
    /// in "Manage Series Files" for manual handling.
    #[serde(default)]
    pub has_decimal_episode: bool,
}

fn default_version() -> i32 {
    1
}

#[cfg(test)]
mod find_season_override_tests {
    use super::*;

    fn settings_with_season_key(key: &str, alias: Option<u32>) -> SeriesSettings {
        let mut settings = SeriesSettings::default();
        settings.season.insert(
            key.to_string(),
            SeasonOverride {
                season: key.to_string(),
                episode_start: None,
                episode_end: None,
                cell_count: None,
                episode_offset: None,
                alias_season_number: alias,
                search_format: None,
                aliases: Vec::new(),
                reg_patterns: Vec::new(),
            },
        );
        settings
    }

    #[test]
    fn matches_every_season_key_format() {
        // Overrides are keyed the way the season UI stores them ("2"), but
        // searches pass other representations — the episode-level auto-search
        // sends `EpisodeViewModel.season` ("S02").  All must resolve.
        let settings = settings_with_season_key("2", Some(1));
        for season in ["2", "02", "S02", "s02", " S02 "] {
            let found = settings.find_season_override(season, false);
            assert!(
                found.is_some(),
                "season {season:?} should match the \"2\" override"
            );
            assert_eq!(found.unwrap().alias_season_number, Some(1));
        }
    }

    #[test]
    fn matches_when_override_is_keyed_with_the_padded_form() {
        let settings = settings_with_season_key("02", Some(1));
        assert_eq!(
            settings
                .find_season_override("S02", false)
                .and_then(|o| o.alias_season_number),
            Some(1)
        );
    }

    #[test]
    fn returns_none_when_no_override_matches_the_season() {
        let settings = settings_with_season_key("2", Some(1));
        assert!(settings.find_season_override("S05", false).is_none());
        assert!(settings.find_season_override("3", false).is_none());
    }

    #[test]
    fn non_numeric_season_matches_only_exactly() {
        let settings = settings_with_season_key("SP", None);
        assert!(settings.find_season_override("SP", false).is_some());
        // "S02" parses to 2; "SP" does not parse to a number, so no match.
        assert!(settings.find_season_override("S02", false).is_none());
    }

    #[test]
    fn respects_the_active_numbering_mode() {
        // Override lives in the normal-mode map only.
        let settings = settings_with_season_key("2", Some(1));
        assert!(settings.find_season_override("S02", false).is_some());
        // Absolute mode consults the other map → no match.
        assert!(settings.find_season_override("S02", true).is_none());
    }
}

#[cfg(test)]
mod expected_episode_count_tests {
    use super::*;

    fn override_with(
        cell_count: Option<i32>,
        start: Option<i32>,
        end: Option<i32>,
    ) -> SeasonOverride {
        SeasonOverride {
            season: "1".to_string(),
            episode_start: start,
            episode_end: end,
            cell_count,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: Vec::new(),
            reg_patterns: Vec::new(),
        }
    }

    #[test]
    fn in_range_cell_count_is_none_without_a_positive_cell_count() {
        assert_eq!(override_with(None, None, None).in_range_cell_count(), None);
        assert_eq!(
            override_with(Some(0), None, None).in_range_cell_count(),
            None
        );
    }

    #[test]
    fn in_range_cell_count_is_the_whole_cell_count_without_bounds() {
        assert_eq!(
            override_with(Some(12), None, None).in_range_cell_count(),
            Some(12)
        );
    }

    #[test]
    fn in_range_cell_count_excludes_slots_outside_the_range() {
        // Cells 13..100 are out of range when the season ends at 12.
        assert_eq!(
            override_with(Some(100), Some(1), Some(12)).in_range_cell_count(),
            Some(12)
        );
        // A range starting above 1 trims the leading slots too (3..7 = 5 cells).
        assert_eq!(
            override_with(Some(24), Some(3), Some(7)).in_range_cell_count(),
            Some(5)
        );
        // A bounded range that misses the cell range entirely yields zero.
        assert_eq!(
            override_with(Some(5), Some(10), Some(20)).in_range_cell_count(),
            Some(0)
        );
    }

    #[test]
    fn expected_count_uses_cell_count_when_present() {
        let mut settings = SeriesSettings::default();
        settings
            .season
            .insert("1".to_string(), override_with(Some(12), None, None));
        // 12 configured cells win over the 5 episodes actually present.
        assert_eq!(settings.expected_episode_count("1", 5, false), 12);
        // And out-of-range cells are excluded: 1..100 clipped to end=12.
        settings.season.insert("2".to_string(), {
            let mut o = override_with(Some(100), Some(1), Some(12));
            o.season = "2".to_string();
            o
        });
        assert_eq!(settings.expected_episode_count("2", 3, false), 12);
    }

    #[test]
    fn expected_count_falls_back_to_present_episodes() {
        // No override at all.
        let settings = SeriesSettings::default();
        assert_eq!(settings.expected_episode_count("1", 7, false), 7);

        // Override exists but has no cell_count (range-only override).
        let mut settings = SeriesSettings::default();
        settings
            .season
            .insert("1".to_string(), override_with(None, Some(1), Some(12)));
        assert_eq!(settings.expected_episode_count("1", 7, false), 7);
    }

    #[test]
    fn expected_count_respects_the_active_numbering_mode() {
        let mut settings = SeriesSettings::default();
        settings
            .season
            .insert("1".to_string(), override_with(Some(12), None, None));
        // Cell count lives in the normal-mode map, so absolute mode ignores it.
        assert_eq!(settings.expected_episode_count("1", 5, false), 12);
        assert_eq!(settings.expected_episode_count("1", 5, true), 5);
    }
}

#[cfg(test)]
mod season_number_tests {
    use super::*;

    #[test]
    fn parse_season_num_accepts_every_stored_form() {
        for form in ["2", "02", "S02", "s02", " S02 ", "S2"] {
            assert_eq!(parse_season_num(form), Some(2), "form {form:?}");
        }
    }

    #[test]
    fn parse_season_num_rejects_non_numeric_seasons() {
        for form in ["SP", "Specials", "Absolute", ""] {
            assert_eq!(parse_season_num(form), None, "form {form:?}");
        }
    }

    #[test]
    fn effective_mode_follows_global_when_unset() {
        // `None` = "inherit global" — the tristate must not collapse to Normal.
        let settings = SeriesSettings::default();
        assert!(settings.effective_absolute_numbering(true));
        assert!(!settings.effective_absolute_numbering(false));
    }

    #[test]
    fn effective_mode_series_override_wins_over_global() {
        let absolute = SeriesSettings {
            absolute_numbering: Some(true),
            ..Default::default()
        };
        assert!(absolute.effective_absolute_numbering(false));

        let normal = SeriesSettings {
            absolute_numbering: Some(false),
            ..Default::default()
        };
        assert!(!normal.effective_absolute_numbering(true));
    }

    // ── Absolute numbering is canonically season 1 ──────────────────────

    #[test]
    fn resolve_season_num_in_absolute_mode_is_always_season_one() {
        // Absolute episode IDs carry no season, but the absolute space is keyed
        // as season 1 everywhere else — so the label is irrelevant in that mode,
        // including labels that are NOT numbers (or empty).
        for label in ["1", "2", "S07", "SP", "Specials", ""] {
            assert_eq!(
                resolve_season_num(label, true),
                Ok(ABSOLUTE_SEASON_NUM),
                "label {label:?}"
            );
        }
        assert_eq!(ABSOLUTE_SEASON_NUM, 1);
    }

    #[test]
    fn resolve_season_num_in_normal_mode_requires_a_number() {
        for label in ["2", "02", "S02"] {
            assert_eq!(resolve_season_num(label, false), Ok(2), "label {label:?}");
        }
        for label in ["SP", "Specials", ""] {
            assert!(resolve_season_num(label, false).is_err(), "label {label:?}");
        }
    }

    #[test]
    fn resolve_season_opt_canonicalises_absolute_and_preserves_normal() {
        // Absolute: always season 1, even when the stored value is NULL/other.
        assert_eq!(resolve_season_opt(None, true), Some(ABSOLUTE_SEASON_NUM));
        assert_eq!(resolve_season_opt(Some(7), true), Some(ABSOLUTE_SEASON_NUM));
        // Normal: reported as-is — an unknown season stays unknown.
        assert_eq!(resolve_season_opt(Some(7), false), Some(7));
        assert_eq!(resolve_season_opt(None, false), None);
    }

    // ── Extraction pre-screen ───────────────────────────────────────────

    fn compile(patterns: &[&str]) -> CompiledPatterns {
        let owned: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        CompiledPatterns::compile(&owned)
    }

    fn extraction_episode(patterns: &[&str], title: &str) -> Option<i32> {
        match crate::parsing::match_title_compiled(title, &compile(patterns), None, false, None) {
            crate::parsing::CustomParseResult::Extracted(info) => info.episodes.first().copied(),
            _ => None,
        }
    }

    #[test]
    fn extraction_screen_preserves_linear_order() {
        let patterns = [
            r"Alpha.*E(?P<episode>\d+)",
            r"Bravo.*E(?P<episode>\d+)",
            r"Charlie.*E(?P<episode>\d+)",
            r"Delta.*E(?P<episode>\d+)",
        ];
        assert_eq!(extraction_episode(&patterns, "Alpha E01"), Some(1));
        assert_eq!(extraction_episode(&patterns, "Bravo E02"), Some(2));
        assert_eq!(extraction_episode(&patterns, "Charlie E03"), Some(3));
        assert_eq!(extraction_episode(&patterns, "Delta E04"), Some(4));
        assert_eq!(extraction_episode(&patterns, "Echo E05"), None);
    }

    #[test]
    fn extraction_screen_falls_through_non_numeric_captures() {
        // The first two patterns match but capture non-numeric text; the third wins.
        let patterns = [
            r"(?P<episode>[a-z]+)",
            r"(?P<episode>[A-Z]+)",
            r"E(?P<episode>\d+)",
        ];
        assert_eq!(extraction_episode(&patterns, "Show E42"), Some(42));
    }

    #[test]
    fn extraction_screen_stays_aligned_after_an_invalid_pattern() {
        // The duplicate-name pattern is invalid and dropped, so the screen must be
        // built from the survivors to stay index-aligned with them.
        let patterns = [
            r"dup(?P<episode>\d+)(?P<episode>\d+)",
            r"One.*E(?P<episode>\d+)",
            r"Two.*E(?P<episode>\d+)",
            r"Three.*E(?P<episode>\d+)",
        ];
        assert_eq!(extraction_episode(&patterns, "Two E07"), Some(7));
        assert_eq!(extraction_episode(&patterns, "Three E08"), Some(8));
    }

    #[test]
    fn blank_pattern_is_a_no_op() {
        let patterns = ["", r"E(?P<episode>\d+)"];
        assert!(matches!(
            crate::parsing::match_title_compiled(
                "no episode here",
                &compile(&patterns),
                None,
                false,
                None
            ),
            crate::parsing::CustomParseResult::NoMatch
        ));
        assert_eq!(extraction_episode(&patterns, "E42"), Some(42));
    }

    #[test]
    fn filter_pattern_wins_over_extraction_screen() {
        let patterns = [
            r"FiltOnly",
            r"One.*E(?P<episode>\d+)",
            r"Two.*E(?P<episode>\d+)",
            r"Three.*E(?P<episode>\d+)",
        ];
        assert!(matches!(
            crate::parsing::match_title_compiled(
                "FiltOnly",
                &compile(&patterns),
                None,
                false,
                None
            ),
            crate::parsing::CustomParseResult::MatchedFilter
        ));
    }
}
