use axum::{Json, extract::State, http::StatusCode};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::api::{AppState, RefreshResult};
use crate::error::AppError;
use axum::extract::Path;

/// Inline async handler returning a JSON body.
type JsonHandlerResult<T> =
    Pin<Box<dyn Future<Output = Result<Json<T>, (StatusCode, String)>> + Send>>;

pub fn add_download(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<jumbie_shared::types::DownloadMediaPayload>,
) -> JsonHandlerResult<jumbie_shared::types::AddDownloadResponse> {
    Box::pin(async move {
        tracing::debug!(
            "add_download called: title={:?}, episode_id={:?}, score={:?}, tag={:?}, series_id={:?}, is_season_pack={:?}",
            payload.title,
            payload.episode_id,
            payload.score,
            payload.tag,
            payload.series_id,
            payload.is_season_pack
        );
        if let Some(downloader_lock) = &state.downloader {
            let link = &payload.link;
            if link.is_empty() {
                tracing::debug!("add_download: no link provided");
                return Err((StatusCode::BAD_REQUEST, "No link provided".to_string()));
            }

            jumbie_shared::validation::validate_download_link(link).map_err(|e| {
                tracing::debug!("add_download: invalid download link: {}", e);
                (StatusCode::BAD_REQUEST, e.to_string())
            })?;

            if let Some(score) = payload.score
                && score < 0
            {
                tracing::debug!("add_download: negative score: {}", score);
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Score cannot be negative".to_string(),
                ));
            }

            if let Some(ref title) = payload.title
                && title.len() > 255
            {
                tracing::debug!("add_download: title exceeds max length");
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Title exceeds maximum length of 255 characters".to_string(),
                ));
            }

            if let Some(ref tag) = payload.tag
                && tag.len() > 100
            {
                tracing::debug!("add_download: tag exceeds max length");
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Tag exceeds maximum length of 100 characters".to_string(),
                ));
            }

            let ep_id_hint = payload.episode_id.clone().unwrap_or_default();
            let series_id_from_payload = payload.series_id.clone().unwrap_or_default();
            let is_season_pack_from_payload = payload.is_season_pack.unwrap_or(false);

            let title = payload.title.unwrap_or_else(|| link.clone());
            // Use the actual score from the search result (frontend always provides it).
            // The is_user_requested flag only affects upgrade evaluation at organize time —
            // it does not inflate the score.
            let score = payload.score.unwrap_or(0);

            // Season string for downstream logic (multiepisode-covering lookup and
            // displaced-episode auto-search). Resolved mode-aware inside Path A;
            // stays `None` on paths that carry no season (never invented).
            let mut season_for_logic: Option<String> = None;

            let (series_id, series_title, season_opt, episode_start, episode_end, resolved_ep_id): (String, String, Option<String>, Option<i32>, Option<i32>, Option<String>) =
                if !ep_id_hint.is_empty() {
                    // Path A: look up by episode_id
                    let mapped_info = sqlx::query_as::<_, (String, Option<i32>, i32)>(
                        "SELECT series_id, season, episode FROM episodes WHERE episode_id = ?",
                    )
                    .bind(&ep_id_hint)
                    .fetch_optional(state.db.get_pool())
                    .await
                    .map_err(|e| {
                        tracing::error!("add_download: DB error looking up episode: {}", e);
                        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                    })?;

                    if let Some((series_id, s_season, e_num)) = mapped_info {
                        let season_num = s_season.map(|s| s.to_string());

                        let (sid, mapping) = match state.db.get_series_mapping(&series_id).await {
                            Ok(Some(m)) => (series_id.clone(), m),
                            _ => {
                                // Fallback: treat series_id as a key into series_mappings
                                match state.db.get_mapping_by_key(&series_id).await {
                                    Ok(Some((key, m))) => (key, m),
                                    _ => {
                                        tracing::debug!(
                                            "add_download: no mapping found for series_id '{}', using fallback",
                                            series_id
                                        );
                                        (
                                            series_id.clone(),
                                            jumbie_shared::types::MappingRule {
                                                target_title: series_id.clone(),
                                                ..Default::default()
                                            },
                                        )
                                    }
                                }
                            }
                        };

                        let series_title = mapping.target_title.clone();
                        // Ensure series_id is populated before generating episode ID
                        let mut mutable_mapping = mapping;
                        mutable_mapping.ensure_series_id();
                        let global_absolute = {
                            let c = state.cfg.read().await;
                            c.general.absolute_numbering
                        };
                        // SSoT: absolute numbering's season is canonically
                        // ABSOLUTE_SEASON_NUM and its label is not consulted when
                        // generating the episode ID; a normal-mode row with no season
                        // recorded has no episode identity — a client error, never a
                        // silent "01".
                        let season_ref = match season_num.as_deref() {
                            Some(s) => s.to_string(),
                            None if mutable_mapping
                                .settings
                                .active_mode(global_absolute)
                                .is_absolute() =>
                            {
                                jumbie_shared::mapping::ABSOLUTE_SEASON_NUM.to_string()
                            }
                            None => {
                                return Err((
                                    StatusCode::BAD_REQUEST,
                                    "episode has no season recorded; cannot generate its episode ID"
                                        .to_string(),
                                ));
                            }
                        };
                        let ep_id = mutable_mapping
                            .get_episode_id(&season_ref, e_num, global_absolute)
                            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
                        season_for_logic = Some(season_ref);
                        (sid, series_title, season_num, Some(e_num), None, Some(ep_id))
                    } else {
                        // Path A fallback: episode_id not found in DB (stale/deleted
                        // episode, or a forged client value). The episode identity is
                        // unknowable, so queue a bare manual download — carrying the
                        // dangling id through would trip the `download_queue.episode_id`
                        // FK and fail the whole request.
                        tracing::debug!(
                            "add_download: episode_id {} not found in episodes table, using manual fallback",
                            ep_id_hint
                        );
                        (
                            "unknown".to_string(),
                            "Manual Download".to_string(),
                            Some("01".to_string()),
                            Some(1),
                            None::<i32>,
                            None::<String>,
                        )
                    }
                } else if !series_id_from_payload.is_empty() {
                    // Path B: series_id from payload (series page download).
                    // Season/episode are not parsed from the release title; smart-link
                    // resolves per-file from disk after completion, and a -1 sentinel
                    // signals that behavior. This keeps season packs scoped correctly.
                    let mut mapping = state
                        .db
                        .get_series_mapping(&series_id_from_payload)
                        .await
                        .map_err(|e| {
                            tracing::error!(
                                "add_download: error looking up series mapping: {}",
                                e
                            );
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                e.to_string(),
                            )
                        })?
                        .ok_or_else(|| {
                            tracing::debug!(
                                "add_download: series {} not found",
                                series_id_from_payload
                            );
                            (
                                StatusCode::NOT_FOUND,
                                format!(
                                    "Series {} not found",
                                    series_id_from_payload
                                ),
                            )
                        })?;
                    mapping.ensure_series_id();

                    // Series-level search: season and episode are resolved per-file by
                    // smart-link once the download completes (using
                    // link_permissive_episode); the -1 sentinel signals this.
                    tracing::debug!(
                        "add_download Path B: series-level search — using sentinel"
                    );

                    (
                        series_id_from_payload,
                        mapping.target_title,
                        None,   // season — resolved by smart-link
                        None,   // episode — resolved by smart-link
                        None,   // episode_end
                        None,   // episode_id
                    )
                } else {
                    // Path C: No context at all
                    (
                        "unknown".to_string(),
                        "Manual Download".to_string(),
                        Some("01".to_string()),
                        Some(1),
                        None::<i32>,
                        None::<String>,
                    )
                };

            // Only for Path A (concrete episode info); Path A resolved
            // `season_for_logic` mode-aware above, so a season-less queue item is
            // never compared as "01".
            let covering_multi =
                if !ep_id_hint.is_empty() && series_title.as_str() != "Manual Download" {
                    match season_for_logic.as_deref() {
                        Some(season_ref) => state
                            .db
                            .get_multiepisode_queue_item_covering(
                                season_ref,
                                episode_start.unwrap_or(1),
                            )
                            .await
                            .unwrap_or(None),
                        None => None,
                    }
                } else {
                    None
                };

            // All three paths converge through enrich_and_enqueue directly. Path A
            // (episode_id) needs the metadata fetch for the existing episode row;
            // Paths B (series_id) and C (no context) don't.
            let submitter = jumbie_shared::parsing::extract_submitter(&title);

            let notif_guard = if let Some(ref n) = state.notifications {
                Some(n.read().await)
            } else {
                None
            };
            let notif = notif_guard.as_deref();

            // Strict boundary: `upload_date`, when present, must be RFC 3339 with
            // an explicit offset — a malformed value is a client error, not
            // silently dropped.
            let parsed_upload_date = payload
                .upload_date
                .as_deref()
                .map(|d| {
                    crate::datetime::parse_request_utc(d)
                        .map(|u| u.naive_utc())
                        .map_err(|e| (StatusCode::BAD_REQUEST, e))
                })
                .transpose()?;

            // Build seasons/episodes lists from manual download params.
            // SSoT: empty lists signal episodes are unknown (resolved by smart-link).
            let seasons_for_enqueue: Vec<i32> = season_opt
                .as_deref()
                .and_then(|s| s.parse().ok())
                .map(|s| vec![s])
                .unwrap_or_default();
            let episodes_for_enqueue: Vec<i32> = if let Some(start) = episode_start {
                if let Some(end) = episode_end {
                    (start..=end).collect()
                } else {
                    vec![start]
                }
            } else {
                vec![]
            };

            let queue_result = crate::organizer::ContentOrganizer::enrich_and_enqueue(
                crate::download_orchestrator::download::EnrichAndEnqueueParams {
                    db: &state.db,
                    notifications: notif,
                    media_name: &title,
                    media_link: link,
                    source: "",
                    series_title: &series_title,
                    series_id: &series_id,
                    seasons: &seasons_for_enqueue,
                    episodes: &episodes_for_enqueue,
                    episode_id: resolved_ep_id.as_deref(),
                    score,
                    is_user_requested: payload.is_user_requested,
                    is_manual: true,
                    is_season_pack: is_season_pack_from_payload,
                    category: payload.category.as_deref().unwrap_or(""),
                    multi_targets: None,
                    episode_intentions: None,
                    // Enrichment: mostly None for manual downloads
                    quality_profile_id: None,
                    meta_date: None, // set by metadata fetch, not by queue time
                    source_pub_date: parsed_upload_date,
                    metadata_ids: None,
                    description: None,
                    runtime: None,
                    image_url: None,
                    download_id: &payload.download_id,
                    title_override: None,
                    submitter: submitter.as_deref(),
                    version: None,
                    scoring_size_bytes: payload.size,
                    scoring_seeders: payload.seeders,
                    scoring_episode_count: None,
                },
            )
            .await;

            // On-demand metadata fetch (shared, runs for any success outcome)
            let spawn_metadata_fetch = || {
                if !ep_id_hint.is_empty() && series_title.as_str() != "Manual Download" {
                    let state_clone = state.clone();
                    let effective_ep_id =
                        resolved_ep_id.clone().unwrap_or_else(|| ep_id_hint.clone());
                    let series_title_clone = series_title.clone();
                    tokio::spawn(async move {
                        if let Err(e) = ensure_episode_metadata(
                            &state_clone,
                            &effective_ep_id,
                            &series_title_clone,
                        )
                        .await
                        {
                            tracing::debug!(
                                "On-demand metadata fetch skipped for {}: {:?}",
                                effective_ep_id,
                                e
                            );
                        }
                    });
                }
            };

            match queue_result {
                Ok(jumbie_shared::types::AddQueueResult::Replaced(old_item)) => {
                    spawn_metadata_fetch();
                    let queue_id = old_item.id;
                    if let Some(hash) = old_item.downloader_id {
                        let downloader = downloader_lock.read().await;
                        if let Err(e) = downloader
                            .delete_download(&hash, true, old_item.client_id.as_deref())
                            .await
                        {
                            tracing::warn!("Failed to delete manually replaced download: {}", e);
                        }
                    }
                    Ok(Json(jumbie_shared::types::AddDownloadResponse {
                        outcome: "replaced".to_string(),
                        queue_id: Some(queue_id),
                    }))
                }
                Ok(result) => {
                    // Added | Skipped | Merged.
                    // If we displaced a multiepisode, cancel it and auto-search for missing episodes
                    if let Some(multi) = covering_multi {
                        let Some(multi_ep_start) = multi.episode else {
                            return Err((
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "covering multi has no episode".to_string(),
                            ));
                        };
                        let multi_ep_end = multi.episode_end.unwrap_or(multi_ep_start);
                        tracing::info!(
                            "Manual download of {} displaces multiepisode queue item {} (S{}E{}-E{})",
                            resolved_ep_id.as_deref().unwrap_or("?"),
                            multi.media_name,
                            season_opt.as_deref().unwrap_or("01"),
                            multi_ep_start,
                            multi_ep_end
                        );

                        if let Some(ref hash) = multi.downloader_id {
                            let downloader = downloader_lock.read().await;
                            if let Err(e) = downloader
                                .delete_download(hash, true, multi.client_id.as_deref())
                                .await
                            {
                                tracing::warn!(
                                    "Failed to cancel displaced multiepisode download: {}",
                                    e
                                );
                            }
                        }
                        let _ = state.db.remove_from_download_queue(multi.id).await;

                        let uncovered: Vec<i32> = (multi_ep_start..=multi_ep_end)
                            .filter(|ep| Some(*ep) != episode_start)
                            .collect();

                        if !uncovered.is_empty()
                            && series_id.as_str() != "unknown"
                            && state.organizer.is_some()
                            && let Some(s_ref) = season_for_logic.as_deref()
                        {
                            let key = format!("{}:{}", series_id, s_ref);
                            let organizer = state.organizer.clone().unwrap();
                            let sid = series_id.clone();
                            let s_str = s_ref.to_string();
                            state
                                .search_queue
                                .submit(key, move || {
                                    let org = organizer;
                                    let sid = sid;
                                    let s = s_str;
                                    let eps = uncovered.clone();
                                    async move {
                                        if let Err(e) =
                                            org.auto_search_missing(&sid, &s, &eps).await
                                        {
                                            tracing::error!(
                                                "Auto-search for displaced episodes failed: {}",
                                                e
                                            );
                                        }
                                    }
                                })
                                .await;
                        }
                    }
                    spawn_metadata_fetch();
                    Ok(Json(jumbie_shared::types::AddDownloadResponse {
                        outcome: result.outcome().to_string(),
                        queue_id: result.queue_id(),
                    }))
                }
                Err(e) => {
                    tracing::error!("Failed to queue manual download: {}", e);
                    Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
                }
            }
        } else {
            tracing::debug!("add_download: downloader not available");
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Downloader not available".to_string(),
            ))
        }
    })
}

/// Check if an episode has metadata (non-empty title) and submit a P1 fetch to
/// the metadata queue if missing. Runs in a background task so it doesn't block
/// the download response. On success `metadata_last_synced_at` is updated by
/// `fetch_metadata_for_series`; the queue handles dedup and concurrency.
async fn ensure_episode_metadata(
    state: &Arc<AppState>,
    episode_id: &str,
    series_title: &str,
) -> Result<(), AppError> {
    let has_title = state
        .db
        .get_episode_title(episode_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
        .is_some_and(|t| !t.is_empty());

    if has_title {
        tracing::trace!(
            "Episode {} already has metadata, skipping on-demand fetch",
            episode_id
        );
        return Ok(());
    }

    tracing::info!(
        "Episode {} has no title — triggering on-demand metadata fetch",
        episode_id
    );

    let (series_id, _) = state
        .db
        .get_mapping_by_key(series_title)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
        .ok_or_else(|| {
            AppError::NotFound(format!("No series mapping found for '{}'", series_title))
        })?;

    let state_for_closure = state.clone();
    state
        .metadata_queue
        .submit(
            series_id.clone(),
            crate::metadata_queue::Priority::Normal,
            move || {
                let state = state_for_closure.clone();
                let sid = series_id.clone();
                async move {
                    if let Err(e) =
                        crate::api_routes::series::fetch_metadata_for_series(&state, &sid, None)
                            .await
                    {
                        tracing::debug!(
                            "On-demand metadata fetch for {} did not complete: {:?}",
                            sid,
                            e
                        );
                    }
                }
            },
        )
        .await;

    Ok(())
}

pub async fn refresh_series(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<RefreshResult>, (StatusCode, String)> {
    tracing::info!("Refreshing series: {}", id);

    let mapping = state
        .db
        .get_series_mapping(&id)
        .await
        .map_err(|e| {
            tracing::error!("refresh_series DB error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| {
            tracing::debug!("refresh_series: series {} not found", id);
            (StatusCode::NOT_FOUND, format!("Series {} not found", id))
        })?;

    let series_title = mapping.target_title.clone();
    let series_id = id.clone();
    let mut series_count = 0;
    let mut downloads_count = 0;

    // Scan the series directory at its canonical location.
    // SSoT: series path via `paths::mapping_path` (template + policy).
    let series_path = crate::paths::mapping_path(&mapping, &state.org_config().await);
    if series_path.exists() {
        // `scan_series_directory` (not `scan_directory`) is the SSoT for a known
        // series directory: it uses SXXEXX patterns + season folders directly,
        // without filename-based series_key matching.
        match crate::scanner::scan_series_directory(&series_path, &mapping, &state).await {
            Ok(count) => {
                series_count = count;
                tracing::debug!("Found {} episodes in organized directory", series_count);
            }
            Err(e) => {
                tracing::warn!("Failed to scan series directory: {}", e);
            }
        }
    } else {
        tracing::debug!(
            "Series directory does not exist yet: '{}'",
            series_path.display()
        );
    }

    // Scan the downloads folder.
    if let Some(downloader_lock) = &state.downloader {
        let downloader = downloader_lock.read().await;
        for downloads_path in downloader.get_organizer_paths().await {
            if downloads_path.exists() {
                match crate::scanner::scan_directory(&downloads_path, &state, false).await {
                    Ok(found) => {
                        downloads_count += found.len();
                        tracing::debug!(
                            "Found {} series in downloads directory '{}'",
                            found.len(),
                            downloads_path.display()
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to scan downloads directory '{}': {}",
                            downloads_path.display(),
                            e
                        );
                    }
                }
            }
        }
    }

    let total_found = state
        .db
        .get_series_episode_count(&series_id)
        .await
        .map_err(|e| {
            tracing::error!("refresh_series: failed to get episode count: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get episode count: {}", e),
            )
        })?;

    // On-demand metadata fetch for newly discovered files: if any episode lacks a
    // title, fetch the whole series metadata in one call rather than per-episode.
    // Covers files manually placed or downloaded externally and picked up by scan.
    let has_missing_metadata = state
        .db
        .get_series_episodes_details(
            &series_id,
            state.effective_absolute_numbering(&mapping).await,
        )
        .await
        .map(|eps| {
            eps.iter()
                .any(|ep| ep.title.as_deref().unwrap_or("").is_empty())
        })
        .unwrap_or(false);

    if has_missing_metadata {
        tracing::info!(
            "Series {} has episodes with missing metadata — submitting to queue (P1)",
            series_title
        );
        let state_clone = state.clone();
        let series_id = id.clone();
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
                                "On-demand metadata fetch after refresh_series failed: {:?}",
                                e
                            );
                        }
                    }
                },
            )
            .await;
    }

    tracing::debug!(
        "refresh_series completed for {} ({}): series_found={}, downloads_found={}, total_episodes={}",
        series_title,
        id,
        series_count,
        downloads_count,
        total_found
    );
    Ok(Json(RefreshResult {
        series_found: series_count,
        downloads_found: downloads_count,
        total_episodes: total_found,
    }))
}
