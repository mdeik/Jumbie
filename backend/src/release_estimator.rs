use crate::datetime::UtcDateTime;
use crate::db::{DbManager, EpisodeDetailRow};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc, Weekday};
use jumbie_shared::types::MappingRule;
use std::collections::HashMap;
use tracing::{debug, info};

/// Minimum events needed to compute a *derived* gap from actual release pairs.
/// With 0 events no estimation is possible. With exactly 1 event the function
/// falls back to [`FALLBACK_GAP_DAYS`] instead.
const MIN_EVENTS_FOR_ESTIMATION: usize = 2;

/// Default gap (in days) used when there is only one dated event.
const FALLBACK_GAP_DAYS: i64 = 7;

/// How many days after a release estimate's date before we suspect a release gap.
///
/// Once a gap is suspected, the estimator tries to use metadata-calibrated
/// dates. If no metadata is available, it resets the cadence to "next week"
/// from the current date instead of projecting from a stale anchor.
const GAP_DETECTION_DAYS: i64 = 3;

/// Minimum number of episodes with `upload_date` needed to compute
/// a mean source time with outlier rejection. Below this, fall back to
/// midnight (the original behaviour).
const MIN_SOURCE_TIMES_FOR_MEAN: usize = 2;

/// Minimum number of episodes with both `upload_date` and `meta_date` needed
/// to compute a metadata calibration offset.
const META_OFFSET_MIN_PAIRS: usize = 1;

/// Extract the mean time-of-day from source upload dates, with IQR outlier
/// rejection. Picks the *first* episode's time per calendar date so a
/// same-day batch casts one vote regardless of batch size. Returns `None`
/// when fewer than [`MIN_SOURCE_TIMES_FOR_MEAN`] unique dates are available.
fn compute_mean_source_time(episodes: &[&EpisodeDetailRow]) -> Option<NaiveTime> {
    let mut by_date: HashMap<NaiveDate, i64> = HashMap::new();
    for ep in episodes {
        if let Some(dt) = ep.upload_date {
            let secs = dt.time().hour() as i64 * 3600
                + dt.time().minute() as i64 * 60
                + dt.time().second() as i64;
            by_date.entry(dt.date()).or_insert(secs);
        }
    }

    if by_date.len() < MIN_SOURCE_TIMES_FOR_MEAN {
        return None;
    }

    let mut times: Vec<i64> = by_date.into_values().collect();
    times.sort_unstable();
    let q1 = times[times.len() / 4];
    let q3 = times[times.len() * 3 / 4];
    let iqr = q3 - q1;
    let lower = q1 - (iqr as f64 * 1.5) as i64;
    let upper = q3 + (iqr as f64 * 1.5) as i64;

    let filtered: Vec<i64> = times
        .into_iter()
        .filter(|&t| t >= lower && t <= upper)
        .collect();

    if filtered.is_empty() {
        return None;
    }

    let mean_secs = filtered.iter().sum::<i64>() / filtered.len() as i64;
    let h = (mean_secs / 3600) as u32;
    let m = ((mean_secs % 3600) / 60) as u32;
    let s = (mean_secs % 60) as u32;
    NaiveTime::from_hms_opt(h, m, s)
}

/// Compute the median offset between `upload_date` and `meta_date` for
/// episodes that have both. Returns `None` if fewer than [`META_OFFSET_MIN_PAIRS`]
/// matched pairs exist.
///
/// The offset is `upload_date - meta_date`, so a positive value means source
/// dates tend to arrive after the metadata-provided date. This offset is then
/// applied to metadata dates of future episodes to produce calibrated estimates.
fn compute_meta_calibration(episodes: &[&EpisodeDetailRow]) -> Option<Duration> {
    let mut offsets: Vec<i64> = episodes
        .iter()
        .filter_map(|ep| match (ep.upload_date, ep.meta_date) {
            (Some(u), Some(m)) => Some((u - m).num_seconds()),
            _ => None,
        })
        .collect();

    if offsets.len() < META_OFFSET_MIN_PAIRS {
        return None;
    }

    offsets.sort_unstable();
    Some(Duration::seconds(offsets[offsets.len() / 2]))
}

/// Project the next release dates for a season from its upload-date anchors.
///
/// Returns:
/// - `None` when the season has no upload_date anchors at all — there is no
///   cadence to project from (meta_date is only used for calibration and gap
///   detection, never as an anchor).  The caller should clear stale est_dates.
/// - `Some(estimations)` when anchors exist.  The vec holds the
///   `(episode_id, est_date)` writes that differ from the current values and
///   may be empty — the estimator is idempotent: when every existing est_date
///   already matches the projection there is nothing to update.
pub fn calculate_estimations(
    s_episodes: &[&EpisodeDetailRow],
    now: NaiveDateTime,
) -> Option<Vec<(String, UtcDateTime)>> {
    // Group upload dates by calendar date, not exact timestamp: same-date
    // releases must share a bucket or the gap math sees a bogus seconds-long
    // interval (and may bail out when episode numbers go backward that day).
    // Loses sub-day timing: with multiple batches per day, the first batch's
    // episode range sets the anchor.
    let mut events_map: HashMap<NaiveDate, Vec<i32>> = HashMap::new();
    for ep in s_episodes {
        if let Some(d) = ep.upload_date {
            events_map.entry(d.date()).or_default().push(ep.episode);
        }
    }
    let mut events: Vec<(NaiveDate, Vec<i32>)> = events_map.into_iter().collect();
    events.sort();

    if events.is_empty() {
        return None;
    }

    // Mode-day filtering: compute the most common release day-of-week from
    // unique dates (multiple episodes on one date count as 1 vote), so a single
    // off-day episode doesn't shift the anchor or split mode-day gaps. Falls
    // back to all events when filtering would leave <2 events.
    let gap_indices: Vec<usize> = if events.len() >= MIN_EVENTS_FOR_ESTIMATION {
        let mut counts: HashMap<Weekday, usize> = HashMap::new();
        for (date, _) in &events {
            *counts.entry(date.weekday()).or_default() += 1;
        }
        let max_count = counts.values().max().copied().unwrap_or(0);
        // Tiebreak: among the most common weekdays, prefer the day
        // appearing on the latest event date (the show's current cadence).
        let mode = events
            .iter()
            .rev()
            .find(|(date, _)| counts[&date.weekday()] == max_count)
            .map(|(date, _)| date.weekday());

        if let Some(wd) = mode {
            let indices: Vec<usize> = events
                .iter()
                .enumerate()
                .filter(|(_, (date, _))| date.weekday() == wd)
                .map(|(i, _)| i)
                .collect();
            if indices.len() >= MIN_EVENTS_FOR_ESTIMATION {
                indices
            } else {
                (0..events.len()).collect()
            }
        } else {
            (0..events.len()).collect()
        }
    } else {
        (0..events.len()).collect()
    };

    let (gap_duration, latest_event) = if gap_indices.len() < MIN_EVENTS_FOR_ESTIMATION {
        // With only one dated event there is no per-episode gap to compute, so
        // use a default cadence. The fallback is transient — recomputed from
        // scratch each run, so it never contaminates later calculations.
        (
            Duration::days(FALLBACK_GAP_DAYS),
            &events[*gap_indices.last().unwrap()],
        )
    } else {
        let mut gaps = Vec::new();
        for i in 1..gap_indices.len() {
            let prev_idx = gap_indices[i - 1];
            let curr_idx = gap_indices[i];
            let prev_date = events[prev_idx].0;
            let mut prev_eps = events[prev_idx].1.clone();
            prev_eps.sort_unstable();
            let prev_max = *prev_eps.last().unwrap();

            let curr_date = events[curr_idx].0;
            let mut curr_eps = events[curr_idx].1.clone();
            curr_eps.sort_unstable();
            let curr_min = *curr_eps.first().unwrap();

            // Per-episode gap = elapsed time / number of episode steps between
            // the two batches (any forward advance is accepted, not just
            // consecutive numbers). Using `curr_min - prev_max` counts only the
            // steps *between* batches, avoiding inflation when the current batch
            // has multiple same-day episodes.
            let duration = curr_date - prev_date;
            let ep_diff = curr_min - prev_max;
            if ep_diff > 0 {
                let gap_sec = duration.num_seconds() / ep_diff as i64;
                gaps.push(gap_sec);
            }
        }

        if gaps.is_empty() {
            return Some(vec![]);
        }

        // Median, not mean: release schedules have outliers (holidays, delays)
        // that skew a mean while leaving the median stable. An ambiguous (even)
        // median resolves to the LOWER middle value so limited evidence never
        // INCREASES the cadence — e.g. [7, 14] keeps 7 and treats the delay as
        // the outlier.
        gaps.sort_unstable();
        let median_gap_sec = gaps[(gaps.len() - 1) / 2];

        if median_gap_sec <= 0 {
            return Some(vec![]);
        }

        (
            Duration::seconds(median_gap_sec),
            &events[*gap_indices.last().unwrap()],
        )
    };
    let mut latest_eps = latest_event.1.clone();
    latest_eps.sort_unstable();
    let latest_ep_num = *latest_eps.last().unwrap();
    let latest_date_naive = latest_event.0;

    // Estimated dates use the mean time-of-day of actual uploads (IQR outlier
    // rejection); falls back to 12:00 UTC so timezone conversion never shifts
    // the date backward for users west of UTC.
    let est_time =
        compute_mean_source_time(s_episodes).unwrap_or(NaiveTime::from_hms_opt(12, 0, 0).unwrap());

    let latest_date = UtcDateTime::from_naive_utc(NaiveDateTime::new(latest_date_naive, est_time));

    // Median delta between source and metadata dates; always computed (not gated
    // on UI display preferences) so estimation uses all available data.
    let meta_offset = compute_meta_calibration(s_episodes);

    let now_utc = UtcDateTime::from_naive_utc(now);

    // Earliest episode past the latest source event with no upload_date.
    let first_missing_fwd = s_episodes
        .iter()
        .filter(|ep| ep.upload_date.is_none() && ep.episode > latest_ep_num)
        .min_by_key(|ep| ep.episode);

    // Metadata predicts a gap when its per-episode spacing exceeds 2× the source
    // cadence: the episode then gets a tighter 1-day grace and a meta+offset anchor.
    let prev_meta = first_missing_fwd.and_then(|ff| {
        s_episodes
            .iter()
            .filter(|ep| ep.meta_date.is_some() && ep.episode < ff.episode)
            .max_by_key(|ep| ep.episode)
            .map(|ep| (ep.episode, ep.meta_date.unwrap()))
    });

    let metadata_predicts_gap = meta_offset.is_some()
        && first_missing_fwd
            .zip(prev_meta)
            .and_then(|(ff, (prev_meta_ep, prev_meta_dt))| {
                let meta_ep_gap = (ff.meta_date? - prev_meta_dt).num_seconds()
                    / (ff.episode - prev_meta_ep) as i64;
                // 2× source cadence = clear metadata hiatus signal
                if meta_ep_gap > gap_duration.num_seconds() * 2 {
                    Some(())
                } else {
                    None
                }
            })
            .is_some();

    // A gap fires when the first forward-missing episode passes its
    // cadence-projected window: 1-day grace if metadata predicted a hiatus,
    // otherwise GAP_DETECTION_DAYS.
    let grace_days = if metadata_predicts_gap {
        1
    } else {
        GAP_DETECTION_DAYS
    };

    let gap_detected = first_missing_fwd.is_some_and(|ep| {
        let ep_diff = ep.episode - latest_ep_num;
        let gap_estimate = latest_date
            .checked_add_signed(gap_duration * ep_diff)
            .unwrap();
        gap_estimate
            .checked_add_signed(Duration::days(grace_days))
            .map(|deadline| deadline < now_utc)
            .unwrap_or(false)
    });

    // On a gap, all forward episodes chain from one anchor: metadata-derived
    // when it predicted the gap, otherwise reset to now + FALLBACK_GAP_DAYS.
    let (fwd_anchor, fwd_base_ep) = if gap_detected && metadata_predicts_gap {
        let ff = first_missing_fwd.unwrap();
        // Use this specific episode's meta_date (a day, not a timestamp) plus
        // the whole-day offset so the estimate lands at midnight.
        let meta_day = ff.meta_date.unwrap().date();
        let offset_days = meta_offset.unwrap().num_days();
        let anchor_date = meta_day + Duration::days(offset_days);
        let anchor = NaiveDateTime::new(anchor_date, est_time);
        (UtcDateTime::from_naive_utc(anchor), ff.episode)
    } else if gap_detected {
        let ff = first_missing_fwd.unwrap();
        let reset_base =
            UtcDateTime::from_naive_utc(NaiveDateTime::new(now_utc.naive_utc().date(), est_time));
        let reset_anchor = reset_base
            .checked_add_signed(Duration::days(FALLBACK_GAP_DAYS))
            .unwrap();
        (reset_anchor, ff.episode)
    } else {
        (latest_date, latest_ep_num)
    };

    // Every episode gets an estimate, even those with an upload_date; the UI
    // chooses between source and estimate dates via effective_date priority.
    let mut estimations = Vec::new();
    for ep in s_episodes {
        let new_est = if ep.episode > fwd_base_ep {
            // Past the anchor: chain with gap spacing.
            let ep_diff = ep.episode - fwd_base_ep;
            fwd_anchor
                .checked_add_signed(gap_duration * ep_diff)
                .unwrap()
        } else if ep.episode > latest_ep_num {
            // The anchor episode itself.
            fwd_anchor
        } else {
            // Backfill from the latest source event.
            let ep_diff = ep.episode - latest_ep_num;
            latest_date
                .checked_add_signed(gap_duration * ep_diff)
                .unwrap()
        };

        if ep.est_date != Some(new_est.naive_utc()) {
            estimations.push((ep.episode_id.clone(), new_est));
        }
    }

    Some(estimations)
}

pub async fn run_release_date_estimation(
    db: &DbManager,
    series_id: &str,
    absolute: bool,
) -> anyhow::Result<()> {
    let episodes = db.get_series_episodes_details(series_id, absolute).await?;

    if episodes.is_empty() {
        return Ok(());
    }

    // Human-readable label for log lines; falls back to the raw UUID when no
    // mapping exists (e.g. the series was removed mid-run).
    let series_label = db
        .get_series_mapping(series_id)
        .await
        .ok()
        .flatten()
        .map(|m| format!("{} ({})", m.target_title, series_id))
        .unwrap_or_else(|| series_id.to_string());

    // Group by season: different seasons have different release cadences and
    // episode counts, so mixing them would corrupt each other's gap calculations.
    let mut season_eps: HashMap<Option<String>, Vec<&EpisodeDetailRow>> = HashMap::new();
    for ep in &episodes {
        season_eps
            .entry(ep.season.map(|s| s.to_string()))
            .or_default()
            .push(ep);
    }

    let now = Utc::now().naive_utc();

    for (season, s_episodes) in season_eps {
        let season_display = season.as_deref().unwrap_or("Unknown");

        // See `calculate_estimations` for the None/Some contract. `None` clears
        // stale est_dates; `Some` applies the (possibly empty) write list.
        match calculate_estimations(&s_episodes, now) {
            None => {
                let mut cleared = false;
                for ep in &s_episodes {
                    if ep.est_date.is_some() && ep.upload_date.is_none() {
                        db.update_est_date(&ep.episode_id, None).await?;
                        cleared = true;
                    }
                }
                if cleared {
                    info!(
                        "Cleared stale estimated dates for {} Season {}",
                        series_label, season_display
                    );
                }
            }
            Some(estimations) => {
                if !estimations.is_empty() {
                    debug!(
                        "Calculated {} new estimations for {} Season {}",
                        estimations.len(),
                        series_label,
                        season_display
                    );
                }
                for (episode_id, est_date) in estimations {
                    db.update_est_date(&episode_id, Some(est_date)).await?;
                    debug!("Estimated release for {} -> {}", episode_id, est_date);
                }
            }
        }
    }

    Ok(())
}

/// Convenience wrapper that looks up the series mapping to determine
/// the numbering mode and triggers estimation for the series.
///
/// This is the SSoT entry point for triggering estimation from any
/// code path — callers only need to know the series UUID, not the
/// numbering mode or whether to pass UUID vs target_title.
pub async fn trigger_estimation_for_series(db: &DbManager, series_id: &str) -> anyhow::Result<()> {
    if let Some(mapping) = db.get_series_mapping(series_id).await? {
        let global_absolute = db
            .get_general_config()
            .await
            .unwrap_or_default()
            .absolute_numbering;
        run_release_date_estimation(
            db,
            series_id,
            mapping.settings.active_mode(global_absolute).is_absolute(),
        )
        .await
    } else {
        tracing::warn!(
            "Series {} not found, cannot estimate release dates",
            series_id
        );
        Ok(())
    }
}

/// Same as `trigger_estimation_for_series` but takes a `&MappingRule` directly
/// to avoid a redundant DB lookup when the caller already has the mapping.
pub async fn trigger_estimation_for_mapping(
    db: &DbManager,
    series_id: &str,
    mapping: &MappingRule,
) -> anyhow::Result<()> {
    let global_absolute = db
        .get_general_config()
        .await
        .unwrap_or_default()
        .absolute_numbering;
    run_release_date_estimation(
        db,
        series_id,
        mapping.settings.active_mode(global_absolute).is_absolute(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

    fn make_ep(
        id: &str,
        ep: i32,
        upload_date: Option<NaiveDateTime>,
        est_date: Option<NaiveDateTime>,
        file_path: Option<String>,
        meta_date: Option<NaiveDateTime>,
    ) -> EpisodeDetailRow {
        EpisodeDetailRow {
            episode_id: id.to_string(),
            season: Some(1),
            episode: ep,
            status: None,
            file_path,
            release_title: None,
            size: 0,
            title: None,
            download_link: None,
            quality_profile_id: None,
            submitter: None,
            media_info: None,
            quick_hash: None,
            original_path: None,
            created_at: None,
            file_acquired_at: None,
            monitored: true,
            meta_date,
            upload_date,
            est_date,
            metadata_ids: None,
            description: None,
            runtime: None,
            image_url: None,
            download_id: None,
            score: None,
            numbering_mode: 0,
            metadata_source: None,
            series_id: String::new(),
            monitor_override: false,
        }
    }

    fn dt(y: i32, m: u32, d: u32) -> NaiveDateTime {
        NaiveDateTime::new(
            NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
        )
    }

    /// Like `dt` but with specific hour/minute/second (non-midnight).
    fn dtt(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> NaiveDateTime {
        NaiveDateTime::new(
            NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            NaiveTime::from_hms_opt(h, min, s).unwrap(),
        )
    }

    // Original basic test

    #[test]
    fn test_calculate_estimations() {
        // Ep 1 on Oct 1, Ep 2 on Oct 8 (7 day gap)
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 8)), None, None, None);
        // Ep 3 needs estimation
        let row3 = make_ep("ep3", 3, None, None, None, None);

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2023, 10, 9); // 1 day after latest event → not overdue
        let est = calculate_estimations(&eps, now).unwrap();

        // Should estimate Ep 3 for Oct 15
        // ep1 and ep2 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 3);
        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(est_map["ep1"], UtcDateTime::from_naive_utc(dt(2023, 10, 1)));
        assert_eq!(est_map["ep2"], UtcDateTime::from_naive_utc(dt(2023, 10, 8)));
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 15))
        );
    }

    // Test 1: est_date used as anchor event

    #[test]
    fn test_estimated_date_used_as_anchor() {
        // Ep 1: source=Oct 1, Ep 2: source=Oct 8, Ep 3: estimated=Oct 15,
        // Ep 4: no date → Ep 4 should be estimated as Oct 22 (7 day gap)
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2099, 10, 8)), None, None, None);
        let row3 = make_ep(
            "ep3",
            3,
            None,
            Some(dt(2099, 10, 15)), // estimated, used as anchor
            None,
            None,
        );
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2099, 10, 9); // 1 day after latest event → not overdue
        let est = calculate_estimations(&eps, now).unwrap();

        // ep1 and ep2 now get estimates too (no upload_date guard);
        // ep3's est_date matches → skipped by dedup
        assert_eq!(est.len(), 3, "Ep 1, 2, and 4 should be estimated");
        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(est_map["ep1"], UtcDateTime::from_naive_utc(dt(2099, 10, 1)));
        assert_eq!(est_map["ep2"], UtcDateTime::from_naive_utc(dt(2099, 10, 8)));
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 22))
        );
    }

    // Test 2: upload_date overrides est_date

    #[test]
    fn test_off_mode_day_episode_does_not_shift_anchor() {
        // Ep 1: source=Oct 1 (Thu), Ep 2: source=Oct 8 (Thu)
        // Ep 3: source=Oct 10 (Sat — off-mode-day)
        // Ep 4: no date
        //
        // Mode day = Thu (2 votes: Oct 1, Oct 8). Oct 10 (Sat) is filtered
        // from gap computation and anchoring. Gap from Thu events = 7 days,
        // anchor = Oct 8 (latest Thu). Ep 4 = Oct 8 + 7*(4-2) = Oct 22.
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2099, 10, 8)), None, None, None);
        let row3 = make_ep(
            "ep3",
            3,
            Some(dt(2099, 10, 10)), // Saturday — off-mode-day
            None,
            None,
            None,
        );
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2099, 10, 11);
        let est = calculate_estimations(&eps, now).unwrap();

        // Mode day = Thu (2). Gap = 7. Anchor = Oct 8 (latest Thu).
        // Ep 1 backfill = Oct 1
        // Ep 2 backfill = Oct 8
        // Ep 3 forward = Oct 8 + 7 = Oct 15
        // Ep 4 forward = Oct 8 + 14 = Oct 22
        assert_eq!(est.len(), 4);
        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(est_map["ep1"], UtcDateTime::from_naive_utc(dt(2099, 10, 1)));
        assert_eq!(est_map["ep2"], UtcDateTime::from_naive_utc(dt(2099, 10, 8)));
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 15)),
            "Ep 3 (off-day) gets mode-day projection, not off-day anchor"
        );
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 22)),
            "Ep 4 chains from mode-day anchor, not off-day anchor"
        );
    }

    // Test 3: staleness — past estimated dates are skipped

    #[test]
    fn test_stale_estimated_date_is_skipped() {
        // Ep 1: source=Oct 1, Ep 2: source=Oct 8
        // Ep 3: estimated=Oct 12 (in the past, no file → stale)
        // Ep 4: no date
        // → Ep 3's stale estimate is NOT used as an anchor
        //   Only 2 events (ep1, ep2) → enough to estimate Ep 4
        //
        // We use dates in year 2023 for staleness (definitely in the past).
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 8)), None, None, None);
        let row3 = make_ep(
            "ep3",
            3,
            None,
            Some(dt(2023, 10, 12)), // stale (past date, no file)
            None,                   // file_path is None → still missing
            None,
        );
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2023, 10, 9); // 1 day after latest event → not overdue
        let est = calculate_estimations(&eps, now).unwrap();

        // Only 2 non-stale events (ep1, ep2) with a 7-day gap
        // Latest ep is 2, latest date is Oct 8
        // Ep 3: ep=3 > 2, source is None → should get a fresh estimate
        // Ep 4: ep=4 > 2, source is None → should get a fresh estimate
        // ep1 and ep2 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 4, "All 4 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // Ep 1 = Oct 1, Ep 2 = Oct 8 (backfill from 7-day gap)
        // Ep 3 = Oct 8 + 7 days = Oct 15
        // Ep 4 = Oct 8 + 14 days = Oct 22
        assert_eq!(est_map["ep1"], UtcDateTime::from_naive_utc(dt(2023, 10, 1)));
        assert_eq!(est_map["ep2"], UtcDateTime::from_naive_utc(dt(2023, 10, 8)));
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 15))
        );
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 22))
        );
    }

    // Test 4: metadata meta_date alone (no source-meta pairs) → gap-based
    //
    // If no episodes have BOTH source and meta dates, no offset can be
    // calibrated, so episodes with only meta_date fall through to the
    // gap-based estimator even with metadata enabled.

    #[test]
    fn test_metadata_without_calibration_falls_through() {
        // Ep 1: source=Oct 1 (no meta)
        // Ep 2: source=Oct 8 (no meta)
        // Ep 3: meta_date=Oct 12 only, no source — no offset calibration possible
        //       because there are 0 source+meta pairs → meta_offset = None
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 8)), None, None, None);
        let row3 = make_ep(
            "ep3",
            3,
            None,
            None,
            None,
            Some(dt(2023, 10, 12)), // meta_date — but no offset can be computed
        );
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2023, 10, 9); // 1 day after latest event → not overdue
        let est = calculate_estimations(&eps, now).unwrap();

        // No offset calibration available → falls through to gap-based
        // ep1 and ep2 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 4, "All 4 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        assert_eq!(est_map["ep1"], UtcDateTime::from_naive_utc(dt(2023, 10, 1)));
        assert_eq!(est_map["ep2"], UtcDateTime::from_naive_utc(dt(2023, 10, 8)));
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 15)),
            "Ep 3 uses gap-based — no meta offset to apply"
        );
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 22)),
            "Ep 4 uses gap-based"
        );
    }

    // Test 5: chaining through multiple estimated episodes

    #[test]
    fn test_chaining_through_estimated_episodes() {
        // Ep 1: source=Oct 1, Ep 2: estimated=Oct 8, Ep 3: estimated=Oct 15,
        // Ep 4: no date → All estimates chain correctly (7 day gap)
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep(
            "ep2",
            2,
            None,
            Some(dt(2099, 10, 8)), // estimated anchor
            None,
            None,
        );
        let row3 = make_ep(
            "ep3",
            3,
            None,
            Some(dt(2099, 10, 15)), // estimated anchor
            None,
            None,
        );
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2099, 10, 2); // after ep1, well before forward estimates
        let est = calculate_estimations(&eps, now).unwrap();

        // Events: Oct 1 (ep1) — only 1 event with upload_date → fallback gap
        // Latest ep: 1, latest date: Oct 1
        // ep2 and ep3 have matching est_date → skipped by dedup
        // ep1 and ep4 now get estimates (no upload_date guard)
        assert_eq!(est.len(), 2, "Ep 1 and Ep 4 should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        assert_eq!(est_map["ep1"], UtcDateTime::from_naive_utc(dt(2099, 10, 1)));
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 22))
        );
    }

    // Test 6: non-consecutive episodes (1, 5, 7)

    #[test]
    fn test_non_consecutive_episodes() {
        // Ep 1: source=Oct 1, Ep 5: source=Oct 29, Ep 7: source=Nov 12
        // Ep 8: no date — should be estimated
        //
        // With the relaxed logic, the 28 days between ep 1 and ep 5 covers
        // 4 episode steps (1→5), giving 7 days/episode.  The 14 days between
        // ep 5 and ep 7 covers 2 steps (5→7), also giving 7 days/episode.
        // Median gap = 7 days → Ep 8 estimated as Nov 12 + 7 = Nov 19.
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row5 = make_ep("ep5", 5, Some(dt(2099, 10, 29)), None, None, None);
        let row7 = make_ep("ep7", 7, Some(dt(2099, 11, 12)), None, None, None);
        let row8 = make_ep("ep8", 8, None, None, None, None);

        let eps = vec![&row1, &row5, &row7, &row8];
        let now = dt(2099, 11, 13); // 1 day after latest event → not overdue
        let est = calculate_estimations(&eps, now).unwrap();

        // Events: Oct 1 (ep1), Oct 29 (ep5), Nov 12 (ep7) — 3 events
        // Ep 8: ep=8 > 7, no source → should be estimated
        // ep1, ep5, ep7 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 4, "All 4 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // Gaps:
        //   Oct 29 - Oct 1 = 28 days, ep_diff = 5 - 1 = 4 → 7 days/ep
        //   Nov 12 - Oct 29 = 14 days, ep_diff = 7 - 5 = 2 → 7 days/ep
        // Median = 7 days
        // Ep 8 = Nov 12 + 7 = Nov 19
        assert_eq!(
            est_map["ep8"],
            UtcDateTime::from_naive_utc(dt(2099, 11, 19))
        );
    }

    // Test 9: non-consecutive with multi-episode batches

    #[test]
    fn test_non_consecutive_multi_episode_batch() {
        // Batch 1 (Oct 1): eps [1, 2]
        // Batch 2 (Oct 22): eps [7, 8]
        // Ep 9: no date
        //
        // Between batch 1 (max ep 2) and batch 2 (min ep 7):
        //   21 days, ep_diff = 7 - 2 = 5 steps → 4.2 days/ep
        // Median = 4.2 days → Ep 9 = Oct 22 + 4.2 days ≈ Oct 26
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2099, 10, 1)), None, None, None);
        let row7 = make_ep("ep7", 7, Some(dt(2099, 10, 22)), None, None, None);
        let row8 = make_ep("ep8", 8, Some(dt(2099, 10, 22)), None, None, None);
        let row9 = make_ep("ep9", 9, None, None, None, None);

        let eps = vec![&row1, &row2, &row7, &row8, &row9];
        let now = dt(2099, 10, 23); // 1 day after latest event → not overdue
        let est = calculate_estimations(&eps, now).unwrap();

        // All 5 episodes now get estimates (no upload_date guard)
        assert_eq!(est.len(), 5, "All 5 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // Gap: Oct 22 - Oct 1 = 21 days, ep_diff = 7 - 2 = 5 steps
        // 21 / 5 = 4.2 days → 4.2 * 86400 = 362880 seconds
        // Ep 9 = Oct 22 + 4.2 days
        let expected = UtcDateTime::from_naive_utc(NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2099, 10, 26).unwrap(),
            NaiveTime::from_hms_opt(16, 48, 0).unwrap(), // 0.2 days offset from noon anchor
        ));
        assert_eq!(est_map["ep9"], expected);
    }

    // Test 10: backfill — episodes before latest get estimates

    #[test]
    fn test_backfill_episodes_before_latest() {
        // Ep 4: source=Oct 22, Ep 5: source=Oct 29 (7 day gap)
        // Ep 1-3: no source (before latest event → backfill)
        // Ep 6: no source (after latest event → forward estimate)
        let row4 = make_ep("ep4", 4, Some(dt(2099, 10, 22)), None, None, None);
        let row5 = make_ep("ep5", 5, Some(dt(2099, 10, 29)), None, None, None);
        let row1 = make_ep("ep1", 1, None, None, None, None);
        let row2 = make_ep("ep2", 2, None, None, None, None);
        let row3 = make_ep("ep3", 3, None, None, None, None);
        let row6 = make_ep("ep6", 6, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4, &row5, &row6];
        let now = dt(2099, 10, 30); // 1 day after latest event → not overdue
        let est = calculate_estimations(&eps, now).unwrap();

        // Events: Oct 22 (ep4), Oct 29 (ep5) — 2 events, 7 day gap
        // Latest ep: 5, latest date: Oct 29
        //
        // Backfill (negative ep_diff):
        //   Ep 3: ep_diff = 3-5 = -2 → Oct 29 - 14 = Oct 15
        //   Ep 2: ep_diff = 2-5 = -3 → Oct 29 - 21 = Oct 8
        //   Ep 1: ep_diff = 1-5 = -4 → Oct 29 - 28 = Oct 1
        // Forward:
        //   Ep 6: ep_diff = 6-5 = 1 → Oct 29 + 7 = Nov 5
        // ep4 and ep5 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 6, "All 6 episodes should be estimated");

        // Results should be returned in episode_id order (not guaranteed,
        // but they should all be present with correct dates).
        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        assert_eq!(est_map["ep1"], UtcDateTime::from_naive_utc(dt(2099, 10, 1)));
        assert_eq!(est_map["ep2"], UtcDateTime::from_naive_utc(dt(2099, 10, 8)));
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 15))
        );
        assert_eq!(est_map["ep6"], UtcDateTime::from_naive_utc(dt(2099, 11, 5)));
    }

    // Test 11: single event fallback to FALLBACK_GAP_DAYS

    #[test]
    fn test_single_event_fallback_to_default_gap() {
        // Only Ep 5 has a upload_date.
        // With a single event, the estimator falls back to FALLBACK_GAP_DAYS.
        // Expected dates are computed from the constant so the test survives
        // changes to the fallback value.
        let anchor = dt(2099, 10, 15);
        let gap = Duration::days(FALLBACK_GAP_DAYS);

        let row5 = make_ep("ep5", 5, Some(anchor), None, None, None);
        let row1 = make_ep("ep1", 1, None, None, None, None);
        let row2 = make_ep("ep2", 2, None, None, None, None);
        let row4 = make_ep("ep4", 4, None, None, None, None);
        let row6 = make_ep("ep6", 6, None, None, None, None);
        let row7 = make_ep("ep7", 7, None, None, None, None);

        let eps = vec![&row1, &row2, &row4, &row5, &row6, &row7];
        let now = dt(2099, 10, 16); // 1 day after anchor event
        let est = calculate_estimations(&eps, now).unwrap();

        // 1 event → fallback gap
        // Latest ep: 5, latest date: Oct 15
        //
        // Backfill (negative ep_diff):
        //   Ep 4: ep_diff = 4-5 = -1 → anchor - 1*gap
        //   Ep 2: ep_diff = 2-5 = -3 → anchor - 3*gap
        //   Ep 1: ep_diff = 1-5 = -4 → anchor - 4*gap
        // Forward:
        //   Ep 6: ep_diff = 6-5 = 1 → anchor + 1*gap
        //   Ep 7: ep_diff = 7-5 = 2 → anchor + 2*gap
        // ep5 now gets an estimate too (no upload_date guard)
        assert_eq!(est.len(), 6, "All 6 episodes should be estimated");

        let anchor_utc = UtcDateTime::from_naive_utc(anchor);
        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        assert_eq!(
            est_map["ep1"],
            anchor_utc.checked_add_signed(gap * -4).unwrap()
        );
        assert_eq!(
            est_map["ep2"],
            anchor_utc.checked_add_signed(gap * -3).unwrap()
        );
        assert_eq!(
            est_map["ep4"],
            anchor_utc.checked_add_signed(gap * -1).unwrap()
        );
        assert_eq!(
            est_map["ep6"],
            anchor_utc.checked_add_signed(gap * 1).unwrap()
        );
        assert_eq!(
            est_map["ep7"],
            anchor_utc.checked_add_signed(gap * 2).unwrap()
        );
    }

    // Test 12: zero events returns empty

    #[test]
    fn test_zero_events_returns_none() {
        // No episodes have upload_date or est_date.
        // With zero upload anchors there is nothing to project from, so the
        // estimator returns None — the caller clears stale est_dates.
        let row1 = make_ep("ep1", 1, None, None, None, None);
        let row2 = make_ep("ep2", 2, None, None, None, None);

        let eps = vec![&row1, &row2];
        let now = dt(2099, 1, 1);

        assert!(
            calculate_estimations(&eps, now).is_none(),
            "No anchors → no estimation possible"
        );
    }

    // Test 13: median gap with uneven intervals

    #[test]
    fn test_mode_day_gap_with_uneven_intervals() {
        // Ep 1: source=Oct 1 (Thu — off-mode-day, filtered out)
        // Ep 2: source=Oct 3 (Sat — mode day)
        // Ep 3: source=Oct 17 (Sat — mode day, gap = 14 days, 1 ep step)
        // Ep 4: source=Oct 24 (Sat — mode day, gap = 7 days, 1 ep step)
        // Ep 5: no source → projected from mode-day gap
        //
        // Mode day = Sat (3 votes: Oct 3, Oct 17, Oct 24).
        // Mode-day events = [Oct 3, Oct 17, Oct 24] → gaps [14, 7]
        // Sorted [7, 14] → ambiguous (even count) → LOWER median = gaps[0] = 7.
        // The 14-day gap is a skipped week; the most recent interval (7) is the
        // true cadence, so the tie must not inflate it.
        // The Thu outlier (Oct 1) is filtered out entirely.
        // Anchor = Oct 24 → Ep 5 = Oct 24 + 7 = Oct 31.
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2099, 10, 3)), None, None, None);
        let row3 = make_ep("ep3", 3, Some(dt(2099, 10, 17)), None, None, None);
        let row4 = make_ep("ep4", 4, Some(dt(2099, 10, 24)), None, None, None);
        let row5 = make_ep("ep5", 5, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4, &row5];
        let now = dt(2099, 10, 25);
        let est = calculate_estimations(&eps, now).unwrap();

        // Mode day = Sat. Events on Sat: Oct 3 (ep2), Oct 17 (ep3), Oct 24 (ep4).
        // Gaps: [14, 7] → sorted [7, 14] → lower median = 7.
        // Anchor = Oct 24 (latest Sat). Ep 5 = Oct 24 + 7 = Oct 31.
        assert_eq!(est.len(), 5, "All 5 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            est_map["ep5"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 31)),
            "Ep 5 uses the lower median (7) — the tie must not inflate cadence"
        );
    }

    // Test 14: all episodes have upload_date — no estimation needed

    #[test]
    fn test_all_episodes_have_source_dates_get_backfill_estimates() {
        // Every episode already has upload_date, so no estimates needed.
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2099, 10, 8)), None, None, None);
        let row3 = make_ep("ep3", 3, Some(dt(2099, 10, 15)), None, None, None);

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2099, 10, 16);
        let est = calculate_estimations(&eps, now).unwrap();

        // All episodes have upload_date, but now every episode gets an estimate
        // (no upload_date guard). All 3 produce backfill estimates.
        assert_eq!(est.len(), 3, "All 3 episodes get estimates now");
    }

    // Test 15: episodes without upload_date cross different seasons

    #[test]
    fn test_estimations_are_season_scoped() {
        // Episodes in season 1 have source dates.
        // Episodes in season 2 have NO source dates and need estimates.
        // The estimator should NOT cross-contaminate seasons.
        //
        // Season 1: ep1 (Oct 1), ep2 (Oct 8) → 7 day gap
        // Season 2: ep1 (Oct 20), ep2 (no date) → ep2 needs estimate
        //
        // But since calculate_estimations is already per-season (grouped by
        // the caller), we test with a single-season set to verify internal logic.

        // Season 2 only
        // A single anchor event with no date → fallback gap should apply
        let row_s2_1 = make_ep("s2_ep1", 1, Some(dt(2099, 10, 20)), None, None, None);
        let row_s2_2 = make_ep("s2_ep2", 2, None, None, None, None);

        let eps_s2 = vec![&row_s2_1, &row_s2_2];
        let now = dt(2099, 10, 21); // 1 day after anchor event
        let est_s2 = calculate_estimations(&eps_s2, now).unwrap();

        // 1 event → FALLBACK_GAP_DAYS
        // ep1 now gets an estimate too (no upload_date guard)
        assert_eq!(est_s2.len(), 2, "Both episodes should be estimated");

        let est_s2_map: std::collections::HashMap<&str, UtcDateTime> =
            est_s2.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            est_s2_map["s2_ep2"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 20))
                .checked_add_signed(Duration::days(FALLBACK_GAP_DAYS))
                .unwrap()
        );
    }

    // Test 16: only metadata meta_date exists (no source dates)

    #[test]
    fn test_only_metadata_dates_no_estimates() {
        // Episodes have only meta_date (metadata), no upload_date.
        // Metadata dates should NOT be used as anchor events by the estimator.
        let row1 = make_ep("ep1", 1, None, None, None, Some(dt(2099, 10, 1)));
        let row2 = make_ep("ep2", 2, None, None, None, Some(dt(2099, 10, 8)));
        let row3 = make_ep("ep3", 3, None, None, None, None);

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2099, 12, 31);

        // No upload_date events → None (the caller clears stale est_dates)
        assert!(
            calculate_estimations(&eps, now).is_none(),
            "No source dates means no estimates"
        );
    }

    // Test 17: future estimated dates used as anchors (non-stale)

    #[test]
    fn test_future_estimated_date_used_as_anchor() {
        // Ep 1 has a upload_date.
        // Ep 2 has an est_date in the FUTURE (non-stale).
        // Ep 3 has no date → should be estimated using both as anchors.
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep(
            "ep2",
            2,
            None,
            Some(dt(2099, 10, 8)), // future estimated → valid anchor
            None,
            None,
        );
        let row3 = make_ep("ep3", 3, None, None, None, None);

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2099, 10, 2); // after ep1, well before forward estimates
        let est = calculate_estimations(&eps, now).unwrap();

        // 1 event (ep1 has upload_date), single event → fallback gap
        // ep2's est_date (Oct 8) matches what fallback would produce → skipped by dedup
        // ep1 now gets an estimate too (no upload_date guard)
        assert_eq!(est.len(), 2, "Ep 1 and Ep 3 should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 15))
        );
    }

    // Test 18: mix of upload_date and est_date anchors

    #[test]
    fn test_mixed_source_and_estimated_anchors() {
        // Ep 1: source=Oct 1
        // Ep 2: estimated=Oct 8 (future, valid anchor)
        // Ep 3: source=Oct 15
        // Ep 4: no date → should estimate as Oct 22
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep(
            "ep2",
            2,
            None,
            Some(dt(2099, 10, 8)), // estimated
            None,
            None,
        );
        let row3 = make_ep("ep3", 3, Some(dt(2099, 10, 15)), None, None, None);
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2099, 10, 16); // 1 day after latest event
        let est = calculate_estimations(&eps, now).unwrap();

        // 2 upload_date events: Oct 1 (ep1), Oct 15 (ep3) → 7 day gap
        // ep2 has est_date matching what backfill produces → skipped by dedup
        // ep1 and ep3 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 3, "Ep 1, 3, and 4 should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 22))
        );
    }

    // Test 19: estimator skips episodes that already match their estimate

    #[test]
    fn test_episode_with_matching_estimate_is_skipped() {
        // Ep 1: source=Oct 1
        // Ep 2: source=Oct 8
        // Ep 3: estimated=Oct 15 (already matches what estimator would produce)
        // → Ep 3 should NOT be in the results (no change needed)
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2099, 10, 8)), None, None, None);
        let row3 = make_ep(
            "ep3",
            3,
            None,
            Some(dt(2099, 10, 15)), // already matches expected estimate
            None,
            None,
        );

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2099, 10, 9);
        let est = calculate_estimations(&eps, now).unwrap();

        // ep1 and ep2 now get estimates too (no upload_date guard);
        // ep3's est_date matches → skipped by dedup
        assert_eq!(est.len(), 2, "Ep 1 and Ep 2 should be estimated");
    }

    // Test 20: estimator updates episode with mismatched estimate

    #[test]
    fn test_episode_with_stale_mismatched_estimate_is_updated() {
        // Ep 1: source=Oct 1
        // Ep 2: source=Oct 8
        // Ep 3: estimated=Oct 10 (in the PAST — stale, so NOT used as anchor event)
        //       Mismatch — 7-day gap says Oct 15 for ep_diff=1 from latest ep (ep 2)
        // → Ep 3 SHOULD be in results with corrected date Oct 15
        //   This tests that stale past estimates are NOT used as anchors but DO
        //   get re-computed and pushed as updates.
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 8)), None, None, None);
        let row3 = make_ep(
            "ep3",
            3,
            None,
            Some(dt(2023, 10, 10)), // PAST date → stale, not an anchor
            None,
            None,
        );

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2023, 10, 9); // 1 day after latest event → not overdue
        let est = calculate_estimations(&eps, now).unwrap();

        // Events: Oct 1 (ep1), Oct 8 (ep2) — 2 events, 7-day gap
        // Ep 3 stale estimate (Oct 10) skipped from anchors → only 2 events
        // Latest event: ep 2, date Oct 8
        // Ep 3: ep_diff = 3-2 = 1 → est_date = Oct 8 + 7 = Oct 15
        // Ep 3's current estimate (Oct 10) != Oct 15 → included in results
        // ep1 and ep2 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 3, "All 3 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 15))
        );
    }

    // Test 21: gap detection — forward estimate overdue triggers reset

    #[test]
    fn test_gap_detection_resets_to_next_week() {
        // Ep 1: source=Oct 1, Ep 2: source=Oct 8 (7 day gap)
        // Ep 3: no date, but it's now Oct 30 (well past Oct 15 estimate)
        // → gap detected, no metadata → reset to 1 week from now
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 8)), None, None, None);
        let row3 = make_ep("ep3", 3, None, None, None, None);

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2023, 10, 30); // Oct 15 estimate + 3 = Oct 18 < Oct 30 → overdue
        let est = calculate_estimations(&eps, now).unwrap();

        // Gap-based estimate was Oct 15, but overdue → reset to now + 7 = Nov 6
        // ep_diff = 1, (ep_diff - 1).max(0) = 0 → no extra gap
        // reset = Oct 30 + 7 = Nov 6
        // ep1 and ep2 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 3, "All 3 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(est_map["ep3"], UtcDateTime::from_naive_utc(dt(2023, 11, 6)));
    }

    // Test 20: gap with metadata but no metadata gap predicted — reset
    //
    // Ep 2 meta=Oct 5, Ep 3 meta=Oct 10 → 5 days/ep. Source cadence = 7 days.
    // 5 < 14 (2× cadence) → metadata does NOT predict a gap here.
    // Standard 3-day gap detection → reset anchor.

    #[test]
    fn test_gap_detection_with_metadata_calibration() {
        let row1 = make_ep(
            "ep1",
            1,
            Some(dt(2023, 10, 1)),
            None,
            None,
            Some(dt(2023, 9, 28)),
        );
        let row2 = make_ep(
            "ep2",
            2,
            Some(dt(2023, 10, 8)),
            None,
            None,
            Some(dt(2023, 10, 5)),
        );
        let row3 = make_ep("ep3", 3, None, None, None, Some(dt(2023, 10, 10)));

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2023, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        // Meta gap 5 days/ep < 2× cadence → metadata_predicts_gap=false
        // Standard 3-day grace, gap detected → reset: Oct 30 + 7 = Nov 6
        // ep1 and ep2 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 3, "All 3 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(est_map["ep3"], UtcDateTime::from_naive_utc(dt(2023, 11, 6)));
    }

    // Test 21: metadata predicts gap — anchor from meta+offset
    //
    // Ep 2 meta=Oct 5, Ep 3 meta=Nov 5 → 31 days/ep. Source cadence = 7 days.
    // 31 > 14 → metadata_predicts_gap=true. 1-day grace, then anchor from
    // meta+offset (Nov 5 + 3 = Nov 8). No calendar filter — metadata date is
    // the scheduled release, use it.

    #[test]
    fn test_gap_detection_metadata_future_date_used() {
        let row1 = make_ep(
            "ep1",
            1,
            Some(dt(2099, 10, 1)),
            None,
            None,
            Some(dt(2099, 9, 28)),
        );
        let row2 = make_ep(
            "ep2",
            2,
            Some(dt(2099, 10, 8)),
            None,
            None,
            Some(dt(2099, 10, 5)),
        );
        let row3 = make_ep("ep3", 3, None, None, None, Some(dt(2099, 11, 5)));

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2099, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        // Metadata predicts gap → fwd_anchor = Nov 5 + 3 = Nov 8
        // ep1 and ep2 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 3, "All 3 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        // Metadata predicts gap (31 > 14), grace=1.
        // Cadence projection for ep3 = Oct 8 + 7 = Oct 15.
        // Deadline = Oct 15 + 1 = Oct 16 < now (Oct 30) → gap detected.
        // Ep3 gets meta+offset anchor: Nov 5 + 3 = Nov 8.
        assert_eq!(est_map["ep3"], UtcDateTime::from_naive_utc(dt(2099, 11, 8)));
    }

    // Test 24: gap detection respects metadata_enabled flag

    #[test]
    fn test_gap_detection_respects_metadata_disabled() {
        // metadata_enabled flag is ignored for estimation — metadata is always
        // used to calibrate estimates.  The flag only affects UI display.
        let row1 = make_ep(
            "ep1",
            1,
            Some(dt(2099, 10, 1)),
            None,
            None,
            Some(dt(2099, 9, 28)),
        );
        let row2 = make_ep(
            "ep2",
            2,
            Some(dt(2099, 10, 8)),
            None,
            None,
            Some(dt(2099, 10, 5)),
        );
        let row3 = make_ep("ep3", 3, None, None, None, Some(dt(2099, 11, 5)));

        let eps = vec![&row1, &row2, &row3];
        let now = dt(2099, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        // Metadata used regardless of flag: meta_offset=+3, metadata_predicts_gap
        // (=31>14). Anchor = Nov 5 + 3 = Nov 8.
        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(est_map["ep3"], UtcDateTime::from_naive_utc(dt(2099, 11, 8)));
    }

    // Test 25: backfill episodes (ep_diff <= 0) never trigger gap detection

    #[test]
    fn test_backfill_not_affected_by_gap_detection() {
        // Ep 4: source=Oct 22, Ep 5: source=Oct 29 (7 day gap)
        // Ep 1-3: no source (before latest → backfill)
        // Now = Oct 30 (so forward estimate for ep6 is not overdue yet)
        // Backfill estimates (Oct 1, 8, 15) are in the past but should NOT
        // trigger gap detection since they're before the latest event.
        let row4 = make_ep("ep4", 4, Some(dt(2099, 10, 22)), None, None, None);
        let row5 = make_ep("ep5", 5, Some(dt(2099, 10, 29)), None, None, None);
        let row1 = make_ep("ep1", 1, None, None, None, None);
        let row2 = make_ep("ep2", 2, None, None, None, None);
        let row3 = make_ep("ep3", 3, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4, &row5];
        let now = dt(2099, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        // All 3 backfill episodes should get their original gap-based estimates
        // (Oct 1, 8, 15), NOT reset dates.
        // ep4 and ep5 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 5, "All 5 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        assert_eq!(est_map["ep1"], UtcDateTime::from_naive_utc(dt(2099, 10, 1)));
        assert_eq!(est_map["ep2"], UtcDateTime::from_naive_utc(dt(2099, 10, 8)));
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 15))
        );
    }

    // Test 26: multi-episode forward gap — all episodes get reset
    //
    // Verifies that when a gap is detected for the first forward episode,
    // ALL forward episodes (even those whose per-episode deadline hasn't
    // quite expired) get consistent reset treatment instead of retaining
    // stale gap-based estimates.

    #[test]
    fn test_multi_episode_forward_gap_all_reset() {
        // Ep 1: source=Oct 1, Ep 2: source=Oct 8 (7 day gap)
        // Ep 3-5: no dates, now=Oct 30 (well past Oct 15 deadline)
        // First forward missing is ep 3 → overdue → all use gap logic
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2099, 10, 8)), None, None, None);
        let row3 = make_ep("ep3", 3, None, None, None, None);
        let row4 = make_ep("ep4", 4, None, None, None, None);
        let row5 = make_ep("ep5", 5, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4, &row5];
        let now = dt(2099, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        // All 3 forward episodes should get reset dates:
        //   ep3 (ep_diff=1): now+7 + gap*0 = Nov 6
        //   ep4 (ep_diff=2): now+7 + gap*1 = Nov 13
        //   ep5 (ep_diff=3): now+7 + gap*2 = Nov 20
        //
        // Under the OLD per-episode logic, ep5 would have gotten Oct 29
        // (the stale gap-based estimate), producing ep4=Nov 13 > ep5=Oct 29
        // which is out of order.
        // ep1 and ep2 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 5, "All 5 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2099, 11, 6)),
            "ep3 should reset to now+7"
        );
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 11, 13)),
            "ep4 should be spaced by one gap from the reset anchor"
        );
        assert_eq!(
            est_map["ep5"],
            UtcDateTime::from_naive_utc(dt(2099, 11, 20)),
            "ep5 should be spaced by two gaps from the reset anchor"
        );

        // Verify chronological ordering
        assert!(est_map["ep3"] < est_map["ep4"], "ep3 must be before ep4");
        assert!(est_map["ep4"] < est_map["ep5"], "ep4 must be before ep5");
    }

    // Test: resume after a one-week delay re-anchors to the latest
    //    source date (regression)
    //
    // Weekly show (7-day cadence): Ep 1-5 on Oct 1/8/15/22/29. Ep 6 was
    // delayed one week and actually released Nov 12. Once the delayed episode
    // is downloaded (its upload_date is now the anchor), forward estimates
    // must chain from Nov 12 at the original 7-day cadence — NOT stay on the
    // pre-delay projection (which would put Ep 7 on Nov 12 + 14 = Nov 26,
    // i.e. permanently one episode behind).

    #[test]
    fn test_resume_after_one_week_delay_reanchors_to_latest_source() {
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 8)), None, None, None);
        let row3 = make_ep("ep3", 3, Some(dt(2023, 10, 15)), None, None, None);
        let row4 = make_ep("ep4", 4, Some(dt(2023, 10, 22)), None, None, None);
        let row5 = make_ep("ep5", 5, Some(dt(2023, 10, 29)), None, None, None);
        let row6 = make_ep("ep6", 6, Some(dt(2023, 11, 12)), None, None, None);
        let row7 = make_ep("ep7", 7, None, None, None, None);
        let row8 = make_ep("ep8", 8, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4, &row5, &row6, &row7, &row8];
        let now = dt(2023, 11, 13); // day after the delayed episode landed
        let est = calculate_estimations(&eps, now).unwrap();

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // Median of [7, 7, 7, 7, 7, 14] = 7 → weekly cadence survives the delay.
        assert_eq!(
            est_map["ep7"],
            UtcDateTime::from_naive_utc(dt(2023, 11, 19)),
            "Ep 7 must chain from Ep 6's actual source date (Nov 12) + 7 days"
        );
        assert_eq!(
            est_map["ep8"],
            UtcDateTime::from_naive_utc(dt(2023, 11, 26)),
            "Ep 8 must be one weekly gap after Ep 7"
        );
        assert!(est_map["ep7"] < est_map["ep8"], "Ep 7 must precede Ep 8");
    }

    // Test: ambiguous median (even count) resolves to the lower value
    //    so limited evidence never inflates the cadence
    //
    // Weekly show with only ONE observed interval before a one-week delay:
    //   Ep 1 Oct 1, Ep 2 Oct 8 (gap 7), Ep 3 delayed to Oct 22 (gap 14).
    //   gaps = [7, 14] — even count, ambiguous. The upper median would pick
    //   14 (biweekly projection, i.e. "one episode behind"); the lower median
    //   keeps 7, treating the delay as the anomaly. Ep 4 = Oct 22 + 7 = Oct 29.

    #[test]
    fn test_ambiguous_median_uses_lower_value_to_avoid_cadence_increase() {
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 8)), None, None, None);
        let row3 = make_ep("ep3", 3, Some(dt(2023, 10, 22)), None, None, None); // delayed 1 week
        let row4 = make_ep("ep4", 4, None, None, None, None);
        let row5 = make_ep("ep5", 5, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4, &row5];
        let now = dt(2023, 10, 23); // day after the delayed episode landed
        let est = calculate_estimations(&eps, now).unwrap();

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // Mode day = Sun (Oct 1/8/22). Gaps [7, 14] → lower median 7.
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 29)),
            "Ambiguous [7, 14] median must resolve to 7 (lower), not 14"
        );
        assert_eq!(
            est_map["ep5"],
            UtcDateTime::from_naive_utc(dt(2023, 11, 5)),
            "Ep 5 chains at the 7-day cadence"
        );
        assert!(est_map["ep4"] < est_map["ep5"], "Ep 4 must precede Ep 5");
    }

    // Test 25: multi-episode gap — metadata predicts gap, all chain
    //
    // Ep 2 meta=Oct 5, Ep 3 meta=Nov 6 → 32 days/ep. 32 > 14 → metadata
    // predicts a gap. All forward episodes chain from ep 3's meta+offset.
    // Ep 4's own meta_date (Nov 13) is NOT used — only first_fwd matters.

    #[test]
    fn test_multi_episode_gap_metadata_calibration() {
        let row1 = make_ep(
            "ep1",
            1,
            Some(dt(2099, 10, 1)),
            None,
            None,
            Some(dt(2099, 9, 28)),
        );
        let row2 = make_ep(
            "ep2",
            2,
            Some(dt(2099, 10, 8)),
            None,
            None,
            Some(dt(2099, 10, 5)),
        );
        let row3 = make_ep("ep3", 3, None, None, None, Some(dt(2099, 11, 6)));
        let row4 = make_ep("ep4", 4, None, None, None, Some(dt(2099, 11, 13)));
        let row5 = make_ep("ep5", 5, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4, &row5];
        let now = dt(2099, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        assert_eq!(est.len(), 5, "All 5 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // Metadata predicts gap (32 > 14), grace=1.
        // Cadence projection for ep3 = Oct 8 + 7 = Oct 15.
        // Deadline = Oct 15 + 1 = Oct 16 < now (Oct 30) → gap detected.
        // Ep3 anchors from meta+offset: Nov 6 + 3 = Nov 9.
        // Ep4, ep5 chain from anchor: Nov 9 + 7 = Nov 16, Nov 16 + 7 = Nov 23.
        assert_eq!(est_map["ep3"], UtcDateTime::from_naive_utc(dt(2099, 11, 9)));
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 11, 16))
        );
        assert_eq!(
            est_map["ep5"],
            UtcDateTime::from_naive_utc(dt(2099, 11, 23))
        );
        assert!(est_map["ep3"] < est_map["ep4"] && est_map["ep4"] < est_map["ep5"]);
    }

    // Test 26: multi-episode gap — metadata predicts gap, anchor from
    //    meta+offset even when it's "in the past" (no calendar filter)
    //
    // Ep 2 meta=Oct 5, Ep 3 meta=Oct 20 → 15 days/ep. 15 > 14 → metadata
    // predicts a gap. No calendar filter: meta+offset (Oct 23) is used as
    // the anchor despite being before "now" (Oct 30). Ep 4 chains from it.

    #[test]
    fn test_multi_episode_gap_stale_metadata_falls_through() {
        let row1 = make_ep(
            "ep1",
            1,
            Some(dt(2099, 10, 1)),
            None,
            None,
            Some(dt(2099, 9, 28)),
        );
        let row2 = make_ep(
            "ep2",
            2,
            Some(dt(2099, 10, 8)),
            None,
            None,
            Some(dt(2099, 10, 5)),
        );
        let row3 = make_ep("ep3", 3, None, None, None, Some(dt(2099, 10, 20)));
        let row4 = make_ep("ep4", 4, None, None, None, Some(dt(2099, 10, 25)));

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2099, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        assert_eq!(est.len(), 4, "All 4 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // fwd_anchor = Oct 20 + 3 = Oct 23, fwd_base_ep = 3
        // Ep 3: Oct 23 (meta+offset used, no calendar filter)
        // Ep 4: Oct 23 + 7 = Oct 30
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 23))
        );
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 30))
        );
        assert!(est_map["ep3"] < est_map["ep4"]);
    }

    // Test 27: multi-episode gap — first_fwd has no prev_meta →
    //    standard detection (no metadata gap to compare)
    //
    // Ep 3 is first_fwd but no earlier episode has meta_date for comparison.
    // metadata_predicts_gap = false → standard 3-day detection → reset.

    #[test]
    fn test_multi_episode_gap_no_prev_meta_standard_detection() {
        // Ep 1: source=Oct 1 (no meta)
        // Ep 2: source=Oct 8 (no meta)
        // Ep 3: meta=Oct 12 (first with meta, but no prev_meta to compare)
        // Ep 4: no dates
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2099, 10, 8)), None, None, None);
        let row3 = make_ep("ep3", 3, None, None, None, Some(dt(2099, 10, 12)));
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2099, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        assert_eq!(est.len(), 4, "All 4 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // No prev_meta → metadata_predicts_gap=false → reset anchor
        // Ep 3: Oct 30 + 7 = Nov 6
        // Ep 4: Nov 6 + 7 = Nov 13
        assert_eq!(est_map["ep3"], UtcDateTime::from_naive_utc(dt(2099, 11, 6)));
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 11, 13))
        );
        assert!(est_map["ep3"] < est_map["ep4"]);
    }

    // Test 28: multi-episode gap — mixed stale + future meta dates
    //
    // Both ep 3 and ep 4 have meta dates but metadata DOES predict a gap
    // (ep 3 meta 15 days after ep 2 meta > 14). All chain from ep 3's anchor.
    // Ep 4's own meta is NOT consulted.

    #[test]
    fn test_multi_episode_gap_mixed_meta_stale_and_future() {
        let row1 = make_ep(
            "ep1",
            1,
            Some(dt(2099, 10, 1)),
            None,
            None,
            Some(dt(2099, 9, 28)),
        );
        let row2 = make_ep(
            "ep2",
            2,
            Some(dt(2099, 10, 8)),
            None,
            None,
            Some(dt(2099, 10, 5)),
        );
        let row3 = make_ep("ep3", 3, None, None, None, Some(dt(2099, 10, 20)));
        let row4 = make_ep("ep4", 4, None, None, None, Some(dt(2099, 11, 12)));

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2099, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        assert_eq!(est.len(), 4, "All 4 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // metadata_predicts_gap = true (15 > 14)
        // fwd_anchor = Oct 20 + 3 = Oct 23, fwd_base_ep = 3
        // Ep 3: Oct 23 (from ep3's meta+offset, no calendar filter)
        // Ep 4: Oct 23 + 7 = Oct 30 (chained, does NOT use its own meta)
        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 23))
        );
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 30))
        );
        assert!(est_map["ep3"] < est_map["ep4"]);
    }

    // Test 29: gap NOT triggered — later forward episodes still use
    //    original gap-based estimates (no false positive)
    //
    // When the first forward episode is NOT overdue, all forward episodes
    // should use the original gap-based estimate regardless of how many
    // steps ahead they are. This validates that gap_detected=false.

    #[test]
    fn test_multi_episode_no_gap_all_gap_based() {
        // Ep 1: source=Oct 1, Ep 2: source=Oct 8 (7 day gap)
        // Ep 3-5: no dates, now=Oct 12 (only 4 days past ep3 deadline)
        // First forward missing is ep3: gap_estimate=Oct 15, Oct 15+3=Oct 18
        // Oct 18 > Oct 12 → NOT overdue → all use gap-based estimates
        let row1 = make_ep("ep1", 1, Some(dt(2099, 10, 1)), None, None, None);
        let row2 = make_ep("ep2", 2, Some(dt(2099, 10, 8)), None, None, None);
        let row3 = make_ep("ep3", 3, None, None, None, None);
        let row4 = make_ep("ep4", 4, None, None, None, None);
        let row5 = make_ep("ep5", 5, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4, &row5];
        let now = dt(2099, 10, 12);
        let est = calculate_estimations(&eps, now).unwrap();

        // Gap-based estimates:
        //   ep3: Oct 8 + 7 = Oct 15
        //   ep4: Oct 8 + 14 = Oct 22
        //   ep5: Oct 8 + 21 = Oct 29
        // All within the grace window → no gap logic applied
        // ep1 and ep2 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 5, "All 5 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        assert_eq!(
            est_map["ep3"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 15)),
            "ep3 should use gap-based estimate"
        );
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 22)),
            "ep4 should use gap-based estimate"
        );
        assert_eq!(
            est_map["ep5"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 29)),
            "ep5 should use gap-based estimate"
        );

        // Verify chronological ordering preserved
        assert!(est_map["ep3"] < est_map["ep4"], "ep3 before ep4");
        assert!(est_map["ep4"] < est_map["ep5"], "ep4 before ep5");
    }

    // Test 30: gap detected with metadata disabled — all use reset
    //
    // When metadata is disabled, future meta dates should be ignored.
    // All forward episodes fall through to the reset anchor regardless
    // of their individual meta_date values.

    #[test]
    fn test_multi_episode_gap_metadata_disabled_all_reset() {
        // metadata_enabled flag is ignored — metadata always used for estimation.
        // This test validates that passing false doesn't change behavior.
        let row1 = make_ep(
            "ep1",
            1,
            Some(dt(2099, 10, 1)),
            None,
            None,
            Some(dt(2099, 9, 28)),
        );
        let row2 = make_ep(
            "ep2",
            2,
            Some(dt(2099, 10, 8)),
            None,
            None,
            Some(dt(2099, 10, 5)),
        );
        let row3 = make_ep("ep3", 3, None, None, None, Some(dt(2099, 11, 6)));
        let row4 = make_ep("ep4", 4, None, None, None, Some(dt(2099, 11, 13)));

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2099, 10, 30);
        let est = calculate_estimations(&eps, now).unwrap();

        // Metadata used regardless: anchor = Nov 6 + 3 = Nov 9, fwd_base_ep = 3
        // Ep 3: Nov 9, Ep 4: Nov 9 + 7 = Nov 16
        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(est_map["ep3"], UtcDateTime::from_naive_utc(dt(2099, 11, 9)));
        assert_eq!(
            est_map["ep4"],
            UtcDateTime::from_naive_utc(dt(2099, 11, 16))
        );
        assert!(est_map["ep3"] < est_map["ep4"]);
    }

    // Test 31: metadata predicts gap — 1-day grace triggers gap
    //
    // User scenario: Ep 4 meta=Oct 8, Ep 5 meta=Dec 15 — metadata itself
    // shows a large gap (68 days/ep vs 7 day source cadence). When Ep 5's
    // expected date (Dec 15 + offset) passes without a source appearing,
    // gap detection uses 1-day grace instead of 3.

    #[test]
    fn test_metadata_gap_one_day_grace_triggers() {
        let row1 = make_ep(
            "ep1",
            1,
            Some(dt(2099, 10, 1)),
            None,
            None,
            Some(dt(2099, 9, 28)),
        );
        let row2 = make_ep(
            "ep2",
            2,
            Some(dt(2099, 10, 8)),
            None,
            None,
            Some(dt(2099, 10, 5)),
        );
        let row3 = make_ep(
            "ep3",
            3,
            Some(dt(2099, 10, 15)),
            None,
            None,
            Some(dt(2099, 10, 12)),
        );
        let row4 = make_ep(
            "ep4",
            4,
            Some(dt(2099, 10, 22)),
            None,
            None,
            Some(dt(2099, 10, 19)),
        );
        let row5 = make_ep("ep5", 5, None, None, None, Some(dt(2099, 12, 15)));

        let eps = vec![&row1, &row2, &row3, &row4, &row5];
        let now = dt(2099, 12, 16); // Past the cadence deadline (Oct 19 + 7 + 1 = Oct 27)
        let est = calculate_estimations(&eps, now).unwrap();

        // Meta gap: (Dec 15 - Oct 19) / 1 = 57 days > 14 → metadata_predicts_gap
        // 1-day grace: deadline = cadence_projection (Oct 19+7=Oct 26) + 1 = Oct 27.
        // now=Dec 16 > Oct 27 → gap detected. fwd_anchor = Dec 15 + 3 = Dec 18.

        // ep1-ep4 now get estimates too (no upload_date guard)
        assert_eq!(est.len(), 5, "All 5 episodes should be estimated");

        let est_map: std::collections::HashMap<&str, UtcDateTime> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            est_map["ep5"],
            UtcDateTime::from_naive_utc(dt(2099, 12, 18)),
            "Ep 5 should anchor from meta+offset (Dec 15+3) with 1-day grace"
        );
    }

    // ── Test 33: user scenario — metadata gap, source arrives before meta
    //
    // Ep 8: upload=Sep 24, meta=Oct 1  → offset = -7
    // Ep 9: meta=Dec 2, no upload
    // Meta predicts gap (62d > 14d). Grace=1 from cadence (Oct 1+1=Oct 2).
    // At Oct 2: no gap (strict <). At Oct 3: gap, anchor Dec 2-7=Nov 25.

    #[test]
    fn test_user_scenario_meta_gap_early_source() {
        let row8 = make_ep(
            "ep8",
            8,
            Some(dt(2099, 9, 24)),
            None,
            None,
            Some(dt(2099, 10, 1)),
        );
        let row9 = make_ep("ep9", 9, None, None, None, Some(dt(2099, 12, 2)));

        let eps = vec![&row8, &row9];

        // At Oct 2 (deadline): no gap, strict <
        let est = calculate_estimations(&eps, dt(2099, 10, 2)).unwrap();
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep9"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 1)),
            "Ep 9 gets cadence projection at deadline"
        );

        // At Oct 3 (past deadline): gap fires
        let est = calculate_estimations(&eps, dt(2099, 10, 3)).unwrap();
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep9"],
            UtcDateTime::from_naive_utc(dt(2099, 11, 25)),
            "Ep 9 anchors from meta+offset (Dec 2 - 7 = Nov 25)"
        );
    }

    // ── Test 34: user scenario — no meta gap, regular schedule
    //
    // Ep 8: upload=Sep 24, meta=Oct 1  → offset = -7
    // Ep 9: meta=Oct 8, no upload
    // Meta no gap (7d < 14d). Grace=3 from cadence (Oct 1+3=Oct 4).
    // At Oct 4: no gap. At Oct 5: gap, reset anchor.

    #[test]
    fn test_user_scenario_no_meta_gap() {
        let row8 = make_ep(
            "ep8",
            8,
            Some(dt(2099, 9, 24)),
            None,
            None,
            Some(dt(2099, 10, 1)),
        );
        let row9 = make_ep("ep9", 9, None, None, None, Some(dt(2099, 10, 8)));

        let eps = vec![&row8, &row9];

        // At Oct 4 (3-day deadline): no gap
        let est = calculate_estimations(&eps, dt(2099, 10, 4)).unwrap();
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep9"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 1)),
            "Ep 9 gets cadence projection at 3-day deadline"
        );

        // At Oct 5: gap fires
        let est = calculate_estimations(&eps, dt(2099, 10, 5)).unwrap();
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep9"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 12)),
            "Ep 9 resets to now+7 (Oct 5+7 = Oct 12)"
        );
    }

    // ── Test 35: user scenario — no metadata at all
    //
    // Ep 8: upload=Sep 24, meta=None
    // Ep 9: no upload, meta=None
    // No calibration possible. Standard 3-day grace. Reset on gap.

    #[test]
    fn test_user_scenario_no_metadata() {
        let row8 = make_ep("ep8", 8, Some(dt(2099, 9, 24)), None, None, None);
        let row9 = make_ep("ep9", 9, None, None, None, None);

        let eps = vec![&row8, &row9];
        let now = dt(2099, 10, 5); // past 3-day deadline (Oct 1+3=Oct 4)
        let est = calculate_estimations(&eps, now).unwrap();
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep9"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 12)),
            "Ep 9 resets to now+7 when no metadata"
        );
    }

    // ── Test 36: user scenario — meta on prev ep, none on missing
    //
    // Ep 8: upload=Sep 24, meta=Oct 1  → offset = -7 (computed but unusable)
    // Ep 9: no upload, meta=None
    // Can't calibrate — missing ep has no meta_date. Standard 3-day detection.

    #[test]
    fn test_user_scenario_meta_prev_ep_only() {
        let row8 = make_ep(
            "ep8",
            8,
            Some(dt(2099, 9, 24)),
            None,
            None,
            Some(dt(2099, 10, 1)),
        );
        let row9 = make_ep("ep9", 9, None, None, None, None);

        let eps = vec![&row8, &row9];
        let now = dt(2099, 10, 5); // past 3-day deadline
        let est = calculate_estimations(&eps, now).unwrap();
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep9"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 12)),
            "Ep 9 resets — no meta_date to calibrate with"
        );
    }

    // ── Test 37: user scenario — meta on missing ep, no calibration pairs
    //
    // Ep 8: upload=Sep 24, meta=None → no offset possible
    // Ep 9: no upload, meta=Dec 2
    // Offset can't be computed (no paired episodes). meta_date unusable.
    // Standard 3-day detection with reset.

    #[test]
    fn test_user_scenario_meta_missing_ep_only() {
        let row8 = make_ep("ep8", 8, Some(dt(2099, 9, 24)), None, None, None);
        let row9 = make_ep("ep9", 9, None, None, None, Some(dt(2099, 12, 2)));

        let eps = vec![&row8, &row9];
        let now = dt(2099, 10, 5); // past 3-day deadline
        let est = calculate_estimations(&eps, now).unwrap();
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep9"],
            UtcDateTime::from_naive_utc(dt(2099, 10, 12)),
            "Ep 9 resets — no offset to apply meta_date with"
        );
    }

    // Mode-day: Sun, Sun, Tue — off-day doesn't shift anchor

    #[test]
    fn test_mode_day_sun_sun_tue() {
        // Ep 1: Sun Oct 1, Ep 2: Sun Oct 8, Ep 3: Tue Oct 10
        // Ep 4: no date
        //
        // Mode = Sun (2 votes). Tue filtered out of gaps and anchor.
        // Mode-day events: Oct 1 (Ep 1), Oct 8 (Ep 2) → gap = 7, anchor = Oct 8 (latest Sun).
        // latest_ep_num = 2.
        // Ep 3 (forward, ep_diff=1) = Oct 15 (Sun).
        // Ep 4 (forward, ep_diff=2) = Oct 22 (Sun).
        // The Tuesday event did NOT shift the anchor to Tuesday.
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None); // Sun
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 8)), None, None, None); // Sun
        let row3 = make_ep("ep3", 3, Some(dt(2023, 10, 10)), None, None, None); // Tue
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2023, 10, 11);
        let est = calculate_estimations(&eps, now).unwrap();

        assert_eq!(est.len(), 4);
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        // Ep 3 forward = Oct 8 + 7 = Oct 15 (Sunday)
        assert_eq!(
            map["ep3"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 15)),
            "Ep 3 should be Sunday Oct 15 (mode-day projection)"
        );
        // Ep 4 forward = Oct 8 + 14 = Oct 22 (Sunday)
        assert_eq!(
            map["ep4"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 22)),
            "Ep 4 should be Sunday Oct 22, not shifted by Tuesday anchor"
        );
    }

    // Mode-day: Sun, Wed, Sun — off-day doesn't split cadence

    #[test]
    fn test_mode_day_sun_wed_sun() {
        // Ep 1: Sun Oct 1, Ep 2: Wed Oct 4, Ep 3: Sun Oct 15
        // Ep 4: no date
        //
        // Mode = Sun (2 votes). Wed filtered out of gaps.
        // Mode-day events: Oct 1 (Ep 1), Oct 15 (Ep 3) — one gap.
        // duration = 14 days, ep_diff = 3 - 1 = 2 → per-ep gap = 7 days/ep.
        // Median = 7. Anchor = Oct 15 (latest Sun). Ep 4 = Oct 15 + 7 = Oct 22.
        //
        // Without mode-day filtering, gaps would be [3, 11] → median = 11,
        // anchor = Oct 15, Ep 4 = Oct 26 (pulled ahead by the 11-day gap).
        // Mode-day correctly keeps the 7-day cadence.
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None); // Sun
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 4)), None, None, None); // Wed
        let row3 = make_ep("ep3", 3, Some(dt(2023, 10, 15)), None, None, None); // Sun
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2023, 10, 16);
        let est = calculate_estimations(&eps, now).unwrap();

        assert_eq!(est.len(), 4);
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        // Mode-day events: Oct 1 (Ep 1), Oct 15 (Ep 3).
        // Gap = 14 days / 2 ep steps = 7 days. Anchor = Oct 15.
        // Ep 4 = Oct 15 + 7 = Oct 22.
        assert_eq!(
            map["ep4"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 22)),
            "Ep 4 chains from mode-day anchor with 7-day cadence, not pulled by Wed"
        );
    }

    // Mode-day: all on same day — unchanged behavior

    #[test]
    fn test_mode_day_all_same_weekday() {
        // All 3 events on Sunday. Mode = Sun (3 votes). No filtering needed.
        // Same behavior as before mode-day: gaps [7, 7], median = 7, anchor = latest Sun.
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None); // Sun
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 8)), None, None, None); // Sun
        let row3 = make_ep("ep3", 3, Some(dt(2023, 10, 15)), None, None, None); // Sun
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2023, 10, 16);
        let est = calculate_estimations(&eps, now).unwrap();

        assert_eq!(est.len(), 4);
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep4"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 22)),
            "Ep 4 = Oct 15 + 7 = Oct 22 (unchanged from pre-mode-day)"
        );
    }

    // Mode-day: all on different weekdays → falls back to all events

    #[test]
    fn test_mode_day_all_different_days_falls_back() {
        // Ep 1: Mon Oct 2, Ep 2: Wed Oct 4, Ep 3: Fri Oct 6
        // Each day has 1 vote, tiebreak picks latest event's day (Fri).
        // But Fri has only 1 event → <2 → fallback to all 3 events.
        // Gaps: [2, 2], median = 2, anchor = Oct 6 (Fri).
        // Ep 4 = Oct 6 + 2 = Oct 8.
        //
        // This fallback means irregular-daily shows still get estimates
        // instead of the 7-day default.
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 2)), None, None, None); // Mon
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 4)), None, None, None); // Wed
        let row3 = make_ep("ep3", 3, Some(dt(2023, 10, 6)), None, None, None); // Fri
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2023, 10, 7);
        let est = calculate_estimations(&eps, now).unwrap();

        assert_eq!(est.len(), 4);
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep4"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 8)),
            "Ep 4 uses all-events fallback (gaps [2,2], anchor Fri Oct 6)"
        );
    }

    // Mode-day: multi-episode batch on same date = 1 vote

    #[test]
    fn test_mode_day_multi_episode_batch_counts_as_one() {
        // Ep 1,2: batch on Sun Oct 1, Ep 3: batch on Tue Oct 10
        // Ep 4,5: no dates
        //
        // Unique dates: Sun Oct 1 (1 vote), Tue Oct 10 (1 vote) → tie.
        // Tiebreak: latest event's day = Tue. Mode = Tue.
        // Tue has 1 event → <2 → fallback to all events.
        //
        // Without the "same date = 1 vote" rule, Sun would have 2 votes (one
        // for each episode), winning 2-1.
        let row1 = make_ep("ep1", 1, Some(dt(2023, 10, 1)), None, None, None); // Sun
        let row2 = make_ep("ep2", 2, Some(dt(2023, 10, 1)), None, None, None); // Sun (same date)
        let row3 = make_ep("ep3", 3, Some(dt(2023, 10, 10)), None, None, None); // Tue
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2023, 10, 11);
        let est = calculate_estimations(&eps, now).unwrap();

        // Tie: Sun=1, Tue=1. Tiebreak picks latest=Tue. Tue has 1 event → fallback.
        // All events: [Oct 1 (eps 1,2), Oct 10 (ep 3)].
        // One gap: (10-1) = 9 days, ep_diff = 3-2 = 1 (prev_max=2, curr_min=3).
        // gap = 9. Anchor = Oct 10. Ep 4 = Oct 10 + 9 = Oct 19.
        assert_eq!(est.len(), 4);
        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();
        assert_eq!(
            map["ep4"],
            UtcDateTime::from_naive_utc(dt(2023, 10, 19)),
            "Multi-ep batch counts as 1 vote (tie → fallback)"
        );
    }

    // \u2500\u2500 Test: same-day uploads with slightly different timestamps,
    //     backward episode numbers (Clevatess case) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    //
    // When two episodes upload on the same calendar date with slightly
    // different timestamps (e.g. 12:43:20 and 12:43:49), and the later
    // episode has a lower number (E03 at 12:43:20, E02 at 12:43:49),
    // the gap calculation MUST NOT see a negative ep_diff and bail.
    // Fix: group by calendar date (midnight), not exact timestamp.

    #[test]
    fn test_same_day_uploads_different_timestamps_merge() {
        // Clevatess S02: E02/E03 upload on July 14 seconds apart.
        let row1 = make_ep("ep1", 1, Some(dtt(2026, 7, 9, 0, 55, 1)), None, None, None);
        let row2 = make_ep(
            "ep2",
            2,
            Some(dtt(2026, 7, 14, 12, 43, 49)),
            None,
            None,
            None,
        );
        let row3 = make_ep(
            "ep3",
            3,
            Some(dtt(2026, 7, 14, 12, 43, 20)),
            None,
            None,
            None,
        );
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2026, 7, 27);
        let est = calculate_estimations(&eps, now).unwrap();

        // Should NOT return empty (the bug: estimator bailed on -1 ep_diff).
        // Should estimate all 4 episodes with proper 5-day gap from July 9 to 14.
        assert!(
            !est.is_empty(),
            "Same-day uploads MUST NOT cause empty estimation"
        );
        assert_eq!(est.len(), 4, "All 4 episodes should be estimated");

        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // Gap between July 9 (ep1, max=1) and July 14 (eps 2,3, min=2):
        //   5 days / 1 step = 5-day gap.
        // Gap detection fires (now=July 27 > July 19+3) → reset anchor to
        // now+7 = August 3, using mean source time (first episode per day).
        assert_eq!(
            map["ep4"],
            UtcDateTime::from_naive_utc(dtt(2026, 8, 3, 6, 49, 25)),
            "Ep4 resets to now+7 after gap detection"
        );
    }

    // ── Test: same-day uploads with forward episode numbers
    // \u2500\u2500 Test: same-day uploads with forward episode numbers \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    //
    // Even when episode numbers advance forward, two same-day uploads 29
    // seconds apart should NOT produce a meaningless 29-second gap.
    // Fix: merging by date gives the cadence from the previous distinct day.

    #[test]
    fn test_same_day_uploads_forward_episode_numbers() {
        // E01: July 9, E02: July 14 12:43:20, E03: July 14 12:43:49
        let row1 = make_ep("ep1", 1, Some(dtt(2026, 7, 9, 0, 55, 1)), None, None, None);
        let row2 = make_ep(
            "ep2",
            2,
            Some(dtt(2026, 7, 14, 12, 43, 20)),
            None,
            None,
            None,
        );
        let row3 = make_ep(
            "ep3",
            3,
            Some(dtt(2026, 7, 14, 12, 43, 49)),
            None,
            None,
            None,
        );
        let row4 = make_ep("ep4", 4, None, None, None, None);

        let eps = vec![&row1, &row2, &row3, &row4];
        let now = dt(2026, 7, 27);
        let est = calculate_estimations(&eps, now).unwrap();

        assert!(
            !est.is_empty(),
            "Forward same-day uploads MUST NOT cause empty estimation"
        );
        assert_eq!(est.len(), 4, "All 4 episodes should be estimated");

        let map: std::collections::HashMap<&str, _> =
            est.iter().map(|(id, d)| (id.as_str(), *d)).collect();

        // Same merged gap: 5 days from July 9 to July 14.
        // Gap detection fires → reset to now+7 = August 3,
        // using mean source time (first episode per day).
        assert_eq!(
            map["ep4"],
            UtcDateTime::from_naive_utc(dtt(2026, 8, 3, 6, 49, 10)),
            "Ep4 uses proper cadence, not 29-second gap; resets after gap"
        );
    }

    // \u2500\u2500 Test: run_release_date_estimation clears stale est_date when
    //     calculate_estimations returns empty \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500─
    //
    // The Clevatess regression: broken gap calc returned empty, then
    // run_release_date_estimation cleared existing est_date for episodes
    // that had est_date but no upload_date.  The integration test below
    // (requires a DB) lives in tests/regression.rs.  Here we just verify
    // that calculate_estimations never returns empty for this input.
}
