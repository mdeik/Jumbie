use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, trace};

use crate::models::media::{MediaEntry, ReleaseCandidate};
use crate::organizer::ContentOrganizer;
use crate::utils::parse_title_with_custom_regex;
use jumbie_shared::formatting::LabelStyle;
use jumbie_shared::mapping::CompiledPatterns;
use jumbie_shared::parsing::{CustomParseResult, match_title_compiled};
use jumbie_shared::types::MappingRule;

/// Whether a release's parsed season matches a season-override key numerically.
///
/// Requires **both** sides to be real numbers: a non-numeric override key (e.g.
/// `"SP"`) only matches via literal comparison, and a release with no parsed
/// season is not implicitly season 1 (season-less releases are handled explicitly
/// by the dedicated branches in `process_entry_inner`).
fn season_key_matches_numerically(release_season: Option<i32>, season_key: &str) -> bool {
    match (
        release_season,
        jumbie_shared::mapping::parse_season_num(season_key),
    ) {
        (Some(release_season), Some(key_season)) => release_season == key_season,
        _ => false,
    }
}

/// Whether one override claims a release, by the non-regex season rules. SSoT for
/// the season rules shared by release identification and the download-side
/// resolver, so the two cannot diverge.
///
/// `parsed_season_str` is the release's season as a string (release matching
/// compares it literally as well as numerically); `parsed_season` is the same
/// value as a number (`None` when the release declares no season).
fn release_season_claimed(
    rule: &jumbie_shared::types::SeasonOverride,
    season_key: &str,
    parsed_season_str: &str,
    parsed_season: Option<i32>,
    info: &jumbie_shared::types::EpisodeInfo,
) -> bool {
    if !rule.contains_source_episode(info.episodes.first().copied().unwrap_or(0)) {
        return false;
    }

    // The numeric alias only means anything for seasons whose episodes are
    // offset-numbered: those are the releases that carry a foreign season number.
    if rule.offset() != 0
        && rule.alias_season_number.is_some_and(|alias_num| {
            let alias_str = alias_num.to_string();
            let alias_fmt =
                jumbie_shared::formatting::fmt_season(alias_num as i32, LabelStyle::Short);
            alias_str == parsed_season_str || alias_fmt == parsed_season_str
        })
    {
        return true;
    }

    if parsed_season_str == season_key || season_key_matches_numerically(parsed_season, season_key)
    {
        return true;
    }

    // A season-less (absolute-numbered) release is claimed by range; a season-level
    // pattern, when present, owns it instead.
    info.seasons.is_empty() && !rule.has_search_patterns()
}

/// Which season a release or downloaded file belongs to, plus the episode offset
/// that maps its source-numbered episodes to local (DB) numbers.
pub(crate) enum ResolvedReleaseSeason<'a> {
    /// Exactly one override claimed the release.
    Season { season_key: &'a str, offset: i32 },
    /// Several overrides claim the release (or season aliases matched without
    /// disambiguating), so it has no single identity and needs manual review.
    Ambiguous,
    /// No override claimed the release; the caller falls back to its local rules
    /// (filename/folder season, or the queue item's season label).
    Unclaimed,
}

/// Resolve the season override for a release or downloaded file.
///
/// This is the download-side season rule (release identification, smart-link, and
/// orphan adoption all call it):
///
/// 1. season alias **titles** decide outright (a season alias beats a co-matching
///    series alias);
/// 2. otherwise every override is tested with [`release_season_claimed`] — exactly
///    one match resolves the season, several match makes it [`Ambiguous`];
///
/// Season-level regex patterns are deliberately not consulted here: they are
/// search gates, not episode identity. Library scanning does not use this at all
/// (locally named files are already in local season/number space).
///
/// [`Ambiguous`]: ResolvedReleaseSeason::Ambiguous
pub(crate) fn resolve_release_season<'a>(
    settings: &'a jumbie_shared::types::SeriesSettings,
    parsed_season: Option<i32>,
    title: &str,
    info: &jumbie_shared::types::EpisodeInfo,
    absolute: bool,
) -> ResolvedReleaseSeason<'a> {
    use jumbie_shared::mapping::SeasonAliasDecision;

    let overrides = settings.season_for_active_mode(absolute);

    match jumbie_shared::mapping::resolve_season_from_aliases(
        title,
        settings,
        absolute,
        parsed_season,
    ) {
        SeasonAliasDecision::UseSeason(season) => {
            for (season_key, rule) in overrides {
                if jumbie_shared::mapping::parse_season_num(season_key) == Some(season) {
                    return ResolvedReleaseSeason::Season {
                        season_key,
                        offset: rule.offset(),
                    };
                }
            }
        }
        SeasonAliasDecision::Unneeded => return ResolvedReleaseSeason::Ambiguous,
        SeasonAliasDecision::NoSignal => {}
    }

    let parsed_season_str = parsed_season
        .map(|season| season.to_string())
        .unwrap_or_default();
    let mut matched: Vec<(&str, i32)> = Vec::new();
    for (season_key, rule) in overrides {
        if release_season_claimed(rule, season_key, &parsed_season_str, parsed_season, info) {
            matched.push((season_key, rule.offset()));
        }
    }

    match matched.as_slice() {
        [] => ResolvedReleaseSeason::Unclaimed,
        [(season_key, offset)] => ResolvedReleaseSeason::Season {
            season_key,
            offset: *offset,
        },
        _ => ResolvedReleaseSeason::Ambiguous,
    }
}

pub(crate) struct MatchedSeasonOverride<'a> {
    pub season_key: &'a str,
    pub rule: &'a jumbie_shared::types::SeasonOverride,
    /// Set when the rule matched via its season-level regex patterns; the caller
    /// reuses this parse instead of re-running the regex.
    pub pattern_match: Option<CustomParseResult>,
}

/// Rejection reason from the absolute-vs-season mode guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeasonModeRejection {
    /// Absolute numbering expects season-less episode numbers; a release that
    /// declares a season above 1 belongs to normal numbering.
    SeasonAboveOneInAbsoluteMode(i32),
    /// Normal numbering received a season-less (absolute-numbered) release that
    /// no season override claims.
    UnownedSeasonlessRelease,
}

/// The season-mode guard's decision for one release / mapping pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeasonModeDecision {
    /// Proceed normally.
    Accept,
    /// Proceed, but only because Phase 1 already confirmed the series identity by
    /// pattern — the release has no season and no override claims it.
    AcceptViaPhaseOneMatch,
    /// Drop the release.
    Reject(SeasonModeRejection),
}

/// Decide whether a release is compatible with the series' numbering mode.
///
/// * **absolute** — a release declaring a season above 1 is rejected: the
///   absolute space has a single season and expects season-less episode numbers.
/// * **normal** — a season-less release is rejected unless a season override
///   claims its episode range or matches the title with a season-level pattern;
///   if neither holds, it is still admitted when Phase 1 already matched the
///   series pattern ([`SeasonModeDecision::AcceptViaPhaseOneMatch`]).
///
/// `overrides` is the active-mode override map (`SeriesSettings::season_for_active_mode`).
pub(crate) fn season_mode_guard(
    absolute: bool,
    seasons: &[i32],
    episodes: &[i32],
    overrides: &HashMap<String, jumbie_shared::types::SeasonOverride>,
    title: &str,
    instance_id: &str,
    phase1_matched: bool,
) -> SeasonModeDecision {
    if absolute {
        return match seasons.first() {
            Some(&s) if s > 1 => {
                SeasonModeDecision::Reject(SeasonModeRejection::SeasonAboveOneInAbsoluteMode(s))
            }
            _ => SeasonModeDecision::Accept,
        };
    }

    if !seasons.is_empty() {
        return SeasonModeDecision::Accept;
    }

    // Season-less release in normal numbering — only overrides can claim it.
    // The regex parse runs in normal mode (that is this branch's mode).
    let episode = episodes.first().copied().unwrap_or(0);
    let has_range_override = overrides
        .values()
        .any(|so| so.contains_source_episode(episode));
    let has_pattern_match = !has_range_override
        && overrides
            .values()
            .filter(|so| so.has_search_patterns())
            .any(|so| {
                matches!(
                    parse_title_with_custom_regex(
                        title,
                        &so.reg_patterns,
                        None,
                        false,
                        Some(instance_id),
                    ),
                    CustomParseResult::Extracted(_) | CustomParseResult::MatchedFilter
                )
            });

    if has_range_override || has_pattern_match {
        SeasonModeDecision::Accept
    } else if phase1_matched {
        SeasonModeDecision::AcceptViaPhaseOneMatch
    } else {
        SeasonModeDecision::Reject(SeasonModeRejection::UnownedSeasonlessRelease)
    }
}

/// Decide which season overrides match this release. `title` / `instance_id`
/// are only used by the regex-pattern branch (4).
///
/// The four match paths, in order:
/// 1. explicit alias season number,
/// 2. the override's own season key (literal or numeric),
/// 3. a season-less release claimed by an override's episode range,
/// 4. a season-level regex pattern.
///
/// `absolute` is the effective numbering mode; it is only needed because the
/// regex branch parses in mode (absolute ignores the season group).
pub(crate) fn match_season_overrides<'a>(
    season_overrides: &'a HashMap<String, jumbie_shared::types::SeasonOverride>,
    parsed_season_str: &str,
    title: &str,
    info: &jumbie_shared::types::EpisodeInfo,
    instance_id: &str,
    absolute: bool,
) -> Vec<MatchedSeasonOverride<'a>> {
    let mut matches = Vec::new();

    for (season_key, rule) in season_overrides {
        let mut matched = release_season_claimed(
            rule,
            season_key,
            parsed_season_str,
            info.seasons.first().copied(),
            info,
        );
        let mut pattern_match = None;

        // 4) Season-level regex patterns (anchored full match)
        if !matched && info.seasons.is_empty() && rule.has_search_patterns() {
            let anchored: Vec<String> = rule
                .reg_patterns
                .iter()
                .map(|p| format!("^(?:{})$", p))
                .collect();
            let result =
                parse_title_with_custom_regex(title, &anchored, None, absolute, Some(instance_id));
            if matches!(
                result,
                CustomParseResult::Extracted(_) | CustomParseResult::MatchedFilter
            ) {
                matched = true;
                pattern_match = Some(result);
            }
        }

        if matched {
            matches.push(MatchedSeasonOverride {
                season_key,
                rule,
                pattern_match,
            });
        }
    }

    matches
}

/// Season-override matching that first applies season-alias title matching.
///
/// A season-alias decision from [`jumbie_shared::mapping::resolve_season_from_aliases`]
/// is applied first:
/// * `UseSeason` selects that season's override directly and **ignores** the
///   parsed season number.
/// * `Unneeded` (ambiguous season aliases with no disambiguating number) claims
///   no season, so the release is dropped (treated as unneeded).
/// * `NoSignal` falls through to the normal season-number rules in
///   [`match_season_overrides`].
pub(crate) fn match_season_overrides_with_settings<'a>(
    settings: &'a jumbie_shared::types::SeriesSettings,
    parsed_season_str: &str,
    title: &str,
    info: &jumbie_shared::types::EpisodeInfo,
    instance_id: &str,
    absolute: bool,
) -> Vec<MatchedSeasonOverride<'a>> {
    use jumbie_shared::mapping::SeasonAliasDecision;

    let overrides = settings.season_for_active_mode(absolute);

    match jumbie_shared::mapping::resolve_season_from_aliases(
        title,
        settings,
        absolute,
        info.seasons.first().copied(),
    ) {
        SeasonAliasDecision::UseSeason(season) => {
            for (season_key, rule) in overrides {
                if jumbie_shared::mapping::parse_season_num(season_key) == Some(season) {
                    return vec![MatchedSeasonOverride {
                        season_key: season_key.as_str(),
                        rule,
                        pattern_match: None,
                    }];
                }
            }
            // No override exists for the alias's season — fall through.
        }
        SeasonAliasDecision::Unneeded => return Vec::new(),
        SeasonAliasDecision::NoSignal => {}
    }

    match_season_overrides(
        overrides,
        parsed_season_str,
        title,
        info,
        instance_id,
        absolute,
    )
}

impl ContentOrganizer {
    pub(crate) async fn process_entry_inner<'a>(
        &self,
        title: &'a str,
        info: jumbie_shared::types::EpisodeInfo,
        mapping_base: MappingRule,
        instance_id: &str,
        entry: &'a MediaEntry,
        phase1_matched: bool,
    ) -> Result<Vec<ReleaseCandidate>> {
        let global_filters = self.db.get_global_filters().await.unwrap_or_default();
        // SSoT: effective numbering-mode default — series tristate overrides
        // (Some(true)/Some(false)) win; None falls back to this global value.
        let global_absolute = self.db_config().await.general.absolute_numbering;

        // Decimal episode guard
        // Episode numbers with fractional components (e.g. S01E1.5) cannot be
        // represented in the system — all episode numbers are integers. Block
        // from auto-download. Users can override via custom RSS patterns.
        if info.has_decimal_episode {
            trace!(
                "Blocking '{}': decimal episode number for series '{}'",
                title, mapping_base.target_title
            );
            return Ok(vec![]);
        }

        // Absolute vs. season mode guard: season-less (absolute-numbered) releases
        // are accepted only when the series is in absolute mode or a season override
        // claims their episode range; a release declaring season > 1 is blocked in
        // absolute mode. The decision lives in `season_mode_guard`.
        match season_mode_guard(
            mapping_base
                .settings
                .active_mode(global_absolute)
                .is_absolute(),
            &info.seasons,
            &info.episodes,
            mapping_base
                .settings
                .season_for_active_mode(global_absolute),
            title,
            instance_id,
            phase1_matched,
        ) {
            SeasonModeDecision::Reject(SeasonModeRejection::SeasonAboveOneInAbsoluteMode(s)) => {
                trace!(
                    "Blocking '{}' — season={} > 1 in absolute-numbering mode for series '{}'",
                    title, s, mapping_base.target_title
                );
                return Ok(vec![]);
            }
            SeasonModeDecision::Reject(SeasonModeRejection::UnownedSeasonlessRelease) => {
                trace!(
                    "Blocking '{}' — no season, no range override, and no pattern match for series '{}'",
                    title, mapping_base.target_title
                );
                return Ok(vec![]);
            }
            SeasonModeDecision::AcceptViaPhaseOneMatch => {
                // Phase 1 already validated this entry via series-level pattern
                // matching, which confirmed the series identity regardless of
                // season number — so the missing season is tolerated.
                trace!(
                    "Phase 1: admitting '{}' with no season via pattern match for series '{}'",
                    title, mapping_base.target_title
                );
            }
            SeasonModeDecision::Accept => {}
        }

        let mapping_arc = Arc::new(mapping_base);

        let mut candidates_to_return = Vec::new();
        let parsed_season_str = info
            .seasons
            .first()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "01".to_string());

        // Season-mapping: user-defined overrides (aliases, episode ranges) first;
        // if none matches the release's native season, fall back.
        type SeasonMappingEntry = (
            Arc<MappingRule>,
            Option<i32>,
            Option<i32>,
            Option<CustomParseResult>,
        );
        let mut matched_season_mappings: Vec<SeasonMappingEntry> = Vec::new();

        let season_overrides = mapping_arc.settings.season_for_active_mode(global_absolute);
        if season_overrides.is_empty() {
            matched_season_mappings.push((mapping_arc.clone(), None, None, None));
        } else {
            // Season-alias title matches are applied first, favouring a unique
            // season alias over the parsed season number.
            let absolute = mapping_arc
                .settings
                .active_mode(global_absolute)
                .is_absolute();
            for matched in match_season_overrides_with_settings(
                &mapping_arc.settings,
                &parsed_season_str,
                title,
                &info,
                instance_id,
                absolute,
            ) {
                trace!("Season mapping matched season_key: {}", matched.season_key);
                let mut mapping = (*mapping_arc).clone();
                if matched.rule.has_search_aliases() {
                    mapping.settings.aliases = matched.rule.aliases.clone();
                }
                if matched.rule.has_search_patterns() {
                    mapping.settings.reg_patterns = matched.rule.reg_patterns.clone();
                }
                matched_season_mappings.push((
                    Arc::new(mapping),
                    jumbie_shared::mapping::parse_season_num(matched.season_key),
                    matched.rule.episode_offset,
                    matched.pattern_match,
                ));
            }

            // Native season fallback
            if let Some(&native_season_num) = info.seasons.first() {
                let native_season_str = format!("{:02}", native_season_num);
                let native_handled = mapping_arc
                    .settings
                    .find_season_override(&native_season_str, global_absolute)
                    .is_some();
                if !native_handled {
                    trace!(
                        "Using native season fallback for season: {:?}",
                        info.seasons.first()
                    );
                    matched_season_mappings.push((
                        mapping_arc.clone(),
                        info.seasons.first().copied(),
                        None,
                        None,
                    ));
                }
            }
        }

        for (mapping, target_season_num, episode_offset, cached_pattern_match) in
            matched_season_mappings
        {
            // Custom regex extraction & pre-filter
            let effective_info = if let Some(cached) = &cached_pattern_match {
                match cached {
                    CustomParseResult::Extracted(extracted) => (**extracted).clone(),
                    CustomParseResult::MatchedFilter => info.clone(),
                    CustomParseResult::NoMatch => continue,
                }
            } else if mapping.settings.has_search_patterns() {
                match parse_title_with_custom_regex(
                    title,
                    &mapping.settings.reg_patterns,
                    target_season_num,
                    mapping_arc
                        .settings
                        .active_mode(global_absolute)
                        .is_absolute(),
                    Some(instance_id),
                ) {
                    CustomParseResult::Extracted(extracted) => *extracted,
                    CustomParseResult::MatchedFilter => info.clone(),
                    CustomParseResult::NoMatch => {
                        trace!(
                            "Entry '{}' rejected by reg_patterns for series '{}'",
                            title, mapping.target_title
                        );
                        continue;
                    }
                }
            } else {
                info.clone()
            };

            let merged_filters = mapping.get_merged_filters(&global_filters);
            let description = entry.description.as_deref();
            let files_ref: Vec<String> = entry.file_list.clone();

            match merged_filters.matches(title, description, Some(&files_ref)) {
                Ok(passed) => {
                    if !passed {
                        trace!("Filter did not pass for title: {}", title);
                        continue;
                    }
                }
                Err(e) => {
                    debug!("Filter error for {}: {}", title, e);
                    continue;
                }
            }

            let r_profile_id = mapping.release_profile.as_deref().unwrap_or("");
            let profile_scoring = self
                .db
                .get_release_profile(r_profile_id)
                .await
                .unwrap_or(None)
                .unwrap_or_default();

            let (title_ref, size, seeders, published) = entry.scoring_fields();

            let episode_count = if effective_info.episodes.len() > 1 {
                Some(effective_info.episodes.len() as u32)
            } else {
                None
            };

            let (base_score, mut reasons) = mapping
                .get_merged_scoring(&profile_scoring)
                .calculate_with_submitter(
                    title_ref,
                    size,
                    seeders,
                    published,
                    episode_count,
                    entry.submitter.as_deref(),
                );

            let mut final_info = effective_info;
            if let Some(ts) = target_season_num {
                final_info.seasons = vec![ts];
            }
            if let Some(offset) = episode_offset {
                let before_ep = final_info.episodes.first().copied().unwrap_or(0);
                for ep in &mut final_info.episodes {
                    *ep -= offset;
                }
                trace!(
                    "Applied episode_offset {}: episode {} -> {} for season {:?}",
                    offset,
                    before_ep,
                    final_info.episodes.first().copied().unwrap_or(0),
                    final_info.seasons.first()
                );
            }

            let (pack_penalty, needed, unneeded) = self
                .calculate_pack_score(&final_info, &mapping, global_absolute)
                .await?;

            // Read the general config from the DB — self.config is TOML-only
            // and never synced with DB-managed sections.
            let general_config = self.db.get_general_config().await.unwrap_or_default();
            let auto_enabled = general_config.automatic_profiles.enabled;
            let mut submitter_score = 0;
            if auto_enabled {
                let submitter = entry
                    .submitter
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string());
                if let Ok(Some(profile)) = self.db.get_automatic_profile(&submitter).await {
                    submitter_score = profile.score;
                    if submitter_score < 0 {
                        reasons.push(format!("Automatic profile score: {}", submitter_score));
                    }
                }
            }

            // Flat per-pack adjustment from settings (SSoT: GeneralConfig).
            let is_pack = final_info.is_season_pack || final_info.is_complete_pack;
            let pack_modifier = if is_pack {
                general_config.effective_pack_score_modifier()
            } else {
                0
            };

            let score = base_score + pack_penalty + submitter_score + pack_modifier;
            if pack_penalty < 0 {
                reasons.push(format!("{} pack penalty", pack_penalty));
            }
            if pack_modifier != 0 {
                reasons.push(format!("{:+} season pack score modifier", pack_modifier));
            }

            // Quality profile filter (SSoT: `crate::release_checks::quality_failure`)
            if let Some(qp_name) = mapping.quality_profile.as_deref().filter(|n| !n.is_empty()) {
                let (qualities, quality_profiles) = self.db.get_all_quality_data().await;
                if crate::release_checks::quality_failure(
                    title,
                    Some(qp_name),
                    &quality_profiles,
                    &qualities,
                )
                .is_some()
                {
                    trace!(
                        "Skipping '{}' — quality not in profile '{}'",
                        title, qp_name
                    );
                    continue;
                }
            }

            let resolved_submitter = entry.submitter.clone();

            // Resolve the configured instance name from the backend-owned mapping —
            // avoids trusting the plugin's self-reported display_name.
            let source_name = self
                .plugin_manager
                .read()
                .await
                .get_instance_name(instance_id);

            candidates_to_return.push(ReleaseCandidate {
                title: title.to_string(),
                download_url: entry.download_url.clone(),
                episode_info: final_info,
                mapping: mapping.clone(),
                meta_date: entry.published,
                score,
                score_breakdown: reasons,
                description: entry.description.clone(),
                file_list: entry.file_list.clone(),
                guid: entry.guid.clone(),
                needed_episodes: needed,
                unneeded_count: unneeded,
                seeders: entry.seeders.unwrap_or(0),
                leechers: entry.leechers.unwrap_or(0),
                download_id: entry.download_id.clone(),
                indexer: source_name,
                size_bytes: entry.size.unwrap_or(0),
                multi_targets: vec![],
                submitter: resolved_submitter,
            });
        }
        Ok(candidates_to_return)
    }
}

/// Build a filter-only regex pattern from a series name or alias.
///
/// The pattern is:
/// - Case-insensitive (`(?i)`)
/// - Spaces replaced with `[._ ]` so "My Show" matches both "My Show" and
///   "My.Show" in the raw release title (consistent with `parse_filename`'s
///   dot-normalization behaviour)
/// - Regex-special characters are escaped
///
/// The resulting pattern is filter-only (no capture groups), so it produces
/// `CustomParseResult::MatchedFilter` on match — the caller must still run
/// `parse_filename` for episode/season extraction.
pub(crate) fn generate_auto_pattern(name_or_alias: &str) -> String {
    let escaped = regex::escape(name_or_alias);
    // Replace literal spaces with a class accepting dots, underscores, or spaces —
    // mirroring parse_filename's dot↔space normalization on the series_key.
    let flexible = escaped.replace(' ', "[._ ]");
    format!("(?i){}", flexible)
}

/// In-memory lookup matching `get_mapping_by_key`'s substring logic.
///
/// Checks the `series_key` (from `parse_filename`) against every series'
/// `name`, `target_title`, and `aliases` using `contains()` with
/// dot↔space normalization and `clean_title` stripping — identical
/// semantics to the DB-side `get_mapping_by_key`.
///
/// Returns the series ID and mapping rule, or `None` if no match.
///
/// This avoids a redundant `get_all_series_mappings()` call when the
/// caller already has the mapping set loaded (e.g. after Phase 1).
pub(crate) fn lookup_mapping_in(
    series_key: &str,
    all_mappings: &HashMap<String, MappingRule>,
) -> Option<(String, MappingRule)> {
    // SSoT: normalization lives in `jumbie_shared::mapping::matching` so release
    // recognition, file-scan lookup, and season-alias resolution agree.
    let key_norm = jumbie_shared::mapping::normalize_for_match(series_key);
    let cleaned_norm = jumbie_shared::mapping::normalize_cleaned_for_match(series_key);

    for (id, rule) in all_mappings {
        // Series name: raw form only (unchanged semantics). Target title and
        // aliases (series + season): raw OR `clean_title` form.
        if key_norm.contains(&jumbie_shared::mapping::normalize_for_match(&rule.name))
            || jumbie_shared::mapping::matches_normalized(
                &key_norm,
                &cleaned_norm,
                &rule.target_title,
            )
            || rule
                .settings
                .all_aliases()
                .any(|a| jumbie_shared::mapping::matches_normalized(&key_norm, &cleaned_norm, a))
        {
            return Some((id.clone(), rule.clone()));
        }
    }

    None
}

#[cfg(test)]
pub(crate) fn strip_named_groups_for_test(pattern: &str) -> String {
    jumbie_shared::mapping::strip_named_groups(pattern)
}

/// The gate: a `RegexSet` over every series' identifying patterns, with each
/// pattern index mapped back to the series that contributed it.
pub(crate) struct SeriesGate {
    set: regex::RegexSet,
    owners: Vec<usize>,
}

impl SeriesGate {
    /// Whether any series' identifying pattern matches `title`.
    pub(crate) fn is_match(&self, title: &str) -> bool {
        self.set.is_match(title)
    }
}

/// A series' id plus the Phase 1 patterns compiled for it.
struct MatcherEntry {
    id: String,
    patterns: CompiledPatterns,
}

/// Precompiled release identification for one mapping set.
///
/// Built once and reused across every entry in a sync: the gate and each series'
/// Phase 1 patterns are compiled a single time instead of per entry. The gate is a
/// superset of Phase 1, so a gate miss skips the entry and a gate hit narrows Phase
/// 1 to the candidate series.
pub(crate) struct SeriesMatcher {
    entries: Vec<MatcherEntry>,
    gate: Option<SeriesGate>,
}

impl SeriesMatcher {
    pub(crate) fn build(all_mappings: &HashMap<String, MappingRule>) -> Self {
        let mut entries = Vec::with_capacity(all_mappings.len());
        let mut gate_patterns = Vec::new();
        let mut owners = Vec::new();

        for (index, (id, mapping)) in all_mappings.iter().enumerate() {
            entries.push(MatcherEntry {
                id: id.clone(),
                patterns: CompiledPatterns::compile(&phase_one_patterns(mapping)),
            });
            let parts = gate_patterns_for(mapping);
            if !parts.is_empty() {
                gate_patterns.push(join_alternation(&parts));
                owners.push(index);
            }
        }

        let gate = if gate_patterns.is_empty() {
            None
        } else {
            regex::RegexSet::new(&gate_patterns)
                .ok()
                .map(|set| SeriesGate { set, owners })
        };

        Self { entries, gate }
    }

    /// Whether the gate admits `title`. Always true when no gate compiled: either
    /// no series has identifying patterns, or the set exceeded the regex size limit.
    pub(crate) fn is_match(&self, title: &str) -> bool {
        match &self.gate {
            Some(gate) => gate.is_match(title),
            None => true,
        }
    }

    /// Phase 1: the first series whose patterns match `title`, with its parse
    /// result. `None` means no series matched, so the caller falls back to Phase 2.
    pub(crate) fn identify(
        &self,
        all_mappings: &HashMap<String, MappingRule>,
        title: &str,
        instance_id: &str,
        global_absolute: bool,
    ) -> Option<(MappingRule, Option<CustomParseResult>)> {
        match &self.gate {
            Some(gate) => {
                for pattern_index in gate.set.matches(title).iter() {
                    let index = gate.owners[pattern_index];
                    if let Some(found) =
                        self.try_entry(index, all_mappings, title, instance_id, global_absolute)
                    {
                        return Some(found);
                    }
                }
                None
            }
            None => (0..self.entries.len()).find_map(|index| {
                self.try_entry(index, all_mappings, title, instance_id, global_absolute)
            }),
        }
    }

    fn try_entry(
        &self,
        index: usize,
        all_mappings: &HashMap<String, MappingRule>,
        title: &str,
        instance_id: &str,
        global_absolute: bool,
    ) -> Option<(MappingRule, Option<CustomParseResult>)> {
        let entry = &self.entries[index];
        let mapping = all_mappings.get(&entry.id)?;
        let absolute = mapping.settings.active_mode(global_absolute).is_absolute();
        let result =
            match_title_compiled(title, &entry.patterns, None, absolute, Some(instance_id));
        match result {
            CustomParseResult::Extracted(_) | CustomParseResult::MatchedFilter => {
                trace!(
                    "identify: series '{}' matched title '{}'",
                    mapping.target_title, title
                );
                Some((mapping.clone(), Some(result)))
            }
            CustomParseResult::NoMatch => None,
        }
    }
}

/// Build just the gate, for tests and callers that do not need Phase 1 patterns.
#[cfg(test)]
pub(crate) fn build_gate_regex(all_mappings: &HashMap<String, MappingRule>) -> Option<SeriesGate> {
    SeriesMatcher::build(all_mappings).gate
}

/// The patterns Phase 1 matches for a series: user patterns when configured,
/// otherwise auto-generated from its identifying strings.
fn phase_one_patterns(mapping: &MappingRule) -> Vec<String> {
    if mapping.settings.has_search_patterns() {
        mapping.settings.reg_patterns.clone()
    } else {
        auto_patterns(mapping)
    }
}

/// Filter patterns auto-generated from a series' name, target_title, and aliases.
fn auto_patterns(mapping: &MappingRule) -> Vec<String> {
    let mut patterns = Vec::new();
    if !mapping.name.is_empty() {
        patterns.push(generate_auto_pattern(&mapping.name));
    }
    if mapping.target_title != mapping.name && !mapping.target_title.is_empty() {
        patterns.push(generate_auto_pattern(&mapping.target_title));
    }
    for alias in mapping.settings.all_aliases() {
        patterns.push(generate_auto_pattern(alias));
    }
    patterns
}

/// Everything that can identify a series: its auto patterns plus every valid user
/// pattern with named groups stripped (the gate only asks whether a title matches).
fn gate_patterns_for(mapping: &MappingRule) -> Vec<String> {
    let mut parts = auto_patterns(mapping);
    for pattern in &mapping.settings.reg_patterns {
        let parsed = jumbie_shared::mapping::parse_source_pattern(pattern);
        let stripped = jumbie_shared::mapping::strip_named_groups(&parsed.pattern);
        if !stripped.is_empty() && regex::Regex::new(&stripped).is_ok() {
            parts.push(stripped);
        }
    }
    parts
}

/// Wrap each alternative in `(?:...)` so inline flags (`(?i)`, ...) stay scoped to
/// their own alternative instead of leaking into the next.
fn join_alternation(parts: &[String]) -> String {
    let wrapped: Vec<String> = parts.iter().map(|part| format!("(?:{})", part)).collect();
    format!("(?:{})", wrapped.join("|"))
}

#[cfg(test)]
mod tests {
    use super::{
        match_season_overrides, match_season_overrides_with_settings,
        season_key_matches_numerically,
    };
    use jumbie_shared::types::{EpisodeInfo, SeasonOverride, SeriesSettings};
    use std::collections::HashMap;

    fn info(seasons: Vec<i32>, episodes: Vec<i32>) -> EpisodeInfo {
        EpisodeInfo {
            raw_title: String::new(),
            series_key: String::new(),
            file_ext: String::new(),
            submitter: None,
            resolution: None,
            version: 1,
            part_number: None,
            is_season_pack: false,
            is_complete_pack: false,
            seasons,
            episodes,
            has_decimal_episode: false,
        }
    }

    fn override_for(key: &str, start: Option<i32>, end: Option<i32>) -> SeasonOverride {
        SeasonOverride {
            season: key.to_string(),
            episode_start: start,
            episode_end: end,
            cell_count: None,
            episode_offset: None,
            alias_season_number: None,
            search_format: None,
            aliases: vec![],
            reg_patterns: vec![],
        }
    }

    fn overrides(entries: Vec<(&str, SeasonOverride)>) -> HashMap<String, SeasonOverride> {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    // Season-alias title matching

    #[test]
    fn season_alias_title_selects_that_override() {
        let mut settings = SeriesSettings::default();
        settings.season.insert(
            "0".to_string(),
            SeasonOverride {
                aliases: vec!["Whisker Mini".to_string()],
                ..override_for("0", None, None)
            },
        );
        let matched = match_season_overrides_with_settings(
            &settings,
            "01",
            "[G] Whisker Mini Anime - 01-13",
            &info(vec![], vec![1]),
            "",
            false,
        );
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].season_key, "0");
    }

    #[test]
    fn series_alias_does_not_downgrade_season_alias() {
        // Title matches both a series alias and season 0's alias — the season is
        // favoured (the parsed season number is ignored).
        let mut settings = SeriesSettings {
            aliases: vec!["Whisker Mini".to_string()],
            ..Default::default()
        };
        settings.season.insert(
            "0".to_string(),
            SeasonOverride {
                aliases: vec!["Whisker Mini".to_string()],
                ..override_for("0", None, None)
            },
        );
        settings
            .season
            .insert("3".to_string(), override_for("3", None, None));

        let matched = match_season_overrides_with_settings(
            &settings,
            "03",
            "Whisker Mini Anime S03E01",
            &info(vec![3], vec![1]),
            "",
            false,
        );
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].season_key, "0");
    }

    #[test]
    fn conflicting_season_aliases_are_unneeded() {
        // Two seasons share the alias and there is no season number to
        // disambiguate → no season is claimed (release treated as unneeded).
        let mut settings = SeriesSettings::default();
        settings.season.insert(
            "0".to_string(),
            SeasonOverride {
                aliases: vec!["Whisker Mini".to_string()],
                ..override_for("0", None, None)
            },
        );
        settings.season.insert(
            "2".to_string(),
            SeasonOverride {
                aliases: vec!["Whisker Mini".to_string()],
                ..override_for("2", None, None)
            },
        );

        let matched = match_season_overrides_with_settings(
            &settings,
            "01",
            "Whisker Mini Anime",
            &info(vec![], vec![1]),
            "",
            false,
        );
        assert!(matched.is_empty(), "conflicting aliases must be unneeded");
    }

    #[test]
    fn numeric_season_match_accepts_every_stored_key_form() {
        for key in ["2", "02", "S02", "s02"] {
            assert!(
                season_key_matches_numerically(Some(2), key),
                "key {key:?} should match release season 2"
            );
        }
    }

    #[test]
    fn numeric_season_match_requires_a_real_release_season() {
        // A release with no parsed season must not be treated as season 1.
        assert!(!season_key_matches_numerically(None, "1"));
        // …and a different season must not match.
        assert!(!season_key_matches_numerically(Some(1), "2"));
    }

    #[test]
    fn numeric_season_match_rejects_non_numeric_keys() {
        // "SP" has no numeric form — only the literal key comparison can match it.
        assert!(!season_key_matches_numerically(Some(1), "SP"));
        assert!(!season_key_matches_numerically(None, "SP"));
    }

    // match_season_overrides (the function production uses)

    #[test]
    fn matches_override_by_numeric_season_key() {
        let map = overrides(vec![("2", override_for("2", Some(1), Some(100)))]);
        let matched = match_season_overrides(&map, "2", "", &info(vec![2], vec![5]), "", false);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].season_key, "2");
    }

    #[test]
    fn season_less_release_matches_override_by_episode_range() {
        // Absolute-numbered release: no season at all, claimed by the range.
        let map = overrides(vec![("1", override_for("1", Some(1), Some(50)))]);
        let matched = match_season_overrides(&map, "01", "", &info(vec![], vec![7]), "", true);
        assert_eq!(
            matched.len(),
            1,
            "a season-less release must be claimable by an override's range"
        );
    }

    #[test]
    fn season_less_release_outside_every_range_matches_nothing() {
        let map = overrides(vec![("1", override_for("1", Some(1), Some(10)))]);
        let matched = match_season_overrides(&map, "01", "", &info(vec![], vec![99]), "", true);
        assert!(matched.is_empty());
    }

    #[test]
    fn non_numeric_season_key_does_not_match_a_numeric_release_season() {
        // "SP" has no numeric form, so it can only match via the literal key.
        let map = overrides(vec![("SP", override_for("SP", Some(1), Some(100)))]);
        let matched = match_season_overrides(&map, "1", "", &info(vec![1], vec![5]), "", false);
        assert!(matched.is_empty());
    }

    #[test]
    fn matches_season_level_regex_pattern_and_returns_its_parse() {
        let mut rule = override_for("1", Some(1), Some(100));
        rule.reg_patterns = vec![r"(?i)batch(?P<episode>\d+)".to_string()];
        let map = overrides(vec![("1", rule)]);
        let matched =
            match_season_overrides(&map, "01", "Batch07", &info(vec![], vec![7]), "src", true);
        assert_eq!(
            matched.len(),
            1,
            "the anchored pattern should claim the title"
        );
        assert!(
            matched[0].pattern_match.is_some(),
            "the pattern branch must hand its parse result back to the caller"
        );
    }
}
