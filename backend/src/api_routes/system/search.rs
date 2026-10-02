//! Search-related endpoints.
//!
//! Manual search returns ALL results ranked by score for the user to pick from;
//! auto search (auto_search.rs) picks the best result and queues it. This module
//! handles the manual path plus the "automatic mode" that queues the best result.

use crate::api::AppState;
use crate::error::AppError;
use crate::error::IntoApiResponse;
use crate::plugins::PluginInstance;
use axum::{Extension, Json, extract::State};
use jumbie_shared::auth::ApiScope;
use jumbie_shared::plugin::Capability;
use jumbie_shared::types::{EpisodeStatus, ValidatePathPayload, ValidatePathResponse};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// POST /api/search/auto-season — dedicated endpoint for season-wide auto-search.
///
/// Separate from the main search rather than a mode flag: a `mode="auto_season"`
/// flag on /api/search would need an inline queue:write check that is invisible
/// from the route definition, making it easy to expose auto-search to search-only
/// users. The compound `search + queue:write` gate lives in this route's
/// definition (`router::scope::search_and_queue_write`).
pub async fn auto_season_search(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::SearchPayload>,
) -> Result<Json<Vec<jumbie_shared::types::SearchResult>>, AppError> {
    // mode must be auto_season (or legacy "season")
    let mode = payload.mode.as_deref().unwrap_or("");
    if mode != "auto_season" && mode != "season" {
        return Err(AppError::BadRequest(
            "Use /api/search for manual search; this endpoint is for auto-season search only"
                .to_string(),
        ));
    }

    let series_id = payload.series_id.clone().ok_or_else(|| {
        AppError::BadRequest("series_id is required for season search".to_string())
    })?;
    let season = payload
        .season
        .clone()
        .ok_or_else(|| AppError::BadRequest("season is required for season search".to_string()))?;
    let episode_numbers = payload.episode_numbers.clone().ok_or_else(|| {
        AppError::BadRequest("episode_numbers is required for season search".to_string())
    })?;

    crate::validation::validate_episode_numbers_not_empty(episode_numbers.len())
        .map_err(|e| AppError::BadRequest(e.0))?;
    let season_num_str = season.trim_start_matches(['S', 's']);
    crate::validation::validate_season_number(season_num_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid season: {}", e.0)))?;
    for &ep in &episode_numbers {
        crate::validation::validate_episode_number(ep)
            .map_err(|e| AppError::BadRequest(format!("Invalid episode number {}: {}", ep, e.0)))?;
    }

    tracing::debug!(
        "Auto-season search triggered: series_id={}, season={}, episodes={:?}",
        series_id,
        season,
        episode_numbers
    );

    // Delegate to the auto-search helper (spawns background task, returns 202)
    let _ = crate::api_routes::system::auto_search::auto_search_season(
        state,
        jumbie_shared::types::AutoSearchSeasonPayload {
            series_id,
            season,
            episode_numbers,
            is_user_requested: payload.is_user_requested,
        },
    )
    .await?;

    tracing::debug!("auto_season_search completed");
    Ok(Json(vec![]))
}

pub async fn search_media(
    State(state): State<Arc<AppState>>,
    Extension(scopes): Extension<Vec<ApiScope>>,
    Json(payload): Json<jumbie_shared::types::SearchPayload>,
) -> Result<Json<Vec<jumbie_shared::types::SearchResult>>, AppError> {
    // This endpoint no longer handles auto-season mode; callers must use
    // POST /api/search/auto-season, which has proper compound scope middleware.
    if payload.mode.as_deref() == Some("auto_season") || payload.mode.as_deref() == Some("season") {
        return Err(AppError::BadRequest(
            "Auto-season search moved to POST /api/search/auto-season. ".to_string(),
        ));
    }

    // Auto-episode mode queues downloads, but this route only requires `search`.
    // Enforce the extra queue:write requirement here with the shared
    // insufficient-scope response; the auto-season endpoint gets the same check
    // from its route middleware instead.
    if payload.mode.as_deref() == Some("auto_episode")
        && !scopes.iter().any(|s| s.permits(ApiScope::QueueWrite))
    {
        return Err(AppError::insufficient_scope(
            &[ApiScope::QueueWrite],
            &scopes,
        ));
    }

    // auto_episode does not use `payload.query` (the backend builds the query), so
    // dispatch it before query validation, which requires a non-empty query.
    if payload.mode.as_deref() == Some("auto_episode") {
        return auto_episode_search(state, payload).await;
    }

    // Validate the request before any DB or plugin work.
    crate::validation::validate_search_query(&payload.query)
        .map_err(|e| AppError::BadRequest(e.0))?;

    tracing::debug!(
        "search_media called: query={:?}, mode={:?}, series_id={:?}, season={:?}",
        payload.query,
        payload.mode,
        payload.series_id,
        payload.season
    );

    let mut results;
    // Source plugins actually queried — included in the summary log so a search
    // can be traced back to the indexers it hit.
    let mut queried_sources: Vec<String> = Vec::new();

    // Scoring setup precedes the search because ranking needs the profile's rules,
    // but the profile requires a series_id — with none, scoring is skipped and raw
    // results are returned (useful for discovery). `get_merged_scoring` merges the
    // series' overrides (e.g. preferred release groups) with the profile's base.
    let mut merged_scoring = None;
    let mut series_mapping = None;

    if let Some(series_id) = &payload.series_id {
        if let Some(mapping) = state.db.get_series_mapping(series_id).await.ok().flatten() {
            let profile_id = mapping.release_profile.as_deref().unwrap_or("");
            let profile_scoring = state
                .db
                .get_release_profile(profile_id)
                .await
                .unwrap_or(None)
                .unwrap_or_default();

            merged_scoring = Some(mapping.get_merged_scoring(&profile_scoring));
            series_mapping = Some(mapping.clone());
            tracing::debug!(
                "Search setup: {} ({}) min_score={} release_profile={:?}",
                mapping.target_title,
                series_id,
                profile_scoring.min_score,
                mapping.release_profile
            );
        } else {
            tracing::warn!(
                "Search setup: series_id={} provided but mapping not found in DB",
                series_id
            );
        }
    } else {
        tracing::debug!("Search setup: no series_id provided");
    }

    {
        let pm = state.plugin_manager.read().await;
        let sources = pm.get_plugins_by_all_capabilities(&[
            jumbie_shared::plugin::Capability::FeedProvider,
            jumbie_shared::plugin::Capability::ManualSearch,
        ]);

        if sources.is_empty() {
            tracing::warn!("Manual search attempted but no source plugins support it");
            return Err(AppError::BadRequest(
                "No Search Plugins are available.".to_string(),
            ));
        }

        let raw_query = payload.query.clone();

        // A query like "@uuid:Test Series" filters the search to the source whose
        // instance UUID matches.
        let parsed_query = jumbie_shared::mapping::parse_alias(&raw_query);
        let query = if parsed_query.source_id.is_some() {
            parsed_query.alias.clone()
        } else {
            raw_query.clone()
        };

        let target_id = parsed_query.source_id;
        let filtered_sources: Vec<Arc<dyn PluginInstance>> = if let Some(ref id) = target_id {
            if let Some(plugin) = sources.iter().find(|s| s.instance_id() == id) {
                tracing::info!(
                    "Manual search filtered to source id '{}' (plugin: {})",
                    id,
                    plugin.instance_id()
                );
                vec![plugin.clone()]
            } else {
                tracing::warn!(
                    "Manual search: source id '{}' not found — searching all sources",
                    id
                );
                sources
            }
        } else {
            sources
        };

        let mut futures = Vec::new();

        for source in filtered_sources {
            let q = query.clone();
            // Resolve the user-configured instance name from the backend-owned
            // mapping before moving the plugin into the spawned task.
            let instance_id = source.instance_id().to_string();
            let source_name = state
                .plugin_manager
                .read()
                .await
                .get_instance_name(&instance_id);
            queried_sources.push(source_name.clone());

            // Canonical query log context (kind = manual).
            let log = crate::search::SearchLog {
                kind: crate::search::SearchKind::Manual,
                source: source_name.clone(),
                series_id: payload.series_id.clone(),
                series_title: series_mapping.as_ref().map(|m| m.target_title.clone()),
                season: payload
                    .season
                    .as_deref()
                    .and_then(jumbie_shared::mapping::parse_season_num),
                source_episodes: Vec::new(),
                aliases: Vec::new(),
            };

            // Per-source tasks run in parallel so total time is the max indexer
            // latency rather than their sum. The 15-second timeout stops one dead
            // indexer from blocking the whole search.
            let sid = source_name.clone();
            futures.push(tokio::spawn(async move {
                let timeout_duration = std::time::Duration::from_secs(15);

                match tokio::time::timeout(
                    timeout_duration,
                    crate::plugins::bridge::sources::search(source.as_ref(), &q, &log),
                )
                .await
                {
                    Ok(Ok(mut entries)) => {
                        tracing::debug!(
                            "Manual search '{}': source {} returned {} result(s)",
                            q,
                            sid,
                            entries.len()
                        );
                        // Tag each entry with the source name so the frontend
                        // can display a source badge on every result.
                        for entry in &mut entries {
                            entry.source.clone_from(&source_name);
                        }
                        Some(entries)
                    }
                    Ok(Err(e)) => {
                        tracing::error!(
                            "Source {} ({}) search failed: {}",
                            source_name,
                            source.instance_id(),
                            e
                        );
                        None
                    }
                    Err(_) => {
                        tracing::error!(
                            "Source {} ({}) search timed out",
                            source_name,
                            source.instance_id()
                        );
                        None
                    }
                }
            }));
        }

        let task_results = futures::future::join_all(futures).await;

        let mut all_entries = Vec::new();
        for res in task_results {
            if let Ok(Some(entries)) = res {
                all_entries.extend(entries);
            }
        }

        results = crate::api_routes::system::search_core::score_results(
            all_entries,
            merged_scoring.as_ref(),
            &state,
        )
        .await;
    }

    // Default sort: canonical release ranking (score ↓ → date ↓ → seeders ↓).
    results.sort_by(|a, b| jumbie_shared::types::compare_release_rank_desc(a.rank(), b.rank()));

    // SSoT: `get_all_quality_data` returns (qualities, quality_profiles); fetched
    // once and reused for the release checks.
    let (qualities, quality_profiles) = if let Some(ref m) = series_mapping {
        if let Some(qp_name) = &m.quality_profile {
            if !qp_name.is_empty() {
                state.db.get_all_quality_data().await
            } else {
                (
                    std::collections::HashMap::new(),
                    std::collections::HashMap::new(),
                )
            }
        } else {
            (
                std::collections::HashMap::new(),
                std::collections::HashMap::new(),
            )
        }
    } else {
        (
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        )
    };

    // The same acceptance gates auto-search applies, evaluated here as
    // informational suggestions (SSoT: `crate::release_checks`). Manual results
    // are never hidden — the user can always download regardless of failures.
    let rejected = state.db.get_rejected_downloads().await.unwrap_or_default();
    let episode_scope = match (&payload.episode_id, &series_mapping) {
        (Some(episode_id), Some(mapping)) => build_episode_scope(&state, episode_id, mapping).await,
        _ => None,
    };
    let check_ctx = crate::release_checks::ReleaseCheckContext {
        mapping: series_mapping.as_ref(),
        merged_scoring: merged_scoring.as_ref(),
        quality_profile_name: series_mapping
            .as_ref()
            .and_then(|m| m.quality_profile.as_deref()),
        quality_profiles: &quality_profiles,
        qualities: &qualities,
        rejected: Some(&rejected),
        episode_scope: episode_scope.as_ref(),
    };

    for result in &mut results {
        let candidate = crate::release_checks::ReleaseCandidate {
            title: &result.title,
            link: result.link.as_deref(),
            download_id: result.download_id.as_deref(),
            score: result.score,
            is_season_pack: result.is_season_pack,
        };
        result.release_checks = crate::release_checks::evaluate(&candidate, &check_ctx);
    }

    // The manual view annotates every result with checks but never hides any, so
    // this "N valid of M" line is the only server-side signal to compare against
    // the auto-search path (which silently drops failing results).
    let found = results.len();
    let valid = results
        .iter()
        .filter(|r| r.release_checks.iter().all(|c| c.passed))
        .count();
    tracing::info!(
        "Search '{}': {} result(s) found, {} valid (pass all auto-search checks); sources={:?}",
        payload.query,
        found,
        valid,
        queried_sources
    );

    // Run the estimator after search to keep release dates current
    if let Some(mapping) = &series_mapping
        && let Err(e) =
            crate::release_estimator::trigger_estimation_for_series(&state.db, &mapping.series_id)
                .await
    {
        tracing::warn!(
            "Failed to run release date estimator for series {} ({}): {}",
            mapping.target_title,
            mapping.series_id,
            e
        );
    }

    Ok::<_, AppError>(results).into_json_response()
}

/// Build the episode scope for a manual search launched from an episode, so the
/// episode gate can be evaluated. Returns `None` when the episode is unknown.
async fn build_episode_scope(
    state: &Arc<AppState>,
    episode_id: &str,
    mapping: &jumbie_shared::types::MappingRule,
) -> Option<crate::release_checks::EpisodeScope> {
    let episode = state
        .db
        .get_episode_by_id(episode_id)
        .await
        .ok()
        .flatten()?;

    let (global_absolute, global_format, global_format_absolute) = {
        let c = state.cfg.read().await;
        (
            c.general.absolute_numbering,
            c.organization.search_format.clone(),
            c.organization.search_format_absolute.clone(),
        )
    };

    // SSoT: absolute numbering is canonically ABSOLUTE_SEASON_NUM; a normal-mode
    // season label must be numeric. `resolve_season_search_meta` expects the
    // season label, so canonicalise before resolving.
    let absolute = mapping.settings.active_mode(global_absolute).is_absolute();
    let season_str = if absolute {
        jumbie_shared::mapping::ABSOLUTE_SEASON_NUM.to_string()
    } else {
        format!(
            "{:02}",
            episode
                .season
                .unwrap_or(jumbie_shared::mapping::DEFAULT_SEASON_NUM)
        )
    };
    let season_num = jumbie_shared::mapping::resolve_season_num(&season_str, absolute).ok()?;

    let meta = crate::search::resolve_season_search_meta(
        mapping,
        &season_str,
        season_num,
        global_absolute,
        &global_format,
        &global_format_absolute,
    );
    let source_episode =
        jumbie_shared::mapping::local_to_source_episode(episode.episode, meta.episode_offset);
    let label = if absolute {
        format!("E{:02}", episode.episode)
    } else {
        format!("S{:02}E{:02}", season_num, episode.episode)
    };

    Some(crate::release_checks::EpisodeScope {
        meta,
        source_episodes: vec![source_episode],
        label,
        current_score: episode.score,
    })
}

/// `mode="auto_episode"`: alias-aware automatic search that queues the best
/// match and returns it, so the caller gets feedback within the request.
async fn auto_episode_search(
    state: Arc<AppState>,
    payload: jumbie_shared::types::SearchPayload,
) -> Result<Json<Vec<jumbie_shared::types::SearchResult>>, AppError> {
    let series_id = payload
        .series_id
        .clone()
        .ok_or_else(|| AppError::BadRequest("series_id is required".to_string()))?;
    let season = payload
        .season
        .clone()
        .ok_or_else(|| AppError::BadRequest("season is required".to_string()))?;
    let episode_numbers = payload
        .episode_numbers
        .clone()
        .ok_or_else(|| AppError::BadRequest("episode_numbers is required".to_string()))?;

    // Validate before touching the DB or any plugin.
    crate::validation::validate_episode_numbers_not_empty(episode_numbers.len())
        .map_err(|e| AppError::BadRequest(e.0))?;
    let season_num_str = season.trim_start_matches(['S', 's']);
    crate::validation::validate_season_number(season_num_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid season: {}", e.0)))?;
    for &ep in &episode_numbers {
        crate::validation::validate_episode_number(ep)
            .map_err(|e| AppError::BadRequest(format!("Invalid episode: {}", e.0)))?;
    }
    let season_num: i32 =
        jumbie_shared::mapping::parse_season_num(season_num_str).ok_or_else(|| {
            AppError::BadRequest(format!("Invalid season number: {}", season_num_str))
        })?;

    // The series must exist; its config drives aliases, profile, and numbering.
    let mapping = state
        .db
        .get_series_mapping(&series_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| AppError::NotFound(format!("Series {} not found", series_id)))?;
    let target_title = mapping.target_title.clone();

    // Scoring: merge the series' release-profile overrides with the base profile.
    let profile_id = mapping.release_profile.as_deref().unwrap_or("");
    let profile_scoring = state
        .db
        .get_release_profile(profile_id)
        .await
        .unwrap_or(None)
        .unwrap_or_default();
    let min_score = profile_scoring.min_score;
    let merged_scoring = mapping.get_merged_scoring(&profile_scoring);

    // SSoT: effective numbering-mode default (series tristate → global).
    let (global_absolute, global_format, global_format_absolute) = {
        let c = state.cfg.read().await;
        (
            c.general.absolute_numbering,
            c.organization.search_format.clone(),
            c.organization.search_format_absolute.clone(),
        )
    };
    let ep_nums = episode_numbers.clone();

    // Resolve aliases (same logic as auto_search_season)
    let meta = crate::search::resolve_season_search_meta(
        &mapping,
        &season,
        season_num,
        global_absolute,
        &global_format,
        &global_format_absolute,
    );
    let search_season_num = meta.search_season_num;
    let episode_offset = meta.episode_offset;

    let pm = state.plugin_manager.read().await;
    let sources = pm
        .get_plugins_by_all_capabilities(&[Capability::FeedProvider, Capability::AutomaticSearch]);
    let (plugin_aliases, final_gen_aliases) =
        crate::search::resolve_search_aliases(&pm, &mapping, &season, global_absolute).await;
    drop(pm);

    // Search all sources with an alias-aware payload.
    let mut search_futures = Vec::new();
    // Source plugins queried (used in the summary log).
    let mut queried_sources: Vec<String> = Vec::new();
    // Episodes in SOURCE numbering (local + season offset).
    let source_episodes = crate::search::source_episode_numbers(&ep_nums, episode_offset);
    for source in sources {
        let source_id = source.instance_id().to_string();
        let per_source_aliases = crate::search::resolve_final_aliases(
            &source_id,
            &plugin_aliases,
            &final_gen_aliases,
            &target_title,
        );
        let log = crate::search::SearchLog {
            kind: crate::search::SearchKind::AutoEpisode,
            source: String::new(),
            series_id: Some(series_id.clone()),
            series_title: Some(target_title.clone()),
            season: Some(search_season_num),
            source_episodes: source_episodes.clone(),
            aliases: per_source_aliases.clone(),
        };
        let per_source_payload = crate::search::build_search_payload(
            &target_title,
            search_season_num,
            &ep_nums,
            per_source_aliases,
            &meta.search_format,
            episode_offset,
        );

        let source_name = state
            .plugin_manager
            .read()
            .await
            .get_instance_name(&source_id);
        queried_sources.push(source_name.clone());
        let log = crate::search::SearchLog {
            source: source_name.clone(),
            ..log
        };

        search_futures.push(tokio::spawn(async move {
            match tokio::time::timeout(
                Duration::from_secs(20),
                crate::plugins::bridge::sources::auto_search(
                    source.as_ref(),
                    per_source_payload,
                    &log,
                ),
            )
            .await
            {
                Ok(Ok(mut entries)) => {
                    for entry in &mut entries {
                        entry.source.clone_from(&source_name);
                    }
                    Some(entries)
                }
                _ => None,
            }
        }));
    }

    let mut all_entries = Vec::new();
    for res in futures::future::join_all(search_futures).await {
        if let Ok(Some(entries)) = res {
            all_entries.extend(entries);
        }
    }

    // Entries returned across all sources, before filtering.
    let all_entries_count = all_entries.len();
    let mut scored_results = crate::api_routes::system::search_core::score_results(
        all_entries,
        Some(&merged_scoring),
        &state,
    )
    .await;
    scored_results
        .sort_by(|a, b| jumbie_shared::types::compare_release_rank_desc(a.rank(), b.rank()));

    let (ql, qp) = if let Some(qp_name) = &mapping.quality_profile {
        if !qp_name.is_empty() {
            state.db.get_all_quality_data().await
        } else {
            (HashMap::new(), HashMap::new())
        }
    } else {
        (HashMap::new(), HashMap::new())
    };

    // Releases rejected after a stalled download — auto-search must not re-select
    // the release that just failed (SSoT: `crate::release_checks::is_rejected`).
    let rejected = state.db.get_rejected_downloads().await.unwrap_or_default();

    let valid_results: Vec<_> = scored_results
        .into_iter()
        .filter(|r| {
            let release = meta.match_release(&r.title);
            // SSoT: the same acceptance gates manual search reports (see
            // `crate::release_checks`), so the two paths cannot drift.
            !crate::release_checks::is_rejected(
                &rejected,
                r.link.as_deref(),
                r.download_id.as_deref(),
            )
                && crate::release_checks::quality_failure(
                    &r.title,
                    mapping.quality_profile.as_deref(),
                    &qp,
                    &ql,
                )
                .is_none()
                && crate::release_checks::score_failure(r.score, min_score).is_none()
                // The release must cover one of the searched episodes (SSoT:
                // `ReleaseMatch::accepts`).
                && source_episodes.iter().any(|&e| release.accepts(e))
        })
        .collect();

    // Same shape as the other search paths so auto-episode can be compared
    // against manual search when diagnosing a missed release. The exact query is
    // logged per source by the bridge; source_episodes are the offset numbers.
    tracing::info!(
        "Episode auto-search: series='{}' season={} source_episodes={:?} aliases={:?} sources={:?} — {} valid of {} found",
        target_title,
        search_season_num,
        source_episodes,
        final_gen_aliases,
        queried_sources,
        valid_results.len(),
        all_entries_count,
    );

    if let Some(mut best) = valid_results.into_iter().next() {
        let season_str = season.trim_start_matches(['S', 's']);
        let ep_num = ep_nums[0];
        let ep_id = mapping
            .get_episode_id(season_str, ep_num, global_absolute)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;

        // Validate season/episode before persisting — negative or zero
        // episode numbers would corrupt the database and break queries.
        crate::validation::validate_season_number(season_str).map_err(|e| {
            tracing::warn!(
                "search_media: invalid season '{}' for series '{}': {}",
                season_str,
                mapping.target_title,
                e
            );
            AppError::BadRequest(format!("Invalid season: {}", e))
        })?;
        crate::validation::validate_episode_number(ep_num).map_err(|e| {
            tracing::warn!(
                "search_media: invalid episode {} for series '{}': {}",
                ep_num,
                mapping.target_title,
                e
            );
            AppError::BadRequest(format!("Invalid episode: {}", e))
        })?;

        // Ensure the episode row exists before queueing — the download_queue
        // table has a FOREIGN KEY constraint on episodes(episode_id).
        // INNER JOIN with download_queue expects at least one row; if the
        // insert fails here the queue step silently skips (best-effort).
        let _ = state
            .db
            .insert_episode(crate::db::episodes::InsertEpisodeParams {
                episode_id: &ep_id,
                series_id: &mapping.series_id,
                season: season_num,
                episode: ep_num,
                file_path: None,
                title: Some(&target_title),
                quality_profile_id: None,
                status: EpisodeStatus::Missing.as_str(),
                meta_date: None,
                est_date: None,
                metadata_ids: &std::collections::HashMap::new(),
                description: None,
                runtime: None,
                image_url: None,
                metadata_source: None,
                numbering_mode: Some(
                    mapping.settings.active_mode(global_absolute).is_absolute() as i32
                ),
            })
            .await;

        match crate::api_routes::system::search_core::queue_search_result(
            crate::api_routes::system::search_core::QueueSearchParams {
                state: &state,
                result: &best,
                series_id: &mapping.series_id,
                series_title: &target_title,
                seasons: &[season_num],
                episodes: &[ep_num],
                episode_ids: std::slice::from_ref(&ep_id),
                is_user_requested: payload.is_user_requested,
                is_season_pack: best.is_season_pack,
                score: best.score,
                category: "Series",
                episode_intentions: None,
            },
        )
        .await
        {
            Ok(jumbie_shared::types::AddQueueResult::Replaced(old_item)) => {
                tracing::info!(
                    "Episode auto-search: queued '{}' (replaced lower)",
                    best.title
                );
                if let Some(hash) = old_item.downloader_id
                    && let Some(downloader_lock) = &state.downloader
                {
                    let downloader = downloader_lock.read().await;
                    let _ = downloader
                        .delete_download(&hash, true, old_item.client_id.as_deref())
                        .await;
                }
                best.queue_action = "replaced".to_string();
                spawn_on_demand_metadata_fetch(&state, &ep_id, &target_title);
                return Ok::<_, AppError>(vec![best]).into_json_response();
            }
            Ok(jumbie_shared::types::AddQueueResult::Added { .. }) => {
                tracing::info!("Episode auto-search: queued '{}'", best.title);
                best.queue_action = "added".to_string();
                spawn_on_demand_metadata_fetch(&state, &ep_id, &target_title);
                return Ok::<_, AppError>(vec![best]).into_json_response();
            }
            Ok(jumbie_shared::types::AddQueueResult::Skipped) => {
                tracing::info!(
                    "Episode auto-search: skipped '{}' (better exists)",
                    best.title
                );
                best.queue_action = "skipped".to_string();
                return Ok::<_, AppError>(vec![best]).into_json_response();
            }
            Ok(jumbie_shared::types::AddQueueResult::Merged { .. }) => {
                tracing::info!(
                    "Episode auto-search: merged '{}' into multi-target",
                    best.title
                );
                best.queue_action = "merged".to_string();
                return Ok::<_, AppError>(vec![best]).into_json_response();
            }
            Err(e) => {
                tracing::error!("Episode auto-search: queue failed: {}", e);
                return Err(AppError::Internal(anyhow::anyhow!(
                    "Queueing failed: {}",
                    e
                )));
            }
        }
    }

    tracing::info!(
        "Episode auto-search: no suitable result for {} S{}",
        target_title,
        search_season_num
    );
    Ok::<_, AppError>(vec![]).into_json_response()
}

/// Spawn a background task to fetch metadata for an episode if it's missing.
/// This updates `last_synced_at` so the background refresh loop skips it.
fn spawn_on_demand_metadata_fetch(state: &Arc<AppState>, episode_id: &str, series_title: &str) {
    if episode_id.is_empty() || series_title.is_empty() {
        return;
    }
    let state = state.clone();
    let episode_id = episode_id.to_string();
    let series_title = series_title.to_string();
    tokio::spawn(async move {
        let has_title = match state.db.get_episode_title(&episode_id).await {
            Ok(Some(t)) => !t.is_empty(),
            _ => false,
        };

        if has_title {
            tracing::trace!(
                "Episode {} already has metadata, skipping on-demand fetch",
                episode_id
            );
            return;
        }

        tracing::info!(
            "Episode {} has no title — submitting to queue (P1)",
            episode_id
        );

        // Look up the series_id from the mapping.
        let series_id = match state.db.get_mapping_by_key(&series_title).await {
            Ok(Some((id, _))) => id,
            _ => {
                tracing::debug!(
                    "On-demand metadata fetch: no mapping found for '{}'",
                    series_title
                );
                return;
            }
        };

        // Submit at Normal priority; the queue handles dedup and concurrency.
        let state_clone = state.clone();
        state
            .metadata_queue
            .submit(
                series_id.clone(),
                crate::metadata_queue::Priority::Normal,
                move || {
                    let state = state_clone.clone();
                    let sid = series_id.clone();
                    async move {
                        if let Err(e) =
                            crate::api_routes::series::fetch_metadata_for_series(&state, &sid, None)
                                .await
                        {
                            tracing::debug!(
                                "On-demand metadata fetch failed for {}: {:?}",
                                episode_id,
                                e
                            );
                        }
                    }
                },
            )
            .await;
    });
}

pub async fn validate_path_endpoint(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ValidatePathPayload>,
) -> Json<ValidatePathResponse> {
    tracing::debug!("validate_path_endpoint called: path={}", payload.path);
    let config = state.cfg.read().await;
    // SSoT: config.organization.primary_root
    let root = config.organization.primary_root().to_path_buf();
    // `paths::*` resolution (create flow only) needs the collision/policy fields,
    // but the read lock must be released before the DB fetch.
    let org_config = config.organization.clone();
    drop(config);

    // No template replacement: this validates the raw path as entered. Template
    // variables like ${series} are not valid filesystem paths and are caught by
    // validation.
    let resolved_path = &payload.path;

    // validate_path runs first so an invalid path (traversal, unsupported, etc.)
    // fails before the more expensive collision checks. Its returned PathBuf is
    // authoritative (leading/trailing whitespace trimmed, relative paths resolved),
    // so using the raw `payload.path` below would risk mismatches.
    let validated_path = match crate::validation::validate_path(resolved_path, &root) {
        Ok(p) => p,
        Err(e) => {
            return Json(ValidatePathResponse {
                is_valid: false,
                message: e.to_string(),
                resolved_path: None,
            });
        }
    };

    let all_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();

    // Create flow only: sanitize the folder name per the illegal-char policy so
    // the preview shows the name that will actually be used. Sanitizing first
    // keeps the claim checks and collision resolution on the effective name
    // (matching create_series, which composes the same steps).
    let working_path = if payload.resolve_collisions {
        crate::paths::sanitize_folder_name(&validated_path, &org_config)
    } else {
        validated_path.clone()
    };

    // Collision check: with `series_id: None` (new series creation) all series are
    // checked; with `Some(id)` (edit) that series is excluded so its own path is
    // not flagged.
    if let Some((_, mapping)) = crate::validation::find_colliding_series(
        &working_path,
        payload.series_id.as_deref(),
        &all_mappings,
        &org_config,
    ) {
        if !payload.resolve_collisions || !mapping.hidden_in_library {
            // A visible-series claim is always rejected; outside the create flow
            // (resolve_collisions = false) any claim, hidden included, is rejected.
            return Json(ValidatePathResponse {
                is_valid: false,
                // SSoT: shared message formatter used by check_series_path_not_taken.
                message: crate::validation::series_path_taken_message(&working_path, mapping),
                resolved_path: None,
            });
        }
        // Create flow + hidden-series claim: `create_series` untracks the hidden
        // series and reclaims the folder as-is, so report it as valid with the
        // path unchanged.
        return Json(ValidatePathResponse {
            is_valid: true,
            message: "Valid path".to_string(),
            resolved_path: Some(working_path.to_string_lossy().to_string()),
        });
    }

    // Folder-level collision resolution — create flow only (the Add Series form
    // sets resolve_collisions = true). Applies the org collision config when the
    // destination folder already exists on disk: rename → the effective suffixed
    // path is returned in `resolved_path`, skip → invalid, overwrite → path as-is.
    // SSoT: the same helper drives `create_series`, so the preview always shows
    // the folder that will actually be created.
    if payload.resolve_collisions {
        match crate::paths::resolve_folder_collision(&working_path, &org_config, |p| {
            crate::validation::find_colliding_series(
                p,
                payload.series_id.as_deref(),
                &all_mappings,
                &org_config,
            )
            .is_some()
        }) {
            Ok(effective) => Json(ValidatePathResponse {
                is_valid: true,
                message: "Valid path".to_string(),
                resolved_path: Some(effective.to_string_lossy().to_string()),
            }),
            Err(msg) => Json(ValidatePathResponse {
                is_valid: false,
                message: msg,
                resolved_path: None,
            }),
        }
    } else {
        // Custom paths are honored verbatim — surface host-illegal characters
        // as invalid so the preview matches what create_series will do.
        // SSoT: same check as resolve_series_folder_path's verbatim branch.
        if let Err(msg) = crate::paths::check_host_legal_components(&working_path) {
            return Json(ValidatePathResponse {
                is_valid: false,
                message: msg,
                resolved_path: None,
            });
        }
        Json(ValidatePathResponse {
            is_valid: true,
            message: "Valid path".to_string(),
            resolved_path: None,
        })
    }
}
