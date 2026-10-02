use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use jumbie_shared::formatting::LabelStyle;
use jumbie_shared::types::EpisodeStatus;
use std::sync::Arc;

use crate::api::{AppState, CalendarQuery, CalendarTokenQuery};
use crate::datetime::{ReleaseDatesExt, compute_effective_date};

/// Parse a human-readable duration string like "45m 30s" from ffprobe
/// back into total minutes. Returns None when unparseable or zero.
fn parse_duration_minutes(duration_str: &str) -> Option<i64> {
    let mut total = 0i64;
    for part in duration_str.split_whitespace() {
        if let Some(m) = part.strip_suffix('m') {
            total += m.parse::<i64>().ok()?;
        } else if let Some(s) = part.strip_suffix('s') {
            let secs = s.parse::<i64>().ok()?;
            total += secs / 60;
            if secs % 60 >= 30 {
                total += 1;
            }
        }
    }
    if total > 0 { Some(total) } else { None }
}

/// Parse a `start_date` / `end_date` query parameter into UTC.
///
/// Strict: RFC 3339 with an explicit offset (the frontend sends the browser's offset).
fn parse_calendar_date_param(s: &str) -> Result<chrono::NaiveDateTime, (StatusCode, String)> {
    crate::datetime::parse_request_utc(s)
        .map(|dt| dt.naive_utc())
        .map_err(|e| {
            tracing::debug!("get_calendar: invalid date '{}': {}", s, e);
            (
                StatusCode::BAD_REQUEST,
                format!(
                    "Invalid date — expected RFC 3339 with a timezone offset: {}",
                    e
                ),
            )
        })
}

pub async fn get_calendar(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CalendarQuery>,
) -> Result<Json<jumbie_shared::types::CalendarResponse>, (StatusCode, String)> {
    tracing::debug!(
        "get_calendar called: start_date={}, end_date={}",
        query.start_date,
        query.end_date
    );
    let start_date = parse_calendar_date_param(&query.start_date)?;
    let end_date = parse_calendar_date_param(&query.end_date)?;

    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };
    let db_rows = state
        .db
        .get_calendar_episodes(start_date, end_date, global_absolute)
        .await
        .map_err(|e| {
            tracing::error!("get_calendar DB error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    let ui_prefs = state.db.get_ui_preferences().await.unwrap_or_default();
    let order = ui_prefs.release_date_display.order.clone();
    let metadata_enabled = ui_prefs.release_date_display.metadata_enabled;
    let source_enabled = ui_prefs.release_date_display.source_enabled;
    let estimated_enabled = ui_prefs.release_date_display.estimated_enabled;

    // `assigned` is DB state; `disk_present` is the disk state of the episode's main
    // file(s) — the direct file, or every currently assigned part. Auxiliary files
    // never contribute. Existence is resolved for direct and part paths in one pass.
    let ep_ids: Vec<String> = db_rows.iter().map(|r| r.episode_id.clone()).collect();
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
    let mut paths: Vec<String> = db_rows
        .iter()
        .filter_map(|r| r.file_path.clone())
        .filter(|p| !p.is_empty())
        .collect();
    for parts in parts_by_episode.values() {
        for part in parts {
            paths.push(part.file_path.clone());
        }
    }
    let existence = crate::api_routes::series_detail_builder::resolve_paths_existence(paths).await;

    // SQL already: 1) JOINs with series_mappings to resolve series_title,
    // 2) filters by the series' active numbering mode.  No Rust-side
    // filtering or title resolution needed.
    let mut episodes = Vec::new();
    for row in db_rows {
        let db_dates = jumbie_shared::types::ReleaseDates {
            meta_date: row.meta_date,
            upload_date: row.upload_date,
            est_date: row.est_date,
        };
        let effective_date = compute_effective_date(
            &db_dates,
            &order,
            metadata_enabled,
            source_enabled,
            estimated_enabled,
        );
        let eff_date_str = effective_date
            .map(|d| d.to_rfc3339_utc())
            .unwrap_or_default();

        let main_path = row.file_path.filter(|p| !p.is_empty());
        let parts = parts_by_episode
            .get(&row.episode_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let assigned = main_path.is_some() || !parts.is_empty();
        let disk_present = match main_path.as_deref() {
            Some(p) => existence.get(p).copied().unwrap_or(false),
            None if parts.is_empty() => false,
            None => parts
                .iter()
                .all(|p| existence.get(&p.file_path).copied().unwrap_or(false)),
        };

        // SSoT: absolute rows resolve to ABSOLUTE_SEASON_NUM; a NULL season on a
        // normal-mode row is legacy data with no season recorded, rendered here as
        // season 1 (display-only default, unchanged).
        episodes.push(jumbie_shared::types::CalendarEpisode {
            series_title: row.series_title,
            series_id: row.series_id,
            episode_id: row.episode_id,
            season: jumbie_shared::mapping::resolve_season_opt(row.season, row.numbering_mode == 1)
                .unwrap_or(1)
                .to_string(),
            episode: row.episode,
            episode_title: row.title,
            dates: db_dates.to_api(),
            eff_date: eff_date_str,
            status: row
                .status
                .unwrap_or_else(|| EpisodeStatus::Unreleased.to_string()),
            assigned,
            disk_present,
        });
    }

    tracing::debug!("get_calendar completed: {} episodes found", episodes.len());
    Ok(Json(jumbie_shared::types::CalendarResponse { episodes }))
}

pub async fn get_calendar_ical(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CalendarTokenQuery>,
) -> Result<impl axum::response::IntoResponse, (StatusCode, String)> {
    tracing::debug!("get_calendar_ical called");
    let token = query.token.unwrap_or_default();
    let (is_authorized, hide_unmonitored, show_as_all_day) = {
        let config = state.cfg.read().await;
        let legacy_match = config
            .auth
            .calendar_token
            .as_ref()
            .map(|t| t == &token)
            .unwrap_or(false);
        if legacy_match {
            // Legacy tokens always hid unmonitored (equivalent to hide_unmonitored = true)
            (true, true, false)
        } else {
            let cal_tokens = state.db.get_calendar_tokens().await.unwrap_or_default();
            match cal_tokens.iter().find(|t| t.token == token) {
                Some(t) => (true, t.hide_unmonitored, t.show_as_all_day),
                None => (false, false, false),
            }
        }
    };

    if !is_authorized {
        tracing::debug!("get_calendar_ical: unauthorized token");
        return Err((
            StatusCode::UNAUTHORIZED,
            "Invalid or missing calendar token".to_string(),
        ));
    }

    let now = chrono::Utc::now().naive_utc();
    let start_date = now - chrono::Duration::days(30);
    let end_date = now + chrono::Duration::days(90);

    let global_absolute = {
        let c = state.cfg.read().await;
        c.general.absolute_numbering
    };
    let db_rows = state
        .db
        .get_calendar_episodes(start_date, end_date, global_absolute)
        .await
        .map_err(|e| {
            tracing::error!("get_calendar_ical DB error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    let mut ical = String::new();
    ical.push_str(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Jumbie//EN\r\nCALSCALE:GREGORIAN\r\n",
    );

    let dtstamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    let id_to_title: std::collections::HashMap<String, String> = state
        .db
        .get_all_series_mappings()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, m)| (id, m.target_title))
        .collect();

    let ui_prefs = state.db.get_ui_preferences().await.unwrap_or_default();
    let order = ui_prefs.release_date_display.order;
    let metadata_enabled = ui_prefs.release_date_display.metadata_enabled;
    let source_enabled = ui_prefs.release_date_display.source_enabled;
    let estimated_enabled = ui_prefs.release_date_display.estimated_enabled;

    for row in db_rows {
        if hide_unmonitored && row.monitored == Some(false) {
            continue;
        }

        let db_dates = jumbie_shared::types::ReleaseDates {
            meta_date: row.meta_date,
            upload_date: row.upload_date,
            est_date: row.est_date,
        };
        let effective_date = compute_effective_date(
            &db_dates,
            &order,
            metadata_enabled,
            source_enabled,
            estimated_enabled,
        )
        .unwrap_or_else(|| crate::datetime::UtcDateTime::from_naive_utc(start_date));

        // All-day output is governed solely by the calendar token's
        // `show_as_all_day` flag (there is no per-episode date-only concept).
        let (dtstart_fmt, dtend_fmt) = if show_as_all_day {
            let date_str = effective_date.format_ical_date();
            (
                format!("DTSTART;VALUE=DATE:{}\r\n", date_str),
                format!("DTEND;VALUE=DATE:{}\r\n", date_str),
            )
        } else {
            let dtstart_str = effective_date.format_ical_utc();
            let duration_minutes = row
                .media_info
                .as_deref()
                .and_then(|json| serde_json::from_str::<jumbie_shared::types::MediaInfo>(json).ok())
                .and_then(|mi| mi.duration)
                .as_deref()
                .and_then(parse_duration_minutes)
                .or(row.runtime.map(|r| r as i64))
                .unwrap_or(30);
            let utc_dt = effective_date.to_chrono_utc();
            let end_utc = utc_dt + chrono::Duration::minutes(duration_minutes);
            let dtend_str = format!("{}Z", end_utc.format("%Y%m%dT%H%M%S"));
            (
                format!("DTSTART:{}\r\n", dtstart_str),
                format!("DTEND:{}\r\n", dtend_str),
            )
        };

        ical.push_str("BEGIN:VEVENT\r\n");
        ical.push_str(&dtstart_fmt);
        ical.push_str(&dtend_fmt);

        let series_title = id_to_title.get(&row.series_id).cloned().unwrap_or_default();
        // SSoT: absolute rows resolve to ABSOLUTE_SEASON_NUM; a NULL season on a
        // normal-mode row is legacy data with no season recorded (display default 1).
        let s_val = jumbie_shared::mapping::resolve_season_opt(row.season, row.numbering_mode == 1)
            .unwrap_or(1);
        let uid_safe_title = series_title.replace(" ", "");
        let uid = format!("{}-S{}E{}@jumbie", uid_safe_title, s_val, row.episode);

        let ep_label = jumbie_shared::formatting::fmt_season_episode(
            s_val,
            row.episode,
            None,
            LabelStyle::Short,
        );
        let summary = match row.title.as_deref().filter(|t| !t.is_empty()) {
            Some(title) => format!("{} - {} - {}", series_title, ep_label, title),
            None => format!("{} - {}", series_title, ep_label),
        };
        let description = format!(
            "Date: {}\nStatus: {}",
            effective_date.to_rfc3339_utc(),
            row.status
                .as_deref()
                .unwrap_or(EpisodeStatus::Unreleased.as_str())
        );

        ical.push_str(&format!("UID:{}\r\n", uid));
        ical.push_str(&format!("DTSTAMP:{}\r\n", dtstamp));
        ical.push_str(&format!("SUMMARY:{}\r\n", summary));
        ical.push_str(&format!("DESCRIPTION:{}\r\n", description));
        ical.push_str("END:VEVENT\r\n");
    }

    ical.push_str("END:VCALENDAR\r\n");

    let ical_size = ical.len();
    tracing::debug!("get_calendar_ical completed: {} bytes generated", ical_size);

    let headers = [
        (
            axum::http::header::CONTENT_TYPE,
            "text/calendar; charset=utf-8",
        ),
        (
            axum::http::header::CONTENT_DISPOSITION,
            "attachment; filename=\"calendar.ics\"",
        ),
    ];

    Ok((headers, ical))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datetime::UtcDateTime;

    /// Create an `ReleaseDates` with all three dates set to the same value,
    /// for passing into `compute_effective_date`.
    fn dates(
        meta: Option<chrono::NaiveDateTime>,
        source: Option<chrono::NaiveDateTime>,
        est: Option<chrono::NaiveDateTime>,
    ) -> jumbie_shared::types::ReleaseDates<chrono::NaiveDateTime> {
        jumbie_shared::types::ReleaseDates {
            meta_date: meta,
            upload_date: source,
            est_date: est,
        }
    }

    /// Create an `Option<NaiveDateTime>` for passing into the `dates` helper.
    fn ndt(s: &str) -> Option<chrono::NaiveDateTime> {
        Some(chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap())
    }

    /// Create the expected `Option<UtcDateTime>` for assertion.
    fn dt(s: &str) -> Option<UtcDateTime> {
        Some(UtcDateTime::from_naive_utc(
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap(),
        ))
    }

    fn default_order() -> Vec<String> {
        vec![
            "metadata".to_string(),
            "source".to_string(),
            "estimated".to_string(),
        ]
    }

    fn reversed_order() -> Vec<String> {
        vec![
            "estimated".to_string(),
            "source".to_string(),
            "metadata".to_string(),
        ]
    }

    // Pass 1: enabled sources WITH data

    #[test]
    fn all_enabled_first_in_order_wins() {
        // All three have data, metadata is first in order → wins
        let result = compute_effective_date(
            &dates(
                ndt("2025-01-15 00:00:00"),
                ndt("2025-01-14 00:00:00"),
                ndt("2025-01-13 00:00:00"),
            ),
            &default_order(),
            true,
            true,
            true,
        );
        assert_eq!(result, dt("2025-01-15 00:00:00"));
    }

    #[test]
    fn respects_custom_priority_order() {
        // Reversed order: estimated first, source second, metadata third
        // All have data, so estimated should win
        let result = compute_effective_date(
            &dates(
                ndt("2025-01-15 00:00:00"),
                ndt("2025-01-14 00:00:00"),
                ndt("2025-01-13 00:00:00"),
            ),
            &reversed_order(),
            true,
            true,
            true,
        );
        assert_eq!(result, dt("2025-01-13 00:00:00"));
    }

    #[test]
    fn skips_to_next_source_when_first_has_no_data() {
        // Metadata has no data → should fall through to source
        let result = compute_effective_date(
            &dates(None, ndt("2025-01-14 00:00:00"), ndt("2025-01-13 00:00:00")),
            &default_order(),
            true,
            true,
            true,
        );
        assert_eq!(result, dt("2025-01-14 00:00:00"));
    }

    #[test]
    fn only_last_source_has_data() {
        // Only estimated has data
        let result = compute_effective_date(
            &dates(None, None, ndt("2025-01-13 00:00:00")),
            &default_order(),
            true,
            true,
            true,
        );
        assert_eq!(result, dt("2025-01-13 00:00:00"));
    }

    // Disabled sources are never used

    #[test]
    fn disabled_source_skipped_even_with_data() {
        // Metadata enabled with data, source enabled with data, estimated disabled with data
        let result = compute_effective_date(
            &dates(
                ndt("2025-01-15 00:00:00"),
                ndt("2025-01-14 00:00:00"),
                ndt("2025-01-13 00:00:00"),
            ),
            &default_order(),
            true,
            true,
            false,
        );
        assert_eq!(result, dt("2025-01-15 00:00:00"));
    }

    #[test]
    fn disabled_source_not_used_even_if_only_one_with_data() {
        // Only source has data, but source is disabled → skip to next enabled
        let result = compute_effective_date(
            &dates(None, ndt("2025-01-14 00:00:00"), None),
            &default_order(),
            true,
            false,
            true,
        );
        // None of the enabled sources (metadata, estimated) have data
        assert_eq!(result, None);
    }

    #[test]
    fn only_source_enabled_and_has_data() {
        // Only source is enabled and it has data
        let result = compute_effective_date(
            &dates(
                ndt("2025-01-15 00:00:00"), // has data but disabled
                ndt("2025-01-14 00:00:00"),
                ndt("2025-01-13 00:00:00"), // has data but disabled
            ),
            &default_order(),
            false,
            true,
            false,
        );
        assert_eq!(result, dt("2025-01-14 00:00:00"));
    }

    // Pass 2: enabled but no data

    #[test]
    fn all_enabled_but_none_have_data_returns_none() {
        // All enabled but no source has data → Pass 2 returns None (no date to give)
        let result =
            compute_effective_date(&dates(None, None, None), &default_order(), true, true, true);
        assert_eq!(result, None);
    }

    #[test]
    fn some_enabled_but_none_have_data_returns_none() {
        // Only metadata and estimated enabled, neither has data
        let result = compute_effective_date(
            &dates(None, ndt("2025-01-14 00:00:00"), None), // source has data but disabled
            &default_order(),
            true,
            false,
            true,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn one_enabled_but_no_data_returns_none() {
        // Only source is enabled but has no data
        let result = compute_effective_date(
            &dates(None, None, None),
            &default_order(),
            false,
            true,
            false,
        );
        assert_eq!(result, None);
    }

    // All disabled

    #[test]
    fn all_disabled_returns_none() {
        let result = compute_effective_date(
            &dates(
                ndt("2025-01-15 00:00:00"),
                ndt("2025-01-14 00:00:00"),
                ndt("2025-01-13 00:00:00"),
            ),
            &default_order(),
            false,
            false,
            false,
        );
        assert_eq!(result, None);
    }

    // Source-specific scenarios

    #[test]
    fn source_feed_date_used_when_highest_priority_with_data() {
        // Metadata disabled, source enabled with data, estimated enabled with data
        // Source is highest priority enabled source with data
        let result = compute_effective_date(
            &dates(
                ndt("2025-01-15 00:00:00"), // disabled
                ndt("2025-01-14 00:00:00"),
                ndt("2025-01-13 00:00:00"),
            ),
            &default_order(),
            false,
            true,
            true,
        );
        assert_eq!(result, dt("2025-01-14 00:00:00"));
    }

    #[test]
    fn source_feed_date_fallback_when_metadata_missing() {
        // Metadata has no data, source has data → picks source
        let result = compute_effective_date(
            &dates(None, ndt("2025-01-14 00:00:00"), ndt("2025-01-13 00:00:00")),
            &default_order(),
            true,
            true,
            true,
        );
        assert_eq!(result, dt("2025-01-14 00:00:00"));
    }

    // parse_duration_minutes

    #[test]
    fn parse_standard_duration() {
        // 45m 30s rounds up to 46 (seconds ≥ 30)
        assert_eq!(parse_duration_minutes("45m 30s"), Some(46));
    }

    #[test]
    fn parse_long_episode() {
        // 1hr+ episode: 63 minutes flat
        assert_eq!(parse_duration_minutes("63m 0s"), Some(63));
    }

    #[test]
    fn parse_movie_length() {
        // 2hr 22min movie
        assert_eq!(parse_duration_minutes("142m 10s"), Some(142));
    }

    #[test]
    fn parse_rounds_up_seconds() {
        assert_eq!(parse_duration_minutes("44m 31s"), Some(45));
    }

    #[test]
    fn parse_rounds_down_seconds() {
        assert_eq!(parse_duration_minutes("44m 29s"), Some(44));
    }

    #[test]
    fn parse_minutes_only() {
        assert_eq!(parse_duration_minutes("23m 0s"), Some(23));
    }

    #[test]
    fn parse_seconds_only() {
        assert_eq!(parse_duration_minutes("0m 45s"), Some(1)); // rounds up
    }

    #[test]
    fn parse_zero_duration_returns_none() {
        assert_eq!(parse_duration_minutes("0m 0s"), None);
    }

    #[test]
    fn parse_empty_string_returns_none() {
        assert_eq!(parse_duration_minutes(""), None);
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert_eq!(parse_duration_minutes("not-a-duration"), None);
    }
}
