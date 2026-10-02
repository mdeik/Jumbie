use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::api::AppState;

/// Guards against concurrent `sweep_monitor_status` executions.
/// A second call while one is in progress returns immediately.
static SWEEP_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Resets `SWEEP_IN_PROGRESS` on drop (including panic).
struct SweepGuard;

impl Drop for SweepGuard {
    fn drop(&mut self) {
        SWEEP_IN_PROGRESS.store(false, Ordering::Release);
    }
}

/// Apply the stored monitor mode to all episodes of a series.
///
/// Thin wrapper around [`source_processor::reapply_monitor_for_series`] — the
/// SSoT loop; the mode is read from the stored mapping.
pub async fn apply_monitor_mode(state: &Arc<AppState>, series_id: &str) -> Result<(), String> {
    crate::source_processor::reapply_monitor_for_series(
        &state.db,
        series_id,
        true, // fresh_apply: user-initiated mode change (clears overrides)
        {
            let c = state.cfg.read().await;
            c.general.absolute_numbering
        },
    )
    .await;

    tracing::debug!("apply_monitor_mode completed for series {}", series_id);
    Ok(())
}

/// Batch variant of [`apply_monitor_mode`]: applies the same monitor mode to
/// many series with a bounded number of DB queries — one batched mappings fetch,
/// at most two batched episode fetches (split by numbering mode), and at most
/// two bulk UPDATEs for the aggregated episode IDs.
///
/// SSoT: per-episode monitor logic stays in
/// [`ContentOrganizer::should_monitor_episode`]; this function is purely a
/// batch orchestrator.
pub async fn batch_apply_monitor_mode(
    state: &Arc<AppState>,
    series_ids: &[String],
    mode: jumbie_shared::types::MonitorMode,
    fresh_apply: bool,
) {
    tracing::debug!(
        "batch_apply_monitor_mode called: {} series, mode={:?}",
        series_ids.len(),
        mode
    );

    if series_ids.is_empty() {
        return;
    }

    // A fresh apply is an explicit reset — clear all user overrides before
    // re-evaluating every episode from scratch.
    if fresh_apply {
        let _ = state.db.clear_monitor_overrides_batch(series_ids).await;
    }

    // Fetch all mappings in one batched query.
    let mappings = match state.db.get_series_mappings_batch(series_ids).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("batch_apply_monitor_mode: failed to fetch mappings: {}", e);
            return;
        }
    };

    // `get_series_episodes_details_batch` requires all series in a call to share
    // the same numbering_mode, so split into normal vs absolute groups to make at
    // most 2 batched queries instead of N individual ones.
    let mut normal_series: Vec<String> = Vec::new();
    let mut absolute_series: Vec<String> = Vec::new();
    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };
    for id in series_ids {
        match mappings.get(id) {
            Some(m) if m.settings.active_mode(global_absolute).is_absolute() => {
                absolute_series.push(id.clone())
            }
            Some(_) => normal_series.push(id.clone()),
            None => {
                tracing::debug!(
                    "batch_apply_monitor_mode: series {} not found, skipping",
                    id
                );
            }
        }
    }

    let mut episodes_by_series: HashMap<String, Vec<crate::db::EpisodeDetailRow>> = HashMap::new();

    if !normal_series.is_empty() {
        match state
            .db
            .get_series_episodes_details_batch(&normal_series, false)
            .await
        {
            Ok(eps) => episodes_by_series.extend(eps),
            Err(e) => {
                tracing::error!(
                    "batch_apply_monitor_mode: failed to fetch normal episodes: {}",
                    e
                );
            }
        }
    }
    if !absolute_series.is_empty() {
        match state
            .db
            .get_series_episodes_details_batch(&absolute_series, true)
            .await
        {
            Ok(eps) => episodes_by_series.extend(eps),
            Err(e) => {
                tracing::error!(
                    "batch_apply_monitor_mode: failed to fetch absolute episodes: {}",
                    e
                );
            }
        }
    }

    // fresh_apply already cleared overrides, so the set is empty then.
    let overridden: HashSet<String> = if fresh_apply {
        HashSet::new()
    } else {
        state
            .db
            .get_overridden_episode_ids(series_ids)
            .await
            .unwrap_or_default()
    };

    // `Future` mode should key off the user's preferred date (metadata, source,
    // or estimated) rather than always the raw metadata `meta_date`.
    let ui_prefs = state.db.get_ui_preferences().await.unwrap_or_default();
    let rd_config = ui_prefs.release_date_display;

    let mut all_to_monitor: Vec<String> = Vec::new();
    let mut all_to_unmonitor: Vec<String> = Vec::new();
    let mut all_to_clear_override: Vec<String> = Vec::new();

    for id in series_ids {
        let mapping = match mappings.get(id) {
            Some(m) => m,
            None => continue,
        };

        let Some(db_episodes) = episodes_by_series.remove(id) else {
            continue;
        };

        // Classify each episode — delegates to the SSoT helper shared with
        // `reapply_monitor_for_series`.
        let (series_to_monitor, series_to_unmonitor, series_to_clear_override) =
            crate::source_processor::classify_db_episodes(
                db_episodes,
                mapping,
                mode,
                &rd_config,
                fresh_apply,
                &overridden,
                global_absolute,
            );
        all_to_monitor.extend(series_to_monitor);
        all_to_unmonitor.extend(series_to_unmonitor);
        all_to_clear_override.extend(series_to_clear_override);
    }

    if !all_to_monitor.is_empty()
        && let Err(e) = state
            .db
            .update_episode_monitor_status(&all_to_monitor, true)
            .await
    {
        tracing::error!(
            "batch_apply_monitor_mode: failed to set monitored status: {}",
            e
        );
    }

    if !all_to_unmonitor.is_empty()
        && let Err(e) = state
            .db
            .update_episode_monitor_status(&all_to_unmonitor, false)
            .await
    {
        tracing::error!(
            "batch_apply_monitor_mode: failed to set unmonitored status: {}",
            e
        );
    }

    if !all_to_clear_override.is_empty()
        && let Err(e) = state
            .db
            .clear_monitor_overrides_for_episodes(&all_to_clear_override)
            .await
    {
        tracing::error!(
            "batch_apply_monitor_mode: failed to clear stale overrides: {}",
            e
        );
    }

    tracing::debug!(
        "batch_apply_monitor_mode completed: {} monitored, {} unmonitored, {} overrides cleared across {} series",
        all_to_monitor.len(),
        all_to_unmonitor.len(),
        all_to_clear_override.len(),
        series_ids.len()
    );
}

/// Sweep the entire library and re-apply the effective monitor mode for every
/// series whose mode depends on time or file state.
///
/// Series are grouped by stored `monitor_mode` and delegated to
/// [`batch_apply_monitor_mode`], reducing a full sweep from O(N × 4) to
/// O(G × 4) queries where G is the number of distinct modes (typically 2–5).
/// `MonitorMode::All` and `MonitorMode::None` are static and skipped.
pub async fn sweep_monitor_status(state: &Arc<AppState>) {
    // Skip if a sweep is already running, so the debounced preference-change
    // trigger cannot overlap the periodic sweep.
    if SWEEP_IN_PROGRESS.swap(true, Ordering::AcqRel) {
        tracing::debug!("sweep_monitor_status: already in progress, skipping");
        return;
    }
    let _guard = SweepGuard;

    use jumbie_shared::types::{DEFAULT_MONITOR_MODE, MonitorMode};
    use std::collections::HashMap;

    let mappings = match state.db.get_all_series_mappings().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("sweep_monitor_status: failed to fetch mappings: {}", e);
            return;
        }
    };

    let mut by_mode: HashMap<MonitorMode, Vec<String>> = HashMap::new();
    for (series_id, mapping) in &mappings {
        let mode = mapping
            .settings
            .monitor_mode
            .unwrap_or(DEFAULT_MONITOR_MODE);
        if mode == MonitorMode::All || mode == MonitorMode::None {
            continue;
        }
        by_mode.entry(mode).or_default().push(series_id.clone());
    }

    let total: usize = by_mode.values().map(|v| v.len()).sum();
    if total == 0 {
        tracing::debug!("sweep_monitor_status: no series need refresh");
        return;
    }

    let mode_count = by_mode.len();
    for (mode, series_ids) in by_mode {
        batch_apply_monitor_mode(state, &series_ids, mode, false).await; // sweep: preserve state
    }

    tracing::debug!(
        "sweep_monitor_status completed: {} series across {} mode(s)",
        total,
        mode_count,
    );
}
