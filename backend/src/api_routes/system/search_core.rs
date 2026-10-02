//! Shared search pipeline — `enrich_and_enqueue` is the single function all
//! download paths converge through; `queue_search_result` is a thin adapter for
//! SearchResult-based callers.
//!
//! SSoT: `enrich_and_enqueue` fetches the mapping once and owns all episode-row
//! insertion, intention building, and `enqueue_download` calls. `is_season_pack`
//! is derived once in `score_results()` and passed explicitly.

use std::sync::Arc;

use crate::api::AppState;
use crate::models::media::MediaEntry;
use crate::organizer::ContentOrganizer;
use jumbie_shared::scoring::ReleaseProfile;
use jumbie_shared::types::{AddQueueResult, SearchResult};

// score_results: apply scoring + pack detection

/// Apply release profile scoring + automatic profile penalty.
///
/// SSoT: `is_season_pack` is set here from `parse_filename` and never
/// re-derived downstream; all callers pass this flag through explicitly.
pub async fn score_results(
    entries: Vec<MediaEntry>,
    merged_scoring: Option<&ReleaseProfile>,
    state: &Arc<AppState>,
) -> Vec<SearchResult> {
    let mut results = Vec::with_capacity(entries.len());

    for entry in entries {
        let mut score = 0;

        let is_season_pack =
            crate::utils::parse_filename(&entry.title, crate::utils::ParseContext::Search)
                .map(|p| p.is_season_pack || p.is_complete_pack)
                .unwrap_or(false);

        let episode_count = if is_season_pack {
            None
        } else {
            let ep_nums = super::parse_episode_numbers_from_title(&entry.title);
            if ep_nums.len() > 1 {
                Some(ep_nums.len() as u32)
            } else {
                None
            }
        };

        if let Some(scoring) = merged_scoring {
            let (title_ref, size, seeders, published) = entry.scoring_fields();
            let (mut s, _reasons) = scoring.calculate_with_submitter(
                title_ref,
                size,
                seeders,
                published,
                episode_count,
                entry.submitter.as_deref(),
            );

            let config = state.cfg.read().await;
            if is_season_pack {
                s += config.general.effective_pack_score_modifier();
            }
            if config.general.automatic_profiles.enabled {
                let submitter = entry
                    .submitter
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string());
                if let Ok(Some(profile)) = state.db.get_automatic_profile(&submitter).await {
                    s += profile.score;
                }
            }
            drop(config);

            score = s;
        }

        results.push(entry.to_search_result(score, is_season_pack));
    }

    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)))
    });

    results
}

// queue_search_result: thin adapter for SearchResult-based callers

/// Parameters for [`queue_search_result`] — a thin adapter that extracts fields
/// from a [`SearchResult`] and delegates to [`ContentOrganizer::enrich_and_enqueue`].
pub struct QueueSearchParams<'a> {
    pub state: &'a Arc<AppState>,
    pub result: &'a SearchResult,
    pub series_id: &'a str,
    pub series_title: &'a str,
    pub seasons: &'a [i32],
    pub episodes: &'a [i32],
    pub episode_ids: &'a [String],
    pub is_user_requested: bool,
    pub is_season_pack: bool,
    pub score: i32,
    pub category: &'a str,
    /// Pre-built episode intentions. When set, skips building intentions
    /// (used by download_winner which pre-builds alongside rich metadata).
    pub episode_intentions: Option<&'a str>,
}

/// Queue a search result into the download queue.
///
/// Thin adapter that extracts the link and submitter from a [`SearchResult`]
/// and delegates the actual work to [`ContentOrganizer::enrich_and_enqueue`].
/// Paths without a `SearchResult` (e.g. `add_download` Path B) call
/// `enrich_and_enqueue` directly.
pub async fn queue_search_result(
    params: QueueSearchParams<'_>,
) -> Result<AddQueueResult, anyhow::Error> {
    let dl_link = params
        .result
        .link
        .as_deref()
        .unwrap_or_default()
        .to_string();

    let notif_guard = if let Some(ref n) = params.state.notifications {
        Some(n.read().await)
    } else {
        None
    };
    let notif = notif_guard.as_deref();

    // Search-driven queuing is an automatic selection within the series'
    // configured profile, so the profile is snapshotted onto the queue item;
    // manual downloads (`add_download`) intentionally leave it unset.
    let quality_profile_id = params
        .state
        .db
        .get_series_mapping(params.series_id)
        .await
        .ok()
        .flatten()
        .and_then(|m| m.quality_profile);

    let result = ContentOrganizer::enrich_and_enqueue(
        crate::download_orchestrator::download::EnrichAndEnqueueParams {
            db: &params.state.db,
            notifications: notif,
            media_name: &params.result.title,
            media_link: &dl_link,
            source: &params.result.source,
            series_title: params.series_title,
            series_id: params.series_id,
            seasons: params.seasons,
            episodes: params.episodes,
            episode_id: params.episode_ids.first().map(|s| s.as_str()),
            score: params.score,
            is_user_requested: params.is_user_requested,
            is_manual: false,
            is_season_pack: params.is_season_pack,
            category: params.category,
            multi_targets: None,
            episode_intentions: params.episode_intentions,
            // Enrichment: most None for search results
            quality_profile_id: quality_profile_id.as_deref(),
            meta_date: None, // set by metadata fetch, not by queue time
            source_pub_date: params.result.published.map(|d| d.naive_utc()),
            metadata_ids: None,
            description: None,
            runtime: None,
            image_url: None,
            download_id: params.result.download_id.as_deref().unwrap_or(""),
            title_override: None,
            submitter: params.result.submitter.as_deref(),
            version: None,
            // Scoring inputs — SearchResult has size/seeders but no meta_date
            scoring_size_bytes: Some(params.result.size),
            scoring_seeders: params.result.seeders,
            scoring_episode_count: if params.episodes.len() > 1 {
                Some(params.episodes.len() as u32)
            } else {
                None
            },
        },
    )
    .await?;

    Ok(result)
}
