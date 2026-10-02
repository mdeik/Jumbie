// Release date display — client-side selection of the best date for display.
//
// SSoT — this is the single place that decides which release date to show based on
// the user's configured priority order and enabled flags. Both EpisodeDetailsModal
// and the Wanted page use it, so changes to General Settings → Release Date Display
// take effect reactively without cache invalidation or an API re-fetch.

use crate::utils::format_datetime_local;
use jumbie_shared::config::TimeFormat;
use jumbie_shared::config::ui::ReleaseDateDisplayConfig;
use jumbie_shared::types::{DateSourceFlags, ReleaseDateSource, select_release_date_source};

/// Result of picking the best release date from the available sources.
#[derive(Debug, Clone, PartialEq)]
pub struct PickedReleaseDate {
    /// Human-readable label (e.g. "Online Release Date", "Source Feed Date").
    pub label: &'static str,
    /// The display-ready date string in US local format.
    /// When no enabled source has data, this is "-" (placeholder).
    pub display_date: String,
    /// The original raw date string before formatting / midnight stripping.
    /// Guaranteed to be a parseable date string (ISO 8601 or legacy format).
    /// Used by callers that need a parseable date (e.g. `format_age`).
    /// When no enabled source has data, this is empty.
    pub raw_date: String,
    /// Which source was selected (or forced). Used by the modal's cycling
    /// logic to build the cycle order without re-running selection.
    pub source: ReleaseDateSource,
}

/// Map a release date source to its human-readable label. SSoT — both
/// `format_source_date` and `pick_release_date` use this so labels never diverge.
pub fn source_label(source: ReleaseDateSource) -> &'static str {
    match source {
        ReleaseDateSource::MetaDate => "Online Release Date",
        ReleaseDateSource::UploadDate => "Source Feed Date",
        ReleaseDateSource::EstDate => "Estimated Release Date",
    }
}

/// Pick the raw date string for a given source.
fn raw_for<'a>(
    source: ReleaseDateSource,
    meta_date: Option<&'a str>,
    upload_date: Option<&'a str>,
    est_date: Option<&'a str>,
) -> &'a str {
    match source {
        ReleaseDateSource::MetaDate => meta_date.unwrap_or(""),
        ReleaseDateSource::UploadDate => upload_date.unwrap_or(""),
        ReleaseDateSource::EstDate => est_date.unwrap_or(""),
    }
}

/// Parse a candidate date string, treating empty strings as absent.
/// Delegates to [`crate::utils::parse_timestamp_utc`] so RFC 3339, naive-UTC,
/// and legacy `"YYYY-MM-DD HH:MM:SS"` inputs all use one parser.
fn parse_optional(d: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    let d = d?;
    if d.is_empty() {
        return None;
    }
    crate::utils::parse_timestamp_utc(d)
}

/// Build a `PickedReleaseDate` from an already-selected source.
/// `parsed` is `None` when the source has no usable data (placeholder `"-"`).
fn build_picked(
    source: ReleaseDateSource,
    raw: &str,
    parsed: Option<chrono::DateTime<chrono::Utc>>,
    time_format: &TimeFormat,
) -> PickedReleaseDate {
    match parsed {
        Some(_) => PickedReleaseDate {
            label: source_label(source),
            display_date: format_datetime_local(raw, time_format),
            raw_date: raw.to_string(),
            source,
        },
        None => PickedReleaseDate {
            label: source_label(source),
            display_date: "-".to_string(),
            raw_date: String::new(),
            source,
        },
    }
}

/// Format a specific release date source for display. SSoT — the single place that
/// maps a `ReleaseDateSource` + raw date strings → human-readable label and formatted
/// date. `pick_release_date` and the cycling logic in `EpisodeDetailsModal` delegate
/// here so labels and formatting never diverge.
pub fn format_source_date(
    source: ReleaseDateSource,
    meta_date: Option<&str>,
    upload_date: Option<&str>,
    est_date: Option<&str>,
    time_format: &TimeFormat,
) -> PickedReleaseDate {
    let raw_date = raw_for(source, meta_date, upload_date, est_date);
    build_picked(
        source,
        raw_date,
        parse_optional(Some(raw_date)),
        time_format,
    )
}

/// Internal: the selected source, its raw string, and (when present) its parsed UTC
/// instant. Parsing happens exactly once here so no entry point re-parses.
struct SelectedReleaseDate<'a> {
    source: ReleaseDateSource,
    raw: &'a str,
    parsed: Option<chrono::DateTime<chrono::Utc>>,
}

/// Select the highest-priority enabled source that has data, parsing each
/// candidate once. Shared by every public selection entry point.
fn select_and_parse<'a>(
    config: &ReleaseDateDisplayConfig,
    meta_date: Option<&'a str>,
    upload_date: Option<&'a str>,
    est_date: Option<&'a str>,
) -> Option<SelectedReleaseDate<'a>> {
    let parsed_meta = parse_optional(meta_date);
    let parsed_upload = parse_optional(upload_date);
    let parsed_est = parse_optional(est_date);

    let source = select_release_date_source(
        &DateSourceFlags {
            meta: parsed_meta.is_some(),
            upload: parsed_upload.is_some(),
            est: parsed_est.is_some(),
        },
        &config.order,
        &DateSourceFlags {
            meta: config.metadata_enabled,
            upload: config.source_enabled,
            est: config.estimated_enabled,
        },
    )?;

    let parsed = match source {
        ReleaseDateSource::MetaDate => parsed_meta,
        ReleaseDateSource::UploadDate => parsed_upload,
        ReleaseDateSource::EstDate => parsed_est,
    };

    Some(SelectedReleaseDate {
        source,
        raw: raw_for(source, meta_date, upload_date, est_date),
        parsed,
    })
}

/// Select the effective release date as a parsed UTC instant.
///
/// Selection-only entry point (no display formatting), so hot paths (calendar
/// positioning, wanted filtering) should prefer this over [`pick_release_date`].
/// Returns `None` when all sources are disabled or none has data.
pub fn pick_release_date_utc(
    config: &ReleaseDateDisplayConfig,
    meta_date: Option<&str>,
    upload_date: Option<&str>,
    est_date: Option<&str>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    select_and_parse(config, meta_date, upload_date, est_date).and_then(|s| s.parsed)
}

/// Given the UI config's release date display settings and the episode's three
/// candidate date fields, pick the highest-priority enabled source that has data.
///
/// Selection delegates to the shared SSoT [`select_release_date_source`]; formatting
/// delegates to [`format_datetime_local`].
///
/// The return value encodes three states:
/// - `Some(PickedReleaseDate { display_date: "..." })` — data found, formatted for display
/// - `Some(PickedReleaseDate { display_date: "-" })` — no data, but at least one source enabled (placeholder)
/// - `None` — all sources disabled; caller should hide the date row entirely
///
/// Callers that only need a parseable date should use [`pick_release_date_utc`].
pub fn pick_release_date(
    config: &ReleaseDateDisplayConfig,
    meta_date: Option<&str>,
    upload_date: Option<&str>,
    est_date: Option<&str>,
    time_format: &TimeFormat,
) -> Option<PickedReleaseDate> {
    let selected = select_and_parse(config, meta_date, upload_date, est_date)?;
    Some(build_picked(
        selected.source,
        selected.raw,
        selected.parsed,
        time_format,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::config::TimeFormat;
    use jumbie_shared::config::ui::ReleaseDateDisplayConfig;

    /// Helper: format an RFC 3339 string the same way `pick_release_date` does
    /// (local timezone, US format). Used so test expectations match regardless
    /// of the test runner's timezone.
    fn local_format(rfc3339: &str, tf: &TimeFormat) -> String {
        format_datetime_local(rfc3339, tf)
    }

    fn config_with_order(
        order: Vec<&str>,
        metadata_enabled: bool,
        source_enabled: bool,
        estimated_enabled: bool,
    ) -> ReleaseDateDisplayConfig {
        ReleaseDateDisplayConfig {
            order: order.into_iter().map(|s| s.to_string()).collect(),
            metadata_enabled,
            source_enabled,
            estimated_enabled,
        }
    }

    // Priority ordering

    #[test]
    fn picks_metadata_when_highest_priority() {
        let cfg = config_with_order(vec!["metadata", "source", "estimated"], true, true, true);
        let result = pick_release_date(
            &cfg,
            Some("2026-06-18T20:00:00+00:00"),
            Some("2026-06-17T00:00:00+00:00"),
            Some("2026-06-19T00:00:00+00:00"),
            &TimeFormat::Hour24,
        );
        let p = result.expect("should pick a date");
        assert_eq!(p.label, "Online Release Date");
        assert_eq!(
            p.display_date,
            local_format("2026-06-18T20:00:00+00:00", &TimeFormat::Hour24)
        );
    }

    #[test]
    fn picks_source_when_metadata_disabled() {
        let cfg = config_with_order(vec!["metadata", "source", "estimated"], false, true, true);
        let result = pick_release_date(
            &cfg,
            Some("2026-06-18T20:00:00+00:00"),
            Some("2026-06-17T00:00:00+00:00"),
            None,
            &TimeFormat::Hour24,
        );
        let p = result.expect("should pick source date");
        assert_eq!(p.label, "Source Feed Date");
        assert_eq!(
            p.display_date,
            local_format("2026-06-17T00:00:00+00:00", &TimeFormat::Hour24)
        );
    }

    #[test]
    fn picks_estimated_when_higher_sources_missing() {
        let cfg = config_with_order(vec!["metadata", "source", "estimated"], true, true, true);
        let result = pick_release_date(
            &cfg,
            None,
            None,
            Some("2026-06-19T00:00:00+00:00"),
            &TimeFormat::Hour24,
        );
        let p = result.expect("should pick estimated date");
        assert_eq!(p.label, "Estimated Release Date");
        assert_eq!(
            p.display_date,
            local_format("2026-06-19T00:00:00+00:00", &TimeFormat::Hour24)
        );
    }

    // Enable/disable flags

    #[test]
    fn skips_disabled_source_even_with_data() {
        let cfg = config_with_order(vec!["metadata", "source"], false, true, true);
        let result = pick_release_date(
            &cfg,
            Some("2026-06-18T00:00:00+00:00"),
            Some("2026-06-17T00:00:00+00:00"),
            None,
            &TimeFormat::Hour24,
        );
        let p = result.expect("should skip metadata and pick source");
        assert_eq!(p.label, "Source Feed Date");
    }

    #[test]
    fn all_disabled_returns_none() {
        let cfg = config_with_order(vec!["metadata"], false, false, false);
        let result = pick_release_date(
            &cfg,
            Some("2026-06-18T00:00:00+00:00"),
            None,
            None,
            &TimeFormat::Hour24,
        );
        assert!(result.is_none(), "all disabled → None");
    }

    // Empty / missing data

    #[test]
    fn empty_string_skipped_like_none() {
        let cfg = config_with_order(vec!["metadata", "source"], true, true, true);
        let result = pick_release_date(
            &cfg,
            Some(""),
            Some("2026-06-17T00:00:00+00:00"),
            None,
            &TimeFormat::Hour24,
        );
        let p = result.expect("should skip empty metadata and pick source");
        assert_eq!(p.label, "Source Feed Date");
    }

    #[test]
    fn no_data_shows_placeholder() {
        let cfg = config_with_order(vec!["metadata", "source"], true, true, true);
        let result = pick_release_date(&cfg, None, None, None, &TimeFormat::Hour24);
        let p = result.expect("should show placeholder");
        assert_eq!(p.label, "Online Release Date");
        assert_eq!(p.display_date, "-");
    }

    #[test]
    fn only_source_enabled_no_data_placeholder_uses_that_label() {
        let cfg = config_with_order(vec!["metadata", "source"], false, true, true);
        let result = pick_release_date(&cfg, None, None, None, &TimeFormat::Hour24);
        let p = result.expect("should show placeholder with source label");
        assert_eq!(p.label, "Source Feed Date");
        assert_eq!(p.display_date, "-");
    }

    // Date formatting (US format, time always shown)

    #[test]
    fn iso_midnight_shows_date_and_time() {
        let cfg = config_with_order(vec!["metadata"], true, true, true);
        let result = pick_release_date(
            &cfg,
            Some("2026-06-18T00:00:00+00:00"),
            None,
            None,
            &TimeFormat::Hour24,
        );
        let p = result.expect("should format");
        assert_eq!(
            p.display_date,
            local_format("2026-06-18T00:00:00+00:00", &TimeFormat::Hour24)
        );
    }

    #[test]
    fn iso_non_midnight_shows_date_and_time() {
        let cfg = config_with_order(vec!["metadata"], true, true, true);
        let result = pick_release_date(
            &cfg,
            Some("2026-06-18T14:30:00+00:00"),
            None,
            None,
            &TimeFormat::Hour24,
        );
        let p = result.expect("should format");
        assert_eq!(
            p.display_date,
            local_format("2026-06-18T14:30:00+00:00", &TimeFormat::Hour24)
        );
    }

    // Reversed priority order

    #[test]
    fn respects_reversed_order() {
        let cfg = config_with_order(vec!["estimated", "source", "metadata"], true, true, true);
        let result = pick_release_date(
            &cfg,
            Some("2026-06-18T00:00:00+00:00"),
            Some("2026-06-17T00:00:00+00:00"),
            Some("2026-06-19T00:00:00+00:00"),
            &TimeFormat::Hour24,
        );
        let p = result.expect("should pick estimated (now highest priority)");
        assert_eq!(p.label, "Estimated Release Date");
    }

    // All three have data, order decides

    #[test]
    fn all_enabled_first_in_order_wins() {
        let cfg = config_with_order(vec!["source", "estimated", "metadata"], true, true, true);
        let result = pick_release_date(
            &cfg,
            Some("2026-06-18T00:00:00+00:00"),
            Some("2026-06-17T00:00:00+00:00"),
            Some("2026-06-19T00:00:00+00:00"),
            &TimeFormat::Hour24,
        );
        let p = result.expect("should pick source (first in order)");
        assert_eq!(p.label, "Source Feed Date");
        assert_eq!(
            p.display_date,
            local_format("2026-06-17T00:00:00+00:00", &TimeFormat::Hour24)
        );
        // raw_date preserves the original parseable input, not the formatted display
        assert_eq!(p.raw_date, "2026-06-17T00:00:00+00:00");
    }

    // pick_release_date_utc (selection-only entry point)

    #[test]
    fn pick_utc_selects_highest_priority_parsed_instant() {
        let cfg = config_with_order(vec!["metadata", "source", "estimated"], true, true, true);
        let dt = pick_release_date_utc(
            &cfg,
            Some("2026-06-18T20:00:00+00:00"),
            Some("2026-06-17T00:00:00+00:00"),
            Some("2026-06-19T00:00:00+00:00"),
        )
        .expect("should select metadata");
        assert_eq!(dt.to_rfc3339(), "2026-06-18T20:00:00+00:00");
    }

    #[test]
    fn pick_utc_none_when_all_disabled() {
        let cfg = config_with_order(vec!["metadata"], false, false, false);
        assert!(
            pick_release_date_utc(&cfg, Some("2026-06-18T20:00:00+00:00"), None, None).is_none()
        );
    }

    #[test]
    fn pick_utc_none_when_enabled_but_no_data() {
        let cfg = config_with_order(vec!["metadata"], true, false, false);
        assert!(pick_release_date_utc(&cfg, None, None, None).is_none());
    }

    // format_source_date (SSoT for cycling display)

    #[test]
    fn fmt_meta_date_with_data() {
        let p = format_source_date(
            ReleaseDateSource::MetaDate,
            Some("2026-06-18T20:00:00+00:00"),
            None,
            None,
            &TimeFormat::Hour24,
        );
        assert_eq!(p.label, "Online Release Date");
        assert_eq!(
            p.display_date,
            local_format("2026-06-18T20:00:00+00:00", &TimeFormat::Hour24)
        );
        assert_eq!(p.raw_date, "2026-06-18T20:00:00+00:00");
        assert_eq!(p.source, ReleaseDateSource::MetaDate);
    }

    #[test]
    fn fmt_upload_date_with_data() {
        let p = format_source_date(
            ReleaseDateSource::UploadDate,
            None,
            Some("2026-06-17T00:00:00+00:00"),
            None,
            &TimeFormat::Hour24,
        );
        assert_eq!(p.label, "Source Feed Date");
        assert_eq!(
            p.display_date,
            local_format("2026-06-17T00:00:00+00:00", &TimeFormat::Hour24)
        );
        assert_eq!(p.source, ReleaseDateSource::UploadDate);
    }

    #[test]
    fn fmt_estimated_date_with_data() {
        let p = format_source_date(
            ReleaseDateSource::EstDate,
            None,
            None,
            Some("2026-06-19T12:00:00+00:00"),
            &TimeFormat::Hour24,
        );
        assert_eq!(p.label, "Estimated Release Date");
        assert_eq!(
            p.display_date,
            local_format("2026-06-19T12:00:00+00:00", &TimeFormat::Hour24)
        );
        assert_eq!(p.source, ReleaseDateSource::EstDate);
    }

    #[test]
    fn fmt_meta_date_none_shows_placeholder() {
        let p = format_source_date(
            ReleaseDateSource::MetaDate,
            None,
            Some("2026-06-17T00:00:00+00:00"),
            None,
            &TimeFormat::Hour24,
        );
        assert_eq!(p.label, "Online Release Date");
        assert_eq!(p.display_date, "-");
        assert_eq!(p.raw_date, "");
        assert_eq!(p.source, ReleaseDateSource::MetaDate);
    }

    #[test]
    fn fmt_upload_date_empty_shows_placeholder() {
        let p = format_source_date(
            ReleaseDateSource::UploadDate,
            Some("2026-06-18T20:00:00+00:00"),
            Some(""),
            None,
            &TimeFormat::Hour24,
        );
        assert_eq!(p.label, "Source Feed Date");
        assert_eq!(p.display_date, "-");
        assert_eq!(p.source, ReleaseDateSource::UploadDate);
    }

    #[test]
    fn fmt_estimated_date_unparseable_shows_placeholder() {
        let p = format_source_date(
            ReleaseDateSource::EstDate,
            None,
            None,
            Some("not-a-date"),
            &TimeFormat::Hour24,
        );
        assert_eq!(p.label, "Estimated Release Date");
        assert_eq!(p.display_date, "-");
        assert_eq!(p.source, ReleaseDateSource::EstDate);
    }

    #[test]
    fn fmt_all_dates_none_respects_source() {
        // Even with all dates missing, the requested source determines the label.
        let p = format_source_date(
            ReleaseDateSource::EstDate,
            None,
            None,
            None,
            &TimeFormat::Hour24,
        );
        assert_eq!(p.label, "Estimated Release Date");
        assert_eq!(p.display_date, "-");
        assert_eq!(p.source, ReleaseDateSource::EstDate);
    }
}
