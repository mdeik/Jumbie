use anyhow::Result;
use std::cmp::Ordering;

use crate::organizer::ContentOrganizer;
use jumbie_shared::config::SeasonPackStrategy;
use jumbie_shared::types::{EpisodeInfo, MappingRule};

/// Whether a pack covering `needed_count` of its `total_episodes` needed episodes
/// clears the consistency replacement threshold.
///
/// SSoT for the "replace existing downloads" decision: used both by the source-queue
/// scoring path ([`pack_penalty`]) and by the auto-season search. A pack with no
/// episodes never replaces.
pub(crate) fn pack_meets_replacement_threshold(
    total_episodes: usize,
    needed_count: usize,
    threshold: u32,
) -> bool {
    total_episodes > 0 && (needed_count as f64 / total_episodes as f64) * 100.0 >= threshold as f64
}

/// Per-episode classification of a multi-episode candidate's coverage, used by
/// [`decide_multi_release`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EpisodeAlt {
    /// The episode is still missing (being searched for).
    pub missing: bool,
    /// Best score of an individual alternative for this episode — a single-episode
    /// search result or an existing local file — or `None` when the episode can only
    /// be obtained via this multi-release.
    pub alternative: Option<i32>,
}

/// Which path a winning multi-release took, so the caller derives the harvest policy
/// (which episodes to claim vs mark unneeded) without re-deriving the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MultiBranch {
    /// A covered episode is missing with no alternative: the multi is the only way to
    /// obtain it, so it wins regardless of score. Claim only the needed episodes.
    Necessity,
    /// Every covered episode has an alternative and the threshold is met: the multi
    /// replaces its whole coverage when its combined score wins.
    CompetitiveReplace,
    /// Threshold not met: the multi may only fill the missing episodes.
    CompetitiveFillGap,
}

/// Outcome of comparing a multi-release against its individual alternatives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MultiDecision {
    /// The individual alternatives are (strictly) better; the multi is not used.
    Drop,
    /// The multi won; use `score` and harvest according to `branch`.
    Win { score: i32, branch: MultiBranch },
}

/// SSoT for automatic multi-release (season pack / episode range) selection.
///
/// `coverage` is the candidate's covered episodes (local numbering) with their
/// per-episode alternatives. A multi wins when it is the only way to obtain a missing
/// episode (necessity), or when its combined value — `base_score × episodes` — is at
/// least the sum of the individual alternatives; an exact tie is broken by the
/// season-pack strategy. Otherwise it is dropped. This function is pure so it can be
/// unit-tested independently of the email/queue plumbing.
pub(crate) fn decide_multi_release(
    base_score: i32,
    coverage: &[EpisodeAlt],
    threshold: u32,
    strategy: &SeasonPackStrategy,
) -> MultiDecision {
    // 1) Coverage necessity: an obtainable-only-here missing episode forces the multi.
    if coverage
        .iter()
        .any(|e| e.missing && e.alternative.is_none())
    {
        return MultiDecision::Win {
            score: base_score * coverage.len() as i32,
            branch: MultiBranch::Necessity,
        };
    }

    // 2) Competitive replacement vs gap-fill. Every covered episode has an alternative
    //    here, so dropping a loser can never strand an episode.
    let covered = coverage.len();
    let needed = coverage.iter().filter(|e| e.missing).count();
    let replace_on = pack_meets_replacement_threshold(covered, needed, threshold);

    let (episodes, total, branch) = if replace_on {
        (
            covered,
            coverage.iter().map(|e| e.alternative.unwrap_or(0)).sum(),
            MultiBranch::CompetitiveReplace,
        )
    } else {
        (
            needed,
            coverage
                .iter()
                .filter(|e| e.missing)
                .map(|e| e.alternative.unwrap_or(0))
                .sum(),
            MultiBranch::CompetitiveFillGap,
        )
    };

    if episodes == 0 {
        return MultiDecision::Drop;
    }

    let combined = base_score * episodes as i32;
    match combined.cmp(&total) {
        Ordering::Greater => MultiDecision::Win {
            score: combined,
            branch,
        },
        Ordering::Less => MultiDecision::Drop,
        Ordering::Equal => match strategy {
            SeasonPackStrategy::FavorSeasonPacks => MultiDecision::Win {
                score: combined,
                branch,
            },
            SeasonPackStrategy::FavorEpisodes => MultiDecision::Drop,
        },
    }
}

/// SSoT: shared pack penalty computation. Returns (penalty, triggered_replacement).
pub(crate) fn pack_penalty(
    total_episodes: f64,
    needed_count: usize,
    unneeded: i32,
    is_pack: bool,
    strategy: &SeasonPackStrategy,
    threshold: u32,
) -> (i32, bool) {
    let mut unneeded = unneeded;
    let mut replaced = false;

    if is_pack && pack_meets_replacement_threshold(total_episodes as usize, needed_count, threshold)
    {
        replaced = true;
        unneeded = 0;
    }

    let base_penalty = if *strategy == SeasonPackStrategy::FavorSeasonPacks {
        10
    } else {
        50
    };

    (-(unneeded * base_penalty), replaced)
}

impl ContentOrganizer {
    /// Resolve episode count for a single season (cell_count → DB → metadata → None).
    async fn resolve_season_episode_count(
        &self,
        season_num: i32,
        mapping: &MappingRule,
        global_absolute: bool,
    ) -> Result<Option<i32>> {
        // SSoT: one mode-aware season-override lookup, tolerant of "2"/"02"/"S02" keys.
        let cell_count = mapping
            .settings
            .find_season_override(&season_num.to_string(), global_absolute)
            .and_then(|o| o.cell_count)
            .filter(|c| *c > 0);

        if let Some(cc) = cell_count {
            return Ok(Some(cc));
        }

        let db_count = self
            .db
            .count_episodes_for_season(&mapping.series_id, season_num)
            .await
            .unwrap_or(0);
        if db_count > 0 {
            return Ok(Some(db_count));
        }

        // Metadata season keys are zero-padded ("01", "02", …).
        let padded = format!("{:02}", season_num);
        // Iterate providers in PRIORITY order (never `HashMap` order) — the same
        // order `fetch_metadata_for_series` writes in.
        let instance_plugins = crate::utils::metadata::instance_plugin_id_map(&self.db).await;
        let ordered_instances = self
            .plugin_manager
            .read()
            .await
            .ordered_metadata_providers();
        for provider in crate::utils::metadata::ordered_series_providers(
            mapping,
            &ordered_instances,
            &instance_plugins,
        ) {
            if let Ok(seasons) = self
                .db
                .get_metadata_season_cache(
                    &provider.metadata_id,
                    &provider.plugin_id,
                    &provider.instance_id,
                    "normal",
                )
                .await
            {
                for (s, count) in &seasons {
                    if s == &padded && *count > 0 {
                        return Ok(Some(*count));
                    }
                }
            }
        }

        Ok(None)
    }

    pub async fn calculate_pack_score(
        &self,
        info: &EpisodeInfo,
        mapping: &MappingRule,
        global_absolute: bool,
    ) -> Result<(i32, Vec<i32>, i32)> {
        // Season the "needed in this season" review list is anchored to (see
        // `season1_needed` below). Absolute numbering is canonically season 1
        // (ABSOLUTE_SEASON_NUM), and the no-season fallback further down builds its
        // episode IDs for season 1 as well, so the list matches those IDs.
        let season_start = info
            .seasons
            .first()
            .copied()
            .unwrap_or(jumbie_shared::mapping::ABSOLUTE_SEASON_NUM);

        // Build episode_ids directly from the seasons/episodes vecs. When
        // `episodes` is non-empty use those exact numbers (gaps handled — [2,4,6]
        // yields three); when empty (season pack) resolve counts per season.
        let mut ep_status: std::collections::HashMap<String, (i32, i32)> =
            std::collections::HashMap::new();
        let mut ep_ids = Vec::new();

        if !info.episodes.is_empty() {
            for &s in &info.seasons {
                let s_str = format!("{:02}", s);
                for &ep in &info.episodes {
                    let ep_id = mapping.get_episode_id(&s_str, ep, global_absolute)?;
                    ep_status.insert(ep_id.clone(), (s, ep));
                    ep_ids.push(ep_id);
                }
            }
        } else if !info.seasons.is_empty() {
            // Season pack(s) — resolve episode count per season with average estimation
            let mut known_sum = 0i32;
            let mut known_count = 0i32;
            let mut season_counts: Vec<(i32, Option<i32>)> = Vec::new();
            for &s in &info.seasons {
                let c = self
                    .resolve_season_episode_count(s, mapping, global_absolute)
                    .await?;
                if let Some(count) = c {
                    known_sum += count;
                    known_count += 1;
                }
                season_counts.push((s, c));
            }
            let avg = if known_count > 0 {
                known_sum / known_count
            } else {
                12
            };
            for (s, c) in &season_counts {
                let count = c.unwrap_or(avg);
                let s_str = format!("{:02}", s);
                for ep in 1..=count {
                    let ep_id = mapping.get_episode_id(&s_str, ep, global_absolute)?;
                    ep_status.insert(ep_id.clone(), (*s, ep));
                    ep_ids.push(ep_id);
                }
            }
        } else {
            // Fallback: no seasons, no episodes (complete series pack with no metadata)
            let s = 1;
            let count = self
                .resolve_season_episode_count(s, mapping, global_absolute)
                .await?
                .unwrap_or(12);
            let s_str = format!("{:02}", s);
            for ep in 1..=count {
                let ep_id = mapping.get_episode_id(&s_str, ep, global_absolute)?;
                ep_status.insert(ep_id.clone(), (s, ep));
                ep_ids.push(ep_id);
            }
        }

        // Determine needed vs unneeded by episode_id
        let total_episodes = ep_ids.len() as f64;
        let statuses = self.db.get_episodes_status_batch(&ep_ids).await?;

        let mut needed_season_ep: Vec<(i32, i32)> = Vec::new();
        let mut unneeded = 0i32;

        for ep_id in &ep_ids {
            let &(s, ep) = ep_status.get(ep_id).unwrap();
            if let Some((downloaded, monitored)) = statuses.get(ep_id) {
                if *downloaded || !*monitored {
                    unneeded += 1;
                } else {
                    needed_season_ep.push((s, ep));
                }
            } else {
                // Episode not in DB — counts as unmonitored, thus unneeded
                unneeded += 1;
            }
        }

        // Shared threshold + penalty SSoT.
        let general_cfg = self.db.get_general_config().await.unwrap_or_default();
        let strategy = general_cfg.season_pack_strategy;
        let threshold = general_cfg.season_pack_replace_threshold;
        let is_pack = info.is_season_pack || info.is_complete_pack;

        let (penalty, replaced) = pack_penalty(
            total_episodes,
            needed_season_ep.len(),
            unneeded,
            is_pack,
            &strategy,
            threshold,
        );

        if replaced {
            needed_season_ep.clear();
            // Rebuild from ep_status which holds (season, episode) for all intended episodes
            for &(s, ep) in ep_status.values() {
                needed_season_ep.push((s, ep));
            }
            unneeded = 0;
        }

        // Return season 1's needed episodes flat. select_winners groups by
        // (series, season_str) and checks c.needed_episodes.contains(ep); multi-season
        // packs live in their first season's group, so only that season's numbers apply.
        let mut season1_needed: Vec<i32> = needed_season_ep
            .iter()
            .filter(|&&(s, _)| s == season_start)
            .map(|&(_, ep)| ep)
            .collect();
        season1_needed.sort_unstable();

        Ok((penalty, season1_needed, unneeded))
    }
}

impl ContentOrganizer {
    pub(crate) async fn get_effective_min_score(&self, profile_id: &str) -> i32 {
        self.db
            .get_effective_min_score(profile_id)
            .await
            .unwrap_or(0)
    }

    /// Test shim only; production uses `calculate_pack_score`.
    pub fn assess_pack_candidacy(
        info: &EpisodeInfo,
        downloaded_eps: &std::collections::HashSet<i32>,
        monitored_eps: &std::collections::HashSet<i32>,
        adjusted_end: Option<i32>,
        strategy: &SeasonPackStrategy,
        threshold: u32,
    ) -> (i32, Vec<i32>, i32) {
        let first_ep = info.episodes.first().copied().unwrap_or(1);
        if adjusted_end.is_none() {
            if downloaded_eps.contains(&first_ep) || !monitored_eps.contains(&first_ep) {
                return (-1000, vec![], 1);
            }
            return (0, vec![first_ep], 0);
        }

        let start = first_ep;
        let end = adjusted_end.unwrap();
        let mut needed = Vec::new();
        let mut unneeded = 0;
        let total = (end - start + 1) as f64;

        for ep in start..=end {
            if downloaded_eps.contains(&ep) || !monitored_eps.contains(&ep) {
                unneeded += 1;
            } else {
                needed.push(ep);
            }
        }

        let is_pack = info.is_season_pack || info.is_complete_pack;
        let (penalty, replaced) =
            pack_penalty(total, needed.len(), unneeded, is_pack, strategy, threshold);

        if replaced {
            needed.clear();
            for ep in start..=end {
                needed.push(ep);
            }
            unneeded = 0;
        }

        (penalty, needed, unneeded)
    }
}
