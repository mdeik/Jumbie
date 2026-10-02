//! Auto Search Season — background retry loop for missing episodes.
//!
//! Runs as a background task (returning 202 Accepted) because searching multiple
//! indexers, ranking, and waiting for downloads takes minutes; the frontend polls
//! the download queue for results. Auto-search delegates to each plugin's
//! `auto_search` method, which receives a structured payload and owns its search
//! strategy. The retry loop runs at most once every 5 minutes and stops when all
//! episodes are found or a round returns nothing new.

use crate::api::AppState;
use crate::error::AppError;
use crate::utils::parse_title_with_custom_regex;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use jumbie_shared::mapping::NumberingMode;
use jumbie_shared::parsing::CustomParseResult;
use jumbie_shared::types::EpisodeStatus;
use serde::Serialize;
use std::sync::Arc;

/// Filters out already-downloaded episodes from the search list when replacement is off.
///
/// When `replacement_on` is `true`, the full remaining list is returned unchanged — the
/// caller will consider upgrade candidates for already-downloaded episodes.
///
/// When `replacement_on` is `false`, episodes that already have a `file_path` on disk
/// are removed from the list so no search queries are wasted on them.
pub async fn filter_downloaded_episodes(
    db: &crate::db::DbManager,
    series_id: &str,
    season: i32,
    remaining: &[i32],
    replacement_on: bool,
    mode: NumberingMode,
) -> Vec<i32> {
    if replacement_on {
        return remaining.to_vec();
    }

    if let Ok(current_eps) = db
        .get_episodes_for_series_season(series_id, season, mode.is_absolute())
        .await
    {
        let downloaded_eps: Vec<i32> = current_eps
            .iter()
            .filter(|e| e.file_path.is_some() && e.file_path.as_deref() != Some(""))
            .map(|e| e.episode)
            .collect();

        if !downloaded_eps.is_empty() {
            let mut result: Vec<i32> = remaining.to_vec();
            result.retain(|ep| !downloaded_eps.contains(ep));
            return result;
        }
    }

    remaining.to_vec()
}

/// Parse all episode numbers that appear in a title string.
/// Recognises patterns like S01E03, S01E03E04, E03, E03E04, 3x05, etc.
///
/// Download results carry no structured episode metadata, so coverage is inferred by
/// parsing the filename. This is inherently fuzzy (non-standard formats, omitted
/// numbers); the retry loop handles false negatives.
pub fn parse_episode_numbers_from_title(title: &str) -> Vec<i32> {
    // `utils::parse_filename` handles common patterns with ambiguity resolution and
    // context (e.g. season packs), so it is tried before regex.
    if let Some(info) = crate::utils::parse_filename(title, crate::utils::ParseContext::Search) {
        if info.is_season_pack || info.is_complete_pack {
            return Vec::new();
        }
        // Decimal episodes (e.g. S01E1.5) cannot be represented in the system; block
        // them from auto-download (custom RSS patterns can override).
        if info.has_decimal_episode {
            return Vec::new();
        }
        return info.episodes.clone();
    }

    // Regex fallback for unusual naming: MULTI_EPISODE_PATTERN is broader but less
    // context-aware.
    let mut nums = Vec::new();
    for cap in crate::patterns::MULTI_EPISODE_PATTERN.captures_iter(title) {
        if let Some(m) = cap.get(1).or_else(|| cap.get(4))
            && let Ok(n) = m.as_str().parse::<i32>()
        {
            nums.push(n);
        }
        if let Some(m) = cap.get(2)
            && let Ok(n) = m.as_str().parse::<i32>()
        {
            nums.push(n);
        }
    }
    nums.sort_unstable();
    nums.dedup();
    nums
}

/// Parse the episode numbers a search result covers, scoped to the season being
/// searched.
///
/// `required_season` comes from [`crate::search::SeasonSearchMeta::required_season`].
/// When set, the result must declare exactly that season: a title with no season
/// (which the plain parser would default to `S01`) or a different season yields no
/// episodes — it is unrelated to the search. When `None`, the series omits season
/// markers from its queries (e.g. absolute numbering) and season-less results are
/// expected and parsed leniently.
///
/// RSS feeds are deliberately not routed through this: a series-scoped feed may
/// legitimately omit the season.
pub fn parse_episode_numbers_for_season(title: &str, required_season: Option<i32>) -> Vec<i32> {
    let Some(season_num) = required_season else {
        return parse_episode_numbers_from_title(title);
    };
    match crate::search::parse_search_result_title(title, Some(season_num)) {
        Some(info)
            if !info.is_season_pack && !info.is_complete_pack && !info.has_decimal_episode =>
        {
            info.episodes
        }
        // `<season>x<episode>` naming (`3x05`) declares a season the filename
        // parser does not read; verify it here so those results still match.
        _ => parse_nx_episodes_for_season(title, season_num),
    }
}

/// Extract episodes from `<season>x<episode>` titles (`3x05`) whose declared
/// season equals `season_num`. Only the `NxNN` alternative of
/// [`MULTI_EPISODE_PATTERN`](crate::patterns::MULTI_EPISODE_PATTERN) carries a
/// season (capture group 3); the `E..` alternative is already handled by the
/// filename parser.
fn parse_nx_episodes_for_season(title: &str, season_num: i32) -> Vec<i32> {
    let mut nums = Vec::new();
    for cap in crate::patterns::MULTI_EPISODE_PATTERN.captures_iter(title) {
        let Some(season) = cap.get(3).and_then(|m| m.as_str().parse::<i32>().ok()) else {
            continue;
        };
        if season != season_num {
            continue;
        }
        if let Some(m) = cap.get(4)
            && let Ok(n) = m.as_str().parse::<i32>()
        {
            nums.push(n);
        }
    }
    nums.sort_unstable();
    nums.dedup();
    nums
}

/// Episode count handed to scoring so `size_score_per_gb` normalizes per episode.
///
/// A season pack's count is the whole season; a multi-episode range release (partial
/// pack) normalizes by the episodes it actually covers; a single episode is not
/// normalized (returning `None`). Returns `None` for a season pack whose season size
/// is unknown, matching the previous inline behavior.
fn episode_count_for_scoring(
    is_season_pack: bool,
    covered_episodes: usize,
    total_episodes_in_season: i32,
) -> Option<u32> {
    if is_season_pack {
        (total_episodes_in_season > 0).then_some(total_episodes_in_season as u32)
    } else if covered_episodes > 1 {
        Some(covered_episodes as u32)
    } else {
        None
    }
}

/// Build the queue's episode intentions for a selected release: the episodes the
/// download keeps (`keep = true`) and the covered-but-not-needed episodes that must be
/// discarded as unneeded (`keep = false`). SSoT for turning a [`MultiBranch`] decision
/// into the harvest policy smart-link consumes.
///
/// [`MultiBranch`]: crate::source_processor::MultiBranch
fn build_episode_intentions(
    keep: &[i32],
    unneeded: &[i32],
    season_num: i32,
    episode_offset: i32,
    absolute_numbering: bool,
    series_id: &str,
    score: i32,
) -> String {
    let intention = |ep: i32, keep: bool| jumbie_shared::types::EpisodeIntention {
        episode_num: ep,
        source_episode_num: jumbie_shared::mapping::local_to_source_episode(ep, episode_offset),
        episode_id: if absolute_numbering {
            jumbie_shared::formatting::fmt_absolute_episode_id(ep, series_id)
        } else {
            jumbie_shared::formatting::fmt_episode_id_num(season_num, ep, series_id)
        },
        score,
        keep,
    };
    let intentions: Vec<jumbie_shared::types::EpisodeIntention> = keep
        .iter()
        .map(|&ep| intention(ep, true))
        .chain(unneeded.iter().map(|&ep| intention(ep, false)))
        .collect();
    serde_json::to_string(&intentions).unwrap_or_default()
}

/// A release selected for download: its [`SearchResult`](jumbie_shared::types::SearchResult)
/// (carrying the final score) plus the local episodes it claims (`claim`) and the
/// covered-but-not-needed overspill to mark unneeded (`unneeded`). Coverage is kept
/// here because it cannot be re-derived from the title once a multi-release is split
/// into claimed vs unneeded episodes.
struct ScoredCandidate {
    result: jumbie_shared::types::SearchResult,
    claim: Vec<i32>,
    unneeded: Vec<i32>,
}

/// Drop guard that marks a search as inactive in `AppState.search_queue` on drop.
///
/// The spawned background task has several exit paths (early exit, loop break,
/// implicit end, and panics), so a Drop guard guarantees the key is always removed —
/// even during stack unwinding. Otherwise a stale entry would block future
/// auto-searches for that season until the server restarts.
///
/// The async mark is done by a spawned task because `Drop::drop` is synchronous. This
/// is safe: the task is fire-and-forget and ordering doesn't matter, and if the runtime
/// is already shutting down the task is dropped (the restart clears the stale entry).
struct AutoSearchGuard {
    state: Arc<AppState>,
    key: String,
}

impl Drop for AutoSearchGuard {
    fn drop(&mut self) {
        let state = self.state.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            state.search_queue.mark_inactive(&key).await;
        });
    }
}

/// Auto-search for a season of episodes in a background retry loop.
///
/// Called internally by `POST /api/search` when `mode="season"`. Searches each
/// plugin's `auto_search`, parses each result's title for episode coverage,
/// downloads the best-scoring result per matched episode, then waits 5 minutes and
/// repeats for still-missing episodes until all are found or a round finds nothing new.
///
/// Returns 202 Accepted immediately; downloads happen in the background and the
/// frontend polls /api/queue. A long operation (30+ minutes) returning 202 avoids
/// HTTP timeouts while keeping the UI responsive.
pub async fn auto_search_season(
    state: Arc<AppState>,
    payload: jumbie_shared::types::AutoSearchSeasonPayload,
) -> Result<StatusCode, AppError> {
    tracing::debug!(
        "auto_search_season called: series_id={}, season={}, {} episodes",
        payload.series_id,
        payload.season,
        payload.episode_numbers.len()
    );
    let series_id = payload.series_id.clone();

    // Dedup check (no insert yet): all fallible DB work happens between here and the
    // spawn. Inserting here would leak the key in search_queue if a query failed with
    // `?`, blocking future searches; instead the key is marked just before the spawn
    // and the AutoSearchGuard inside it cleans up on exit or panic.
    let search_key = format!("{}:{}", series_id, payload.season);
    if state.search_queue.is_active(&search_key).await {
        return Err(AppError::Conflict(format!(
            "Auto-search already running for series {} season {}",
            series_id, payload.season
        )));
    }

    // Everything the 'static spawn closure needs is extracted up front — DB lookups
    // inside the spawn would require complex lifetime management.
    let (
        _mapping_path,
        qb_category,
        min_score,
        merged_scoring,
        pack_strategy,
        pack_threshold,
        metadata_providers,
        ordering_mode,
        target_title,
        mapping_rule,
        quality_profile_name,
    ) = {
        let mapping = state
            .db
            .get_series_mapping(&series_id)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
            .ok_or_else(|| AppError::NotFound(format!("Series {} not found", series_id)))?;

        let profile_id = mapping.release_profile.as_deref().unwrap_or("").to_string();
        let profile_scoring = state
            .db
            .get_release_profile(&profile_id)
            .await
            .unwrap_or(None)
            .unwrap_or_default();

        let min_score = profile_scoring.min_score;
        let merged_scoring = mapping.get_merged_scoring(&profile_scoring);

        let (strategy, threshold, global_absolute) = {
            let config = state.cfg.read().await;
            (
                config.general.season_pack_strategy.clone(),
                config.general.season_pack_replace_threshold,
                config.general.absolute_numbering,
            )
        };

        let instance_plugins = crate::utils::metadata::instance_plugin_id_map(&state.db).await;
        let ordered_instances = state
            .plugin_manager
            .read()
            .await
            .ordered_metadata_providers();
        let metadata_providers = crate::utils::metadata::ordered_series_providers(
            &mapping,
            &ordered_instances,
            &instance_plugins,
        );
        let ordering_mode = if mapping.settings.active_mode(global_absolute).is_absolute() {
            "absolute"
        } else {
            "normal"
        }
        .to_string();

        (
            mapping.settings.path.clone().unwrap_or_default(),
            mapping
                .qb_category
                .clone()
                .unwrap_or_else(|| "Series".to_string()),
            min_score,
            merged_scoring,
            strategy,
            threshold,
            metadata_providers,
            ordering_mode,
            mapping.target_title.clone(),
            mapping.clone(),
            mapping.quality_profile.clone().unwrap_or_default(),
        )
    };

    let remaining_episodes = payload.episode_numbers.clone();
    let season_str = payload.season.clone();
    let season_num: i32 = jumbie_shared::mapping::parse_season_num(&season_str)
        .ok_or_else(|| AppError::BadRequest(format!("Invalid season number: {}", season_str)))?;

    let pm_opt = state.plugin_manager.clone();

    // SSoT: effective numbering-mode default — read once; series tristate
    // overrides fall back to this (used before and inside the spawn below).
    let (global_absolute, global_format, global_format_absolute) = {
        let c = state.cfg.read().await;
        (
            c.general.absolute_numbering,
            c.organization.search_format.clone(),
            c.organization.search_format_absolute.clone(),
        )
    };

    let db_seasons = state
        .db
        .get_series_seasons(&series_id)
        .await
        .unwrap_or_default();
    let suppressed: std::collections::HashSet<i32> = state
        .db
        .get_suppressed_seasons(&series_id, global_absolute as i32)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    // Configured episode cells are materialized (as DB rows) before searching.
    let normalized_seasons = crate::api_routes::series::build_season_list(
        db_seasons,
        mapping_rule
            .settings
            .season_for_active_mode(global_absolute)
            .values(),
        &suppressed,
    );

    if let Err(e) = state
        .db
        .ensure_episode_cells(&mapping_rule, &normalized_seasons, global_absolute)
        .await
    {
        tracing::warn!(
            "Failed to ensure episode cells for series '{}': {}",
            target_title,
            e
        );
    }

    // Marked active only now (after all fallible work): every line above can return
    // `?`, and marking earlier would leak the key. The AutoSearchGuard in the spawn
    // removes it on exit or panic.
    state.search_queue.mark_active(&search_key).await;

    let guard_key = search_key.clone();
    tokio::spawn(async move {
        let _guard = AutoSearchGuard {
            state: state.clone(),
            key: guard_key,
        };
        let merged_scoring: jumbie_shared::scoring::ReleaseProfile = merged_scoring;
        // SSoT: `get_all_quality_data` returns (qualities, quality_profiles),
        // fetched once and reused across rounds.
        let (qualities, quality_profiles) = if !quality_profile_name.is_empty() {
            state.db.get_all_quality_data().await
        } else {
            (
                std::collections::HashMap::new(),
                std::collections::HashMap::new(),
            )
        };

        let mut remaining = remaining_episodes;

        // Releases rejected after a stalled download — auto-search must not
        // re-select the release that just failed.
        let rejected = state.db.get_rejected_downloads().await.unwrap_or_default();

        // Skip already-downloaded episodes when upgrades are disabled: they'd never
        // be replaced, so searching wastes indexer queries and latency.
        let replacement_on = true;
        remaining = filter_downloaded_episodes(
            &state.db,
            &series_id,
            season_num,
            &remaining,
            replacement_on,
            mapping_rule.settings.active_mode(global_absolute),
        )
        .await;

        if remaining.is_empty() {
            tracing::info!(
                "Auto search season: all episodes already downloaded, nothing to search."
            );
            // Don't return — fall through to cleanup below
        } else {
            let mut round = 0u32;
            let meta = crate::search::resolve_season_search_meta(
                &mapping_rule,
                &season_str,
                season_num,
                global_absolute,
                &global_format,
                &global_format_absolute,
            );
            let search_season_num = meta.search_season_num;
            let episode_offset = meta.episode_offset;
            // Releases are titled in SOURCE numbering; translate parsed numbers
            // back to LOCAL before matching against `remaining` / generating IDs.
            // SSoT inverse of the offset `build_search_payload` applies.
            let source_to_local =
                move |ep: i32| jumbie_shared::mapping::source_to_local_episode(ep, episode_offset);

            // Aliases are resolved once upfront and reused across all rounds.
            let (plugin_aliases, generic_aliases) = {
                let pm = pm_opt.read().await;
                crate::search::resolve_search_aliases(
                    &pm,
                    &mapping_rule,
                    &season_str,
                    global_absolute,
                )
                .await
            };

            loop {
                round += 1;
                if remaining.is_empty() {
                    break;
                }

                tracing::debug!(
                    "Auto search season: round {}, searching for episodes {:?}",
                    round,
                    remaining,
                );

                // Every entry returned by the sources (before the quality/score/pattern
                // filters below), so the round summary can report found vs. survived.
                let mut raw_entries = 0usize;
                // Source plugins queried this round — included in the summary log.
                let mut queried_sources: Vec<String> = Vec::new();
                // Episodes in SOURCE numbering (local + season offset): what the
                // sources actually search for.
                let source_episodes =
                    crate::search::source_episode_numbers(&remaining, episode_offset);

                // Releases selected this round (final scores + harvest coverage).
                let mut candidates: Vec<ScoredCandidate> = Vec::new();

                {
                    let sources = {
                        let pm = pm_opt.read().await;
                        pm.get_plugins_by_all_capabilities(&[
                            jumbie_shared::plugin::Capability::FeedProvider,
                            jumbie_shared::plugin::Capability::AutomaticSearch,
                        ])
                    };
                    let mut futures = Vec::new();

                    for source in sources {
                        // Each source gets its source-specific aliases plus all generic
                        // ones; with none, it falls back to the target title.
                        let source_id = source.instance_id().to_string();
                        let source_name = pm_opt.read().await.get_instance_name(&source_id);
                        queried_sources.push(source_name.clone());
                        let final_aliases = crate::search::resolve_final_aliases(
                            &source_id,
                            &plugin_aliases,
                            &generic_aliases,
                            &target_title,
                        );

                        let log = crate::search::SearchLog {
                            kind: crate::search::SearchKind::AutoSeason,
                            source: source_name,
                            series_id: Some(series_id.clone()),
                            series_title: Some(target_title.clone()),
                            season: Some(search_season_num),
                            source_episodes: source_episodes.clone(),
                            aliases: final_aliases.clone(),
                        };

                        let per_source_payload = crate::search::build_search_payload(
                            &target_title,
                            search_season_num,
                            &remaining,
                            final_aliases,
                            &meta.search_format,
                            episode_offset,
                        );

                        // Delegate everything to the plugin — it owns the strategy.
                        let source_clone = source.clone();
                        let sid = source_id.clone();
                        futures.push(tokio::spawn(async move {
                            let timeout_duration = std::time::Duration::from_secs(20);
                            match tokio::time::timeout(
                                timeout_duration,
                                crate::plugins::bridge::sources::auto_search(
                                    source_clone.as_ref(),
                                    per_source_payload,
                                    &log,
                                ),
                            )
                            .await
                            {
                                Ok(Ok(entries)) => (sid.clone(), Some(entries)),
                                Ok(Err(e)) => {
                                    tracing::error!(
                                        "Auto search source {} ({}) failed: {}",
                                        log.source,
                                        source_clone.instance_id(),
                                        e
                                    );
                                    (sid.clone(), None)
                                }
                                Err(_) => {
                                    tracing::error!(
                                        "Auto search source {} ({}) timed out",
                                        log.source,
                                        source_clone.instance_id()
                                    );
                                    (sid, None)
                                }
                            }
                        }));
                    }

                    let task_results = futures::future::join_all(futures).await;

                    // Season-count total from the first provider (priority order)
                    // that has stored metadata for this season.
                    let mut total_episodes_in_season = 0;
                    for provider in &metadata_providers {
                        let season_info = state
                            .db
                            .get_metadata_season_cache(
                                &provider.metadata_id,
                                &provider.plugin_id,
                                &provider.instance_id,
                                &ordering_mode,
                            )
                            .await
                            .unwrap_or_default();
                        if let Some(count) = season_info.iter().find(|s| s.0 == season_str.as_str())
                        {
                            total_episodes_in_season = count.1;
                            break;
                        }
                    }

                    let current_episodes = state
                        .db
                        .get_episodes_for_series_season(
                            &series_id,
                            season_num,
                            mapping_rule
                                .settings
                                .active_mode(global_absolute)
                                .is_absolute(),
                        )
                        .await
                        .unwrap_or_default();
                    let current_score_map: std::collections::HashMap<i32, i32> = current_episodes
                        .iter()
                        .map(|e| (e.episode, e.score.unwrap_or(0)))
                        .collect();

                    let mut scored_entries = Vec::new();
                    let mut best_individual_scores: std::collections::HashMap<i32, i32> =
                        std::collections::HashMap::new();

                    // Season-scoped patterns depend only on the season, not on the
                    // individual result, so resolve them once per round instead of
                    // once per entry.
                    let season_override = mapping_rule
                        .settings
                        .find_season_override(&season_str, global_absolute);
                    let effective_patterns = crate::search::resolve_effective_patterns(
                        season_override,
                        &mapping_rule.settings.reg_patterns,
                    );

                    for res in task_results {
                        if let Ok((sid, Some(entries))) = res {
                            let instance_id = sid.as_str();
                            tracing::debug!(
                                "Auto search season: source {} returned {} result(s)",
                                sid,
                                entries.len()
                            );
                            for entry in entries {
                                if crate::release_checks::is_rejected(
                                    &rejected,
                                    entry.link.as_deref(),
                                    entry.download_id.as_deref(),
                                ) {
                                    tracing::debug!(
                                        "Auto search season: skipping rejected release '{}'",
                                        entry.title
                                    );
                                    continue;
                                }
                                raw_entries += 1;
                                // Episode count is needed before scoring so
                                // size_score_per_gb can normalize per episode.
                                let (ep_nums, is_season_pack) =
                                    if jumbie_shared::mapping::has_non_empty(effective_patterns) {
                                        let season_num =
                                            jumbie_shared::mapping::parse_season_num(&season_str);
                                        match parse_title_with_custom_regex(
                                            &entry.title,
                                            effective_patterns,
                                            season_num,
                                            mapping_rule
                                                .settings
                                                .active_mode(global_absolute)
                                                .is_absolute(),
                                            Some(instance_id),
                                        ) {
                                            CustomParseResult::Extracted(extracted) => {
                                                // Propagate pack status (season-only-group
                                                // patterns) and translate SOURCE episode
                                                // numbers to LOCAL.
                                                let is_pack = extracted.is_season_pack
                                                    || extracted.is_complete_pack;
                                                let nums: Vec<i32> = if is_pack {
                                                    Vec::new()
                                                } else {
                                                    extracted
                                                        .episodes
                                                        .iter()
                                                        .map(|&e| source_to_local(e))
                                                        .collect()
                                                };
                                                (nums, is_pack)
                                            }
                                            CustomParseResult::MatchedFilter => {
                                                // Pattern matched but no named groups —
                                                // use default parsing, then keep only
                                                // episodes that pass `matches_result`.
                                                let parsed =
                                                    crate::search::parse_search_result_title(
                                                        &entry.title,
                                                        None,
                                                    );
                                                let is_pack = parsed
                                                    .as_ref()
                                                    .map(|p| p.is_season_pack || p.is_complete_pack)
                                                    .unwrap_or(false);
                                                let nums = if !is_pack {
                                                    let release = meta.match_release(&entry.title);
                                                    parse_episode_numbers_for_season(
                                                        &entry.title,
                                                        None,
                                                    )
                                                    .into_iter()
                                                    .filter(|&e| release.accepts(e))
                                                    .map(source_to_local)
                                                    .collect()
                                                } else {
                                                    Vec::new()
                                                };
                                                (nums, is_pack)
                                            }
                                            CustomParseResult::NoMatch => {
                                                // Patterns only gate automatic searches; a
                                                // release that fails to match is dropped here —
                                                // the same release still shows in manual search,
                                                // which never applies patterns.
                                                tracing::trace!(
                                                    "Auto search season: skipping '{}' — did not match the season patterns",
                                                    entry.title
                                                );
                                                continue;
                                            }
                                        }
                                    } else {
                                        // No patterns defined — use default parsing,
                                        // then keep only episodes that pass
                                        // `matches_result`.
                                        let parsed = crate::search::parse_search_result_title(
                                            &entry.title,
                                            None,
                                        );
                                        let is_pack = parsed
                                            .as_ref()
                                            .map(|p| p.is_season_pack || p.is_complete_pack)
                                            .unwrap_or(false);
                                        let nums = if !is_pack {
                                            let release = meta.match_release(&entry.title);
                                            parse_episode_numbers_for_season(&entry.title, None)
                                                .into_iter()
                                                .filter(|&e| release.accepts(e))
                                                .map(source_to_local)
                                                .collect()
                                        } else {
                                            Vec::new()
                                        };
                                        (nums, is_pack)
                                    };

                                // SSoT for how many episodes this release counts as when
                                // scoring normalizes size_score_per_gb per episode.
                                let episode_count = episode_count_for_scoring(
                                    is_season_pack,
                                    ep_nums.len(),
                                    total_episodes_in_season,
                                );

                                let (base_score, _) = merged_scoring.calculate_with_submitter(
                                    &entry.title,
                                    entry.size.unwrap_or(0),
                                    entry.seeders.unwrap_or(0),
                                    entry.published,
                                    episode_count,
                                    entry.submitter.as_deref(),
                                );
                                let mut score = base_score;

                                let config_guard = state.cfg.read().await;
                                if is_season_pack {
                                    score += config_guard.general.effective_pack_score_modifier();
                                }
                                if config_guard.general.automatic_profiles.enabled {
                                    let submitter = entry
                                        .submitter
                                        .clone()
                                        .unwrap_or_else(|| "Unknown".to_string());
                                    if let Ok(Some(profile)) =
                                        state.db.get_automatic_profile(&submitter).await
                                    {
                                        score += profile.score;
                                    }
                                }
                                drop(config_guard);

                                // Quality profile filter (SSoT: `crate::release_checks::quality_failure`)
                                if crate::release_checks::quality_failure(
                                    &entry.title,
                                    Some(&quality_profile_name),
                                    &quality_profiles,
                                    &qualities,
                                )
                                .is_some()
                                {
                                    tracing::trace!(
                                        "Auto search season: skipping '{}' — quality not in profile '{}'",
                                        entry.title,
                                        quality_profile_name
                                    );
                                    continue;
                                }

                                // Upgrade-target restriction: for entries that would replace
                                // an existing download (non-zero previous score), verify the
                                // profile's upgrade_only_qualities (SSoT:
                                // `crate::release_checks::upgrade_failure`).
                                if !is_season_pack {
                                    let is_upgrade_for_any = ep_nums.iter().any(|ep| {
                                        let cs = current_score_map.get(ep).cloned().unwrap_or(0);
                                        cs > 0 && score > cs
                                    });
                                    if is_upgrade_for_any
                                        && crate::release_checks::upgrade_failure(
                                            &mapping_rule,
                                            &entry.title,
                                            &quality_profiles,
                                            &qualities,
                                        )
                                        .is_some()
                                    {
                                        tracing::trace!(
                                            "Auto search season: skipping '{}' — quality not an upgrade target in profile '{}'",
                                            entry.title,
                                            quality_profile_name
                                        );
                                        continue;
                                    }
                                }

                                // Individual alternatives pool: only true single-episode
                                // releases count — a multi-release must not inflate the very
                                // total it is compared against. Recorded after the filters so
                                // a rejected single never counts as an alternative.
                                if !is_season_pack && ep_nums.len() == 1 {
                                    let ep = ep_nums[0];
                                    let current_score =
                                        current_score_map.get(&ep).cloned().unwrap_or(0);
                                    if remaining.contains(&ep) || score > current_score {
                                        let best =
                                            best_individual_scores.entry(ep).or_insert(i32::MIN);
                                        if score > *best {
                                            *best = score;
                                        }
                                    }
                                }

                                scored_entries.push((entry, score, is_season_pack, ep_nums));
                            }
                        }
                    }

                    // A multi-release (season pack or episode range) is compared against
                    // the individual alternatives for the episodes it covers. Only episodes
                    // that are monitored participate; the season-override range is already
                    // enforced through `monitored`/`out_of_range`.
                    let is_valid_episode = |ep_num: i32| -> bool {
                        let db_info = current_episodes.iter().find(|e| e.episode == ep_num);
                        let is_monitored = db_info.map(|e| e.monitored).unwrap_or(true);
                        let is_explicit_out_of_range = db_info.and_then(|e| e.status.as_deref())
                            >= Some(EpisodeStatus::OutOfRange.as_str());
                        is_monitored && !is_explicit_out_of_range
                    };

                    for (entry, base_score, is_season_pack, ep_nums) in scored_entries {
                        // Full physical coverage of the release in local numbering. A season
                        // pack covers the whole season; when its episode count is unknown it
                        // still covers every episode being searched for.
                        let coverage_full: Vec<i32> = if is_season_pack {
                            let season: Vec<i32> = if total_episodes_in_season > 0 {
                                (1..=total_episodes_in_season).collect()
                            } else {
                                remaining.clone()
                            };
                            season
                                .into_iter()
                                .filter(|&ep| is_valid_episode(ep))
                                .collect()
                        } else {
                            ep_nums
                                .iter()
                                .copied()
                                .filter(|&ep| is_valid_episode(ep))
                                .collect()
                        };

                        if coverage_full.len() <= 1 {
                            // Single episode (or unparseable): no combined comparison.
                            if base_score < min_score && base_score >= 0 {
                                continue;
                            }
                            candidates.push(ScoredCandidate {
                                result: entry.to_search_result(base_score, is_season_pack),
                                claim: coverage_full,
                                unneeded: Vec::new(),
                            });
                            continue;
                        }

                        // Per-episode alternatives: a single-episode search result or an
                        // existing local file, whichever scores higher. `None` means the
                        // episode can only be obtained through this multi-release.
                        let coverage: Vec<crate::source_processor::EpisodeAlt> = coverage_full
                            .iter()
                            .map(|&ep| {
                                let single = best_individual_scores.get(&ep).copied();
                                let local = current_score_map.get(&ep).copied().filter(|s| *s > 0);
                                let alternative = match (single, local) {
                                    (Some(a), Some(b)) => Some(a.max(b)),
                                    (Some(a), None) => Some(a),
                                    (None, Some(b)) => Some(b),
                                    (None, None) => None,
                                };
                                crate::source_processor::EpisodeAlt {
                                    missing: remaining.contains(&ep),
                                    alternative,
                                }
                            })
                            .collect();

                        match crate::source_processor::decide_multi_release(
                            base_score,
                            &coverage,
                            pack_threshold,
                            &pack_strategy,
                        ) {
                            crate::source_processor::MultiDecision::Drop => continue,
                            crate::source_processor::MultiDecision::Win { score, branch } => {
                                use crate::source_processor::MultiBranch;
                                // Necessity and gap-fill claim only the missing episodes
                                // (the rest is overspill to discard); a competitive
                                // replacement claims the whole coverage.
                                let (claim, unneeded) = match branch {
                                    MultiBranch::CompetitiveReplace => {
                                        (coverage_full.clone(), Vec::new())
                                    }
                                    MultiBranch::Necessity | MultiBranch::CompetitiveFillGap => (
                                        coverage_full
                                            .iter()
                                            .copied()
                                            .filter(|ep| remaining.contains(ep))
                                            .collect(),
                                        coverage_full
                                            .iter()
                                            .copied()
                                            .filter(|ep| !remaining.contains(ep))
                                            .collect(),
                                    ),
                                };
                                if claim.is_empty() {
                                    continue;
                                }
                                // A necessity win is the only way to obtain an episode, so
                                // min_score must not filter it out.
                                if branch != MultiBranch::Necessity
                                    && score < min_score
                                    && score >= 0
                                {
                                    continue;
                                }
                                candidates.push(ScoredCandidate {
                                    result: entry.to_search_result(score, is_season_pack),
                                    claim,
                                    unneeded,
                                });
                            }
                        }
                    }
                }

                // Mirrors the manual-search summary so the two paths can be compared when
                // diagnosing a rejection. The exact query is logged per source by the
                // bridge; "valid" = survived the quality profile, upgrade-target,
                // pattern, and min_score filters.
                tracing::info!(
                    "Auto search season: round {} series='{}' season={} source_episodes={:?} aliases={:?} sources={:?} — {} valid of {} found",
                    round,
                    target_title,
                    search_season_num,
                    source_episodes,
                    generic_aliases,
                    queried_sources,
                    candidates.len(),
                    raw_entries,
                );

                // score ↓ → pack strategy (user preference) → date ↓ → seeders ↓.
                candidates.sort_by(|a, b| {
                    let (ra, rb) = (a.result.rank(), b.result.rank());
                    rb.score
                        .cmp(&ra.score)
                        .then_with(|| {
                            jumbie_shared::types::compare_multi_preference(
                                a.result.is_season_pack,
                                b.result.is_season_pack,
                                &pack_strategy,
                            )
                        })
                        .then_with(|| {
                            jumbie_shared::types::compare_release_keys_desc(
                                ra,
                                rb,
                                &jumbie_shared::types::RELEASE_RANK_KEYS[1..],
                            )
                        })
                });

                // First (best) candidate to claim each remaining episode.
                let mut best_per_episode: std::collections::HashMap<i32, usize> =
                    std::collections::HashMap::new();
                for (idx, candidate) in candidates.iter().enumerate() {
                    for ep in &candidate.claim {
                        if remaining.contains(ep) && !best_per_episode.contains_key(ep) {
                            best_per_episode.insert(*ep, idx);
                        }
                    }
                }

                if best_per_episode.is_empty() {
                    tracing::info!(
                        "Auto search season: round {} found no new results for episodes {:?}. Stopping.",
                        round,
                        remaining
                    );
                    // If no source returned a result matching any remaining episode,
                    // retrying the same query won't help (releases may not exist yet,
                    // e.g. pre-air), so stop rather than loop forever.
                    break;
                }

                let mut submitted_links: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut found_episodes: Vec<i32> = Vec::new();

                for (ep_num, idx) in &best_per_episode {
                    let candidate = &candidates[*idx];
                    let result = &candidate.result;
                    let dl_link = result
                        .link
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_default()
                        .to_string();

                    if dl_link.is_empty() {
                        tracing::warn!(
                            "Auto search season: no link for '{}' (ep {}), skipping",
                            result.title,
                            ep_num
                        );
                        continue;
                    }

                    // Deduplicate links: one season pack covers multiple episodes, and
                    // without this we'd add the same magnet link once per covered episode.
                    if submitted_links.contains(&dl_link) {
                        tracing::debug!(
                            "Auto search season: ep {} covered by already-submitted release",
                            ep_num
                        );
                        found_episodes.push(*ep_num);
                        continue;
                    }

                    let season_i32: i32 = season_num;
                    let absolute_numbering = mapping_rule
                        .settings
                        .active_mode(global_absolute)
                        .is_absolute();

                    // Claimed episodes (keep) — the coverage the decision selected.
                    let episodes_for_enqueue: Vec<i32> = if candidate.claim.is_empty() {
                        vec![*ep_num]
                    } else {
                        candidate.claim.clone()
                    };
                    let episode_ids: Vec<String> = episodes_for_enqueue
                        .iter()
                        .map(|&ep| {
                            if absolute_numbering {
                                jumbie_shared::formatting::fmt_absolute_episode_id(
                                    ep,
                                    &mapping_rule.series_id,
                                )
                            } else {
                                jumbie_shared::formatting::fmt_episode_id_num(
                                    season_num,
                                    ep,
                                    &mapping_rule.series_id,
                                )
                            }
                        })
                        .collect();

                    // Harvest policy: keep the claimed episodes, discard the overspill.
                    let intentions = build_episode_intentions(
                        &episodes_for_enqueue,
                        &candidate.unneeded,
                        season_num,
                        episode_offset,
                        absolute_numbering,
                        &mapping_rule.series_id,
                        result.score,
                    );

                    match crate::api_routes::system::search_core::queue_search_result(
                        crate::api_routes::system::search_core::QueueSearchParams {
                            state: &state,
                            result,
                            series_id: &series_id,
                            series_title: &target_title,
                            seasons: &[season_i32],
                            episodes: &episodes_for_enqueue,
                            episode_ids: &episode_ids,
                            is_user_requested: payload.is_user_requested,
                            is_season_pack: result.is_season_pack,
                            score: result.score,
                            category: &qb_category,
                            episode_intentions: Some(&intentions),
                        },
                    )
                    .await
                    {
                        Ok(_) => {
                            tracing::debug!(
                                "Auto search season: started download for ep {} → '{}'",
                                ep_num,
                                result.title
                            );
                            submitted_links.insert(dl_link);
                            found_episodes.push(*ep_num);

                            // Mark every claimed episode as found so the same release
                            // isn't downloaded again for a sibling episode.
                            for covered_ep in &episodes_for_enqueue {
                                if remaining.contains(covered_ep)
                                    && !found_episodes.contains(covered_ep)
                                {
                                    found_episodes.push(*covered_ep);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "Auto search season: download failed for ep {}: {}",
                                ep_num,
                                e
                            );
                            // Continue on failure so one episode's failure doesn't block
                            // the others; the failed one is retried next round.
                        }
                    }
                }

                remaining.retain(|ep| !found_episodes.contains(ep));

                tracing::debug!(
                    "Auto search season: round {} complete — downloaded {} episodes, {} still missing",
                    round,
                    found_episodes.len(),
                    remaining.len()
                );

                if remaining.is_empty() {
                    tracing::info!(
                        "Auto search season: all episodes found after round {}.",
                        round
                    );
                    break;
                }

                tracing::debug!(
                    "Auto search season: waiting 5 minutes before retrying for {:?}",
                    remaining
                );
                // Releases may not immediately be on all indexers; this gives them time
                // to propagate while avoiding hammering indexers. A season typically
                // resolves in 2-3 rounds.
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            }
        }
        // _guard dropped here → AutoSearchGuard::drop marks key inactive in search_queue
    });

    Ok(StatusCode::ACCEPTED)
}

/// Response type for the auto-search status endpoint.
#[derive(Serialize)]
pub struct AutoSearchStatus {
    pub running: bool,
}

/// Check whether an auto-search season task is currently running for a given
/// series + season combination.
///
/// A lightweight endpoint (no DB queries, just a HashMap lookup) the frontend polls
/// while showing a "Searching..." pending state, re-enabling the button when
/// `running` becomes `false`.
pub async fn get_auto_season_status(
    State(state): State<Arc<AppState>>,
    Path((series_id, season)): Path<(String, String)>,
) -> Result<Json<AutoSearchStatus>, AppError> {
    crate::validation::validate_season_number(&season).map_err(|e| {
        tracing::debug!("get_auto_season_status: invalid season '{}': {}", season, e);
        AppError::BadRequest(e.0)
    })?;
    let search_key = format!("{}:{}", series_id, season);
    let running = state.search_queue.is_active(&search_key).await;
    Ok(Json(AutoSearchStatus { running }))
}

#[cfg(test)]
mod tests;
