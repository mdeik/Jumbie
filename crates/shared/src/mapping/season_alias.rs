//! Season-alias resolution.
//!
//! A season alias is an alternate *title* for a season (e.g. a specials season
//! named "Whisker Mini"). When a release or an on-disk file is titled with a
//! season alias, that alias is a stronger signal of the season than a bare
//! season number, so it is favoured.
//!
//! Matching uses exactly the normalization already used for release/scan
//! recognition (`lowercase`, dots→spaces, plus the `clean_title` form — the same
//! comparison `lookup_mapping_in` performs). There is no fuzzy matching.
//!
//! Rules (the SSoT for every caller — release identification and file scanning):
//!
//! * No season alias matches → the caller keeps its normal season logic
//!   ([`SeasonAliasDecision::NoSignal`]).
//! * Season aliases match, and the longest (most specific) match belongs to a
//!   single season → use that season and ignore any parsed season number
//!   ([`SeasonAliasDecision::UseSeason`]). A co-matching **series** alias does
//!   not downgrade this — the season is still favoured.
//! * The longest matches tie across two or more seasons → fall back to the
//!   parsed season number, but only if it names one of the matched seasons;
//!   otherwise the file/release is [`SeasonAliasDecision::Unneeded`].

use super::matching::{matches_normalized, normalize_cleaned_for_match, normalize_for_match};
use super::types::{SeriesSettings, parse_season_num};

/// Outcome of season-alias matching for a title/key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeasonAliasDecision {
    /// A season alias decided the season — use it and ignore the parsed number.
    UseSeason(i32),
    /// Conflicting season aliases that the parsed season number cannot resolve;
    /// the caller must treat the file/release as unneeded.
    Unneeded,
    /// No season alias matched — the caller keeps its normal season logic.
    NoSignal,
}

/// Resolve a season from season-alias matches in `text`.
///
/// `text` is a release title or a filename-derived series key. `absolute` is the
/// effective numbering mode (selects the `season` vs `season_absolute` override
/// map). `parsed_season` is the season parsed from the title/filename, used only
/// to break a tie between seasons.
pub fn resolve_season_from_aliases(
    text: &str,
    settings: &SeriesSettings,
    absolute: bool,
    parsed_season: Option<i32>,
) -> SeasonAliasDecision {
    let candidate_raw = normalize_for_match(text);
    if candidate_raw.is_empty() {
        return SeasonAliasDecision::NoSignal;
    }
    let candidate_clean = normalize_cleaned_for_match(text);

    let overrides = settings.season_for_active_mode(absolute);

    // Longest matching alias per season (most specific match wins).
    let mut best_len_by_season: std::collections::BTreeMap<i32, usize> =
        std::collections::BTreeMap::new();

    for (season_key, rule) in overrides {
        let Some(season) = parse_season_num(season_key) else {
            continue;
        };
        let mut best = 0usize;
        for alias in rule.search_aliases() {
            let alias = alias.trim();
            if alias.is_empty() {
                continue;
            }
            if matches_normalized(&candidate_raw, &candidate_clean, alias) {
                best = best.max(alias.chars().count());
            }
        }
        if best > 0 {
            best_len_by_season.insert(season, best);
        }
    }

    if best_len_by_season.is_empty() {
        return SeasonAliasDecision::NoSignal;
    }

    let max_len = *best_len_by_season.values().max().unwrap();
    let best_seasons: Vec<i32> = best_len_by_season
        .iter()
        .filter(|(_, len)| **len == max_len)
        .map(|(season, _)| *season)
        .collect();

    // The most specific match belongs to exactly one season → decisive. A
    // matching series alias is intentionally ignored: the season is favoured.
    if best_seasons.len() == 1 {
        return SeasonAliasDecision::UseSeason(best_seasons[0]);
    }

    // Tie across seasons: fall back to the parsed season number only when it
    // names one of the matched seasons; otherwise the file/release is unneeded.
    let matched_seasons: Vec<i32> = best_len_by_season.keys().copied().collect();
    match parsed_season {
        Some(p) if matched_seasons.contains(&p) => SeasonAliasDecision::UseSeason(p),
        _ => SeasonAliasDecision::Unneeded,
    }
}

crate::test_module! {
    use crate::mapping::types::{SeasonOverride, SeriesSettings};

    fn override_with_aliases(season: &str, aliases: &[&str]) -> SeasonOverride {
        SeasonOverride {
            season: season.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn settings(series_aliases: &[&str], season_aliases: &[(&str, &[&str])]) -> SeriesSettings {
        let mut season = std::collections::HashMap::new();
        for (key, aliases) in season_aliases {
            season.insert(key.to_string(), override_with_aliases(key, aliases));
        }
        SeriesSettings {
            aliases: series_aliases.iter().map(|s| s.to_string()).collect(),
            season,
            ..Default::default()
        }
    }

    #[test]
    fn unique_season_alias_ignores_season_number() {
        let s = settings(&[], &[("0", &["Whisker Mini"])]);
        assert_eq!(
            resolve_season_from_aliases("[G] Whisker Mini Anime - 01-13", &s, false, Some(1)),
            SeasonAliasDecision::UseSeason(0)
        );
    }

    #[test]
    fn more_specific_alias_wins() {
        // "Whisker Mini" is longer than "Whisker" → season 3.
        let s = settings(&[], &[("0", &["Whisker"]), ("3", &["Whisker Mini"])]);
        assert_eq!(
            resolve_season_from_aliases("Whisker Mini Anime", &s, false, None),
            SeasonAliasDecision::UseSeason(3)
        );
    }

    #[test]
    fn series_alias_does_not_downgrade_season_alias() {
        // Season alias + series alias → favour the season.
        let s = settings(&["Whisker Mini"], &[("0", &["Whisker Mini"])]);
        assert_eq!(
            resolve_season_from_aliases("Whisker Mini Anime", &s, false, Some(1)),
            SeasonAliasDecision::UseSeason(0)
        );
    }

    #[test]
    fn season_alias_tie_resolved_by_parsed_season() {
        // Two seasons share the same alias → tie → parsed season decides.
        let s = settings(&[], &[("0", &["Whisker Mini"]), ("2", &["Whisker Mini"])]);
        assert_eq!(
            resolve_season_from_aliases("Whisker Mini Anime", &s, false, Some(2)),
            SeasonAliasDecision::UseSeason(2)
        );
    }

    #[test]
    fn season_alias_tie_without_matching_number_is_unneeded() {
        let s = settings(&[], &[("0", &["Whisker Mini"]), ("2", &["Whisker Mini"])]);
        // No parsed season …
        assert_eq!(
            resolve_season_from_aliases("Whisker Mini Anime", &s, false, None),
            SeasonAliasDecision::Unneeded
        );
        // … or a parsed season that matches neither candidate.
        assert_eq!(
            resolve_season_from_aliases("Whisker Mini Anime", &s, false, Some(5)),
            SeasonAliasDecision::Unneeded
        );
    }

    #[test]
    fn no_alias_signal_returns_no_signal() {
        let s = settings(&[], &[("0", &["Whisker Mini"])]);
        assert_eq!(
            resolve_season_from_aliases("Charcoal Tabby - 03", &s, false, Some(3)),
            SeasonAliasDecision::NoSignal
        );
    }

    #[test]
    fn absolute_mode_uses_absolute_overrides() {
        let mut settings = SeriesSettings::default();
        settings
            .season_absolute
            .insert("1".to_string(), override_with_aliases("1", &["Absolute Alias"]));
        assert_eq!(
            resolve_season_from_aliases("Absolute Alias - 05", &settings, false, None),
            SeasonAliasDecision::NoSignal
        );
        assert_eq!(
            resolve_season_from_aliases("Absolute Alias - 05", &settings, true, None),
            SeasonAliasDecision::UseSeason(1)
        );
    }

    #[test]
    fn normalization_is_dot_and_case_insensitive() {
        let s = settings(&[], &[("2", &["My.Show.Name"])]);
        assert_eq!(
            resolve_season_from_aliases("my show name - 05", &s, false, None),
            SeasonAliasDecision::UseSeason(2)
        );
    }
}
