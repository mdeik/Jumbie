//! Formatting helpers for episode numbering.
//!
//! SSoT: every episode-label, episode-key, or search-term format lives **only** in
//! the helper functions below. Callers must **never** hardcode `E{:03}`, `E{:02}`,
//! `S{:02}E{:02}`, etc. directly — always route through one of these functions.
//!
//! | Function                          | Output example (Short) | Output example (Human)       |
//! |-----------------------------------|------------------------|------------------------------|
//! | [`fmt_abs_episode`]               | `E01`                  | `Episode 01`                 |
//! | [`fmt_episode`]                   | `E01`                  | `Episode 05`                 |
//! | [`fmt_season`]                    | `S01`                  | `Season 01`                  |
//! | [`fmt_season_episode`]            | `S01E05`               | `Season 01 Episode 05`       |
//! | [`fmt_episode_id`]                | `{uuid}_S01E05` (standard) / `{uuid}_ABS0003` (absolute) | — |
//!
//! `fmt_episode_id` uses the `ABS` prefix with 4-digit padding in absolute mode so
//! absolute IDs never collide with standard `S01E05` IDs and still sort
//! lexicographically (up to 9,999 episodes).

/// Controls the label style for season / episode formatting functions.
///
/// * [`Short`](LabelStyle::Short) — compact codes (`S01`, `E05`, `S01E05`).
/// * [`Human`](LabelStyle::Human) — human-readable text (`Season 01`, `Episode 05`, …).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelStyle {
    /// Compact short codes: `S01`, `E05`, `S01E05`.
    #[default]
    Short,
    /// Human-readable text: `Season 01`, `Episode 05`, `Season 01 Episode 05`.
    Human,
}

// Search-query construction is owned by the plugin SDK (`plugin_sdk::query`) so
// every source — built-in or external — shares one query format. Re-exported here
// so the app and frontend keep importing the builders from
// `jumbie_shared::formatting`.
pub use plugin_sdk::query::{SearchParams, build_search_queries, extract_search_params};

use std::collections::HashMap;

/// Render the search key for one episode from a search-format template.
///
/// SSoT for turning `${season}`/`${episode}` into the token a source is queried
/// with (e.g. `S01E05`). A blank template renders to an empty key.
pub fn render_search_key(format: &str, season: i32, episode: i32) -> String {
    let mut vars = HashMap::new();
    vars.insert("season".to_string(), season.to_string());
    vars.insert("episode".to_string(), episode.to_string());
    crate::template::apply_template(format, &vars, None)
}

/// Hail-mary season/episode extraction for search results.
///
/// Used only during search, where the desired `season`/`episode` are both known.
/// When the structured parser cannot place a release, this scans the title for
/// standalone numbers and checks that `season` and `episode` both appear, in that
/// order. A number only counts when every adjacent character is whitespace or a
/// non-alphanumeric symbol (so `1080` in `1080p` and `02` in `S02` are ignored),
/// and comparison is padding-insensitive (`2` matches `02`). This extracts
/// season/episode only — the series title is still matched through the normal
/// pipeline.
pub fn contains_ordered_season_episode(title: &str, season: i32, episode: i32) -> bool {
    let numbers = standalone_numbers(title);
    match numbers.iter().position(|&n| n == season) {
        Some(season_idx) => numbers[season_idx + 1..].contains(&episode),
        None => false,
    }
}

/// The numeric tokens in `title` that stand alone: each maximal run of digits whose
/// neighbours are whitespace or non-alphanumeric. Leading zeros are dropped when
/// the run is parsed as an integer, making the comparison padding-insensitive.
fn standalone_numbers(title: &str) -> Vec<i32> {
    let chars: Vec<char> = title.chars().collect();
    let mut numbers = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let before_ok = start == 0 || !chars[start - 1].is_alphanumeric();
            let after_ok = i == chars.len() || !chars[i].is_alphanumeric();
            if before_ok
                && after_ok
                && let Ok(n) = chars[start..i].iter().collect::<String>().parse::<i32>()
            {
                numbers.push(n);
            }
        } else {
            i += 1;
        }
    }
    numbers
}

/// Format an absolute episode number with **2-digit** zero-padding.
///
/// When `style` is [`LabelStyle::Short`] (the default) the output uses the
/// compact `E` prefix — e.g. `E01`.
/// When `style` is [`LabelStyle::Human`] the output uses the word `Episode`.
///
/// ```
/// # use jumbie_shared::formatting::{fmt_abs_episode, LabelStyle};
/// assert_eq!(fmt_abs_episode(1, LabelStyle::Short), "E01");
/// assert_eq!(fmt_abs_episode(1, LabelStyle::Human), "Episode 01");
/// assert_eq!(fmt_abs_episode(42, LabelStyle::Short), "E42");
/// ```
pub fn fmt_abs_episode(ep: i32, style: LabelStyle) -> String {
    match style {
        LabelStyle::Short => format!("E{:02}", ep),
        LabelStyle::Human => format!("Episode {:02}", ep),
    }
}

/// Format an episode number with **2-digit** zero-padding.
///
/// This is the general-purpose episode formatter, used both for search queries
/// and display.  When `style` is [`LabelStyle::Short`] (the default) the output
/// uses the compact `E` prefix — e.g. `E01`.
/// When `style` is [`LabelStyle::Human`] the output uses the word `Episode`.
///
/// ```
/// # use jumbie_shared::formatting::{fmt_episode, LabelStyle};
/// assert_eq!(fmt_episode(1, LabelStyle::Short), "E01");
/// assert_eq!(fmt_episode(5, LabelStyle::Human), "Episode 05");
/// assert_eq!(fmt_episode(42, LabelStyle::Short), "E42");
/// ```
pub fn fmt_episode(ep: i32, style: LabelStyle) -> String {
    match style {
        LabelStyle::Short => format!("E{:02}", ep),
        LabelStyle::Human => format!("Episode {:02}", ep),
    }
}

/// Format a season number, zero-padded to 2 digits.
///
/// When `style` is [`LabelStyle::Short`] (the default) the output uses the
/// compact `S` prefix — e.g. `S01`.
/// When `style` is [`LabelStyle::Human`] the output uses the word `Season`.
///
/// ```
/// # use jumbie_shared::formatting::{fmt_season, LabelStyle};
/// assert_eq!(fmt_season(1, LabelStyle::Short), "S01");
/// assert_eq!(fmt_season(1, LabelStyle::Human), "Season 01");
/// ```
pub fn fmt_season(season: i32, style: LabelStyle) -> String {
    match style {
        LabelStyle::Short => format!("S{:02}", season),
        LabelStyle::Human => format!("Season {:02}", season),
    }
}

/// Format a season and episode pair, zero-padded to 2 digits each.
///
/// When `style` is [`LabelStyle::Short`] (the default) the output uses compact
/// codes — e.g. `S01E05`.
/// When `style` is [`LabelStyle::Human`] the output uses full words.
///
/// When `episode_end` is `Some(end)` and `end > episode`, a range is produced:
/// `S01E01-E03` (Short) or `Season 01 Episodes 01-03` (Human).
///
/// ```
/// # use jumbie_shared::formatting::{fmt_season_episode, LabelStyle};
/// assert_eq!(fmt_season_episode(1, 5, None, LabelStyle::Short), "S01E05");
/// assert_eq!(fmt_season_episode(1, 5, None, LabelStyle::Human), "Season 01 Episode 05");
/// assert_eq!(fmt_season_episode(1, 1, Some(3), LabelStyle::Short), "S01E01-E03");
/// assert_eq!(fmt_season_episode(1, 1, Some(12), LabelStyle::Human), "Season 01 Episodes 01-12");
/// ```
pub fn fmt_season_episode(
    season: i32,
    episode: i32,
    episode_end: Option<i32>,
    style: LabelStyle,
) -> String {
    match (style, episode_end) {
        (LabelStyle::Short, Some(end)) if end > episode => {
            format!("S{:02}E{:02}-E{:02}", season, episode, end)
        }
        (LabelStyle::Human, Some(end)) if end > episode => {
            format!("Season {:02} Episodes {:02}-{:02}", season, episode, end)
        }
        (LabelStyle::Short, _) => format!("S{:02}E{:02}", season, episode),
        (LabelStyle::Human, _) => format!("Season {:02} Episode {:02}", season, episode),
    }
}

/// Compute the next (season, episode) tuple after applying season-based rollover.
///
/// If `rollover_enabled` and `current_episode` reaches or exceeds the max
/// episode count for `current_season`, increments the season and resets the
/// episode to 1. Returns `(next_season, next_episode, did_roll_over)`.
///
/// SSoT: both the backend (file assignment) and frontend (episode manager modal)
/// call this rather than reimplementing the rollover logic.
pub fn episode_rollover_next(
    current_season: i32,
    current_episode: i32,
    metadata_season_counts: &std::collections::HashMap<i32, i32>,
    rollover_enabled: bool,
) -> (i32, i32, bool) {
    if rollover_enabled
        && let Some(&max_ep) = metadata_season_counts.get(&current_season)
        && current_episode >= max_ep
    {
        let next_season = current_season + 1;
        if metadata_season_counts.contains_key(&next_season) {
            return (next_season, 1, true);
        }
    }
    (current_season, current_episode + 1, false)
}

/// Convert a sorted list of episode numbers into contiguous ranges.
///
/// SSoT: `file_manager.rs` (`format_episode_range`) and `db/episodes.rs` build
/// contiguous ranges from episode lists; they must call this rather than
/// reimplementing the loop.
///
/// # Example
///
/// ```
/// # use jumbie_shared::formatting::consecutive_to_ranges;
/// let eps = vec![1, 2, 3, 5, 7, 8];
/// assert_eq!(consecutive_to_ranges(&eps), vec![(1, 3), (5, 5), (7, 8)]);
/// ```
pub fn consecutive_to_ranges(episodes: &[i32]) -> Vec<(i32, i32)> {
    if episodes.is_empty() {
        return Vec::new();
    }
    let mut ranges: Vec<(i32, i32)> = Vec::new();
    let mut start = episodes[0];
    let mut end = episodes[0];
    for &ep in &episodes[1..] {
        if ep == end + 1 {
            end = ep;
        } else {
            ranges.push((start, end));
            start = ep;
            end = ep;
        }
    }
    ranges.push((start, end));
    ranges
}

/// Normal-mode episode ID built from an **already-resolved** season number.
///
/// Infallible: unlike [`fmt_episode_id`] there is no label to misread and no
/// season substitution. Callers holding a season number should use this directly.
pub fn fmt_episode_id_num(season_num: i32, episode: i32, series_id: &str) -> String {
    debug_assert!(!series_id.is_empty(), "{}", EMPTY_SERIES_ID_MSG);
    format!("{}_S{:02}E{:02}", series_id, season_num, episode)
}

/// Absolute-mode episode ID.
///
/// Absolute numbering carries **no season** — explicit in the ID shape, which is
/// why this is a separate constructor rather than an invented season parameter.
pub fn fmt_absolute_episode_id(episode: i32, series_id: &str) -> String {
    debug_assert!(!series_id.is_empty(), "{}", EMPTY_SERIES_ID_MSG);
    format!("{}_ABS{:04}", series_id, episode)
}

/// Inverse of the season component of [`fmt_episode_id`].
///
/// * normal ID (`{series}_S{ss}E{ee}`) → `Some(ss)`
/// * absolute ID (`{series}_ABS{eeee}`) → `Some(ABSOLUTE_SEASON_NUM)` — the ID
///   carries no season, and the absolute space is canonically season 1
/// * malformed → `None`
///
/// Splits from the **right**: series ids are opaque and legacy ones contain
/// underscores, so a left split could pick up a fragment of the series id.
///
/// ```
/// # use jumbie_shared::formatting::parse_season_from_episode_id;
/// assert_eq!(parse_season_from_episode_id("abc_S02E05"), Some(2));
/// assert_eq!(parse_season_from_episode_id("my_show_S10E03"), Some(10));
/// assert_eq!(parse_season_from_episode_id("abc_ABS1074"), Some(1));
/// assert_eq!(parse_season_from_episode_id("nonsense"), None);
/// ```
pub fn parse_season_from_episode_id(episode_id: &str) -> Option<i32> {
    let (_, suffix) = episode_id.rsplit_once('_')?;
    if let Some(stripped) = suffix.strip_prefix('S') {
        stripped
            .split('E')
            .next()
            .and_then(|s| s.parse::<i32>().ok())
    } else if suffix.starts_with("ABS") {
        Some(crate::mapping::ABSOLUTE_SEASON_NUM)
    } else {
        None
    }
}

/// GUARD: An empty series_id produces identical episode IDs across ALL series
/// (e.g. `_S01E01`), causing catastrophic data corruption when the result is
/// used as an SQLite PRIMARY KEY. This must NEVER happen — call
/// `MappingRule::ensure_series_id()` before constructing episode IDs.
const EMPTY_SERIES_ID_MSG: &str = "episode ID built with an empty series_id! This collides across ALL series. Call MappingRule::ensure_series_id() first.";

/// Build a stable, globally-unique episode identifier from a season **label**.
///
/// Selected by the `absolute` flag:
///
/// | `absolute` | Format | Example |
/// |------------|--------|---------|
/// | `false`    | `{uuid}_S{season:02}E{episode:02}` | `abc_S01E05` |
/// | `true`     | `{uuid}_ABS{episode:04}` | `abc_ABS0003` |
///
/// The `ABS` prefix guarantees zero collisions between the modes; 4-digit padding
/// ensures correct lexicographic sorting for SQLite TEXT keys. In absolute mode
/// `season` is ignored and may be empty or non-numeric.
///
/// # Errors
/// In normal mode a `season` label that is not a number is an error — the
/// codebase previously substituted season 0/1, which generated IDs (and rows)
/// under a season the caller never asked for. Callers must reject or skip the
/// invalid input instead.
///
/// ```
/// # use jumbie_shared::formatting::fmt_episode_id;
/// assert_eq!(fmt_episode_id("1", 5, "abc", false).unwrap(), "abc_S01E05");
/// assert_eq!(fmt_episode_id("S02", 5, "abc", false).unwrap(), "abc_S02E05");
/// assert_eq!(fmt_episode_id("", 1074, "abc", true).unwrap(), "abc_ABS1074");
/// assert!(fmt_episode_id("SP", 5, "abc", false).is_err());
/// ```
pub fn fmt_episode_id(
    season: &str,
    episode: i32,
    series_id: &str,
    absolute: bool,
) -> Result<String, crate::mapping::SeasonParseError> {
    if absolute {
        // Explicit: absolute IDs have no season component.
        return Ok(fmt_absolute_episode_id(episode, series_id));
    }
    let Some(season_num) = crate::mapping::parse_season_num(season) else {
        return Err(crate::mapping::SeasonParseError::new(season));
    };
    Ok(fmt_episode_id_num(season_num, episode, series_id))
}

use crate::types::EpisodeStatus;

/// Derive whether an episode is "missing" (release datetime has passed) or
/// "unreleased" (in the future / unknown).
///
/// SSoT: every caller (backend series detail builder, wanted query, frontend
/// calendar and season accordion) calls this rather than reimplementing the
/// comparison.
pub fn derive_episode_status(
    effective_date: Option<chrono::NaiveDateTime>,
    now: chrono::NaiveDateTime,
) -> EpisodeStatus {
    match effective_date {
        Some(pd) if pd <= now => EpisodeStatus::Missing,
        _ => EpisodeStatus::Unreleased,
    }
}

/// Normalize a file name for the blocked-file list (SSoT — shared by the caller
/// that records a block and every caller that tests one).
///
/// Minimal normalization: **lowercase** (ASCII), **trim** leading/trailing
/// whitespace, and **collapse runs of spaces** to a single space.
///
/// Deliberately *not* fuzzy: a single differing character yields a different
/// normalized name, so renaming a file to claim a different episode unblocks it.
/// Only ASCII spaces are collapsed — tabs/newlines are left as-is.
pub fn normalize_file_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_space = false;
    for ch in name.trim().chars() {
        if ch == ' ' {
            if prev_space {
                continue;
            }
            prev_space = true;
        } else {
            prev_space = false;
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// Human ("natural") string comparison: runs of ASCII digits compare by numeric
/// value, so `E2` sorts before `E10`; everything else compares case-insensitively.
/// Used to order file rows by name.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    natural_cmp_inner(a, b, false)
}

/// Like [`natural_cmp`], but non-digit characters compare **case-sensitively**
/// (so `Apple` sorts before `apple`). Digit runs still compare numerically. Used
/// as a deterministic, case-sensitive tie-break (e.g. ordering file rows with
/// equal extensions).
pub fn natural_cmp_case_sensitive(a: &str, b: &str) -> std::cmp::Ordering {
    natural_cmp_inner(a, b, true)
}

fn natural_cmp_inner(a: &str, b: &str, case_sensitive: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let mut run_a = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    run_a.push(c);
                    ai.next();
                }
                let mut run_b = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    run_b.push(c);
                    bi.next();
                }
                let trim_a = run_a.trim_start_matches('0');
                let trim_b = run_b.trim_start_matches('0');
                let ord = trim_a
                    .len()
                    .cmp(&trim_b.len())
                    .then_with(|| trim_a.cmp(trim_b))
                    .then_with(|| run_a.len().cmp(&run_b.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(ca), Some(cb)) => {
                let ord = if case_sensitive {
                    ca.cmp(&cb)
                } else {
                    ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase())
                };
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_cmp_orders_digit_runs_numerically() {
        use std::cmp::Ordering;
        assert_eq!(natural_cmp("E2", "E10"), Ordering::Less);
        assert_eq!(natural_cmp("E10", "E2"), Ordering::Greater);
        assert_eq!(natural_cmp("E02", "E2"), Ordering::Greater);
        assert_eq!(
            natural_cmp("Show - S01E01", "show - s01e01"),
            Ordering::Equal
        );
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_case_sensitive_orders_by_case() {
        use std::cmp::Ordering;
        // Case-insensitive: equal. Case-sensitive: uppercase sorts first (ASCII).
        assert_eq!(natural_cmp("Apple", "apple"), Ordering::Equal);
        assert_eq!(natural_cmp_case_sensitive("Apple", "apple"), Ordering::Less);
        assert_eq!(
            natural_cmp_case_sensitive("apple", "Apple"),
            Ordering::Greater
        );
        // Digit runs still compare numerically (case-sensitivity only affects the
        // non-digit branch).
        assert_eq!(natural_cmp_case_sensitive("E2", "E10"), Ordering::Less);
        assert_eq!(natural_cmp_case_sensitive("a", "ab"), Ordering::Less);
    }

    // render_search_key

    #[test]
    fn render_search_key_pads_and_substitutes() {
        assert_eq!(
            render_search_key("S${season:02}E${episode:02}", 1, 5),
            "S01E05"
        );
        assert_eq!(render_search_key("E${episode}", 3, 1168), "E1168");
        assert_eq!(render_search_key("", 1, 5), "");
        assert_eq!(render_search_key("S${season:02} ${episode}", 2, 7), "S02 7");
    }

    #[test]
    fn hail_mary_matches_ordered_bare_numbers() {
        assert!(contains_ordered_season_episode("Show 2 - 45 1080p", 2, 45));
        assert!(contains_ordered_season_episode("Show 2.45", 2, 45));
        assert!(contains_ordered_season_episode("Show 2-45", 2, 45));
        assert!(contains_ordered_season_episode(
            "[Group] Show 2 45 [1080p]",
            2,
            45
        ));
    }

    #[test]
    fn hail_mary_is_padding_insensitive() {
        assert!(contains_ordered_season_episode("Show 02 - 045", 2, 45));
        assert!(contains_ordered_season_episode("Show 2 - 45", 2, 45));
        assert!(!contains_ordered_season_episode("Show 020 - 045", 2, 45));
    }

    #[test]
    fn hail_mary_requires_both_numbers_in_order() {
        // Only the season, or only the episode, is not enough.
        assert!(!contains_ordered_season_episode("Show 2 1080p", 2, 45));
        assert!(!contains_ordered_season_episode("Show 45 1080p", 2, 45));
        // Reversed order does not count.
        assert!(!contains_ordered_season_episode("Show 45 - 2", 2, 45));
    }

    #[test]
    fn hail_mary_ignores_numbers_glued_to_alphanumerics() {
        // `1080p`, `x265`, `S02` (season marker handled by the real parser) and
        // `2x45` are not standalone tokens.
        assert!(!contains_ordered_season_episode("Show 1080p 245", 2, 45));
        assert!(!contains_ordered_season_episode("Show S02 45", 2, 45));
        assert!(!contains_ordered_season_episode("Show 2x45", 2, 45));
        // `x265` is glued to `x`, so it is not a candidate season.
        assert!(!contains_ordered_season_episode("Show x265 2 45", 265, 45));
    }

    #[test]
    fn hail_mary_requires_two_occurrences_for_equal_values() {
        assert!(!contains_ordered_season_episode("Show 2", 2, 2));
        assert!(contains_ordered_season_episode("Show 2 2", 2, 2));
    }

    // fmt_season_episode
    #[test]
    fn test_fmt_season_episode_single_short() {
        assert_eq!(fmt_season_episode(1, 5, None, LabelStyle::Short), "S01E05");
    }

    #[test]
    fn test_fmt_season_episode_single_human() {
        assert_eq!(
            fmt_season_episode(1, 5, None, LabelStyle::Human),
            "Season 01 Episode 05"
        );
    }

    #[test]
    fn test_fmt_season_episode_range_short() {
        assert_eq!(
            fmt_season_episode(1, 1, Some(3), LabelStyle::Short),
            "S01E01-E03"
        );
    }

    #[test]
    fn test_fmt_season_episode_range_human() {
        assert_eq!(
            fmt_season_episode(2, 5, Some(8), LabelStyle::Human),
            "Season 02 Episodes 05-08"
        );
    }

    #[test]
    fn test_fmt_season_episode_single_episode_end_equal_episode() {
        // When episode_end == episode, no range (not a multi-episode)
        assert_eq!(
            fmt_season_episode(3, 10, Some(10), LabelStyle::Short),
            "S03E10"
        );
    }

    #[test]
    fn test_fmt_season_episode_single_episode_end_less_than_episode() {
        // When episode_end < episode, no range (invalid data guard)
        assert_eq!(
            fmt_season_episode(1, 5, Some(3), LabelStyle::Short),
            "S01E05"
        );
    }

    #[test]
    fn test_fmt_season_episode_high_season() {
        assert_eq!(
            fmt_season_episode(12, 1, Some(24), LabelStyle::Short),
            "S12E01-E24"
        );
    }

    #[test]
    fn test_fmt_season_episode_roundtrip_single() {
        // Verify that a parseable single episode formats consistently
        let formatted = fmt_season_episode(2, 3, None, LabelStyle::Short);
        assert_eq!(formatted, "S02E03");
        assert_eq!(formatted.len(), 6);
    }

    #[test]
    fn test_fmt_season_episode_roundtrip_range() {
        // Verify that a range formats consistently
        let formatted = fmt_season_episode(1, 1, Some(12), LabelStyle::Short);
        assert_eq!(formatted, "S01E01-E12");
        assert_eq!(formatted.len(), 10);
    }

    // fmt_episode_id / fmt_episode_id_num

    #[test]
    fn test_fmt_episode_id_parses_all_stored_forms() {
        for label in ["2", "02", "S02", "s02", " S02 "] {
            assert_eq!(
                fmt_episode_id(label, 3, "sid", false).unwrap(),
                "sid_S02E03",
                "label {label:?}"
            );
        }
    }

    #[test]
    fn test_fmt_episode_id_rejects_non_numeric_season_in_normal_mode() {
        // No silent substitution: a non-numeric label is an error, never a
        // coerced season 0/1.
        for label in ["SP", "Specials", ""] {
            assert!(
                fmt_episode_id(label, 3, "sid", false).is_err(),
                "label {label:?} must not be coerced to a season"
            );
        }
    }

    #[test]
    fn test_fmt_episode_id_absolute_mode_ignores_season_label() {
        // Explicit ABS handling: absolute numbering has no season, so any
        // label — including a non-numeric one — is accepted and unused.
        for label in ["1", "SP", "Specials", ""] {
            assert_eq!(
                fmt_episode_id(label, 7, "sid", true).unwrap(),
                "sid_ABS0007",
                "label {label:?}"
            );
        }
    }

    #[test]
    fn test_fmt_episode_id_num_is_infallible() {
        assert_eq!(fmt_episode_id_num(2, 3, "sid"), "sid_S02E03");
        assert_eq!(fmt_absolute_episode_id(7, "sid"), "sid_ABS0007");
    }

    // build_or_chain_search_query is gone; per-episode queries live in the plugin SDK.

    // normalize_file_name

    #[test]
    fn test_normalize_file_name_lowercases() {
        assert_eq!(
            normalize_file_name("My.Show.S01E01.MKV"),
            "my.show.s01e01.mkv"
        );
    }

    #[test]
    fn test_normalize_file_name_trims() {
        assert_eq!(normalize_file_name("  show.mkv  "), "show.mkv");
    }

    #[test]
    fn test_normalize_file_name_collapses_spaces() {
        assert_eq!(
            normalize_file_name("My   Show   S01E01.mkv"),
            "my show s01e01.mkv"
        );
    }

    #[test]
    fn test_normalize_file_name_combined() {
        assert_eq!(normalize_file_name("  My   SHOW.mkv "), "my show.mkv");
    }

    #[test]
    fn test_normalize_file_name_idempotent() {
        let once = normalize_file_name("  My   Show.MKV ");
        assert_eq!(normalize_file_name(&once), once);
    }

    #[test]
    fn test_normalize_file_name_single_char_differs() {
        // A one-character change must produce a different key (rename unblocks).
        assert_ne!(
            normalize_file_name("Show.S01E05.mkv"),
            normalize_file_name("Show.S01E06.mkv")
        );
    }

    #[test]
    fn test_normalize_file_name_empty_and_whitespace_only() {
        assert_eq!(normalize_file_name(""), "");
        assert_eq!(normalize_file_name("   "), "");
    }
}
