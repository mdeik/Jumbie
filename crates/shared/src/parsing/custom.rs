use super::extract::*;
use crate::mapping::DEFAULT_SEASON_NUM;
use crate::mapping::EpisodeInfo;
use crate::mapping::types::CompiledPatterns;
use regex::RegexSet;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

/// Cache of compiled regex patterns, keyed by the pattern strings themselves.
///
/// `Regex::new()` is expensive, so series sharing the same `reg_patterns` share one
/// compilation. Unique pattern sets are bounded by the number of series, so no
/// eviction is needed. Keying on the strings (rather than a hash) makes a collision
/// impossible.
static COMPILED_CACHE: LazyLock<Mutex<HashMap<Vec<String>, CompiledPatterns>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn get_or_compile_patterns(patterns: &[String]) -> CompiledPatterns {
    let mut cache = COMPILED_CACHE.lock().unwrap();
    if let Some(compiled) = cache.get(patterns) {
        return compiled.clone();
    }
    let compiled = CompiledPatterns::compile(patterns);
    cache.insert(patterns.to_vec(), compiled.clone());
    compiled
}

/// Result of trying to parse a title with custom regex patterns.
#[derive(Debug, Clone)]
pub enum CustomParseResult {
    /// Successfully extracted episode info from named capture groups.
    /// The caller should use these values instead of the default `parse_filename` result.
    Extracted(Box<EpisodeInfo>),
    /// A pattern matched but has no named capture groups.
    /// The caller should use the default `parse_filename` result (current filter behavior).
    MatchedFilter,
    /// No pattern matched the title at all.
    NoMatch,
}

/// Try to parse a release title using user-defined regex patterns with named capture groups.
///
/// # Named capture groups
///
/// | Group name     | Meaning                      | Required |
/// |----------------|------------------------------|----------|
/// | `episode`      | The episode number           | Yes      |
/// | `season`       | The season number            | No       |
///
/// # Behavior
///
/// - `episode` group → `Extracted(EpisodeInfo)`; the caller uses these values instead
///   of the default `parse_filename` result. A `season` group is ignored in absolute
///   mode; with no `season` group, `current_season` supplies it.
/// - `season` group but no `episode` group → a **season pack**: `is_season_pack` is
///   true, the anchor episode is 1, and the season is the captured value (letting
///   patterns like `S(?P<season>\d+) Complete` identify packs with non-standard
///   naming). `is_complete_pack` is also set when the title contains "complete".
/// - No named groups → `MatchedFilter` (filter-only; caller uses `parse_filename`).
/// - No match → `NoMatch`; the entry should be rejected.
///
/// # Season pack caution
///
/// A pattern with an `episode` group always yields `is_season_pack: false`. A loose
/// pattern like `(?P<episode>\d+)` can match a pack title (`"Show - S01 [1080p]"` →
/// episode 1) and misclassify it as a single episode; use the `season`-only approach
/// for pack detection.
///
/// # Source scope
///
/// Patterns may carry a `@source-slug:pattern` prefix (see `parse_source_pattern`);
/// when `instance_id` is given, only patterns matching that ID are considered.
///
/// This is the string-based entry point; it caches compiled regexes so repeated calls
/// with the same patterns reuse one `CompiledPatterns`. See [`match_title_compiled`]
/// for the pre-compiled API.
pub fn parse_title_with_custom_regex(
    title: &str,
    patterns: &[String],
    current_season: Option<i32>,
    absolute_numbering: bool,
    instance_id: Option<&str>,
) -> CustomParseResult {
    let compiled = get_or_compile_patterns(patterns);
    match_title_compiled(
        title,
        &compiled,
        current_season,
        absolute_numbering,
        instance_id,
    )
}

/// Match a title against pre-compiled `CompiledPatterns`.
///
/// Filter-only patterns are collapsed into one alternation and extraction patterns
/// are pre-screened, so the cost grows with the input length rather than the pattern
/// count.
///
/// # Match order (preserving ordinal semantics)
///
/// 1. **Generic filter** — single combined alternation regex (at most 1 match).
/// 2. **Generic extraction** — pre-screened candidates, captures tried in order.
/// 3. **Source-scoped filter** — combined alternation (if the source matches).
/// 4. **Source-scoped extraction** — pre-screened candidates, captures tried in order.
///
/// Filter patterns take priority over extraction patterns; extraction patterns keep
/// ordinal semantics (first match wins), and a non-numeric capture falls through to
/// the next extraction pattern. Compilation cost is amortized via `COMPILED_CACHE`.
pub fn match_title_compiled(
    title: &str,
    patterns: &CompiledPatterns,
    current_season: Option<i32>,
    absolute_numbering: bool,
    instance_id: Option<&str>,
) -> CustomParseResult {
    // 1. Try generic filter regex first (filter patterns take priority).
    if let Some(re) = &patterns.generic_filter
        && re.is_match(title)
    {
        return CustomParseResult::MatchedFilter;
    }

    // 2. Try generic extraction patterns in order.
    if let Some(result) = match_extraction(
        &patterns.generic_extraction,
        patterns.generic_extraction_screen.as_ref(),
        title,
        current_season,
        absolute_numbering,
    ) {
        return result;
    }

    // 3. Try per-source patterns (only if a source context is provided).
    if let Some(id) = instance_id
        && let Some(group) = patterns.per_source.get(id)
    {
        // 3a. Source-scoped filter first.
        if let Some(re) = &group.filter
            && re.is_match(title)
        {
            return CustomParseResult::MatchedFilter;
        }
        // 3b. Source-scoped extraction patterns in order.
        if let Some(result) = match_extraction(
            &group.extraction,
            group.extraction_screen.as_ref(),
            title,
            current_season,
            absolute_numbering,
        ) {
            return result;
        }
    }

    CustomParseResult::NoMatch
}

/// The first extraction pattern that matches and yields a numeric capture, in
/// order. A pre-screen, when present and index-aligned with `extraction`, avoids
/// running `captures` on patterns that cannot match.
fn match_extraction(
    extraction: &[regex::Regex],
    screen: Option<&RegexSet>,
    title: &str,
    current_season: Option<i32>,
    absolute_numbering: bool,
) -> Option<CustomParseResult> {
    match screen {
        Some(screen) => screen.matches(title).iter().find_map(|i| {
            try_compiled_extraction(&extraction[i], title, current_season, absolute_numbering)
        }),
        None => extraction
            .iter()
            .find_map(|re| try_compiled_extraction(re, title, current_season, absolute_numbering)),
    }
}

/// Construct the common fields of an `EpisodeInfo` from a parsed title.
///
/// Both the episode-extraction and season-pack paths in
/// `try_compiled_extraction` share these fields.
fn make_episode_info(
    title: &str,
    series_key: String,
    seasons: Vec<i32>,
    episodes: Vec<i32>,
) -> EpisodeInfo {
    EpisodeInfo {
        raw_title: title.to_string(),
        series_key,
        seasons,
        episodes,
        file_ext: std::path::Path::new(title)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mkv")
            .to_lowercase(),
        submitter: extract_submitter(title),
        resolution: None,
        version: extract_version(title),
        part_number: detect_part_number(title),
        is_season_pack: false,
        is_complete_pack: false,
        has_decimal_episode: crate::patterns::DECIMAL_EPISODE_PATTERN.is_match(title),
    }
}

/// Try to extract episode info from a single compiled regex pattern.
///
/// Returns `Some(Extracted(...))` on a match with valid numeric captures, or `None`
/// if it doesn't match or the capture is non-numeric (caller tries the next pattern).
/// This mirrors the original `continue` behaviour: only the failed pattern is skipped.
fn try_compiled_extraction(
    re: &regex::Regex,
    title: &str,
    current_season: Option<i32>,
    absolute_numbering: bool,
) -> Option<CustomParseResult> {
    let caps = re.captures(title)?;

    // Check for episode named group — the signal for an extraction pattern.
    if let Some(ep_match) = caps.name("episode") {
        let episode_num: i32 = match ep_match.as_str().parse() {
            Ok(n) => n,
            Err(_) => return None,
        };

        let season: Option<i32> = if let Some(s_match) = caps.name("season") {
            if absolute_numbering {
                None
            } else {
                match s_match.as_str().parse() {
                    Ok(n) => Some(n),
                    Err(_) => return None,
                }
            }
        } else if absolute_numbering {
            None
        } else {
            // Submitter convention: a title with no season number is season 1.
            current_season.or(Some(DEFAULT_SEASON_NUM))
        };

        let title_part = clean_title(title);
        let series_key = if title_part.is_empty() {
            title.to_string()
        } else {
            title_part
        };

        return Some(CustomParseResult::Extracted(Box::new(make_episode_info(
            title,
            series_key,
            season.map(|s| vec![s]).unwrap_or_default(),
            vec![episode_num],
        ))));
    }

    // Check for season-only named group — treat as a season pack.
    if let Some(s_match) = caps.name("season") {
        let season_num: i32 = match s_match.as_str().parse() {
            Ok(n) => n,
            Err(_) => return None,
        };

        let title_part = clean_title(title);
        let series_key = if title_part.is_empty() {
            title.to_string()
        } else {
            title_part
        };

        let has_complete = title.to_lowercase().contains("complete");

        // The episode list (`vec![1]`) is the pack's anchor episode: a season-only
        // release is a pack of the named season, anchored at its episode 1. This is
        // NOT a season default — the season is `season_num` above.
        let mut info = make_episode_info(title, series_key, vec![season_num], vec![1]);
        info.is_season_pack = true;
        info.is_complete_pack = has_complete;
        return Some(CustomParseResult::Extracted(Box::new(info)));
    }

    // Pattern matched but has no named groups — should not happen for
    // extraction patterns, but handle gracefully.
    Some(CustomParseResult::MatchedFilter)
}
