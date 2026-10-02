// Calendar helpers — cache overlap, window computation, week utilities.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use jumbie_shared::config::TimeFormat;
use jumbie_shared::config::ui::ReleaseDateDisplayConfig;
use jumbie_shared::types::{CalendarEpisode, CalendarResponse, EpisodeViewModel};

/// Parse an RFC 3339 / ISO 8601 `meta_date` string (with timezone offset)
/// and return the local time as `NaiveDateTime`. Used by the calendar
/// overlap filter so cached episodes are compared against the local date
/// range (cache keys are computed from local `current_date`).
pub fn parse_calendar_date(meta_date: &str) -> Option<NaiveDateTime> {
    crate::utils::parse_timestamp_utc(meta_date).map(|dt| dt.with_timezone(&Local).naive_local())
}

/// Format a local date as an RFC 3339 datetime string with the browser's
/// timezone offset, e.g. "2026-05-01T00:00:00+09:00".
/// The backend parses the offset and converts to UTC.
pub fn local_date_to_rfc3339(date: NaiveDate, end_of_day: bool) -> String {
    let time = if end_of_day {
        NaiveTime::from_hms_opt(23, 59, 59).unwrap()
    } else {
        NaiveTime::from_hms_opt(0, 0, 0).unwrap()
    };
    let naive = NaiveDateTime::new(date, time);
    resolve_local(naive, Local.from_local_datetime(&naive)).to_rfc3339()
}

/// Resolve a naive local wall-clock time to a zoned instant. `from_local_datetime`
/// yields `LocalResult::None` for a wall-clock time that does not exist because of a
/// DST spring-forward (zones whose transition is at midnight, e.g. Chile, skip
/// `00:00:00`), where the previous `.unwrap()` panicked and crashed the calendar.
/// Prefer the earliest occurrence (a fall-back hour repeats, and the start of a day
/// is the first one); otherwise treat the value as UTC so a range query can never
/// panic. Split out from [`local_date_to_rfc3339`] so this policy is testable without
/// depending on the host timezone (`Local` reads it globally).
fn resolve_local(
    naive: NaiveDateTime,
    resolved: chrono::LocalResult<DateTime<Local>>,
) -> DateTime<Local> {
    resolved
        .earliest()
        .unwrap_or_else(|| Local.from_utc_datetime(&naive))
}

/// Result of scanning the calendar cache for overlapping entries.
/// Used by `Calendar` to reuse cached data when navigating between adjacent months.
pub struct CalendarOverlapResult {
    /// Episodes from cached entries that fall within the desired date range.
    pub cached_episodes: Vec<jumbie_shared::types::CalendarEpisode>,
    /// The minimal date range that still needs fetching (the gap not covered by cache),
    /// or None if the entire desired range is already covered by fresh cache entries.
    pub delta_range: Option<(NaiveDate, NaiveDate)>,
}

/// Scans the cache for `fetch_calendar_*` entries that overlap with the desired
/// date range. Returns cached episodes (filtered to the desired range) and the
/// delta range still needed.
///
/// When the user navigates between adjacent months, the new 3-month window
/// overlaps significantly with the previous one. Instead of fetching the full
/// range again, this function lets the caller reuse cached data for the
/// overlapping portion and only fetch the non-overlapping delta.
pub fn find_calendar_cache_overlap(
    desired_start: NaiveDate,
    desired_end: NaiveDate,
) -> CalendarOverlapResult {
    use jumbie_shared::types::CalendarEpisode;

    // (cache write timestamp, episode) — the timestamp decides which of two
    // overlapping windows is freshest for the same episode_id.
    let mut cached_episodes: Vec<(f64, CalendarEpisode)> = Vec::new();
    let mut covered_intervals: Vec<(NaiveDate, NaiveDate)> = Vec::new();

    crate::utils::API_CACHE.with(|cache| {
        crate::utils::API_CACHE_TIMESTAMPS.with(|ts| {
            let ts_map = ts.borrow();
            for (key, json) in cache.borrow().iter() {
                if !key.starts_with("fetch_calendar_") {
                    continue;
                }

                // Parse dates from key: "fetch_calendar_YYYY-MM-DD_YYYY-MM-DD"
                let parts: Vec<&str> = key.splitn(4, '_').collect();
                if parts.len() < 4 {
                    continue;
                }
                let cached_start = NaiveDate::parse_from_str(parts[2], "%Y-%m-%d").ok();
                let cached_end = NaiveDate::parse_from_str(parts[3], "%Y-%m-%d").ok();
                let (Some(cs), Some(ce)) = (cached_start, cached_end) else {
                    continue;
                };

                // Skip if no overlap with desired range
                if cs > desired_end || ce < desired_start {
                    continue;
                }

                let overlap_start = cs.max(desired_start);
                let overlap_end = ce.min(desired_end);

                if let Ok(response) = serde_json::from_str::<CalendarResponse>(json) {
                    let ts = ts_map.get(key).copied().unwrap_or(0.0);
                    let overlapping: Vec<CalendarEpisode> = response
                        .episodes
                        .into_iter()
                        .filter(|ep| {
                            parse_calendar_date(&ep.eff_date)
                                .map(|dt| {
                                    let d = dt.date();
                                    d >= overlap_start && d <= overlap_end
                                })
                                .unwrap_or(false)
                        })
                        .collect();
                    cached_episodes.extend(overlapping.into_iter().map(|ep| (ts, ep)));
                    covered_intervals.push((overlap_start, overlap_end));
                }
            }
        });
    });

    // Freshest wins — if two cached windows hold the same episode, the one written
    // later supersedes the earlier one. (A sort+dedup_by would keep the first
    // occurrence in arbitrary HashMap order, which could resurrect a stale episode.)
    let mut by_id: std::collections::HashMap<String, (f64, CalendarEpisode)> =
        std::collections::HashMap::new();
    for (ts, ep) in cached_episodes {
        by_id
            .entry(ep.episode_id.clone())
            .and_modify(|existing| {
                if ts > existing.0 {
                    *existing = (ts, ep.clone());
                }
            })
            .or_insert((ts, ep));
    }
    let cached_episodes: Vec<CalendarEpisode> = by_id.into_values().map(|(_, ep)| ep).collect();

    // Merge overlapping/sorted covered intervals, then compute gaps
    let delta_range = if covered_intervals.is_empty() {
        // Nothing cached — need the full range
        Some((desired_start, desired_end))
    } else {
        covered_intervals.sort_by_key(|a| a.0);
        let mut merged: Vec<(NaiveDate, NaiveDate)> = Vec::new();
        for (s, e) in covered_intervals {
            if let Some(last) = merged.last_mut() {
                // Adjacent or overlapping intervals merge
                if s <= last.1 + Duration::days(1) {
                    last.1 = last.1.max(e);
                    continue;
                }
            }
            merged.push((s, e));
        }

        // Find gaps
        let mut gaps = Vec::new();
        let mut cursor = desired_start;
        for (start, end) in &merged {
            if *start > cursor {
                gaps.push((cursor, *start - Duration::days(1)));
            }
            cursor = cursor.max(*end) + Duration::days(1);
        }
        if cursor <= desired_end {
            gaps.push((cursor, desired_end));
        }

        if gaps.is_empty() {
            None // fully covered by cache
        } else {
            // Merge all gaps into one contiguous range: the common case (1-month
            // navigation) produces a single gap on one side; multi-gap is rare.
            Some((gaps.first().unwrap().0, gaps.last().unwrap().1))
        }
    };

    CalendarOverlapResult {
        cached_episodes,
        delta_range,
    }
}

/// Return a clean 3-calendar-month window: from the 1st of the previous month to
/// the last day of the next month, relative to any date in the current month.
///
/// SSoT for computing calendar date ranges: both the calendar component's Effect
/// and the background preloader call this so their cache keys match exactly.
pub fn calc_calendar_window(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    let prev_month = if date.month() == 1 {
        NaiveDate::from_ymd_opt(date.year() - 1, 12, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(date.year(), date.month() - 1, 1).unwrap()
    };
    let next_month = if date.month() == 12 {
        NaiveDate::from_ymd_opt(date.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1).unwrap()
    };
    let next_month_end = if next_month.month() == 12 {
        NaiveDate::from_ymd_opt(next_month.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(next_month.year(), next_month.month() + 1, 1).unwrap()
    } - Duration::days(1);

    (prev_month, next_month_end)
}

/// Returns the Sunday-to-Saturday week range containing the given date.
/// Weeks are Sunday-based, matching the calendar grid layout.
pub fn week_range(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    let days_from_sunday = date.weekday().num_days_from_sunday();
    let week_start = date - Duration::days(days_from_sunday as i64);
    let week_end = week_start + Duration::days(6);
    (week_start, week_end)
}

/// Returns the 1-indexed week number within the given `month_year` for a
/// Sunday-based calendar. `sunday` must be a Sunday and serves as the anchor
/// day of the week to number.
///
/// `month_year` is the month being viewed (not necessarily the month `sunday`
/// falls in). For example, May 1 (Friday) has its week-start Sunday on Apr 26,
/// but the displayed month is May, so the week is "Week 1 of May".
///
/// Week 1 = the first Sunday-based week that contains any day of `month_year`.
pub fn week_of_month(sunday: NaiveDate, month_year: (i32, u32)) -> u32 {
    debug_assert_eq!(
        sunday.weekday().num_days_from_sunday(),
        0,
        "input must be a Sunday"
    );
    let first_of_month = NaiveDate::from_ymd_opt(month_year.0, month_year.1, 1).unwrap();
    let days_from_sunday = first_of_month.weekday().num_days_from_sunday();
    let first_sunday = first_of_month - Duration::days(days_from_sunday as i64);
    ((sunday - first_sunday).num_days() / 7 + 1) as u32
}

/// SSoT — refresh a `CalendarEpisode` from the freshest episode view model.
///
/// The calendar displays episodes via a projection of `EpisodeViewModel` (dates,
/// status, assigned, title, season, episode, series_title) plus a recomputed
/// `eff_date` (used by the overlap-aware cache merger). Both the calendar's targeted
/// post-save patch and the calendar-cache patcher call this so a modal save shows up
/// on the grid immediately instead of waiting for the 60-second poll.
pub fn refresh_calendar_episode_from_view_model(
    ep: &mut CalendarEpisode,
    episode: &EpisodeViewModel,
    series_title: &str,
    config: &ReleaseDateDisplayConfig,
    time_format: &TimeFormat,
) {
    ep.series_title = series_title.to_string();
    ep.season = episode.season.clone();
    ep.episode = episode.episode;
    ep.episode_title = episode.title.clone();
    ep.dates = episode.dates.clone();
    ep.status = episode.status.clone();
    // `assigned`/`disk_present` mirror the episode view model, which is the SSoT
    // for "has a main file" (DB state) and "that file is on disk".
    ep.assigned = episode.assigned;
    ep.disk_present = episode.disk_present;
    // Keep eff_date in sync with the (possibly changed) dates so the
    // overlap-aware cache merger filters by the fresh date.
    let picked = crate::utils::release_date::pick_release_date(
        config,
        ep.dates.meta_date.as_deref(),
        ep.dates.upload_date.as_deref(),
        ep.dates.est_date.as_deref(),
        time_format,
    );
    ep.eff_date = picked.map(|p| p.raw_date).unwrap_or_default();
}

// Tests — local→instant resolution policy. These live in-module (like
// `release_date.rs`) so the private `resolve_local` seam stays private. The DST-gap
// branch can't be forced through `Local` (it reads the host timezone globally), so
// it is exercised by feeding the `LocalResult` variant a gap produces.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_date_to_rfc3339_carries_explicit_offset() {
        // The backend normalizes the offset to UTC, but it must be present: a
        // zone-less string would be read as UTC, shifting the local day boundary.
        let s = local_date_to_rfc3339(NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(), false);
        let parsed = chrono::DateTime::parse_from_rfc3339(&s)
            .expect("range boundary must be RFC 3339 with an explicit offset");
        assert_eq!(
            parsed.date_naive(),
            NaiveDate::from_ymd_opt(2026, 5, 1).unwrap()
        );
        assert_eq!(parsed.time(), NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    }

    #[test]
    fn local_date_to_rfc3339_end_of_day_is_235959() {
        let s = local_date_to_rfc3339(NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(), true);
        let parsed = chrono::DateTime::parse_from_rfc3339(&s).unwrap();
        assert_eq!(parsed.time(), NaiveTime::from_hms_opt(23, 59, 59).unwrap());
    }

    #[test]
    fn resolve_local_falls_back_to_utc_for_skipped_time() {
        // Spring-forward gap: `from_local_datetime` returned None. Must not panic;
        // the wall-clock is read as UTC.
        let naive = NaiveDate::from_ymd_opt(2026, 9, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(
            resolve_local(naive, chrono::LocalResult::None),
            Local.from_utc_datetime(&naive)
        );
    }

    #[test]
    fn resolve_local_prefers_earliest_when_ambiguous() {
        // Fall-back hour repeats: the start of day is the earliest occurrence.
        let naive = NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let first = Local.from_utc_datetime(&naive);
        let second = first + Duration::hours(1);
        assert_eq!(
            resolve_local(naive, chrono::LocalResult::Ambiguous(first, second)),
            first
        );
    }
}
