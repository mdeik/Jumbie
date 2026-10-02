// Download lifecycle pipeline: process_downloads runs, in order, process_download_queue
// (dispatch + poll), adopt_orphans, process_retry_queue, and organize_completed. Each
// stage is idempotent and can restart after a crash; the pipeline runs on a timer and on
// demand after source processing adds items.
//
// Cancellation: if `cancel` is provided, long-running stages check the token between
// items and skip remaining work on shutdown. process_download_queue dispatches to an
// external client and is left alone.
use std::collections::HashMap;

use anyhow::Result;
use chrono::NaiveDateTime;
use tracing::{debug, info, warn};

use crate::db::download_queue::AddToDownloadQueueParams;
use crate::models::media::ReleaseCandidate;
use crate::organizer::ContentOrganizer;
use crate::plugins::notifiers::{NotifierContext, NotifierEvent, NotifierManager};
use jumbie_shared::types::episode_status::EpisodeStatus;
use jumbie_shared::types::{AddQueueResult, EpisodeIntention};

pub struct EnqueueDownloadParams<'a> {
    pub db: &'a crate::db::DbManager,
    pub notifications: Option<&'a NotifierManager>,
    pub media_name: &'a str,
    pub media_link: &'a str,
    /// Name of the source/indexer plugin that found this download.
    /// Empty string for manual downloads.
    pub source: &'a str,
    pub series_title: &'a str,
    pub series_id: &'a str,
    pub seasons: &'a [i32],
    pub episodes: &'a [i32],
    pub episode_id: Option<&'a str>,
    pub score: i32,
    pub is_user_requested: bool,
    /// True when the user picked this specific release. Drives the `Manual` badge
    /// and exempts the item from no-progress autoresolve. See [`EnrichAndEnqueueParams`].
    pub is_manual: bool,
    pub is_season_pack: bool,
    pub category: &'a str,
    pub multi_targets: Option<&'a str>,
    pub episode_intentions: Option<&'a str>,
    /// Source feed publish date, stored on the queue item and applied to the
    /// episode only when the file is organized (deferred).
    pub source_pub_date: Option<&'a str>,
    pub download_id: &'a str,
    pub scoring_size_bytes: Option<i64>,
    pub scoring_seeders: Option<i32>,
    pub scoring_episode_count: Option<i32>,
    /// Resolved submitter/release-group from the source plugin boundary.
    /// None if the plugin didn't provide one and none was parseable.
    /// Never re-derived from the title downstream.
    pub submitter: Option<&'a str>,
    /// Quality profile UUID snapshotted from the series mapping at queue time.
    /// Written to the episode row at organize time.
    pub quality_profile_id: Option<&'a str>,
    /// Parsed version number from the release title (e.g. v2).
    /// Defaults to 1 if no version marker was found.
    pub version: i32,
}
/// Parameters for [`ContentOrganizer::enrich_and_enqueue`] — the single function
/// that ALL download entry points converge through.
///
/// Core fields are required for all downloads. Enrichment fields are optional;
/// `download_winner` provides them when it has quality profile + metadata cache
/// data; other callers leave them as `None`.
pub struct EnrichAndEnqueueParams<'a> {
    // Core fields (always required)
    pub db: &'a crate::db::DbManager,
    pub notifications: Option<&'a NotifierManager>,
    pub media_name: &'a str,
    pub media_link: &'a str,
    /// Name of the source/indexer plugin that found this download.
    /// Empty string for manual downloads.
    pub source: &'a str,
    pub series_title: &'a str,
    pub series_id: &'a str,
    pub seasons: &'a [i32],
    pub episodes: &'a [i32],
    pub episode_id: Option<&'a str>,
    pub score: i32,
    pub is_user_requested: bool,
    /// True when the user explicitly picked this specific release (link +
    /// download_id) rather than the system selecting it by score. Set only on the
    /// manual `add_download` path; every auto-search/scanner path leaves it false.
    pub is_manual: bool,
    pub is_season_pack: bool,
    pub category: &'a str,
    pub multi_targets: Option<&'a str>,
    pub episode_intentions: Option<&'a str>,

    // Scoring inputs passed to ReleaseProfile::calculate() at download time, stored so
    // the exact score can be reproduced when profile terms/weights change.
    pub scoring_size_bytes: Option<u64>,
    pub scoring_seeders: Option<u32>,
    pub scoring_episode_count: Option<u32>,

    // Episode row enrichment (optional)
    /// Explicit quality profile UUID (a queue-time snapshot).  Callers that
    /// resolve it (download_winner from the candidate, queue_search_result from
    /// the series mapping) pass it here; manual downloads (add_download) leave
    /// it `None` so the episode isn't mis-attributed to the series profile.
    pub quality_profile_id: Option<&'a str>,
    pub meta_date: Option<NaiveDateTime>,
    /// Source feed publish date of the release file (RSS `<pubDate>` / search
    /// result date). Flows to `download_queue.source_pub_date`, then to the
    /// content's `release_info.upload_date`. Never the download time.
    pub source_pub_date: Option<NaiveDateTime>,
    pub metadata_ids: Option<&'a HashMap<String, String>>,
    pub description: Option<&'a str>,
    pub runtime: Option<i32>,
    pub image_url: Option<&'a str>,
    pub download_id: &'a str,
    pub title_override: Option<&'a str>,
    pub submitter: Option<&'a str>,
    /// Parsed version number from the release title (e.g. v2).
    /// `None` for manual downloads where version wasn't parsed.
    pub version: Option<i32>,
}

impl ContentOrganizer {
    /// Shared enqueue — single point where items enter the download queue
    /// and automated notifications fire.
    pub async fn enqueue_download(params: EnqueueDownloadParams<'_>) -> Result<AddQueueResult> {
        let insert = AddToDownloadQueueParams {
            media_name: params.media_name,
            media_link: params.media_link,
            series_title: params.series_title,
            series_id: params.series_id,
            seasons: params.seasons,
            episodes: params.episodes,
            episode_id: params.episode_id,
            score: params.score,
            is_user_requested: params.is_user_requested,
            is_season_pack: params.is_season_pack,
            category: params.category,
            multi_targets: params.multi_targets,
            episode_intentions: params.episode_intentions,
            source_pub_date: params.source_pub_date,
            download_id: params.download_id,
            scoring_size_bytes: params.scoring_size_bytes,
            scoring_seeders: params.scoring_seeders,
            scoring_episode_count: params.scoring_episode_count,
            submitter: params.submitter,
            quality_profile_id: params.quality_profile_id,
            version: params.version,
        };
        let result = if params.is_manual {
            params.db.add_manual_download_to_queue(insert).await?
        } else {
            params.db.add_to_download_queue(insert).await?
        };

        // Notify once per actual insert (Added/Replaced only).
        // Only notify when a new download is created (Added) or replaces an
        // existing lower-score entry (Replaced).  Skipped/Merged means the
        // item was already queued — no notification needed.
        if matches!(
            result,
            AddQueueResult::Added { .. } | AddQueueResult::Replaced(..)
        ) && !params.is_user_requested
            && let Some(notif) = params.notifications
        {
            let mut ctx = NotifierContext::new()
                .with_series(params.series_title)
                .with_series_id(params.series_id)
                .with_release_title(params.media_name)
                .with_indexer(params.source)
                .with_size_bytes(params.scoring_size_bytes.unwrap_or(0) as u64);
            if let (Some(&first_s), Some(&first_e)) =
                (params.seasons.first(), params.episodes.first())
            {
                ctx = ctx.with_episode(first_s, first_e);
            }
            if params.is_season_pack {
                ctx = ctx.with_season_pack();
            }
            notif.notify(NotifierEvent::DownloadStarted, &ctx).await;
        }

        Ok(result)
    }

    /// Enqueue a download with full metadata enrichment.
    ///
    /// This is the SINGLE function that all download entry points converge through:
    ///   • `queue_search_result` (manual search, auto_search_season, add_download Path A)
    ///   • `add_download` Path B (series page download)
    ///   • `download_winner` (polling / auto-search pipeline)
    ///
    /// Responsibilities:
    ///   1. Fetch the series mapping (once)
    ///   2. Build EpisodeIntentions (or use pre-built from caller)
    ///   3. Pre-insert episode rows with all available enrichment fields
    ///   4. FK fallback: ensure base episode row exists when intentions empty
    ///   5. Call [`Self::enqueue_download`]
    pub async fn enrich_and_enqueue(
        params: EnrichAndEnqueueParams<'_>,
    ) -> Result<AddQueueResult, anyhow::Error> {
        // SSoT: download_id is required from the source plugin.
        if params.download_id.is_empty() {
            anyhow::bail!(
                "Cannot enqueue '{}': no download_id provided — source plugin must \
                 supply one so the download client can track this item.",
                params.media_name,
            );
        }

        if params.media_link.is_empty() {
            tracing::warn!(
                "enrich_and_enqueue: no link for '{}' — skipping",
                params.media_name
            );
            return Ok(AddQueueResult::Skipped);
        }
        if let Err(e) = jumbie_shared::validation::validate_download_link(params.media_link) {
            tracing::warn!(
                "enrich_and_enqueue: invalid download link for '{}': {}",
                params.media_name,
                e
            );
            return Ok(AddQueueResult::Skipped);
        }

        // 1. Fetch mapping (once)
        let mapping = if !params.series_id.is_empty() && params.series_id != "unknown" {
            params
                .db
                .get_series_mapping(params.series_id)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        // SSoT: effective numbering mode for episode-ID generation.
        let global_absolute = params
            .db
            .get_general_config()
            .await
            .map(|g| g.absolute_numbering)
            .unwrap_or(false);
        let absolute = mapping
            .as_ref()
            .map(|m| m.settings.active_mode(global_absolute).is_absolute())
            .unwrap_or(global_absolute);

        // SSoT: absolute numbering is canonically ABSOLUTE_SEASON_NUM; in normal
        // mode an absent season stays absent (never invented) and is only an error
        // where an episode identity actually needs it (series-level searches pass
        // no season and no episode, and build none).
        let season_opt =
            jumbie_shared::mapping::resolve_season_opt(params.seasons.first().copied(), absolute);

        // 2. Build EpisodeIntentions
        let intentions: Vec<EpisodeIntention> = if let Some(prebuilt) = params.episode_intentions {
            serde_json::from_str(prebuilt)?
        } else if !params.episodes.is_empty()
            && let Some(ref mapping) = mapping
        {
            let season = season_opt.ok_or_else(|| {
                anyhow::anyhow!(
                    "Cannot queue '{}': episode IDs need a season in normal numbering mode",
                    params.media_name
                )
            })?;
            let season_str = format!("{:02}", season);
            // Pin both numbering spaces at queue time: `episode_num` is local/DB,
            // `source_episode_num` is what the release files will be named with.
            let offset = mapping.effective_episode_offset(&season_str, global_absolute);
            params
                .episodes
                .iter()
                .map(|&ep_num| -> anyhow::Result<EpisodeIntention> {
                    let ep_id = mapping.get_episode_id(&season_str, ep_num, global_absolute)?;
                    Ok(EpisodeIntention {
                        episode_num: ep_num,
                        source_episode_num: jumbie_shared::mapping::local_to_source_episode(
                            ep_num, offset,
                        ),
                        episode_id: ep_id,
                        score: params.score,
                        keep: true,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?
        } else {
            vec![]
        };

        // Pre-insert episode rows (SSoT: set_episode_status).
        // At queue time the episode is only marked "Queued" with its download_id; all
        // user-facing metadata is written later at organize time by finalize_download.
        // Already-downloaded episodes keep status 'organized' so the organize pipeline
        // can detect the upgrade via the scanner-claimed guard and replace the file.
        let keep_ids: Vec<String> = intentions
            .iter()
            .filter(|i| i.keep)
            .map(|i| i.episode_id.clone())
            .collect();
        let existing_statuses = params
            .db
            .get_episodes_status_batch(&keep_ids)
            .await
            .unwrap_or_default();

        for intention in &intentions {
            if intention.keep {
                let already_has_file = existing_statuses
                    .get(&intention.episode_id)
                    .map(|(downloaded, _)| *downloaded)
                    .unwrap_or(false);

                let new_status = if already_has_file {
                    // Keep 'Organized' so the organize pipeline can detect the upgrade.
                    EpisodeStatus::Organized.as_str()
                } else {
                    // Genuinely new episode — mark as queued.
                    "Queued"
                };

                let season = season_opt.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Cannot queue '{}': episode '{}' has no season in normal numbering mode",
                        params.media_name,
                        intention.episode_id
                    )
                })?;

                let _ = params
                    .db
                    .set_episode_status(
                        &intention.episode_id,
                        params.series_id,
                        season,
                        intention.episode_num,
                        new_status,
                    )
                    .await;
            }
        }

        // 4. FK fallback
        // Reachable only when intentions couldn't be built (no prebuilt intentions
        // and no mapping); use the first episode from the list.
        if intentions.is_empty()
            && let Some(ep_id) = params.episode_id
            && !params.series_id.is_empty()
            && params.series_id != "unknown"
        {
            // Episode 1 is the correct anchor here, not a silent default: the
            // caller's `episode_id` was itself built with an episode-1 anchor
            // (`first_ep.unwrap_or(1)` in download_winner), and this call exists to
            // create the parent `episodes` row that the `download_queue.episode_id`
            // FK requires — skipping it would trip that FK on enqueue.
            let ep_num = params.episodes.first().copied().unwrap_or(1);

            // SSoT: same status decision as the intention loop above.
            let already_has_file = params
                .db
                .get_episodes_status_batch(&[ep_id.to_string()])
                .await
                .unwrap_or_default()
                .get(ep_id)
                .map(|(downloaded, _)| *downloaded)
                .unwrap_or(false);

            let new_status = if already_has_file {
                EpisodeStatus::Organized.as_str()
            } else {
                "Queued"
            };

            let season = season_opt.ok_or_else(|| {
                anyhow::anyhow!(
                    "Cannot queue '{}': episode '{}' has no season in normal numbering mode",
                    params.media_name,
                    ep_id
                )
            })?;
            let _ = params
                .db
                .set_episode_status(ep_id, params.series_id, season, ep_num, new_status)
                .await;
        }

        // 5. Serialize intentions + enqueue
        let intentions_json: Option<String> = if intentions.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&intentions)?)
        };

        Self::enqueue_download(EnqueueDownloadParams {
            db: params.db,
            notifications: params.notifications,
            media_name: params.media_name,
            media_link: params.media_link,
            source: params.source,
            series_title: params.series_title,
            series_id: params.series_id,
            seasons: params.seasons,
            episodes: params.episodes,
            episode_id: params.episode_id,
            score: params.score,
            is_user_requested: params.is_user_requested,
            is_manual: params.is_manual,
            is_season_pack: params.is_season_pack,
            category: params.category,
            multi_targets: params.multi_targets,
            episode_intentions: intentions_json.as_deref(),
            source_pub_date: params
                .source_pub_date
                .map(|d| d.format(crate::datetime::DB_TIMESTAMP_FORMAT).to_string())
                .as_deref(),
            download_id: params.download_id,
            scoring_size_bytes: params.scoring_size_bytes.map(|s| s as i64),
            scoring_seeders: params.scoring_seeders.map(|s| s as i32),
            scoring_episode_count: params.scoring_episode_count.map(|c| c as i32),
            submitter: params.submitter,
            quality_profile_id: params.quality_profile_id,
            version: params.version.unwrap_or(1),
        })
        .await
    }

    pub(crate) async fn download_winner(&self, winner: &ReleaseCandidate) -> Result<()> {
        let link = winner.download_url.as_deref().filter(|s| !s.is_empty());

        let Some(link) = link else {
            warn!("No magnet link or download URL for {}", winner.title);
            return Ok(());
        };

        debug!("Processing candidate: {}", winner.title);

        let is_user_requested = false;

        // SSoT: effective numbering mode for episode-ID generation.
        let global_absolute = self.global_absolute_default().await;
        let absolute = winner
            .mapping
            .settings
            .active_mode(global_absolute)
            .is_absolute();

        // SSoT: absolute numbering is canonically ABSOLUTE_SEASON_NUM (its label is
        // not consulted); a normal-mode release with no season has no episode
        // identity to build. Skip it rather than inventing season 1 — one bad
        // release must not abort the rest of the batch.
        let seasons: &[i32] = &winner.episode_info.seasons;
        let season_start = match seasons.first().copied() {
            Some(s) => s,
            None if absolute => jumbie_shared::mapping::ABSOLUTE_SEASON_NUM,
            None => {
                debug!(
                    "Skipping release '{}' — no season to build episode IDs in normal numbering mode",
                    winner.title
                );
                return Ok(());
            }
        };
        let season_str = format!("{:02}", season_start);
        let seasons_iter: Vec<i32> = if seasons.is_empty() {
            vec![season_start]
        } else {
            seasons.to_vec()
        };

        // Episode metadata enrichment from cache.
        let mut episode_cache_lookup: std::collections::HashMap<
            (i32, i32),
            crate::db::metadata_cache::EpisodeMetadataCacheRow,
        > = std::collections::HashMap::new();
        let mut metadata_ids_map = std::collections::HashMap::new();

        // Resolve providers in PRIORITY order (never `HashMap` order) so the read
        // key matches what `fetch_metadata_for_series` wrote.
        let ordering_mode = if winner
            .mapping
            .settings
            .active_mode(self.db_config().await.general.absolute_numbering)
            .is_absolute()
        {
            "absolute"
        } else {
            "normal"
        };

        let instance_plugins = crate::utils::metadata::instance_plugin_id_map(&self.db).await;
        let ordered_instances = self
            .plugin_manager
            .read()
            .await
            .ordered_metadata_providers();
        let providers = crate::utils::metadata::ordered_series_providers(
            &winner.mapping,
            &ordered_instances,
            &instance_plugins,
        );

        // The queued item records the active (highest-priority) provider's external id.
        if let Some(active) = providers.first() {
            metadata_ids_map.insert(active.instance_id.clone(), active.metadata_id.clone());
        }
        for provider in &providers {
            if let Ok(cached_episodes) = self
                .db
                .get_metadata_episodes_cache(
                    &provider.metadata_id,
                    &provider.plugin_id,
                    &provider.instance_id,
                    ordering_mode,
                )
                .await
            {
                if cached_episodes.is_empty() {
                    continue;
                }
                for cached_ep in cached_episodes {
                    episode_cache_lookup.insert(
                        (cached_ep.season_number, cached_ep.episode_number),
                        cached_ep,
                    );
                }
                break;
            }
        }

        // Build EpisodeIntentions by iterating `episode_info.episodes` directly, so gapped
        // releases like [2, 4, 6] produce exactly 3 intentions (all keep=true) — no range
        // loop, no season_max, no phantom keep=false entries.
        let episode_list = &winner.episode_info.episodes;
        // Episode 1 is the anchor for a release whose parsed episode list is empty
        // (season pack / complete pack): it identifies the base episode row that
        // the queue's FK points at, and the same anchor is used when building that
        // `episode_id` below — it is an identity, not a stand-in episode number.
        let first_ep = episode_list.first().copied().unwrap_or(1);

        let mut episode_intentions: Vec<jumbie_shared::types::EpisodeIntention> = Vec::new();
        let mut multi_targets_for_download: Vec<jumbie_shared::types::MultiTarget> = Vec::new();

        for &s in &seasons_iter {
            let s_str = format!("{:02}", s);
            let is_first_season = s == season_start;
            // Each target season has its own offset; pin the release numbering for
            // this season's episodes so smart-link can match parsed filenames.
            let season_offset = winner
                .mapping
                .effective_episode_offset(&s_str, global_absolute);

            for &ep_num in episode_list {
                let episode_id = winner
                    .mapping
                    .get_episode_id(&s_str, ep_num, global_absolute)?;
                let source_episode_num =
                    jumbie_shared::mapping::local_to_source_episode(ep_num, season_offset);

                if is_first_season {
                    // Primary season: add to episode_intentions directly
                    episode_intentions.push(jumbie_shared::types::EpisodeIntention {
                        episode_num: ep_num,
                        source_episode_num,
                        episode_id,
                        score: winner.score,
                        keep: true,
                    });
                } else {
                    // Additional season: add as multi_target so the file is
                    // copied to this season's episode after organize.
                    multi_targets_for_download.push(jumbie_shared::types::MultiTarget {
                        series_id: winner.mapping.series_id.clone(),
                        season: s,
                        episode: ep_num,
                    });
                    // Also add an EpisodeIntention for tracking in notifications.
                    episode_intentions.push(jumbie_shared::types::EpisodeIntention {
                        episode_num: ep_num,
                        source_episode_num,
                        episode_id,
                        score: winner.score,
                        keep: true,
                    });
                }
            }
        }

        let base_episode_id = winner.mapping.get_episode_id(
            &format!("{:02}", season_start),
            first_ep,
            global_absolute,
        )?;

        // Per-episode enrichment from the metadata cache; the first episode's data feeds
        // title_override/dates. Metadata is cached under the resolved season (canonical 1
        // in absolute mode).
        let season_num = season_start;
        let first_cached = episode_cache_lookup.get(&(season_num, first_ep));
        let title = first_cached
            .map(|c| c.title.as_str())
            .unwrap_or(&winner.title);
        let description = first_cached.and_then(|c| c.description.as_deref());
        let runtime = first_cached.and_then(|c| c.runtime);
        let image_url = first_cached.and_then(|c| c.image_url.as_deref());
        let meta_date = first_cached
            .and_then(|c| {
                c.meta_date
                    .as_deref()
                    .and_then(|d| crate::datetime::parse_utc(d).ok())
                    .map(|u| u.naive_utc())
            })
            .or_else(|| winner.meta_date.map(|d| d.naive_utc()));

        if meta_date.is_none() {
            tracing::trace!(
                "No meta_date available for '{}' (ep {}): cached metadata and winner meta_date both missing",
                winner.title,
                first_ep,
            );
        }

        let mut all_multi_targets = winner.multi_targets.clone();
        all_multi_targets.extend(multi_targets_for_download);

        let multi_json = if all_multi_targets.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&all_multi_targets).unwrap_or_default())
        };

        let intentions_json = serde_json::to_string(&episode_intentions).unwrap_or_default();

        let notifications = self.notifications.read().await;
        match Self::enrich_and_enqueue(EnrichAndEnqueueParams {
            db: &self.db,
            notifications: Some(&*notifications),
            media_name: &winner.title,
            media_link: link,
            source: &winner.indexer,
            series_title: &winner.mapping.target_title,
            series_id: &winner.mapping.series_id,
            seasons: &winner.episode_info.seasons,
            episodes: &winner.episode_info.episodes,
            episode_id: Some(&base_episode_id),
            score: winner.score,
            is_user_requested,
            is_manual: false,
            is_season_pack: winner.episode_info.is_season_pack
                || winner.episode_info.is_complete_pack,
            category: "",
            multi_targets: multi_json.as_deref(),
            episode_intentions: Some(&intentions_json),
            // Enrichment: quality profile (explicit snapshot), dates, metadata cache
            quality_profile_id: winner.mapping.quality_profile.as_deref(),
            meta_date,
            // The release candidate's `meta_date` is its source feed publish date;
            // the episode's metadata/air date is resolved separately into `meta_date`.
            source_pub_date: winner.meta_date.map(|d| d.naive_utc()),
            metadata_ids: Some(&metadata_ids_map),
            description,
            runtime,
            image_url,
            download_id: winner.download_id.as_deref().unwrap_or(""),
            title_override: Some(title),
            submitter: winner.submitter.as_deref(),
            version: Some(winner.episode_info.version),
            // Scoring inputs (stored for retroactive rescore on profile changes)
            scoring_size_bytes: Some(winner.size_bytes),
            scoring_seeders: Some(winner.seeders),
            scoring_episode_count: if episode_list.len() > 1 {
                Some(episode_list.len() as u32)
            } else {
                None
            },
        })
        .await
        {
            Ok(AddQueueResult::Added { .. }) => {
                let log_ref = jumbie_shared::types::EpisodeLogRef::new(
                    &winner.title,
                    &season_str,
                    first_ep,
                    None,
                );
                info!("Queued {} with score {}", log_ref, winner.score);
            }
            Ok(AddQueueResult::Replaced(old_item)) => {
                let log_ref = jumbie_shared::types::EpisodeLogRef::new(
                    &winner.title,
                    &season_str,
                    first_ep,
                    None,
                );
                info!("Queued {} replacing lower score", log_ref);

                if let Some(hash) = old_item.downloader_id {
                    let download_mgr = self.downloader.read().await;
                    if let Err(e) = download_mgr
                        .delete_download(&hash, true, old_item.client_id.as_deref())
                        .await
                    {
                        tracing::warn!("Failed to delete replaced download files: {}", e);
                    }
                }
            }
            Ok(AddQueueResult::Skipped) => {
                debug!("Skipped queueing {} - higher exists", winner.title);
                return Ok(());
            }
            Ok(AddQueueResult::Merged {
                existing_id: _,
                new_targets,
            }) => {
                info!(
                    "Merged {} into existing queue entry (targets now cover {} targets)",
                    winner.title, new_targets
                );
                return Ok(());
            }
            Err(e) => {
                tracing::error!("Failed to enqueue download {}: {}", winner.title, e);
                {
                    let notifications = self.notifications.read().await;
                    notifications
                        .notify(
                            NotifierEvent::Error,
                            &NotifierContext::new()
                                .with_error("Enqueue Failed", format!("{}: {}", winner.title, e)),
                        )
                        .await;
                }
                return Ok(());
            }
        }

        // Run the estimator after queuing new episodes for this series
        if let Err(e) = crate::release_estimator::trigger_estimation_for_mapping(
            &self.db,
            &winner.mapping.series_id,
            winner.mapping.as_ref(),
        )
        .await
        {
            tracing::warn!("Failed to trigger release date estimation: {}", e);
        }

        Ok(())
    }

    /// Shared completion path — called when a download's content is confirmed
    /// on disk.  Fingerprints files, writes release metadata, runs smart_link,
    /// and marks the queue item as Completed.
    pub(crate) async fn finalize_download(
        &self,
        item: &jumbie_shared::types::DownloadQueueItem,
        content_path: &str,
    ) -> Result<()> {
        // Display-only: `season_num` feeds the notification context below, never an
        // episode ID. Absent season falls back to the canonical ABSOLUTE_SEASON_NUM.
        let season_num: i32 = item
            .season
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(jumbie_shared::mapping::ABSOLUTE_SEASON_NUM);

        // Resolve episode info for activity/notification. When episode == -1
        // (series-level search sentinel) we don't know it, so omit it rather than
        // showing a wrong "Series - S01E01".
        let (ep_for_activity, ep_end_for_activity) = if let Some(ep) = item.episode {
            let ep_end = if item.episode_end.unwrap_or(0) > ep {
                item.episode_end
            } else {
                None
            };
            (Some(ep), ep_end)
        } else {
            (None, None)
        };

        self.db
            .record_activity(crate::models::activity::ActivityEvent {
                event_type: jumbie_shared::types::ActivityType::Download,
                series_title: item.series_title.clone(),
                season: item.season.clone(),
                episode: ep_for_activity,
                episode_end: ep_end_for_activity,
                title: Some(item.media_name.clone()),
                details: None,
                status: "Success".to_string(),
            })
            .await?;

        // Submitter was stored on the queue item at queue time; fall back to parsing
        // the release title so attribution isn't lost for legacy items or when the
        // plugin resolved none.
        let resolved_submitter: Option<String> = item
            .submitter
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| jumbie_shared::parsing::extract_submitter(&item.media_name));

        // Fingerprint all files and write release metadata.
        let mut dirs_to_visit = vec![std::path::PathBuf::from(content_path)];
        while let Some(dir) = dirs_to_visit.pop() {
            if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if let Ok(file_type) = entry.file_type().await {
                        if file_type.is_dir() {
                            dirs_to_visit.push(entry.path());
                        } else if file_type.is_file() {
                            let fp = entry.path();
                            let fn_name = fp.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if fn_name.ends_with(".!qB")
                                || fn_name.ends_with(".part")
                                || fn_name.ends_with(".tmp")
                            {
                                continue;
                            }
                            let _ = crate::state::FileStateManager::fingerprint_file(
                                &self.db,
                                &fp,
                                crate::state::FileState::Complete,
                            )
                            .await;

                            let _ = self
                                .db
                                .set_release_info_by_path(
                                    fp.to_str().unwrap_or(""),
                                    Some(&item.media_name),
                                    Some(&item.media_link),
                                    resolved_submitter.as_deref(),
                                )
                                .await;
                        }
                    }
                }
            } else if tokio::fs::metadata(&dir)
                .await
                .map(|m| m.is_file())
                .unwrap_or(false)
            {
                let fp_str = dir.to_string_lossy().to_string();
                let _ = crate::state::FileStateManager::fingerprint_file(
                    &self.db,
                    &dir,
                    crate::state::FileState::Complete,
                )
                .await;

                let _ = self
                    .db
                    .set_release_info_by_path(
                        &fp_str,
                        Some(&item.media_name),
                        Some(&item.media_link),
                        resolved_submitter.as_deref(),
                    )
                    .await;
            }
        }

        // Smart-link files → episodes.
        let link_outcome = self
            .smart_link_downloaded_files(content_path, item)
            .await
            .unwrap_or_default();

        // Fire download-completed notification — runs after fingerprinting + smart-link
        // so the embed can include media info (resolution, codec, duration, languages).
        {
            let media_info: Option<jumbie_shared::types::MediaInfo> = {
                // Try exact path match first (single-file download)
                let row: Option<String> = sqlx::query_scalar(
                    "SELECT fc.media_info FROM file_paths fp \
                     JOIN file_contents fc ON fc.fingerprint = fp.fingerprint \
                     WHERE fp.file_path = ? AND fc.media_info IS NOT NULL",
                )
                .bind(content_path)
                .fetch_optional(self.db.get_pool())
                .await
                .ok()
                .flatten();
                if let Some(json_str) = row {
                    serde_json::from_str(&json_str).ok()
                } else {
                    // Fall back to first video file under content_path directory
                    let pattern = crate::db::sql_child_path_like_pattern(content_path);
                    let child: Option<String> = sqlx::query_scalar(
                        "SELECT fc.media_info FROM file_paths fp \
                         JOIN file_contents fc ON fc.fingerprint = fp.fingerprint \
                         WHERE replace(fp.file_path, '\\', '/') LIKE ? AND fc.media_info IS NOT NULL LIMIT 1",
                    )
                    .bind(&pattern)
                    .fetch_optional(self.db.get_pool())
                    .await
                    .ok()
                    .flatten();
                    child.and_then(|s| serde_json::from_str(&s).ok())
                }
            };

            let notifications = self.notifications.read().await;
            let mut ctx = crate::plugins::notifiers::NotifierContext::new()
                .with_series(&item.series_title)
                .with_series_id(&item.series_id)
                .with_release_title(&item.media_name)
                .with_episode_id(item.episode_id.as_deref().unwrap_or(""));
            if let Some(ep) = item.episode {
                ctx = ctx.with_episode(season_num, ep);
                if let Some(ep_end) = item.episode_end
                    && ep_end > ep
                {
                    ctx = ctx.with_episode_end(ep_end);
                }
            }
            if item.is_season_pack {
                ctx = ctx.with_season_pack();
            }
            if let Some(mi) = media_info {
                ctx = ctx.with_media_info(mi);
            }

            notifications
                .notify(
                    crate::plugins::notifiers::NotifierEvent::DownloadCompleted,
                    &ctx,
                )
                .await;
        }

        // Score and scoring inputs (size, seeders, episode_count) are stored on
        // release_info (keyed by content quick_hash) for EVERY episode covered, so the
        // rescore pipeline can reproduce the exact score when profiles change.
        let target_episodes: Vec<String> = item
            .episode_intentions
            .as_deref()
            .and_then(|json| {
                serde_json::from_str::<Vec<jumbie_shared::types::EpisodeIntention>>(json)
                    .ok()
                    .map(|intents| {
                        intents
                            .into_iter()
                            .map(|i| i.episode_id)
                            .collect::<Vec<String>>()
                    })
            })
            .unwrap_or_else(|| {
                item.episode_id
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|id| vec![id.to_string()])
                    .unwrap_or_default()
            });

        if !target_episodes.is_empty()
            && (item.scoring_size_bytes.is_some() || item.scoring_seeders.is_some())
        {
            for ep_id in &target_episodes {
                let _ = sqlx::query(&format!(
                    "UPDATE release_info SET \
                     score = ?, \
                     scoring_size_bytes = COALESCE(scoring_size_bytes, ?), \
                     scoring_seeders = COALESCE(scoring_seeders, ?), \
                     scoring_episode_count = COALESCE(scoring_episode_count, ?) \
                     WHERE quick_hash IN (\
                         SELECT fp.fingerprint FROM file_paths fp WHERE {pred}\
                     )",
                    pred = crate::db::EPISODE_FILES_PREDICATE,
                ))
                .bind(item.score)
                .bind(item.scoring_size_bytes)
                .bind(item.scoring_seeders)
                .bind(item.scoring_episode_count)
                .bind(ep_id.as_str())
                .execute(self.db.get_pool())
                .await
                .ok();
            }
        } else if let Some(ep_id) = item.episode_id.as_deref().filter(|s| !s.is_empty()) {
            // Fallback: no intentions and no scoring inputs — just set the score.
            let _ = sqlx::query(&format!(
                "UPDATE release_info SET score = ? WHERE quick_hash IN (\
                     SELECT fp.fingerprint FROM file_paths fp WHERE {pred}\
                 )",
                pred = crate::db::EPISODE_FILES_PREDICATE,
            ))
            .bind(item.score)
            .bind(ep_id)
            .execute(self.db.get_pool())
            .await
            .ok();
        }

        // A download assigned at queue time that placed no intended file needs a
        // human: mark it Review (not an error) so the kept files aren't mistaken for
        // a completed import.
        if link_outcome.unplaced_assignment {
            tracing::warn!(
                "Download '{}' placed no intended files ({} kept for review) — marking Review",
                item.media_name,
                link_outcome.kept_for_review
            );
            self.db
                .update_queue_item_status(item.id, "Review", None, None, None)
                .await?;
        } else {
            self.db
                .update_queue_item_status(item.id, "Completed", None, None, None)
                .await?;
            self.db.reset_queue_item_retry(item.id).await?;

            // The release worked — clear the autoresolve budget for its episodes so a
            // future stall of the same episode starts with fresh attempts.
            if let Some(targets) = crate::organizer::no_progress_targets(
                &item.series_id,
                item.season.as_deref(),
                item.episode,
                item.episode_end,
                item.episode_intentions.as_deref(),
            ) && let Ok(season) = targets.season.parse::<i32>()
                && let Err(e) = self
                    .db
                    .reset_autoresolve_attempts(&targets.series_id, season, &targets.episodes)
                    .await
            {
                tracing::warn!("Failed to reset autoresolve attempts: {}", e);
            }
        }
        Ok(())
    }
}
