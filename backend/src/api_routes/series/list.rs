use crate::api::{AppState, PadNumeric};
use crate::error::AppError;

use axum::{
    Json,
    extract::{Path, State},
};
use jumbie_shared::types::{EpisodeStatus, SeriesInfo, WantedEpisode};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub fn build_season_list<'a>(
    db_seasons: impl IntoIterator<Item = String>,
    overrides: impl IntoIterator<Item = &'a jumbie_shared::types::SeasonOverride>,
    suppressed: &HashSet<i32>,
) -> Vec<String> {
    let mut all_seasons: Vec<String> = Vec::new();

    // A season the user deleted (`suppressed_seasons`) is hidden everywhere — it
    // must not reappear via a DB row, an override, or a placeholder cell.
    let is_suppressed = |s: &str| -> bool {
        s.trim()
            .parse::<i32>()
            .map(|n| suppressed.contains(&n))
            .unwrap_or(false)
    };

    // Overrides are processed first so they take the primary slot: they can REMAP
    // season identifiers (e.g. "Season 2" → "Season 1"), and DB seasons are then
    // dedup'd against them so an override with a matching DB entry never appears
    // twice. Overrides win as explicit user intent.
    for override_item in overrides {
        if is_suppressed(&override_item.season) {
            continue;
        }
        all_seasons.push(override_item.season.clone());
    }

    for db_season in db_seasons {
        if is_suppressed(&db_season) {
            continue;
        }
        if !all_seasons.contains(&db_season) {
            all_seasons.push(db_season);
        }
    }

    let mut normalized_seasons: Vec<String> =
        all_seasons.iter().map(|s| s.pad_numeric(2)).collect();
    normalized_seasons.sort();
    normalized_seasons.dedup();

    normalized_seasons
}

/// Expected total episode count for a series.
///
/// SSoT for `episodes_counts.1` across the library list and the detail endpoints.
/// Each displayed season contributes its configured in-range cell count when it has
/// a `cell_count`, otherwise the in-range episodes present. Seasons with episodes
/// on disk but no display label (e.g. legacy rows with a NULL season) are still
/// counted, while suppressed seasons never are.
///
/// Downloaded/organized counts are intentionally not derived here — they always
/// reflect files on disk, even outside the configured range.
pub fn expected_total_episodes(
    settings: &jumbie_shared::types::SeriesSettings,
    normalized_seasons: &[String],
    suppressed: &HashSet<i32>,
    season_counts: &[jumbie_shared::types::SeasonEpisodeCount],
    global_absolute: bool,
) -> i32 {
    let present_for = |season: &str| -> i32 {
        jumbie_shared::mapping::parse_season_num(season)
            .and_then(|n| {
                season_counts
                    .iter()
                    .find(|s| s.season == n)
                    .map(|s| s.expected)
            })
            .unwrap_or(0)
    };

    // Start from the display seasons (which already honour suppression) and add any
    // season that has episodes but no display label, so no real episode is dropped.
    let mut seasons: Vec<String> = normalized_seasons.to_vec();
    for season in season_counts {
        if suppressed.contains(&season.season) {
            continue;
        }
        let already_counted = seasons
            .iter()
            .any(|s| jumbie_shared::mapping::parse_season_num(s) == Some(season.season));
        if !already_counted {
            seasons.push(season.season.to_string());
        }
    }

    seasons
        .iter()
        .map(|season| settings.expected_episode_count(season, present_for(season), global_absolute))
        .sum()
}

/// Seasons the user deleted, each annotated with whether it can be restored from
/// the provider cache.
///
/// `cache_available` reuses `metadata_seasons` (the provider season cache already
/// exposed for the season "Match" affordance) — no extra cache query or endpoint.
pub fn suppressed_season_infos(
    suppressed: &[i32],
    metadata_seasons: &[jumbie_shared::types::MetadataSeasonInfo],
) -> Vec<jumbie_shared::types::SuppressedSeasonInfo> {
    suppressed
        .iter()
        .map(|&season| jumbie_shared::types::SuppressedSeasonInfo {
            season,
            cache_available: metadata_seasons.iter().any(|m| m.season_number == season),
        })
        .collect()
}

/// Count `monitored_missing` episodes per series — the red indicator.
///
/// "Missing" is a wanted episode (`get_wanted_episodes`: monitored, released,
/// unassigned) whose effective status is still `missing`.
///
/// The yellow "queued" indicator is deliberately NOT derived here: it must also
/// light up for unmonitored episodes that are actively downloading, so it comes
/// from [`DbManager::get_queued_episode_counts_by_series`] instead.
fn monitored_missing_counts_by_series(wanted: &[WantedEpisode]) -> HashMap<String, i32> {
    let mut counts: HashMap<String, i32> = HashMap::new();
    for ep in wanted {
        if ep.status == EpisodeStatus::Missing.as_str() {
            *counts.entry(ep.series_id.clone()).or_insert(0) += 1;
        }
    }
    counts
}

pub async fn get_series(State(state): State<Arc<AppState>>) -> Json<Vec<SeriesInfo>> {
    tracing::debug!("get_series called");
    let mut series_list = Vec::new();

    let stats = state
        .db
        .get_series_stats_with_seasons()
        .await
        .unwrap_or_default();

    // Stats keyed by series_id (UUID) — no name/title-based resolution, so
    // lookups are unambiguous even when multiple series share a display name.
    let stats_map: std::collections::HashMap<String, crate::db::SeriesStatRow> = stats
        .into_iter()
        .map(|row| (row.series_id.clone(), row))
        .collect();

    let mappings = state.db.get_all_series_mappings().await.unwrap_or_default();

    // SSoT: effective numbering-mode default — read once for the whole loop.
    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };

    // Counts are derived from `get_wanted_episodes()` (respecting the user's release
    // date display preferences, SSoT `derive_episode_status`) rather than the
    // SQL-level `monitored_missing_count`. Only episodes whose effective status is
    // truly "missing" count; a preference order that resolves to a future date shows
    // "unreleased" even if a lower-priority source has a past date.
    //
    // The yellow "queued" count is sourced separately from the download queue so an
    // unmonitored episode that is downloading still lights it up.
    let missing_by_series = monitored_missing_counts_by_series(
        &state.db.get_wanted_episodes().await.unwrap_or_default(),
    );
    let queued_by_series = state
        .db
        .get_queued_episode_counts_by_series()
        .await
        .unwrap_or_default();

    // SSoT: per-season episode counts. Each series' expected total is derived from
    // these (a season with `cell_count` expects that many cells; otherwise the
    // in-range episodes it actually holds), so it matches the Edit Series header.
    let season_counts = state
        .db
        .get_series_season_completion_counts()
        .await
        .unwrap_or_default();

    for (key, rule) in &mappings {
        if rule.hidden_in_library {
            // Soft-delete: data is preserved in the DB but the series is removed from
            // the library list (still visible in "Manage Folders" for restore/delete).
            continue;
        }

        let default_stat = crate::db::SeriesStatRow {
            series_id: key.clone(),
            series_title: rule.target_title.clone(),
            downloaded_count: 0,
            total_count: 0,
            size: 0,
            monitored_missing_count: 0,
            seasons: Vec::new(),
        };
        let stat_row = stats_map.get(key).cloned().unwrap_or(default_stat);

        let downloaded_count = stat_row.downloaded_count;
        let size = stat_row.size;
        let db_seasons = stat_row.seasons;

        let active_mode = rule.settings.active_mode(global_absolute);
        let suppressed: HashSet<i32> = state
            .db
            .get_suppressed_seasons(&rule.series_id, active_mode.is_absolute() as i32)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        let normalized_seasons = build_season_list(
            db_seasons,
            rule.settings.season_for_mode(active_mode).values(),
            &suppressed,
        );
        let formatted_seasons = crate::utils::combine_seasons(&normalized_seasons);
        let season_count = if active_mode.is_absolute() {
            0
        } else {
            normalized_seasons.len() as i32
        };
        // SSoT: expected total across the series' seasons (cell_count when set,
        // otherwise the in-range episodes present).
        let total = expected_total_episodes(
            &rule.settings,
            &normalized_seasons,
            &suppressed,
            season_counts.get(key).map(Vec::as_slice).unwrap_or(&[]),
            global_absolute,
        );

        // Missing files: downloaded episodes whose stored path no longer exists on
        // disk are dangling references the user needs to know about.
        let has_not_found_files = downloaded_count > 0
            && rule
                .settings
                .path
                .as_ref()
                .is_some_and(|p| !p.is_empty() && !std::path::Path::new(p).exists());

        let missing_count = missing_by_series.get(key).copied().unwrap_or(0);
        let queued_count = queued_by_series.get(key).copied().unwrap_or(0);
        series_list.push(SeriesInfo {
            id: key.clone(),
            title: rule.target_title.clone(),
            seasons: formatted_seasons,
            season_count,
            release_profile: rule.release_profile.clone().unwrap_or_default(),
            quality_profile: rule.quality_profile.clone().unwrap_or_default(),
            episodes_counts: (downloaded_count as i32, total),
            monitored_missing_count: missing_count,
            queued_count,
            size: size as u64,
            has_not_found_files,
            path: rule.settings.path.clone().unwrap_or_default(),
            scan_queue_count: 0,
            absolute_numbering: rule.settings.absolute_numbering.unwrap_or(false),
            aliases: rule.settings.aliases.clone(),
        });
    }

    tracing::debug!("get_series completed: {} series in list", series_list.len());
    Json(series_list)
}

pub async fn get_series_details(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<Option<jumbie_shared::types::SeriesDetails>> {
    tracing::debug!("get_series_details called: series_id={}", id);
    let mapping = match state.db.get_series_mapping(&id).await.ok().flatten() {
        Some(m) => m,
        None => {
            tracing::debug!("get_series_details: series {} not found", id);
            return Json(None);
        }
    };

    // Basic stats — looked up by series_id (UUID), not title.
    let stats = state.db.get_series_stats().await.unwrap_or_default();
    let (count, size, _mmc) = stats
        .iter()
        .find(|(sid, _, _, _, _)| sid == &id)
        .map(|(_, _, c, s, _)| (*c, *s, 0))
        .unwrap_or((0, 0, 0));

    // See `get_series` for the fallback-date rationale. Red = monitored missing
    // (wanted); yellow = anything actually in the download queue, monitored or not.
    let mmc = monitored_missing_counts_by_series(
        &state.db.get_wanted_episodes().await.unwrap_or_default(),
    )
    .get(&id)
    .copied()
    .unwrap_or(0);
    let queued_count = state
        .db
        .get_queued_episode_counts_by_series()
        .await
        .unwrap_or_default()
        .get(&id)
        .copied()
        .unwrap_or(0);

    let db_seasons = state.db.get_series_seasons(&id).await.unwrap_or_default();

    // SSoT: effective numbering mode (series tristate → global default).
    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };
    let active_mode = mapping.settings.active_mode(global_absolute);
    let suppressed_seasons: HashSet<i32> = state
        .db
        .get_suppressed_seasons(&id, active_mode.is_absolute() as i32)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let normalized_seasons = build_season_list(
        db_seasons,
        mapping.settings.season_for_mode(active_mode).values(),
        &suppressed_seasons,
    );

    // Cells are stored in the episodes table with episode_cell_type = 0 so they
    // participate in release date estimation, calendar view, etc.
    if let Err(e) = state
        .db
        .ensure_episode_cells(&mapping, &normalized_seasons, active_mode.is_absolute())
        .await
    {
        tracing::warn!(
            "Failed to ensure episode cells for series '{}': {}",
            mapping.target_title,
            e
        );
    }

    let formatted_seasons = crate::utils::combine_seasons(&normalized_seasons);
    let season_count = if active_mode.is_absolute() {
        0
    } else {
        normalized_seasons.len() as i32
    };
    // SSoT: expected total — same helper as the library list, so both agree.
    let all_season_counts = state
        .db
        .get_series_season_completion_counts()
        .await
        .unwrap_or_default();
    let total = expected_total_episodes(
        &mapping.settings,
        &normalized_seasons,
        &suppressed_seasons,
        all_season_counts.get(&id).map(Vec::as_slice).unwrap_or(&[]),
        global_absolute,
    );

    let has_not_found_files = count > 0
        && mapping
            .settings
            .path
            .as_ref()
            .is_some_and(|p| !p.is_empty() && !std::path::Path::new(p).exists());

    let info = SeriesInfo {
        id: id.clone(),
        title: mapping.target_title.clone(),
        seasons: formatted_seasons,
        season_count,
        release_profile: mapping.release_profile.clone().unwrap_or_default(),
        quality_profile: mapping.quality_profile.clone().unwrap_or_default(),
        episodes_counts: (count as i32, total),
        monitored_missing_count: mmc,
        queued_count,
        size: size as u64,
        has_not_found_files,
        path: mapping.settings.path.clone().unwrap_or_default(),
        scan_queue_count: 0,
        absolute_numbering: active_mode.is_absolute(),
        aliases: mapping.settings.aliases.clone(),
    };

    // Get detailed episodes
    let db_episodes = match state
        .db
        .get_series_episodes_details(&id, active_mode.is_absolute())
        .await
    {
        Ok(eps) => eps,
        Err(e) => {
            tracing::error!(
                "Failed to get episodes for series '{}': {}",
                mapping.target_title,
                e
            );
            Vec::new()
        }
    };

    let ep_ids: Vec<String> = db_episodes.iter().map(|r| r.episode_id.clone()).collect();
    let episodes_in_queue = state
        .db
        .get_active_queue_episode_ids(&ep_ids)
        .await
        .unwrap_or_default();
    let parts_flat = state
        .db
        .get_parts_for_series(&ep_ids)
        .await
        .unwrap_or_default();

    let mut parts_by_episode: std::collections::HashMap<String, Vec<crate::db::EpisodePartRow>> =
        std::collections::HashMap::new();
    for (ep_id, part_row) in parts_flat {
        parts_by_episode.entry(ep_id).or_default().push(part_row);
    }

    let ui_prefs = state.db.get_ui_preferences().await.unwrap_or_default();
    let release_date_config = ui_prefs.release_date_display;

    let file_existence = crate::api_routes::series_detail_builder::resolve_file_existence(
        &db_episodes,
        &parts_by_episode,
    )
    .await;
    let aux_by_episode = state
        .db
        .get_auxiliary_files_for_series(&id)
        .await
        .unwrap_or_default();
    let (episode_map, _) = crate::api_routes::series_detail_builder::map_db_rows_to_view_models(
        db_episodes,
        &parts_by_episode,
        &aux_by_episode,
        &episodes_in_queue,
        &release_date_config,
        &file_existence,
    );

    // Multiple providers may have the same series data. Rather than merging (risking
    // duplicates), the first provider returning non-empty data wins; the primary
    // provider should be listed first in `metadata_ids`.
    let metadata_seasons = {
        let mut info_vec: Vec<jumbie_shared::types::MetadataSeasonInfo> = Vec::new();

        let ordering_mode = if active_mode.is_absolute() {
            "absolute"
        } else {
            "normal"
        };

        // Iterate providers in PRIORITY order (enabled + capability filtered) until
        // one has metadata seasons — deterministic, never `HashMap` order.
        let instance_plugins = crate::utils::metadata::instance_plugin_id_map(&state.db).await;
        let ordered_instances = state
            .plugin_manager
            .read()
            .await
            .ordered_metadata_providers();
        let providers = crate::utils::metadata::ordered_series_providers(
            &mapping,
            &ordered_instances,
            &instance_plugins,
        );
        for provider in &providers {
            let primary = state
                .db
                .get_metadata_season_cache(
                    &provider.metadata_id,
                    &provider.plugin_id,
                    &provider.instance_id,
                    ordering_mode,
                )
                .await
                .unwrap_or_default();

            let (rows, is_fallback) = (primary, false);

            if !rows.is_empty() {
                for (season, count) in rows {
                    // SSoT: absolute metadata is canonically season 1; a normal-mode
                    // label with no numeric equivalent has no episode identity, so
                    // skip that row rather than labelling it season 0.
                    let season_num = match jumbie_shared::mapping::resolve_season_num(
                        &season,
                        active_mode.is_absolute(),
                    ) {
                        Ok(n) => n,
                        Err(e) => {
                            tracing::warn!(
                                "get_series_details: skipping metadata season {:?} for series {} ({}): {}",
                                season,
                                mapping.target_title,
                                id,
                                e
                            );
                            continue;
                        }
                    };
                    info_vec.push(jumbie_shared::types::MetadataSeasonInfo {
                        season_number: season_num,
                        episode_count: count,
                        title: None,
                        premiere_date: None,
                        end_date: None,
                        image_url: None,
                        summary: None,
                        provider_instance_id: provider.instance_id.clone(),
                        is_fallback_mode: is_fallback,
                    });
                }
                break;
            }
        }

        info_vec
    };

    let view_episodes = crate::api_routes::series_detail_builder::fill_missing_episodes(
        &mapping,
        &normalized_seasons,
        &episode_map,
        active_mode.is_absolute(),
    );

    tracing::debug!(
        "get_series_details completed for series {} ({}): {} episodes",
        mapping.target_title,
        id,
        view_episodes.len()
    );

    let mut suppressed_sorted: Vec<i32> = suppressed_seasons.iter().copied().collect();
    suppressed_sorted.sort_unstable();
    let suppressed_infos = suppressed_season_infos(&suppressed_sorted, &metadata_seasons);

    Json(Some(jumbie_shared::types::SeriesDetails {
        info,
        // DB stores canonical UTC; the API contract is RFC 3339.
        config: {
            let mut cfg = mapping.clone();
            crate::datetime::synced_map_to_rfc3339(&mut cfg.settings.metadata_last_synced_at);
            cfg
        },
        episodes: view_episodes,
        metadata_seasons,
        suppressed_seasons: suppressed_infos,
    }))
}

#[derive(serde::Deserialize)]
pub struct BatchDetailsRequest {
    pub ids: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

pub async fn get_series_details_batch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchDetailsRequest>,
) -> Result<Json<HashMap<String, jumbie_shared::types::SeriesDetails>>, AppError> {
    tracing::debug!(
        "get_series_details_batch called: {} series ids",
        req.ids.len()
    );
    // These queries return ALL rows from each table and are filtered client-side
    // below: the tables are small (hundreds of rows) and otherwise each would need
    // N round-trips for N series.
    let all_mappings = state
        .db
        .get_all_series_mappings()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;

    let stats = state.db.get_series_stats().await.unwrap_or_default();

    let id_set: HashSet<&str> = req.ids.iter().map(|s| s.as_str()).collect();

    let mut requested: Vec<(&String, &jumbie_shared::types::MappingRule)> = all_mappings
        .iter()
        .filter(|(id, _)| id_set.contains(id.as_str()))
        .collect();

    // Apply pagination: sort for deterministic ordering, then slice.
    // When limit/offset are not provided, all matching IDs are returned.
    requested.sort_by_key(|(a, _)| *a);
    if let Some(offset) = req.offset {
        if offset >= requested.len() {
            return Ok(Json(HashMap::new()));
        }
        requested.drain(..offset);
    }
    if let Some(limit) = req.limit {
        requested.truncate(limit);
    }

    if requested.is_empty() {
        return Ok(Json(HashMap::new()));
    }

    let series_ids: Vec<String> = requested.iter().map(|(id, _)| (*id).clone()).collect();

    let seasons_batch = state
        .db
        .get_series_seasons_batch(&series_ids)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;

    // Configured cell slots are created as DB rows before the main episode fetch so
    // they are picked up by `get_series_episodes_details_batch`.
    let cell_global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };
    for (id, mapping) in &requested {
        let active_mode = mapping.settings.active_mode(cell_global_absolute);
        let db_seasons_for_cells = seasons_batch.get(*id).cloned().unwrap_or_default();
        let suppressed: HashSet<i32> = state
            .db
            .get_suppressed_seasons(id, active_mode.is_absolute() as i32)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        let norm_seasons = build_season_list(
            db_seasons_for_cells,
            mapping.settings.season_for_mode(active_mode).values(),
            &suppressed,
        );
        if let Err(e) = state
            .db
            .ensure_episode_cells(mapping, &norm_seasons, cell_global_absolute)
            .await
        {
            tracing::warn!(
                "Failed to ensure episode cells for series '{}': {}",
                mapping.target_title,
                e
            );
        }
    }

    // Re-fetch episodes to include any newly created cell rows
    // Split series_ids by numbering mode because the batch-mode query filters on mode.
    let abs_series_ids: Vec<String> = requested
        .iter()
        .filter(|(_, m)| m.settings.active_mode(cell_global_absolute).is_absolute())
        .map(|(id, _)| (*id).clone())
        .collect();
    let norm_series_ids: Vec<String> = requested
        .iter()
        .filter(|(_, m)| !m.settings.active_mode(cell_global_absolute).is_absolute())
        .map(|(id, _)| (*id).clone())
        .collect();

    let mut episodes_batch: HashMap<String, Vec<crate::db::EpisodeDetailRow>> = HashMap::new();
    if !abs_series_ids.is_empty() {
        let abs_batch = state
            .db
            .get_series_episodes_details_batch(&abs_series_ids, true)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
        episodes_batch.extend(abs_batch);
    }
    if !norm_series_ids.is_empty() {
        let norm_batch = state
            .db
            .get_series_episodes_details_batch(&norm_series_ids, false)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?;
        episodes_batch.extend(norm_batch);
    }

    let all_ep_ids: Vec<String> = episodes_batch
        .values()
        .flat_map(|eps| eps.iter().map(|r| r.episode_id.clone()))
        .collect();

    let parts_flat = state
        .db
        .get_parts_for_series(&all_ep_ids)
        .await
        .unwrap_or_default();

    let mut parts_by_episode: HashMap<String, Vec<crate::db::EpisodePartRow>> = HashMap::new();
    for (ep_id, part_row) in parts_flat {
        parts_by_episode.entry(ep_id).or_default().push(part_row);
    }

    let stats_map: HashMap<&str, (i64, i64, i64)> = stats
        .iter()
        .map(|(sid, _, count, size, mmc)| (sid.as_str(), (*count, *size, *mmc)))
        .collect();

    // One batched query per primary ordering mode.
    let mut primary_queries: Vec<(String, String, String, String)> = Vec::new();

    // instance id → plugin_id (backend TYPE id) and the priority-ordered provider
    // instances: built once for the whole batch. Cache key is
    // (metadata_id, plugin_id, instance_id).
    let instance_plugins = crate::utils::metadata::instance_plugin_id_map(&state.db).await;
    let ordered_instances = state
        .plugin_manager
        .read()
        .await
        .ordered_metadata_providers();

    for (_id, mapping) in &requested {
        let ordering_mode = if mapping
            .settings
            .active_mode(cell_global_absolute)
            .is_absolute()
        {
            "absolute"
        } else {
            "normal"
        };
        for provider in crate::utils::metadata::ordered_series_providers(
            mapping,
            &ordered_instances,
            &instance_plugins,
        ) {
            primary_queries.push((
                provider.metadata_id,
                provider.plugin_id,
                provider.instance_id,
                ordering_mode.to_string(),
            ));
        }
    }

    let primary_batch = state
        .db
        .get_metadata_season_cache_batch(&primary_queries)
        .await
        .unwrap_or_default();

    // Cache: metadata_id -> (rows, is_fallback); primary results only, no
    // cross-mode fallback.
    let mut metadata_seasons_cache: HashMap<String, (Vec<(String, i32)>, bool)> = HashMap::new();
    for (tid, _provider, _instance_id, _ordering_mode) in &primary_queries {
        if let Some(rows) = primary_batch.get(tid)
            && !rows.is_empty()
        {
            metadata_seasons_cache.insert(tid.clone(), (rows.clone(), false));
        }
    }

    let episodes_in_queue = state
        .db
        .get_active_queue_episode_ids(&all_ep_ids)
        .await
        .unwrap_or_default();

    let ui_prefs = state.db.get_ui_preferences().await.unwrap_or_default();
    let release_date_config = ui_prefs.release_date_display;

    // Same SSoT as `get_series`: red = monitored missing (wanted), yellow = anything
    // actually in the download queue (monitored or not).
    let missing_by_series = monitored_missing_counts_by_series(
        &state.db.get_wanted_episodes().await.unwrap_or_default(),
    );
    let queued_by_series = state
        .db
        .get_queued_episode_counts_by_series()
        .await
        .unwrap_or_default();

    let mut result: HashMap<String, jumbie_shared::types::SeriesDetails> = HashMap::new();

    // SSoT: effective numbering-mode default — read once for the whole loop.
    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };

    // SSoT: per-season episode counts, used with `expected_total_episodes` so the
    // batch detail totals match the library list.
    let all_season_counts = state
        .db
        .get_series_season_completion_counts()
        .await
        .unwrap_or_default();

    for (id, mapping) in &requested {
        let id = (*id).clone();

        let (count, size, _mmc) = stats_map.get(id.as_str()).copied().unwrap_or((0, 0, 0));
        let missing_count = missing_by_series.get(&id).copied().unwrap_or(0);
        let queued_count = queued_by_series.get(&id).copied().unwrap_or(0);

        let db_seasons = seasons_batch.get(&id).cloned().unwrap_or_default();

        let active_mode = mapping.settings.active_mode(global_absolute);
        let suppressed: HashSet<i32> = state
            .db
            .get_suppressed_seasons(&id, active_mode.is_absolute() as i32)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        let normalized_seasons = build_season_list(
            db_seasons,
            mapping.settings.season_for_mode(active_mode).values(),
            &suppressed,
        );
        let formatted_seasons = crate::utils::combine_seasons(&normalized_seasons);
        let season_count = if active_mode.is_absolute() {
            0
        } else {
            normalized_seasons.len() as i32
        };
        let total = expected_total_episodes(
            &mapping.settings,
            &normalized_seasons,
            &suppressed,
            all_season_counts.get(&id).map(Vec::as_slice).unwrap_or(&[]),
            global_absolute,
        );

        let has_not_found_files = count > 0
            && mapping
                .settings
                .path
                .as_ref()
                .is_some_and(|p| !p.is_empty() && !std::path::Path::new(p).exists());

        let info = SeriesInfo {
            id: id.to_string(),
            title: mapping.target_title.clone(),
            seasons: formatted_seasons,
            season_count,
            release_profile: mapping.release_profile.clone().unwrap_or_default(),
            quality_profile: mapping.quality_profile.clone().unwrap_or_default(),
            episodes_counts: (count as i32, total),
            monitored_missing_count: missing_count,
            queued_count,
            size: size as u64,
            has_not_found_files,
            path: mapping.settings.path.clone().unwrap_or_default(),
            scan_queue_count: 0,
            absolute_numbering: active_mode.is_absolute(),
            aliases: mapping.settings.aliases.clone(),
        };

        // Episode view models (episodes_batch is keyed by series_id)
        let db_episodes = episodes_batch.get(&id as &str).cloned().unwrap_or_default();

        let file_existence = crate::api_routes::series_detail_builder::resolve_file_existence(
            &db_episodes,
            &parts_by_episode,
        )
        .await;
        let aux_by_episode = state
            .db
            .get_auxiliary_files_for_series(&id)
            .await
            .unwrap_or_default();
        let (episode_map, _) = crate::api_routes::series_detail_builder::map_db_rows_to_view_models(
            db_episodes,
            &parts_by_episode,
            &aux_by_episode,
            &episodes_in_queue,
            &release_date_config,
            &file_existence,
        );

        // Uses the pre-fetched cache above instead of N per-series DB round-trips;
        // only for the frontend "Match" button and season header display, not for
        // fill_missing_episodes.
        let metadata_seasons = {
            let mut info_vec: Vec<jumbie_shared::types::MetadataSeasonInfo> = Vec::new();

            for provider in crate::utils::metadata::ordered_series_providers(
                mapping,
                &ordered_instances,
                &instance_plugins,
            ) {
                if let Some((rows, is_fallback)) = metadata_seasons_cache.get(&provider.metadata_id)
                    && !rows.is_empty()
                {
                    for (season, ep_count) in rows {
                        // SSoT: absolute metadata is canonically season 1; a normal-mode
                        // label with no numeric equivalent has no episode identity, so
                        // skip that row rather than labelling it season 0.
                        let season_num = match jumbie_shared::mapping::resolve_season_num(
                            season,
                            active_mode.is_absolute(),
                        ) {
                            Ok(n) => n,
                            Err(e) => {
                                tracing::warn!(
                                    "get_series_details_batch: skipping metadata season {:?} for series {} ({}): {}",
                                    season,
                                    mapping.target_title,
                                    id,
                                    e
                                );
                                continue;
                            }
                        };
                        info_vec.push(jumbie_shared::types::MetadataSeasonInfo {
                            season_number: season_num,
                            episode_count: *ep_count,
                            title: None,
                            premiere_date: None,
                            end_date: None,
                            image_url: None,
                            summary: None,
                            provider_instance_id: provider.instance_id.clone(),
                            is_fallback_mode: *is_fallback,
                        });
                    }
                    break;
                }
            }

            info_vec
        };

        let view_episodes = crate::api_routes::series_detail_builder::fill_missing_episodes(
            mapping,
            &normalized_seasons,
            &episode_map,
            active_mode.is_absolute(),
        );

        let mut suppressed_sorted: Vec<i32> = state
            .db
            .get_suppressed_seasons(&id, active_mode.is_absolute() as i32)
            .await
            .unwrap_or_default();
        suppressed_sorted.sort_unstable();
        let suppressed_infos = suppressed_season_infos(&suppressed_sorted, &metadata_seasons);

        result.insert(
            id.to_string(),
            jumbie_shared::types::SeriesDetails {
                info,
                // DB stores canonical UTC; the API contract is RFC 3339.
                config: {
                    let mut cfg = (*mapping).clone();
                    crate::datetime::synced_map_to_rfc3339(
                        &mut cfg.settings.metadata_last_synced_at,
                    );
                    cfg
                },
                episodes: view_episodes,
                metadata_seasons,
                suppressed_seasons: suppressed_infos,
            },
        );
    }

    tracing::debug!(
        "get_series_details_batch completed: {} results",
        result.len()
    );
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(series_id: &str, status: &str) -> WantedEpisode {
        WantedEpisode {
            series_id: series_id.to_string(),
            series_title: String::new(),
            episode_id: String::new(),
            season: None,
            episode: 0,
            title: None,
            eff_date: String::new(),
            dates: jumbie_shared::types::ReleaseDates {
                meta_date: None,
                upload_date: None,
                est_date: None,
            },
            status: status.to_string(),
        }
    }

    #[test]
    fn test_missing_counts_only_missing() {
        // `in_queue` and `unreleased` must not count as missing; the queued
        // indicator is sourced from the download queue, not these rows.
        let rows = vec![
            wanted("s1", "missing"),
            wanted("s1", "in_queue"),
            wanted("s1", "unreleased"),
            wanted("s2", "in_queue"),
            wanted("s2", "missing"),
            wanted("s2", "missing"),
        ];
        let counts = monitored_missing_counts_by_series(&rows);
        assert_eq!(counts.get("s1"), Some(&1));
        assert_eq!(counts.get("s2"), Some(&2));
        assert_eq!(counts.get("s3"), None);
    }
}
