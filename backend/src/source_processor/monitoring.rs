use std::collections::HashSet;

use chrono::{NaiveDateTime, Utc};

use crate::datetime::compute_effective_date;
use crate::db::{DbManager, EpisodeDetailRow};
use crate::organizer::ContentOrganizer;
use jumbie_shared::config::ui::ReleaseDateDisplayConfig;
use jumbie_shared::mapping::{MonitorMode, SeasonOverride};
use jumbie_shared::types::MappingRule;

/// Decision returned by [`override_decision`] for how to handle a monitored
/// flag that may have been user-overridden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverrideAction {
    /// No override — proceed with normal evaluation.
    Proceed,
    /// Override is stale (mode_wants == current) — caller should clear the
    /// override flag. The monitored value is already correct, no write needed.
    SelfHeal,
    /// User's override diverges from the mode — skip this episode entirely.
    Skip,
}

/// SSoT for the override decision lifecycle.
///
/// Determines whether an episode with a possible `monitor_override` should be
/// evaluated, skipped, or self-healed. Called by both `classify_db_episodes`
/// (batch sweep) and `reapply_monitor_for_episode` (single-episode path).
pub(crate) fn override_decision(
    has_override: bool,
    mode_wants: bool,
    current: bool,
) -> OverrideAction {
    if !has_override {
        OverrideAction::Proceed
    } else if mode_wants == current {
        OverrideAction::SelfHeal
    } else {
        OverrideAction::Skip
    }
}

pub struct MonitorParams<'a> {
    pub mode: Option<MonitorMode>,
    pub season_str: &'a str,
    pub ep_num: i32,
    pub has_file: bool,
    pub currently_monitored: bool,
    pub effective_date: Option<NaiveDateTime>,
    pub season_override: Option<&'a SeasonOverride>,
}

impl ContentOrganizer {
    /// SSoT for monitor-mode logic. Determines whether an episode SHOULD be treated
    /// as monitored based on the configured MonitorMode and episode metadata. Used by
    /// both `apply_monitor_mode` (existing DB episodes) and `select_winners`
    /// (newly-discovered RSS episodes).
    ///
    /// # `effective_date` semantics
    /// The best available date for deciding if an episode is "future" (unreleased).
    /// Callers compute it for their context:
    ///
    ///   - **DB episodes** (`reapply_monitor_for_series`, `batch_apply_monitor_mode`):
    ///     call `compute_effective_date(meta_date, upload_date, est_date, …)` with the
    ///     user's release-date display preferences so `Future` mode respects their
    ///     chosen priority.
    ///
    ///   - **Non-DB RSS episodes** (`episode_is_effectively_monitored`): always
    ///     `false` — an episode cell must exist in the DB before auto-download will
    ///     consider it, regardless of monitor mode.
    ///
    /// Episode range constraints (season overrides) are checked BEFORE the mode
    /// match, so they act as a hard gate — outside a season's configured range the
    /// episode is NEVER monitored regardless of mode.
    ///
    /// # `currently_monitored` semantics
    /// Only meaningful for `Future` mode. Pass the episode's current DB `monitored`
    /// flag so episodes previously future-dated (monitored = 1) stay monitored once
    /// their date passes but no file was downloaded yet. Non-DB callers (RSS) pass
    /// `false` since there is no prior state.
    pub fn should_monitor_episode(params: MonitorParams<'_>) -> bool {
        let mode = match params.mode {
            Some(m) => m,
            // No mode configured → nothing is monitored (same as MonitorMode::None)
            None => return false,
        };

        // Season-override range check: ep_num is already in DB/local numbering (no offset).
        if let Some(override_rule) = params.season_override
            && !override_rule.contains_local_episode(params.ep_num)
        {
            return false;
        }

        let is_special = params.season_str == "0" || params.season_str == "00";
        let is_season_1 = params.season_str == "1" || params.season_str == "01";

        match mode {
            MonitorMode::All => !is_special,
            MonitorMode::Future => {
                let now = Utc::now().naive_utc();
                match params.effective_date {
                    // Strictly in the future → monitor (waiting for release)
                    Some(d) if d > now => true,
                    // Same calendar day as now (date-only metadata, release later today)
                    // and still missing → monitor (the release hasn't happened yet today)
                    Some(d) if d.date() >= now.date() && !params.has_file => true,
                    // Calendar day has passed: keep monitoring ONLY if it was previously
                    // future-dated (already monitored) and still needs a file.
                    // If it was never monitored, don't start now — the user
                    // chose Future mode, not Missing mode.
                    Some(_) => params.currently_monitored && !params.has_file,
                    // No date at all → can't determine if it's future
                    None => false,
                }
            }
            MonitorMode::Missing => !params.has_file,
            MonitorMode::Existing => params.has_file,
            MonitorMode::Pilot => is_season_1 && params.ep_num == 1,
            MonitorMode::FirstSeason => is_season_1,
            MonitorMode::Specials => is_special,
            MonitorMode::None => false,
        }
    }

    /// Hard gate: an episode cannot be automatically downloaded if its episode
    /// cell does not exist in the DB.  This applies to ALL monitor modes.
    /// Users must register the episode first (e.g. via metadata scan/sync) before
    /// auto-download will consider it.  The episode cell count is the source of
    /// truth — even if metadata would imply more episodes should exist, the hard
    /// gate enforces that only explicitly registered episodes are auto-downloaded.
    pub(crate) fn episode_is_effectively_monitored(
        _mapping: &MappingRule,
        _season_str: &str,
        _ep_num: i32,
    ) -> bool {
        false
    }

    /// Re-apply the current monitor mode for a single series by delegating to
    /// [`reapply_monitor_for_series`] — the SSoT loop.
    pub async fn refresh_monitor_status_for_series(&self, series_id: &str) {
        let global_absolute = self.db_config().await.general.absolute_numbering;
        reapply_monitor_for_series(&self.db, series_id, false, global_absolute).await;
    }

    /// Re-apply monitor status for a single episode after its state changed
    /// (e.g. organize completed).  Respects `monitor_override` — if the user
    /// explicitly toggled this episode, their choice is preserved.
    pub async fn refresh_monitor_status_for_episode(&self, series_id: &str, episode_id: &str) {
        let global_absolute = self.db_config().await.general.absolute_numbering;
        reapply_monitor_for_episode(&self.db, series_id, episode_id, false, global_absolute).await;
    }
}

/// SSoT loop: fetch episodes for a series, classify each with
/// [`ContentOrganizer::should_monitor_episode`] given current conditions (has_file),
/// and batch-update the DB `monitored` flag. Shared by the organize flow and the
/// standalone `apply_monitor_mode` / `batch_apply_monitor_mode`.
///
/// # Parameters
/// * `fresh_apply = true`:   treat as a fresh mode change — `Future` mode ignores
///   the current DB `monitored` flag so past-dated episodes aren't retroactively
///   picked up.  Used by API-initiated mode switches.  Also clears all
///   `monitor_override` flags for the series.
/// * `fresh_apply = false`:  preserve existing `monitored` state — previously-future
///   episodes stay monitored until downloaded.  Respects `monitor_override`:
///   overridden episodes are skipped (user toggle wins), with self-heal if the
///   mode's evaluation has caught up to the user's choice.
/// * `global_absolute`:      the global `general.absolute_numbering` default —
///   series tristate overrides (`Some(true)`/`Some(false)`) win; `None` falls
///   back to this value when resolving the active numbering mode.
pub async fn reapply_monitor_for_series(
    db: &DbManager,
    series_id: &str,
    fresh_apply: bool,
    global_absolute: bool,
) {
    let mapping = match db.get_series_mapping(series_id).await {
        Ok(Some(m)) => m,
        _ => return,
    };
    let Some(mode) = mapping.settings.monitor_mode else {
        return;
    };

    // Fresh apply (mode switch) is an explicit reset — clear all user overrides first.
    if fresh_apply {
        let _ = db.clear_monitor_overrides_for_series(series_id).await;
    }

    let db_episodes: Vec<EpisodeDetailRow> = match db
        .get_series_episodes_details(
            series_id,
            mapping.settings.active_mode(global_absolute).is_absolute(),
        )
        .await
    {
        Ok(eps) => eps,
        Err(_) => return,
    };

    // `Future` mode should use the user's preferred date source (metadata, source,
    // or estimated) rather than always the raw metadata `meta_date`.
    let ui_prefs = db.get_ui_preferences().await.unwrap_or_default();
    let rd_config = ui_prefs.release_date_display;

    // fresh_apply=true already cleared overrides, so the set is empty there.
    let overridden: HashSet<String> = if fresh_apply {
        HashSet::new()
    } else {
        db.get_overridden_episode_ids(&[series_id.to_string()])
            .await
            .unwrap_or_default()
    };

    let (to_monitor, to_unmonitor, to_clear_override) = classify_db_episodes(
        db_episodes,
        &mapping,
        mode,
        &rd_config,
        fresh_apply,
        &overridden,
        global_absolute,
    );

    if !to_monitor.is_empty() {
        let _ = db.update_episode_monitor_status(&to_monitor, true).await;
    }
    if !to_unmonitor.is_empty() {
        let _ = db.update_episode_monitor_status(&to_unmonitor, false).await;
    }
    if !to_clear_override.is_empty() {
        let _ = db
            .clear_monitor_overrides_for_episodes(&to_clear_override)
            .await;
    }
}

/// Apply monitor status for a single episode after a state change (organize).
/// Respects `monitor_override`: if the user explicitly toggled this episode,
/// their choice is preserved unless the mode's evaluation now agrees (self-heal).
pub async fn reapply_monitor_for_episode(
    db: &DbManager,
    series_id: &str,
    episode_id: &str,
    fresh_apply: bool,
    global_absolute: bool,
) {
    let mapping = match db.get_series_mapping(series_id).await {
        Ok(Some(m)) => m,
        _ => return,
    };
    let Some(mode) = mapping.settings.monitor_mode else {
        return;
    };

    let episode = match db.get_episode_by_id(episode_id).await {
        Ok(Some(ep)) => ep,
        _ => return,
    };

    let ui_prefs = db.get_ui_preferences().await.unwrap_or_default();
    let rd_config = ui_prefs.release_date_display;

    let effective_date = db_episode_effective_date(&episode, &rd_config);
    // Submitter convention: an episode row with no season is season 1 (see
    // DEFAULT_SEASON_NUM).
    let season = episode
        .season
        .unwrap_or(jumbie_shared::mapping::DEFAULT_SEASON_NUM)
        .to_string();
    let status = episode.status.unwrap_or_default();
    let has_file = status == "downloaded" || status == "organized";
    let currently_monitored = if fresh_apply {
        false
    } else {
        episode.monitored
    };

    let should_monitor = ContentOrganizer::should_monitor_episode(MonitorParams {
        mode: Some(mode),
        season_str: &season,
        ep_num: episode.episode,
        has_file,
        currently_monitored,
        effective_date,
        season_override: mapping
            .settings
            .find_season_override(season.as_str(), global_absolute),
    });

    // SSoT: delegate to override_decision.
    match override_decision(episode.monitor_override, should_monitor, episode.monitored) {
        OverrideAction::SelfHeal => {
            let _ = db
                .clear_monitor_overrides_for_episodes(&[episode_id.to_string()])
                .await;
        }
        OverrideAction::Skip => {
            return;
        }
        OverrideAction::Proceed => {}
    }

    if should_monitor != episode.monitored {
        let _ = db
            .update_episode_monitor_status(&[episode_id.to_string()], should_monitor)
            .await;
    }
}

/// Compute the effective date for a DB episode row using the user's release-date
/// display preferences (priority order + enabled flags).
///
/// Returns `None` when no enabled date source has data for this episode.
pub(crate) fn db_episode_effective_date(
    row: &EpisodeDetailRow,
    config: &ReleaseDateDisplayConfig,
) -> Option<NaiveDateTime> {
    compute_effective_date(
        &jumbie_shared::types::ReleaseDates {
            meta_date: row.meta_date,
            upload_date: row.upload_date,
            est_date: row.est_date,
        },
        &config.order,
        config.metadata_enabled,
        config.source_enabled,
        config.estimated_enabled,
    )
    .map(|d| d.naive_utc())
}

/// Classify a batch of DB episode rows into to-monitor, to-unmonitor, and
/// to-clear-override lists.
///
/// SSoT for the per-episode classification loop used by BOTH
/// [`reapply_monitor_for_series`] and [`batch_apply_monitor_mode`].
///
/// # Override behavior
/// * `fresh_apply = true`:  `overridden` set is always empty (caller clears
///   overrides before calling).  All episodes are classified normally.
/// * `fresh_apply = false`: Episodes in `overridden` are skipped — their
///   `monitored` flag is left untouched.  Self-heal: if an overridden episode's
///   `mode_wants == current_monitored`, it's added to `to_clear_override` so the
///   caller can clear the stale override flag.
pub(crate) fn classify_db_episodes(
    episodes: Vec<EpisodeDetailRow>,
    mapping: &MappingRule,
    mode: MonitorMode,
    rd_config: &ReleaseDateDisplayConfig,
    fresh_apply: bool,
    overridden: &HashSet<String>,
    global_absolute: bool,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut to_monitor = Vec::new();
    let mut to_unmonitor = Vec::new();
    let mut to_clear_override = Vec::new();

    for db_row in episodes {
        let ep_id = db_row.episode_id.clone();
        let effective_date = db_episode_effective_date(&db_row, rd_config);

        // Submitter convention: an episode row with no season is season 1 (see
        // DEFAULT_SEASON_NUM).
        let season = db_row
            .season
            .unwrap_or(jumbie_shared::mapping::DEFAULT_SEASON_NUM)
            .to_string();
        let status = db_row.status.unwrap_or_default();
        let has_file = status == "downloaded" || status == "organized";

        // On fresh apply (user just switched to Future mode) treat all episodes as
        // not-yet-monitored so old past-dated episodes aren't picked up; on
        // sweep/refresh use the actual DB state so previously-future episodes stay
        // monitored until downloaded.
        let currently_monitored = if fresh_apply { false } else { db_row.monitored };

        let should_monitor = ContentOrganizer::should_monitor_episode(
            crate::source_processor::monitoring::MonitorParams {
                mode: Some(mode),
                season_str: season.as_str(),
                ep_num: db_row.episode,
                has_file,
                currently_monitored,
                effective_date,
                season_override: mapping
                    .settings
                    .find_season_override(season.as_str(), global_absolute),
            },
        );

        // SSoT: delegate to override_decision.
        if !fresh_apply {
            match override_decision(
                overridden.contains(&ep_id),
                should_monitor,
                db_row.monitored,
            ) {
                OverrideAction::SelfHeal => {
                    to_clear_override.push(ep_id);
                    continue;
                }
                OverrideAction::Skip => {
                    continue;
                }
                OverrideAction::Proceed => {}
            }
        }

        if mode == MonitorMode::Specials {
            if should_monitor {
                to_monitor.push(ep_id);
            }
        } else if should_monitor {
            to_monitor.push(ep_id);
        } else {
            to_unmonitor.push(ep_id);
        }
    }

    (to_monitor, to_unmonitor, to_clear_override)
}
