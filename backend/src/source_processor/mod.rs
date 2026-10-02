// Source processing lives on ContentOrganizer (not a separate service) because every
// helper it calls is also a ContentOrganizer method and it shares the same locked state.

pub(crate) mod identification;
mod monitoring;
mod processing;
mod scoring;

pub(crate) use identification::SeriesMatcher;
#[cfg(test)]
pub(crate) use identification::build_gate_regex;
pub(crate) use identification::lookup_mapping_in;
pub(crate) use monitoring::classify_db_episodes;
pub use monitoring::reapply_monitor_for_episode;
pub use monitoring::reapply_monitor_for_series;
pub(crate) use scoring::{EpisodeAlt, MultiBranch, MultiDecision, decide_multi_release};

use anyhow::Result;
use tracing::{debug, info, trace};

use crate::models::media::ReleaseCandidate;
use crate::organizer::ContentOrganizer;

impl ContentOrganizer {
    pub(crate) async fn select_winners(
        &self,
        candidates: Vec<ReleaseCandidate>,
    ) -> Result<Vec<ReleaseCandidate>> {
        use std::collections::HashMap;

        // SSoT: releases rejected after a stalled download are dropped before any
        // selection, so no selector re-picks one. Filtering here covers every
        // caller of `select_winners` (feed polling and auto_search_missing), which
        // is why callers must not repeat the check.
        let rejected = self.db.get_rejected_downloads().await.unwrap_or_default();
        let mut candidates: Vec<ReleaseCandidate> = candidates
            .into_iter()
            .filter(|c| {
                !crate::release_checks::is_rejected(
                    &rejected,
                    c.download_url.as_deref(),
                    c.download_id.as_deref(),
                )
            })
            .collect();

        let general_config = self.db.get_general_config().await.unwrap_or_default();
        let default_manual_score = general_config.default_score_for_manual_files;
        // SSoT: effective numbering-mode default for this batch (all episode IDs use it).
        let global_absolute = general_config.absolute_numbering;

        // SSoT: `get_all_quality_data` — all callers load upgrade-target data from one snapshot.
        let (qualities, quality_profiles) = self.db.get_all_quality_data().await;

        // Phase 1: within a release group (e.g. "MockFansub") keep only the highest
        // version — v2/v3 are assumed corrections of the earlier one. Cross-group dedup
        // happens later via score comparison.
        let mut highest_versions: HashMap<String, i32> = HashMap::new();
        for c in &candidates {
            if let Some(ref group_name) = c.episode_info.submitter {
                trace!(
                    "Phase 1 dedup: submitter={}, version={}",
                    group_name, c.episode_info.version
                );
                let max_v = highest_versions
                    .entry(group_name.clone())
                    .or_insert(c.episode_info.version);
                if c.episode_info.version > *max_v {
                    *max_v = c.episode_info.version;
                }
            }
        }
        candidates.retain(|c| {
            if let Some(ref group_name) = c.episode_info.submitter
                && let Some(&max_v) = highest_versions.get(group_name)
            {
                return c.episode_info.version == max_v;
            }
            true
        });

        // Phase 2: group candidates by (series_id, season) so conflicts resolve
        // independently per series — the display name is not unique.
        let by_season = group_candidates_by_season(candidates);

        let mut winners: Vec<ReleaseCandidate> = Vec::new();

        for ((_series_id, season_str), mut group) in by_season {
            // Canonical ranking (score ↓ → date ↓ → seeders ↓). The group comes from a
            // HashMap, so a score-only sort left the winner to iteration order on ties.
            group.sort_by(|a, b| {
                jumbie_shared::types::compare_release_rank_desc(a.rank(), b.rank())
            });

            let all_ep_nums: std::collections::HashSet<i32> = group
                .iter()
                .flat_map(|c| c.needed_episodes.iter().copied())
                .collect();

            // Active multiepisode queue items overlapping these candidates: ep_num → item.
            let mut ep_to_multi: HashMap<i32, jumbie_shared::types::DownloadQueueItem> =
                HashMap::new();
            for ep in &all_ep_nums {
                if let Ok(Some(item)) = self
                    .db
                    .get_multiepisode_queue_item_covering(&season_str, *ep)
                    .await
                {
                    ep_to_multi.insert(*ep, item);
                }
            }

            let mut multi_groups: HashMap<
                i64,
                (jumbie_shared::types::DownloadQueueItem, Vec<i32>),
            > = HashMap::new();
            for (ep, item) in &ep_to_multi {
                multi_groups
                    .entry(item.id)
                    .or_insert_with(|| (item.clone(), Vec::new()))
                    .1
                    .push(*ep);
            }

            // Phase 3: when a multi-episode item (e.g. S01E01-04) is already queued,
            // single episodes in its range stay blocked unless the ENTIRE range can be
            // replaced by a better-scoring set — preventing one episode from ending up
            // in a separate torrent. Replacement also requires an allowed upgrade and
            // the new average score to meet the release profile's min_score.
            // Tracks which multiepisode queue ids are being fully replaced
            let mut allowed_multi_replacements: std::collections::HashSet<i64> =
                std::collections::HashSet::new();
            // Which eps are "contested" by an unresolved multi replacement — block them
            let mut contested_blocked_eps: std::collections::HashSet<i32> =
                std::collections::HashSet::new();

            for (queue_id, (multi_item, _matched_eps)) in &multi_groups {
                let Some(multi_ep_start) = multi_item.episode else {
                    continue; // sentinel items have no episode range to compare
                };
                let multi_ep_end = multi_item.episode_end.unwrap_or(multi_ep_start);
                let multi_range: Vec<i32> = (multi_ep_start..=multi_ep_end).collect();

                let mut ep_to_best: HashMap<i32, &ReleaseCandidate> = HashMap::new();
                for ep in &multi_range {
                    // `group` is sorted best-first, so the first candidate containing ep is the best.
                    if let Some(best) = group.iter().find(|c| c.needed_episodes.contains(ep)) {
                        ep_to_best.insert(*ep, best);
                    }
                }

                let all_eps_covered = multi_range.iter().all(|ep| ep_to_best.contains_key(ep));

                if !all_eps_covered {
                    // Not all episodes have a candidate — block every ep in the multi's range.
                    for ep in &multi_range {
                        contested_blocked_eps.insert(*ep);
                    }
                    debug!(
                        "Cannot replace multiepisode queue item {} (S{}E{}-E{}) — not all episodes covered by candidates",
                        multi_item.media_name, season_str, multi_ep_start, multi_ep_end
                    );
                    continue;
                }

                // Compare averages (not totals) so wide packs aren't favoured by size;
                // integer division is deliberate to avoid floating-point pitfalls.
                let new_avg_score = {
                    let total: i32 = multi_range.iter().map(|ep| ep_to_best[ep].score).sum();
                    total / multi_range.len() as i32
                };

                let existing_score = multi_item.score;

                let first_candidate = ep_to_best.values().next().unwrap();

                // SSoT: same upgrade-target check as the single-episode path.
                let blocked_by_profile = jumbie_shared::quality::is_upgrade_blocked_by_profile(
                    first_candidate.mapping.as_ref(),
                    &first_candidate.title,
                    &quality_profiles,
                    &qualities,
                );

                if blocked_by_profile {
                    debug!(
                        "Cannot replace multiepisode queue item {} (S{}E{}-E{}) — quality not an upgrade target",
                        multi_item.media_name, season_str, multi_ep_start, multi_ep_end
                    );
                    for ep in &multi_range {
                        contested_blocked_eps.insert(*ep);
                    }
                    continue;
                }

                let r_profile_id = first_candidate
                    .mapping
                    .release_profile
                    .as_deref()
                    .unwrap_or("");
                let min_score = self.get_effective_min_score(r_profile_id).await;

                trace!(
                    "Phase 3: multi-ep replacement check — new_avg_score={}, existing_score={}, min_score={}",
                    new_avg_score, existing_score, min_score
                );
                if new_avg_score > existing_score && new_avg_score >= min_score {
                    info!(
                        "Replacing multiepisode queue item {} (S{}E{}-E{}, score={}) with {} candidates (avg score={})",
                        multi_item.media_name,
                        season_str,
                        multi_ep_start,
                        multi_ep_end,
                        existing_score,
                        multi_range.len(),
                        new_avg_score
                    );
                    allowed_multi_replacements.insert(*queue_id);
                    // Remove the old multiepisode from the queue and cancel its download
                    let _ = self.db.remove_from_download_queue(*queue_id).await;
                    if let Some(ref hash) = multi_item.downloader_id {
                        let download_mgr = self.downloader.read().await;
                        if let Err(e) = download_mgr
                            .delete_download(hash, true, multi_item.client_id.as_deref())
                            .await
                        {
                            tracing::warn!(
                                "Failed to cancel replaced multiepisode download: {}",
                                e
                            );
                        }
                    }
                } else {
                    for ep in &multi_range {
                        contested_blocked_eps.insert(*ep);
                    }
                    debug!(
                        "Skipping candidates for S{}E{}-E{}: new avg score {} <= existing {}",
                        season_str, multi_ep_start, multi_ep_end, new_avg_score, existing_score
                    );
                }
            }

            // Phase 4: normal single-episode logic for episodes not contested.
            // Group candidates by their "primary episode id" (first needed ep)
            let mut ep_groups: HashMap<String, Vec<ReleaseCandidate>> = HashMap::new();
            let mut all_ep_ids = Vec::new();

            for c in group {
                // For packs with empty needed_episodes (version upgrade),
                // use episode_info.episode_num as the group key so they
                // reach the upgrade gate below for evaluation.
                let first_ep = c.needed_episodes.first().copied().or({
                    if c.episode_info.is_season_pack || c.episode_info.is_complete_pack {
                        c.episode_info.episodes.first().copied()
                    } else {
                        None
                    }
                });

                if let Some(first_ep) = first_ep {
                    if c.needed_episodes
                        .iter()
                        .any(|ep| contested_blocked_eps.contains(ep))
                    {
                        continue;
                    }
                    let key = c
                        .mapping
                        .get_episode_id(&season_str, first_ep, global_absolute)?;
                    if !all_ep_ids.contains(&key) {
                        all_ep_ids.push(key.clone());
                    }
                    ep_groups.entry(key).or_default().push(c);
                }
            }

            let statuses = self.db.get_episodes_status_batch(&all_ep_ids).await?;

            for (ep_id, mut ep_group) in ep_groups {
                ep_group.sort_by(|a, b| {
                    let a_is_multi = (a.episode_info.episodes.len() > 1
                        && a.episode_info.episodes.last() > Some(&a.episode_info.episodes[0]))
                        || a.episode_info.is_season_pack
                        || a.episode_info.is_complete_pack;
                    let b_is_multi = (b.episode_info.episodes.len() > 1
                        && b.episode_info.episodes.last() > Some(&b.episode_info.episodes[0]))
                        || b.episode_info.is_season_pack
                        || b.episode_info.is_complete_pack;

                    b.score.cmp(&a.score).then_with(|| {
                        // Equal scores: the season-pack strategy decides whether the
                        // multi-release or the individual episode wins.
                        jumbie_shared::types::compare_multi_preference(
                            a_is_multi,
                            b_is_multi,
                            &general_config.season_pack_strategy,
                        )
                    })
                });

                if let Some(mut winner) = ep_group.into_iter().next() {
                    let mut should_download = false;

                    // For episodes in the DB use the stored monitor state; for newly
                    // discovered ones compute it from the series' monitor mode so the
                    // mode is enforced even before registration.
                    let (downloaded, monitored) = if let Some(&existing) = statuses.get(&ep_id) {
                        existing
                    } else {
                        // The candidate's `ep_id` (first needed episode) isn't in the DB.
                        // For packs/ranges (e.g. S01E01-E12 where E01 is unknown but E03
                        // is monitored and missing), check ALL needed episodes: if ANY
                        // would be monitored, treat the whole pack as monitored.
                        let min_ep =
                            winner.needed_episodes.iter().min().copied().unwrap_or(
                                winner.episode_info.episodes.first().copied().unwrap_or(1),
                            );
                        let ep_monitored = Self::episode_is_effectively_monitored(
                            winner.mapping.as_ref(),
                            &season_str,
                            min_ep,
                        );

                        if winner.needed_episodes.len() > 1
                            || winner.episode_info.episodes.len() > 1
                            || winner.episode_info.is_season_pack
                            || winner.episode_info.is_complete_pack
                        {
                            // Pack: at least one needed episode must be monitored
                            let any_monitored = winner.needed_episodes.iter().any(|ep| {
                                let check_id = match winner.mapping.get_episode_id(
                                    &season_str,
                                    *ep,
                                    global_absolute,
                                ) {
                                    Ok(id) => id,
                                    Err(e) => {
                                        tracing::debug!("skipping non-numeric season: {e}");
                                        return false;
                                    }
                                };
                                if let Some(&(_, mon)) = statuses.get(&check_id) {
                                    mon
                                } else {
                                    Self::episode_is_effectively_monitored(
                                        winner.mapping.as_ref(),
                                        &season_str,
                                        *ep,
                                    )
                                }
                            });
                            (false, any_monitored)
                        } else {
                            (false, ep_monitored)
                        }
                    };

                    if !monitored {
                        continue;
                    }

                    if downloaded {
                        let r_profile_id = winner.mapping.release_profile.as_deref().unwrap_or("");
                        let min_score = self.get_effective_min_score(r_profile_id).await;

                        // SSoT: same check as `organize_completed` via `is_upgrade_blocked_by_profile`.
                        let blocked_by_profile =
                            jumbie_shared::quality::is_upgrade_blocked_by_profile(
                                winner.mapping.as_ref(),
                                &winner.title,
                                &quality_profiles,
                                &qualities,
                            );

                        if blocked_by_profile {
                            trace!(
                                "Skipping upgrade for {}: {} — quality not an upgrade target in profile",
                                ep_id, winner.title
                            );
                            continue;
                        }

                        let (
                            current_score,
                            current_submitter,
                            current_version,
                            current_release_title,
                        ) = self
                            .db
                            .get_episode_release_info(&ep_id)
                            .await
                            .unwrap_or(None)
                            .unwrap_or((default_manual_score.unwrap_or(0), None, 1, None));

                        let candidate_submitter = winner.submitter.as_deref();
                        let candidate_version = winner.episode_info.version;

                        if jumbie_shared::quality::should_upgrade(
                            jumbie_shared::quality::ReleaseVersion {
                                score: current_score,
                                submitter: current_submitter.as_deref(),
                                version: current_version,
                                release_title: current_release_title.as_deref(),
                            },
                            jumbie_shared::quality::ReleaseVersion {
                                score: winner.score,
                                submitter: candidate_submitter,
                                version: candidate_version,
                                release_title: Some(&winner.title),
                            },
                            min_score,
                        ) {
                            // Season pack version upgrade: a version-bump pack replacing a
                            // complete v1 pack may have empty needed_episodes (all episodes
                            // were already downloaded). Rebuild it from the full season range
                            // so download_winner creates intentions for all episodes.
                            if winner.needed_episodes.is_empty()
                                && (winner.episode_info.is_season_pack
                                    || winner.episode_info.is_complete_pack)
                            {
                                // Submitter convention: a pack with no season is season 1
                                // (DEFAULT_SEASON_NUM); a normal-mode label must otherwise
                                // resolve numerically.
                                let season_num = jumbie_shared::mapping::resolve_season_num(
                                    &season_str,
                                    winner
                                        .mapping
                                        .settings
                                        .active_mode(global_absolute)
                                        .is_absolute(),
                                )
                                .unwrap_or(jumbie_shared::mapping::DEFAULT_SEASON_NUM);
                                let total = self
                                    .db
                                    .count_episodes_for_season(
                                        &winner.mapping.series_id,
                                        season_num,
                                    )
                                    .await
                                    .unwrap_or(0);
                                if total > 0 {
                                    winner.needed_episodes = (1..=total).collect();
                                } else {
                                    // Fallback: match calculate_pack_score's default.
                                    // Season packs always start at episode 1; default
                                    // to 12 when no DB/metadata count is available.
                                    let end =
                                        winner.episode_info.episodes.last().copied().unwrap_or(12);
                                    winner.needed_episodes = (1..=end).collect();
                                }
                            }

                            debug!(
                                "Upgrade candidate found for {}: {} (Score: {} vs Current: {})",
                                ep_id, winner.title, winner.score, current_score
                            );
                            should_download = true;
                        }
                    } else {
                        should_download = winner.score >= -900;
                        let r_profile_id = winner.mapping.release_profile.as_deref().unwrap_or("");
                        let min_score = self.get_effective_min_score(r_profile_id).await;
                        if winner.score < min_score {
                            should_download = false;
                        }
                    }

                    if should_download {
                        trace!(
                            "Winner selected: title={}, score={}",
                            winner.title, winner.score
                        );
                        winners.push(winner);
                    }
                }
            }
        }

        Ok(winners)
    }

    /// Merge winners sharing a download source into one entry with `multi_targets`,
    /// avoiding N downloads of the same file matching multiple targets.
    ///
    /// SSoT: the source URL (magnet > download_url) is the merge key; the first
    /// winner becomes primary, later ones append to its `multi_targets`.
    pub(crate) async fn merge_shared_downloads(
        &self,
        winners: Vec<ReleaseCandidate>,
    ) -> Result<Vec<ReleaseCandidate>> {
        // Resolved once: each multi-target needs the numbering mode to know whether
        // the season is canonical (absolute = season 1) or comes from the release.
        let global_absolute = self.global_absolute_default().await;
        merge_winners_by_source(winners, global_absolute)
    }
}

/// Group candidates by `(series_id, season)` for conflict resolution.
///
/// The key is the stable series id, not the display name (two distinct series can
/// share a name). Season-0 packs and single episodes with nothing needed are dropped:
/// they can never win.
pub(crate) fn group_candidates_by_season(
    candidates: Vec<ReleaseCandidate>,
) -> std::collections::HashMap<(String, String), Vec<ReleaseCandidate>> {
    let mut by_season: std::collections::HashMap<(String, String), Vec<ReleaseCandidate>> =
        std::collections::HashMap::new();

    for c in candidates {
        // Season 0 pack guard: specials packs have unpredictable counts and
        // metadata, so block auto-download (users can still download them manually).
        if (c.episode_info.is_season_pack || c.episode_info.is_complete_pack)
            && c.episode_info.seasons.first().copied() == Some(0)
        {
            trace!(
                "Skipping season 0 pack '{}' (unpredictable for auto-download)",
                c.title
            );
            continue;
        }

        // Packs with empty needed_episodes may be version-upgrade candidates evaluated
        // later; single episodes with nothing needed are useless.
        if c.needed_episodes.is_empty()
            && !c.episode_info.is_season_pack
            && !c.episode_info.is_complete_pack
        {
            continue;
        }

        let season_str = c
            .episode_info
            .seasons
            .first()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "01".to_string());
        by_season
            .entry((c.mapping.series_id.clone(), season_str))
            .or_default()
            .push(c);
    }

    by_season
}

/// Merge winners sharing a download source into one entry with `multi_targets`.
///
/// SSoT: the source URL (magnet > download_url) is the merge key; the first
/// winner becomes primary, later ones append to its `multi_targets`.
pub(crate) fn merge_winners_by_source(
    winners: Vec<ReleaseCandidate>,
    global_absolute: bool,
) -> Result<Vec<ReleaseCandidate>> {
    use std::collections::HashMap;

    let input_count = winners.len();
    let mut merged: Vec<ReleaseCandidate> = Vec::new();
    let mut by_source: HashMap<String, usize> = HashMap::new();

    for winner in winners {
        let source_key = winner.download_url.as_deref().unwrap_or("").to_string();

        if source_key.is_empty() {
            merged.push(winner);
            continue;
        }

        if let Some(&idx) = by_source.get(&source_key) {
            let absolute = winner
                .mapping
                .settings
                .active_mode(global_absolute)
                .is_absolute();
            let season = jumbie_shared::mapping::resolve_season_opt(
                winner.episode_info.seasons.first().copied(),
                absolute,
            )
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot resolve season for multi-target of '{}': season-less release in normal numbering mode",
                    winner.mapping.target_title
                )
            })?;
            let target = jumbie_shared::types::MultiTarget {
                series_id: winner.mapping.series_id.clone(),
                season,
                episode: winner.episode_info.episodes.first().copied().unwrap_or(1),
            };
            merged[idx].multi_targets.push(target);
        } else {
            by_source.insert(source_key, merged.len());
            merged.push(winner);
        }
    }

    if merged.len() != input_count {
        info!(
            "Merged {} shared downloads into {} queue entries",
            input_count,
            merged.len()
        );
    }

    Ok(merged)
}

#[cfg(test)]
mod tests;
