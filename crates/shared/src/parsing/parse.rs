use super::extract::*;
use crate::mapping::{DEFAULT_SEASON_NUM, EpisodeInfo};
use crate::patterns::*;

/// Controls which filename-parsing heuristics are active.
///
/// # FileScan
/// Used when scanning local files on disk. A single video file is always an episode,
/// never a season pack — so season pack patterns are skipped. Lone-number fallback is
/// active to catch bare-numbered files (e.g. `Show.Name.1.mkv`).
///
/// # Search
/// Used when parsing search/download result titles. Torrent/NZB titles can be season
/// packs (e.g. `Show.Name.S01.Complete.1080p`), so those patterns are included.
/// Lone-number fallback is skipped — search titles use proper naming conventions.
///
/// Both contexts return `None` when no pattern matches; callers handle `None`
/// gracefully (skip file, return empty results, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseContext {
    FileScan,
    Search,
}

/// Parse a media filename into structured `EpisodeInfo`.
///
/// # Unicode dash normalization
///
/// Different platforms and editors produce different dash characters (en-dash `–`,
/// em-dash `—`, minus sign `−`, hyphen `‑`, non-breaking hyphen `‑`) that are visually
/// similar but lexically distinct; all are normalized to ASCII `-`, preventing parse
/// failures on filenames copied from Word docs, web pages, or non-English keyboards.
///
/// # Dot normalization
///
/// Scene releases commonly use dots as word separators (e.g. `Show.Name.S01E01.mkv`).
/// Filenames with dots but no spaces are converted to spaces so the title is extracted
/// as `"Show Name"` rather than `"Show.Name"`. Filenames already containing spaces are
/// left untouched.
pub fn parse_filename(filename: &str, context: ParseContext) -> Option<EpisodeInfo> {
    parse_filename_inner(filename, context, None)
}

/// Parse a title that must belong to `season_num`.
///
/// Identical to [`parse_filename`] except that a title which does not declare
/// `season_num` is rejected (returns `None`). Two cases are dropped:
///
/// * **No season declared** — the `DEFAULT_SEASON_NUM` fallback is disabled, so
///   a season-less title is never silently treated as season 1.
/// * **A different season declared** — the title is unrelated to the search.
///
/// Used by search paths to match a result against the season being searched for.
/// RSS feeds (series-scoped) and local file scans keep using [`parse_filename`],
/// which still allows a missing season.
pub fn parse_filename_for_season(
    filename: &str,
    context: ParseContext,
    season_num: i32,
) -> Option<EpisodeInfo> {
    parse_filename_inner(filename, context, Some(season_num))
}

fn parse_filename_inner(
    filename: &str,
    context: ParseContext,
    expected_season: Option<i32>,
) -> Option<EpisodeInfo> {
    let normalized = filename.replace(
        ['\u{2013}', '\u{2014}', '\u{2212}', '\u{2010}', '\u{2011}'],
        "-",
    );
    let trimmed = normalized.trim();

    // If the stem has dots but no spaces, treat it as scene-style dot-separated
    // naming and convert dots to spaces so patterns match the series name. The
    // extension is preserved and re-appended so patterns that look for it still work.
    let dot_normalized: String = {
        let stem = std::path::Path::new(trimmed)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(trimmed);
        let ext = std::path::Path::new(trimmed)
            .extension()
            .and_then(|e| e.to_str());
        let normalized_stem = if stem.contains('.') && !stem.contains(' ') {
            stem.replace('.', " ")
        } else {
            stem.to_string()
        };
        if let Some(e) = ext {
            format!("{}.{}", normalized_stem, e)
        } else {
            normalized_stem
        }
    };
    // Strip quality markers before matching so resolution numbers aren't captured as
    // episode numbers by `\s+S(\d+)\s+(\d+)` patterns (e.g. "S01 1080p" → 1080).
    let quality_stripped = CLEAN_QUALITY.replace_all(&dot_normalized, " ").to_string();
    let trimmed = quality_stripped.as_str();

    // OP/ED file filter (file-scan context only): anime releases bundle opening/ending
    // theme files ("NCOP", "NCED", "OP1") that are not episodes and must be skipped.
    // FileScan only, because search titles can legitimately contain "OP"/"ED".
    if matches!(context, ParseContext::FileScan) {
        let stem = std::path::Path::new(trimmed)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(trimmed);
        let cleaned_stem = clean_title(stem);
        if OP_ED_PATTERN.is_match(&cleaned_stem) {
            // Caller (e.g. scanner) will log this as an unparseable file.
            return None;
        }
    }

    let part_number = detect_part_number(trimmed);

    // Shared EpisodeInfo construction so the pattern-matching blocks (ranges,
    // singles, season packs) don't duplicate the struct-with-derivation logic.
    // Decimal detection is done once here and threaded through so all parse paths
    // consistently report decimal episodes.
    let has_decimal_episode = crate::patterns::DECIMAL_EPISODE_PATTERN.is_match(filename);

    // Season policy for a strict parse: when `expected_season` is set, a title
    // must declare that season. A title that declares none (which the default
    // parse would fall back to `DEFAULT_SEASON_NUM`) or a different season is
    // rejected, so an unrelated search result never matches.
    let season_ok = |season: i32| expected_season.is_none_or(|exp| exp == season);
    let seasons_ok = |seasons: &[i32]| expected_season.is_none_or(|exp| seasons.contains(&exp));

    let build_info = |title_part: &str,
                      seasons: Vec<i32>,
                      episodes: Vec<i32>,
                      is_season_pack: bool,
                      is_complete_pack: bool|
     -> EpisodeInfo {
        // Strip trailing descriptive noise from season pack titles only when fronted
        // by ` - Complete` (conservative — leaves standalone "Complete" untouched).
        let refined = if is_season_pack || is_complete_pack {
            CLEAN_TRAILING_DESCRIPTORS
                .replace(title_part, "")
                .to_string()
        } else {
            title_part.to_string()
        };
        let series_key = clean_title(&refined);
        EpisodeInfo {
            raw_title: filename.to_string(),
            series_key: if series_key.is_empty() {
                refined
            } else {
                series_key
            },
            seasons,
            episodes,
            file_ext: std::path::Path::new(filename)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("mkv")
                .to_lowercase(),
            submitter: extract_submitter(filename),
            resolution: None,
            version: extract_version(filename),
            part_number,
            is_season_pack,
            is_complete_pack,
            has_decimal_episode,
        }
    };

    // Concatenated multi-episode runs with no separator ("Show.S01E01E02"): a season
    // plus directly appended episode markers. Parsed as an explicit episode list (not
    // a start..=end range) so a non-contiguous run is preserved. None of the
    // separator-based `RANGE_PATTERNS` match this form, so without this a file like
    // `S01E01E02` would be read as its first episode only.
    for re in CONCAT_EPISODE_PATTERNS.iter() {
        if let Some(caps) = re.captures(trimmed) {
            let title_part = caps.get(1).map_or("", |m| m.as_str().trim());
            let season = caps.get(2).and_then(|m| m.as_str().parse::<i32>().ok());
            let mut episodes: Vec<i32> = Vec::new();
            if let Some(run) = caps.get(3) {
                for m in EP_IN_TAIL.find_iter(run.as_str()) {
                    if let Ok(n) = m.as_str()[1..].parse::<i32>()
                        && !episodes.contains(&n)
                    {
                        episodes.push(n);
                    }
                }
            }
            if let Some(season) = season
                && season_ok(season)
                && episodes.len() > 1
            {
                return Some(build_info(title_part, vec![season], episodes, false, false));
            }
        }
    }

    // Range patterns first so multi-episode releases are matched before single
    // patterns. Absolute-numbered ranges (indices 10-12) apply a date guard: a
    // range that looks like MM-DD (1-12 / 1-31) is likely a date and is skipped.
    for (idx, re) in RANGE_PATTERNS.iter().enumerate() {
        if let Some(caps) = re.captures(trimmed) {
            let title_part = caps.get(1).map_or("", |m| m.as_str().trim());
            let (season_opt, ep_start_str, ep_end_str) = if idx <= 9 {
                // First 10 patterns include season number
                (
                    caps.get(2).and_then(|m| m.as_str().parse().ok()),
                    caps.get(3).map_or("0", |m| m.as_str()),
                    caps.get(4).map_or("0", |m| m.as_str()),
                )
            } else {
                // Last 3 patterns are absolute-numbered ranges (no season)
                (
                    None,
                    caps.get(2).map_or("0", |m| m.as_str()),
                    caps.get(3).map_or("0", |m| m.as_str()),
                )
            };
            if let (Ok(ep_start), Ok(ep_end)) =
                (ep_start_str.parse::<i32>(), ep_end_str.parse::<i32>())
                && ep_end > ep_start
            {
                // Date guard for absolute-numbered ranges: a month-like start (1-12),
                // day-like end (1-31) and a preceding digit suggest a YYYY-MM-DD date.
                if idx > 9
                    && ep_start <= 12
                    && ep_end <= 31
                    && let Some(m) = caps.get(2)
                    && m.start() >= 2
                    && trimmed.as_bytes()[m.start() - 2].is_ascii_digit()
                {
                    continue;
                }
                let season = match season_opt {
                    Some(s) if !season_ok(s) => continue,
                    Some(s) => vec![s],
                    // No season captured — submitter convention (season 1),
                    // unless a specific season is required.
                    None if expected_season.is_some() => continue,
                    None => vec![DEFAULT_SEASON_NUM],
                };
                return Some(build_info(
                    title_part,
                    season,
                    (ep_start..=ep_end).collect(),
                    false,
                    false,
                ));
            }
        }
    }

    // Spelled-out "Season N Episode M" pairs (e.g. "Show Season 3 Episode 4",
    // "Show Season 3, Episode 4", "Show Season 3 - Episode 4"). Must run before
    // the episode-list and single patterns, whose generic `Episode N` handling
    // would otherwise read the pair as a season-less episode (season 1).
    for re in SEASON_EPISODE_PATTERNS.iter() {
        if let Some(caps) = re.captures(trimmed) {
            let title_part = caps.get(1).map_or("", |m| m.as_str().trim());
            let season = caps.get(2).and_then(|m| m.as_str().parse::<i32>().ok());
            let episode = caps.get(3).and_then(|m| m.as_str().parse::<i32>().ok());
            if let (Some(season), Some(episode)) = (season, episode) {
                if !season_ok(season) {
                    continue;
                }
                return Some(build_info(
                    title_part,
                    vec![season],
                    vec![episode],
                    false,
                    false,
                ));
            }
        }
    }

    // Season-range patterns (search context only): "S01-S02" / "Season 1-2" must be
    // matched before single patterns, otherwise the dash pattern reads it as
    // title="... Season 1", episode=2.
    if matches!(context, ParseContext::Search) {
        for (pi, re) in SEASON_RANGE_PATTERNS.iter().enumerate() {
            if let Some(caps) = re.captures(trimmed) {
                let title_part = caps.get(1).map_or("", |m| m.as_str().trim());
                let s1_str = caps.get(2).map_or("", |m| m.as_str());
                let s2_str = caps.get(3).map_or("", |m| m.as_str());
                if let (Ok(s1), Ok(s2)) = (s1_str.parse::<i32>(), s2_str.parse::<i32>())
                    && s2 > s1
                {
                    let is_complete = title_part.to_lowercase().contains("complete")
                        || trimmed.to_lowercase().contains("complete");

                    // For comma- (idx 3), space- (idx 5) and spelled-out comma-list
                    // (idx 6) patterns, parse the tail group for seasons beyond the first two.
                    let mut all_seasons = Vec::new();
                    if pi == 3 || pi == 5 {
                        // Tail like ", S03, S04" or " S03 S04" — S-prefixed markers (group 4)
                        if let Some(tail) = caps.get(4).map(|m| m.as_str()) {
                            all_seasons.push(s1);
                            all_seasons.push(s2);
                            for m in S_IN_TAIL.find_iter(tail) {
                                if let Ok(s) = m.as_str()[1..].parse::<i32>()
                                    && !all_seasons.contains(&s)
                                {
                                    all_seasons.push(s);
                                }
                            }
                        }
                    } else if pi == 6 {
                        // "Season 1, 2, 3" — tail (group 4) like ", 3"
                        all_seasons.push(s1);
                        all_seasons.push(s2);
                        if let Some(tail) = caps.get(4).map(|m| m.as_str()) {
                            for part in tail.split(',') {
                                let trimmed = part.trim();
                                if !trimmed.is_empty()
                                    && let Ok(n) = trimmed.parse::<i32>()
                                    && !all_seasons.contains(&n)
                                {
                                    all_seasons.push(n);
                                }
                            }
                        }
                    }

                    let seasons = if all_seasons.is_empty() {
                        (s1..=s2).collect()
                    } else {
                        all_seasons
                    };
                    if !seasons_ok(&seasons) {
                        continue;
                    }
                    return Some(build_info(title_part, seasons, vec![], true, is_complete));
                }
            }
        }
    }

    // Comma-separated episode patterns (search context only): non-contiguous lists
    // like "Title - S01E02, E03, E04" must match before SINGLE_PATTERNS, which would
    // otherwise catch only the first episode.
    if matches!(context, ParseContext::Search) {
        for re in COMMA_EPISODE_PATTERNS.iter() {
            if let Some(caps) = re.captures(trimmed) {
                let title_part = caps.get(1).map_or("", |m| m.as_str().trim());
                let season_opt = caps.get(2).and_then(|m| m.as_str().parse().ok());
                let first_ep_str = caps.get(3).map_or("0", |m| m.as_str());
                if let Ok(first_ep) = first_ep_str.parse::<i32>() {
                    let mut all_eps = vec![first_ep];
                    if let Some(tail) = caps.get(4).map(|m| m.as_str()) {
                        for m in EP_IN_TAIL.find_iter(tail) {
                            if let Ok(n) = m.as_str()[1..].parse::<i32>() {
                                // m.as_str() is like "E03" — skip the 'E'.
                                if !all_eps.contains(&n) {
                                    all_eps.push(n);
                                }
                            }
                        }
                    }
                    let season = match season_opt {
                        Some(s) if !season_ok(s) => continue,
                        Some(s) => s,
                        None if expected_season.is_some() => continue,
                        None => DEFAULT_SEASON_NUM,
                    };
                    return Some(build_info(title_part, vec![season], all_eps, false, false));
                }
            }
        }
    }

    // Spelled-out episode list patterns (search context only): "Episodes 1, 2, 3".
    if matches!(context, ParseContext::Search) {
        for re in SPELLED_OUT_EPISODE_PATTERNS.iter() {
            if let Some(caps) = re.captures(trimmed) {
                let title_part = caps.get(1).map_or("", |m| m.as_str().trim());
                let first_ep_str = caps.get(2).map_or("0", |m| m.as_str());
                if let Ok(first_ep) = first_ep_str.parse::<i32>() {
                    let mut all_eps = vec![first_ep];
                    if let Some(tail) = caps.get(3).map(|m| m.as_str()) {
                        for part in tail.split(',') {
                            let trimmed = part.trim();
                            if !trimmed.is_empty()
                                && let Ok(n) = trimmed.parse::<i32>()
                                && !all_eps.contains(&n)
                            {
                                all_eps.push(n);
                            }
                        }
                    }
                    // The pattern carries no season, so a strict parse cannot
                    // verify that the result belongs to the searched season.
                    if expected_season.is_some() {
                        continue;
                    }
                    return Some(build_info(
                        title_part,
                        vec![DEFAULT_SEASON_NUM],
                        all_eps,
                        false,
                        false,
                    ));
                }
            }
        }
    }

    // Single episode patterns (e.g., "S01E01", " - 01", "Episode 1")
    for (idx, re) in SINGLE_PATTERNS.iter().enumerate() {
        if let Some(caps) = re.captures(trimmed) {
            let title_part = caps.get(1).map_or("", |m| m.as_str().trim());
            let (season_opt, ep_num_str) =
                if idx == 0 || idx == 2 || idx == 3 || idx == 4 || idx == 5 || idx == 6 || idx == 7
                {
                    // Patterns with season: [Group] Title - S01E01, Title - S01E01, Title S01E01, [Group] Title 3 E31, Title 3 E31, [Group] Title S03 122, Title S03 122
                    (
                        caps.get(2).and_then(|m| m.as_str().parse().ok()),
                        caps.get(3).map_or("0", |m| m.as_str()),
                    )
                } else {
                    // Absolute-numbered patterns: [Group] Title - 01, Title - 01, Episode 1
                    (None, caps.get(2).map_or("0", |m| m.as_str()))
                };
            if let Ok(ep_num) = ep_num_str.parse::<i32>() {
                let season = match season_opt {
                    Some(s) if !season_ok(s) => continue,
                    Some(s) => s,
                    None if expected_season.is_some() => continue,
                    None => DEFAULT_SEASON_NUM,
                };
                return Some(build_info(
                    title_part,
                    vec![season],
                    vec![ep_num],
                    false,
                    false,
                ));
            }
        }
    }

    // Season pack patterns (search context only): a torrent/NZB title may cover an
    // entire season (e.g. "Show.Name.S01.Complete.1080p"), unlike a single disk file.
    if matches!(context, ParseContext::Search) {
        for re in SEASON_PACK_PATTERNS.iter() {
            if let Some(caps) = re.captures(trimmed) {
                let title_part = caps.get(1).map_or("", |m| m.as_str().trim());
                // If group 2 is missing, this is the "Complete"-only pattern
                // (no season number captured). Treat as complete series pack —
                // but a strict parse has no season to verify against.
                if caps.get(2).is_none() {
                    if expected_season.is_some() {
                        continue;
                    }
                    return Some(build_info(title_part, vec![], vec![], true, true));
                }
                let season_str = caps.get(2).map_or("", |m| m.as_str());
                if let Ok(season_num) = season_str.parse::<i32>() {
                    if !season_ok(season_num) {
                        continue;
                    }
                    // Check if title also mentions "complete" — still a normal season
                    // pack (covers all episodes of that season), not a full-series pack.
                    let is_complete = title_part.to_lowercase().contains("complete")
                        || trimmed.to_lowercase().contains("complete");
                    return Some(build_info(
                        title_part,
                        vec![season_num],
                        vec![],
                        true,
                        is_complete,
                    ));
                }
            }
        }
    }

    // Lone number fallback (file-scan context only): when no standard pattern matched
    // and the cleaned stem has exactly one numeric group (1-3 digits), use it as the
    // episode. Season defaults to 1 so the scanner's `resolve_season` can override it.
    // File-scan only — search titles use proper conventions, and lone-number matching
    // would pick up surviving resolution/year digits.
    if matches!(context, ParseContext::FileScan) {
        {
            let stem_for_num = std::path::Path::new(trimmed)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(trimmed);
            let cleaned_stem = clean_title(stem_for_num);

            if !cleaned_stem.is_empty() {
                let number_matches: Vec<_> = LONE_NUMBERS.find_iter(&cleaned_stem).collect();

                // A strict parse has no season to verify for a bare number.
                if number_matches.len() == 1 && expected_season.is_none() {
                    let m = number_matches[0];
                    if let Ok(ep_num) = m.as_str().parse::<i32>() {
                        // Build the title part by removing the number from the cleaned stem
                        let before = &cleaned_stem[..m.start()];
                        let after = &cleaned_stem[m.end()..];
                        let title = format!("{}{}", before, after).trim().to_string();

                        // If removing the number leaves nothing, use the original stem
                        let title = if title.is_empty() {
                            cleaned_stem.clone()
                        } else {
                            title
                        };

                        return Some(build_info(
                            &title,
                            vec![DEFAULT_SEASON_NUM],
                            vec![ep_num],
                            false,
                            false,
                        ));
                    }
                }
            }
        }
    }

    // No pattern matched; callers handle `None` gracefully.
    None
}
