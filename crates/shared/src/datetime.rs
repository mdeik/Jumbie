// Canonical UTC timestamp — SSoT for every date/time value, shared by backend
// and frontend. DB, backend, and API deal only in UTC; the backend normalizes
// inbound timestamps at the boundary and only the frontend converts to local for
// display.
//
// `UtcDateTime` wraps `NaiveDateTime` with a compile-time guarantee that the value
// is UTC (there is no date-only variant; date-only input is anchored at midnight
// UTC). A newtype rather than `DateTime<Utc>` because SQLite has no timezone-aware
// type: it reuses sqlx's transparent `NaiveDateTime` encoding while documenting
// UTC at the Rust level.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// A UTC-normalised timestamp stored as a SQLite-compatible naive datetime.
///
/// Every datetime column in the database uses this type.
///
/// # Conversion rules
/// - To store: construct via `from_chrono_utc`, `now`, `from_naive_date`, or
///   `from_naive_utc` (only when you already have a UTC-naive value).
/// - To display: use `.to_local()` → chrono's `DateTime<Local>`.
/// - To compare: use the derived `Ord`/`PartialOrd` — all comparisons are UTC.
///
/// Serialized as an RFC 3339 string (see the manual `Serialize`/`Deserialize`
/// impls below), matching the documented plugin contract.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct UtcDateTime {
    naive: NaiveDateTime,
}

impl UtcDateTime {
    /// Current instant in UTC.
    pub fn now() -> Self {
        Self {
            naive: Utc::now().naive_utc(),
        }
    }

    /// Build from a `DateTime<Utc>` (RSS feed parse, TVMaze airstamp).
    pub fn from_chrono_utc(dt: DateTime<Utc>) -> Self {
        Self {
            naive: dt.naive_utc(),
        }
    }

    /// Build from a calendar date, anchored at midnight UTC.
    pub fn from_naive_date(d: NaiveDate) -> Self {
        Self {
            naive: d.and_hms_opt(0, 0, 0).unwrap(),
        }
    }

    /// Build from a `DateTime` with an arbitrary timezone offset (e.g. parsed
    /// RFC 3339 with offset).  Converts to UTC automatically.
    pub fn from_chrono_with_offset<Tz: chrono::TimeZone>(dt: DateTime<Tz>) -> Self {
        Self {
            naive: dt.with_timezone(&Utc).naive_utc(),
        }
    }

    /// Wrap a `NaiveDateTime` that the caller guarantees is UTC.
    /// Use sparingly — prefer `from_chrono_utc` or `from_naive_date` when possible.
    pub fn from_naive_utc(naive: NaiveDateTime) -> Self {
        Self { naive }
    }

    /// Extract the inner `NaiveDateTime` (which is UTC).
    pub fn naive_utc(self) -> NaiveDateTime {
        self.naive
    }

    /// Canonical storage string for the DB: `YYYY-MM-DD HH:MM:SS` (UTC).
    /// This is the shape every timestamp column stores.
    pub fn to_db_string(self) -> String {
        self.naive.format(DB_TIMESTAMP_FORMAT).to_string()
    }

    /// Convert to `DateTime<Utc>` (zero-cost, just adds the type).
    pub fn to_chrono_utc(self) -> DateTime<Utc> {
        Utc.from_utc_datetime(&self.naive)
    }

    /// Convert to the system/container local timezone.
    /// Uses the `TZ` environment variable if set, falls back to UTC.
    pub fn to_local(self) -> DateTime<Local> {
        self.to_chrono_utc().with_timezone(&Local)
    }

    /// Get the UTC date component.
    pub fn date(self) -> NaiveDate {
        self.naive.date()
    }

    /// Apply a duration, returning a new `UtcDateTime`.
    pub fn checked_add_signed(self, dur: Duration) -> Option<Self> {
        self.naive
            .checked_add_signed(dur)
            .map(|n| UtcDateTime { naive: n })
    }

    /// Format in the **process/server** local timezone (`chrono::Local` → the `TZ`
    /// env var, or the host's system zone) — *not* UTC, and *not* the browser's
    /// timezone.
    ///
    /// Used for filename template variables (`%{release:…}` / `%{download:…}`),
    /// which are therefore server-TZ-dependent. Prefer [`Self::to_rfc3339_utc`] for
    /// anything that crosses the API boundary.
    pub fn format_local_date(self) -> String {
        self.to_local().format("%Y-%m-%d %H:%M:%S").to_string()
    }

    /// Format as RFC 3339 in UTC (Z suffix). All API response dates use this
    /// so the frontend receives unambiguous UTC timestamps and converts to the
    /// user's local timezone via `dt.with_timezone(&Local)`.
    pub fn to_rfc3339_utc(self) -> String {
        naive_utc_to_rfc3339(self.naive)
    }

    /// Format as `YYYYMMDD` in UTC. Used for iCal's all-day `VALUE=DATE`.
    ///
    /// UTC (not server-local) so the output is deterministic and server-TZ
    /// independent. A date-only field is stored at midnight UTC, so the UTC date
    /// is exactly the intended date.
    pub fn format_ical_date(self) -> String {
        self.naive.format("%Y%m%d").to_string()
    }

    /// iCal UTC format with Z suffix (for simple UTC-based DTSTART).
    pub fn format_ical_utc(self) -> String {
        format!("{}Z", self.naive.format("%Y%m%dT%H%M%S"))
    }
}

impl fmt::Display for UtcDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.naive.format("%Y-%m-%d %H:%M:%S"))
    }
}

impl Serialize for UtcDateTime {
    /// Serialize as an RFC 3339 string with an explicit UTC offset.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&naive_utc_to_rfc3339(self.naive))
    }
}

impl<'de> Deserialize<'de> for UtcDateTime {
    /// Deserialize an RFC 3339 string — this is the plugin contract
    /// (`meta_date: string | null`), so an explicit offset is required.
    /// Naive/date-only strings are rejected; plugins must supply the zone.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        parse_rfc3339(&raw).map_err(serde::de::Error::custom)
    }
}

// DB ⇄ wire conversions

/// Format a naive-UTC value as RFC 3339 with an explicit offset.
///
/// SSoT for the DB→wire timestamp conversion: used by
/// [`UtcDateTime::to_rfc3339_utc`] and by [`crate::serde_utc`].
pub fn naive_utc_to_rfc3339(naive: NaiveDateTime) -> String {
    Utc.from_utc_datetime(&naive).to_rfc3339()
}

/// Canonical DB timestamp format: `YYYY-MM-DD HH:MM:SS` in UTC.
/// Every timestamp column stores this shape (see the `00000000000003` migration
/// which adds triggers enforcing it).
pub const DB_TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// Helper to convert a `NaiveDateTime` from the database to `UtcDateTime`.
pub fn from_db_naive(dt: Option<NaiveDateTime>) -> Option<UtcDateTime> {
    dt.map(|n| UtcDateTime { naive: n })
}

/// Convert a `DateTime<Utc>` to `UtcDateTime`.
pub fn from_chrono_utc(dt: DateTime<Utc>) -> UtcDateTime {
    UtcDateTime {
        naive: dt.naive_utc(),
    }
}

/// Convert a naive-UTC timestamp string as stored by SQLite (e.g.
/// `"2026-06-18 20:00:00"`) into an RFC 3339 UTC string for API responses.
///
/// # API timestamp standard
/// Every timestamp leaving the API must carry an explicit zone, so DB-sourced
/// strings are normalized here rather than leaked to clients as naive values.
/// Delegates to [`parse_utc`] — the single inbound parser — so naive, zoned, and
/// date-only values all take the same path. Unparseable input (empty/legacy) is
/// returned unchanged so callers never lose data.
pub fn naive_utc_str_to_rfc3339(s: &str) -> String {
    match parse_utc(s) {
        Ok(dt) => dt.to_rfc3339_utc(),
        Err(_) => s.to_string(),
    }
}

/// Convert a mapping's DB-canonical `metadata_last_synced_at` values to RFC 3339
/// for API responses.
///
/// SSoT for the DB-canonical → RFC 3339 conversion of this nested config field
/// (it lives inside `series_mappings.data` JSON, so there is no column-level
/// serde boundary). Tolerant of already-zoned values.
pub fn synced_map_to_rfc3339(map: &mut std::collections::HashMap<String, String>) {
    for value in map.values_mut() {
        *value = naive_utc_str_to_rfc3339(value);
    }
}

// Timestamp parsing — two parsers, two boundaries:
//   * `parse_rfc3339` — STRICT. Requires an explicit offset (`Z` or ±HH:MM).
//     The only parser used at external boundaries (HTTP API, plugin contract).
//     Naive/date-only input is rejected with `MissingOffset`.
//   * `parse_utc` — LENIENT. Also accepts naive UTC and date-only (→ midnight
//     UTC). Reserved for values we already own: DB-canonical strings, persisted
//     config, and tolerant display parsing on the frontend.

/// Why a timestamp was rejected. Carries no allocation; the accepted-format
/// guidance lives once in its `Display` impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The input was empty (or whitespace only).
    Empty,
    /// The input did not match any accepted format.
    Invalid,
    /// The input was a valid date/datetime but lacked a timezone offset.
    /// RFC 3339 requires one (e.g. `2026-06-18T20:00:00Z`).
    MissingOffset,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "timestamp is empty"),
            ParseError::Invalid => write!(f, "invalid timestamp"),
            ParseError::MissingOffset => write!(
                f,
                "timestamp is missing a timezone offset (RFC 3339 requires one, \
                 e.g. 2026-06-18T20:00:00Z or 2026-06-18T20:00:00+00:00)"
            ),
        }
    }
}

impl std::error::Error for ParseError {}

/// Strict parser for **external input** (HTTP requests, plugin output).
///
/// The input MUST be RFC 3339 with an explicit offset (`Z` or `±HH:MM`):
/// `2026-06-18T20:00:00+00:00`, `2026-06-18T20:30:00+09:00`, `2026-06-18T20:00:00Z`.
/// Naive (`2026-06-18T20:00:00`) and date-only (`2026-06-18`) inputs are
/// rejected — the caller, not we, is responsible for knowing the zone.
///
/// Normalized to UTC.
pub fn parse_rfc3339(s: &str) -> Result<UtcDateTime, ParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    match DateTime::parse_from_rfc3339(s) {
        Ok(dt) => Ok(UtcDateTime::from_chrono_utc(dt.with_timezone(&Utc))),
        // Distinguish "valid but zone-less" from outright garbage so the error
        // tells the client exactly what to fix.
        Err(_) => match parse_utc(s) {
            Ok(_) => Err(ParseError::MissingOffset),
            Err(ParseError::Empty) => Err(ParseError::Empty),
            Err(_) => Err(ParseError::Invalid),
        },
    }
}

/// Lenient parser for values we already own — DB-canonical strings, persisted
/// config, and tolerant display parsing.
///
/// Accepts:
/// - RFC 3339 with offset/Z: `2026-06-18T20:00:00+00:00`
/// - Naive datetime (`T` or space separator, optional fractional seconds):
///   `2026-06-18 20:00:00` — interpreted as UTC
/// - Date-only: `2026-06-18` → **00:00:00 UTC**
///
/// Do **not** use this for external input; use [`parse_rfc3339`] there.
pub fn parse_utc(s: &str) -> Result<UtcDateTime, ParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(UtcDateTime::from_chrono_utc(dt.with_timezone(&Utc)));
    }
    // Naive datetime, ignoring any fractional seconds.
    let normalized = s.replace(' ', "T");
    let core = normalized.split('.').next().unwrap_or(&normalized);
    if let Ok(naive) = NaiveDateTime::parse_from_str(core, "%Y-%m-%dT%H:%M:%S") {
        return Ok(UtcDateTime::from_naive_utc(naive));
    }
    // Date-only → midnight UTC.
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(UtcDateTime::from_naive_date(date));
    }
    Err(ParseError::Invalid)
}

/// Canonicalize an arbitrary stored/ingested timestamp to the DB format
/// (`YYYY-MM-DD HH:MM:SS` UTC).
///
/// Returns the input unchanged when it cannot be parsed, so callers never
/// silently drop data (a trigger will reject a bad value at write time instead).
pub fn canonicalize_to_db_string(s: &str) -> String {
    match parse_utc(s) {
        Ok(dt) => dt.to_db_string(),
        Err(_) => s.to_string(),
    }
}

// Episode status + release-date selection

/// Derive whether an episode is "missing" (release datetime has passed) or
/// "unreleased" (release datetime is in the future / unknown).
///
/// Delegates to [`crate::formatting::derive_episode_status`] — the SSoT shared
/// between backend and frontend. This thin wrapper converts `UtcDateTime` to
/// `NaiveDateTime` for the shared function.
pub fn derive_episode_status(
    effective_date: Option<UtcDateTime>,
    now: UtcDateTime,
) -> crate::types::EpisodeStatus {
    crate::formatting::derive_episode_status(effective_date.map(|d| d.naive_utc()), now.naive_utc())
}

// SSoT `ReleaseDates<N>` formatting impls (NaiveDateTime→RFC 3339, SQL refs).

/// Extension trait adding formatting methods to `ReleaseDates<NaiveDateTime>`.
///
/// SSoT: `to_api()` is the only place naive→RFC 3339 conversion is coded.
pub trait ReleaseDatesExt {
    /// Convert all three dates to RFC 3339 strings in one call.
    fn to_api(&self) -> crate::types::ReleaseDates<String>;
}

impl ReleaseDatesExt for crate::types::ReleaseDates<NaiveDateTime> {
    fn to_api(&self) -> crate::types::ReleaseDates<String> {
        let f = |d: NaiveDateTime| naive_utc_to_rfc3339(d);
        crate::types::ReleaseDates {
            meta_date: self.meta_date.map(f),
            upload_date: self.upload_date.map(f),
            est_date: self.est_date.map(f),
        }
    }
}

/// Delegates to the shared `select_release_date_source` and maps the result to
/// `UtcDateTime`. The selection algorithm (priority order, enabled flags) is the
/// SSoT in [`crate::types::select_release_date_source`].
pub fn compute_effective_date(
    dates: &crate::types::ReleaseDates<NaiveDateTime>,
    order: &[String],
    metadata_enabled: bool,
    source_enabled: bool,
    estimated_enabled: bool,
) -> Option<UtcDateTime> {
    let source = crate::types::select_release_date_source(
        &crate::types::DateSourceFlags {
            meta: dates.meta_date.is_some(),
            upload: dates.upload_date.is_some(),
            est: dates.est_date.is_some(),
        },
        order,
        &crate::types::DateSourceFlags {
            meta: metadata_enabled,
            upload: source_enabled,
            est: estimated_enabled,
        },
    )?;
    let dt = match source {
        crate::types::ReleaseDateSource::MetaDate => dates.meta_date?,
        crate::types::ReleaseDateSource::UploadDate => dates.upload_date?,
        crate::types::ReleaseDateSource::EstDate => dates.est_date?,
    };
    Some(UtcDateTime::from_naive_utc(dt))
}

/// Pre-loaded release date display preferences. The selection logic SSoT is
/// `compute_effective_date` above; this is a convenience wrapper.
pub struct ReleaseDatePrefs {
    pub order: Vec<String>,
    pub metadata_enabled: bool,
    pub source_enabled: bool,
    pub estimated_enabled: bool,
}

impl ReleaseDatePrefs {
    /// Compute the effective date from three raw date columns, using the loaded prefs.
    /// Thin wrapper around `compute_effective_date` that avoids constructing
    /// `ReleaseDates` manually at each call site.
    pub fn effective_date(
        &self,
        meta_date: Option<NaiveDateTime>,
        upload_date: Option<NaiveDateTime>,
        est_date: Option<NaiveDateTime>,
    ) -> Option<UtcDateTime> {
        let dates = crate::types::ReleaseDates {
            meta_date,
            upload_date,
            est_date,
        };
        compute_effective_date(
            &dates,
            &self.order,
            self.metadata_enabled,
            self.source_enabled,
            self.estimated_enabled,
        )
    }
}

// sqlx integration (backend only). Stored as TEXT in SQLite using the same format
// as NaiveDateTime, so encoding/decoding delegates to the inner value. Gated so
// the frontend build never pulls sqlx.

#[cfg(feature = "backend")]
mod sqlx_impls {
    use super::*;
    use sqlx::sqlite::{SqliteArgumentValue, SqliteValueRef};
    type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

    impl sqlx::encode::Encode<'_, sqlx::Sqlite> for UtcDateTime {
        fn encode_by_ref(
            &self,
            buf: &mut Vec<SqliteArgumentValue<'_>>,
        ) -> Result<sqlx::encode::IsNull, BoxDynError> {
            let s = self.naive.format("%F %T%.f").to_string();
            <String as sqlx::encode::Encode<'_, sqlx::Sqlite>>::encode(s, buf)
        }
    }

    impl sqlx::decode::Decode<'_, sqlx::Sqlite> for UtcDateTime {
        fn decode(value: SqliteValueRef<'_>) -> Result<Self, BoxDynError> {
            let s = <String as sqlx::decode::Decode<'_, sqlx::Sqlite>>::decode(value)?;
            // Stored values are DB-canonical naive UTC. Use the same lenient parser
            // as everywhere else so the accepted shapes stay in one place.
            parse_utc(&s)
                .map(|d| d.naive_utc())
                .map(|naive| UtcDateTime { naive })
                .map_err(|e| Box::new(e) as BoxDynError)
        }
    }

    impl sqlx::types::Type<sqlx::Sqlite> for UtcDateTime {
        fn type_info() -> <sqlx::Sqlite as sqlx::Database>::TypeInfo {
            <NaiveDateTime as sqlx::types::Type<sqlx::Sqlite>>::type_info()
        }

        fn compatible(ty: &<sqlx::Sqlite as sqlx::Database>::TypeInfo) -> bool {
            <NaiveDateTime as sqlx::types::Type<sqlx::Sqlite>>::compatible(ty)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ReleaseDates;
    use chrono::Datelike;
    use chrono::TimeZone;
    use chrono::Timelike;

    #[test]
    fn test_now_is_utc() {
        let dt = UtcDateTime::now();
        let now_utc = Utc::now().naive_utc();
        // Should be within 1 second
        let diff = (dt.naive - now_utc).num_seconds().abs();
        assert!(diff < 2, "UtcDateTime::now should be close to Utc::now");
    }

    #[test]
    fn test_from_naive_date_is_midnight_utc() {
        let d = NaiveDate::from_ymd_opt(2025, 6, 17).unwrap();
        let dt = UtcDateTime::from_naive_date(d);
        assert_eq!(dt.naive_utc().format("%Y-%m-%d").to_string(), "2025-06-17");
        // Time should be midnight
        assert_eq!(dt.naive_utc().format("%H:%M:%S").to_string(), "00:00:00");
    }

    #[test]
    fn test_from_chrono_utc() {
        let chrono_dt = Utc.with_ymd_and_hms(2025, 6, 17, 14, 30, 0).unwrap();
        let dt = UtcDateTime::from_chrono_utc(chrono_dt);
        assert_eq!(dt.naive_utc().format("%H:%M:%S").to_string(), "14:30:00");
    }

    #[test]
    fn test_from_chrono_with_offset_converts_to_utc() {
        // 2 PM EST = 7 PM UTC
        let est = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
        let chrono_dt = est.with_ymd_and_hms(2025, 6, 17, 14, 0, 0).unwrap();
        let dt = UtcDateTime::from_chrono_with_offset(chrono_dt);
        assert_eq!(dt.naive_utc().hour(), 19); // 14:00 EST = 19:00 UTC
        assert_eq!(dt.naive_utc().day(), 17);
    }

    #[test]
    fn test_ordering() {
        let earlier = UtcDateTime::from_naive_date(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap());
        let later = UtcDateTime::from_naive_date(NaiveDate::from_ymd_opt(2025, 6, 17).unwrap());
        assert!(earlier < later);
    }

    #[test]
    fn test_display_is_full_datetime() {
        // A calendar-date value is a normal midnight instant: displayed with time.
        let dt = UtcDateTime::from_naive_date(NaiveDate::from_ymd_opt(2025, 6, 17).unwrap());
        assert_eq!(dt.to_string(), "2025-06-17 00:00:00");
    }

    #[test]
    fn test_serde_serializes_as_rfc3339_string() {
        let dt = UtcDateTime::from_naive_utc(
            NaiveDateTime::parse_from_str("2026-06-18 20:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
        );
        assert_eq!(
            serde_json::to_string(&dt).unwrap(),
            "\"2026-06-18T20:00:00+00:00\""
        );
    }

    #[test]
    fn test_serde_deserializes_rfc3339_string() {
        // Documented plugin contract: RFC 3339 with an explicit offset.
        let dt: UtcDateTime = serde_json::from_str("\"2026-06-18T22:00:00+02:00\"").unwrap();
        assert_eq!(dt.to_db_string(), "2026-06-18 20:00:00");
    }

    #[test]
    fn test_serde_rejects_zone_less_strings() {
        // Plugins are responsible for supplying the zone.
        for s in [
            "\"2026-06-18\"",
            "\"2026-06-18T20:00:00\"",
            "\"2026-06-18 20:00:00\"",
        ] {
            assert!(
                serde_json::from_str::<UtcDateTime>(s).is_err(),
                "{s} must be rejected (no offset)"
            );
        }
    }

    #[test]
    fn test_parse_rfc3339_accepts_offset_bearing_only() {
        assert_eq!(
            parse_rfc3339("2026-06-18T22:00:00+02:00")
                .unwrap()
                .to_db_string(),
            "2026-06-18 20:00:00"
        );
        assert_eq!(
            parse_rfc3339("2026-06-18T20:00:00Z")
                .unwrap()
                .to_db_string(),
            "2026-06-18 20:00:00"
        );
        // Fractional seconds are accepted (dropped on store).
        assert_eq!(
            parse_rfc3339("2026-06-18T20:00:00.123456Z")
                .unwrap()
                .to_db_string(),
            "2026-06-18 20:00:00"
        );
    }

    #[test]
    fn test_parse_rfc3339_rejects_zone_less() {
        assert_eq!(
            parse_rfc3339("2026-06-18T20:00:00"),
            Err(ParseError::MissingOffset)
        );
        assert_eq!(
            parse_rfc3339("2026-06-18 20:00:00"),
            Err(ParseError::MissingOffset)
        );
        assert_eq!(parse_rfc3339("2026-06-18"), Err(ParseError::MissingOffset));
        assert_eq!(parse_rfc3339("not-a-date"), Err(ParseError::Invalid));
        assert_eq!(parse_rfc3339(""), Err(ParseError::Empty));
    }

    #[test]
    fn test_serde_rejects_invalid_string() {
        assert!(serde_json::from_str::<UtcDateTime>("\"not-a-date\"").is_err());
    }

    #[test]
    fn test_display_timestamp() {
        let chrono_dt = Utc.with_ymd_and_hms(2025, 6, 17, 14, 30, 0).unwrap();
        let dt = UtcDateTime::from_chrono_utc(chrono_dt);
        assert_eq!(dt.to_string(), "2025-06-17 14:30:00");
    }

    #[test]
    fn test_naive_utc_to_rfc3339() {
        let naive =
            NaiveDateTime::parse_from_str("2026-06-18 20:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(naive_utc_to_rfc3339(naive), "2026-06-18T20:00:00+00:00");
    }

    #[test]
    fn test_naive_utc_str_to_rfc3339_typical() {
        assert_eq!(
            naive_utc_str_to_rfc3339("2026-06-18 20:00:00"),
            "2026-06-18T20:00:00+00:00"
        );
    }

    #[test]
    fn test_naive_utc_str_to_rfc3339_normalizes_offset() {
        assert_eq!(
            naive_utc_str_to_rfc3339("2026-06-18T22:00:00+02:00"),
            "2026-06-18T20:00:00+00:00"
        );
    }

    #[test]
    fn test_naive_utc_str_to_rfc3339_passthrough_on_unparseable() {
        assert_eq!(naive_utc_str_to_rfc3339(""), "");
        assert_eq!(naive_utc_str_to_rfc3339("garbage"), "garbage");
    }

    #[test]
    fn test_naive_utc_str_to_rfc3339_date_only_becomes_midnight_utc() {
        // Unified on `parse_utc`: a date-only legacy value is no longer leaked
        // zone-less but anchored at midnight UTC.
        assert_eq!(
            naive_utc_str_to_rfc3339("2026-06-18"),
            "2026-06-18T00:00:00+00:00"
        );
    }

    #[test]
    fn test_parse_utc_date_only_is_midnight() {
        assert_eq!(
            parse_utc("2026-06-18").unwrap().to_db_string(),
            "2026-06-18 00:00:00"
        );
    }

    #[test]
    fn test_parse_utc_normalizes_to_utc() {
        // Naive is taken as UTC.
        assert_eq!(
            parse_utc("2026-06-18 20:00:00").unwrap().to_db_string(),
            "2026-06-18 20:00:00"
        );
        // Offset is converted to UTC.
        assert_eq!(
            parse_utc("2026-06-18T22:00:00+02:00")
                .unwrap()
                .to_db_string(),
            "2026-06-18 20:00:00"
        );
        // Fractional seconds are dropped.
        assert_eq!(
            parse_utc("2026-06-18T20:00:00.123456Z")
                .unwrap()
                .to_db_string(),
            "2026-06-18 20:00:00"
        );
    }

    #[test]
    fn test_parse_utc_rejects_invalid() {
        assert_eq!(parse_utc("not-a-date"), Err(ParseError::Invalid));
        assert_eq!(parse_utc(""), Err(ParseError::Empty));
        assert_eq!(parse_utc("   "), Err(ParseError::Empty));
    }

    #[test]
    fn test_canonicalize_to_db_string() {
        assert_eq!(
            canonicalize_to_db_string("2026-06-18T20:00:00+00:00"),
            "2026-06-18 20:00:00"
        );
        assert_eq!(
            canonicalize_to_db_string("2026-06-18"),
            "2026-06-18 00:00:00"
        );
        // Unparseable input is preserved (never silently dropped).
        assert_eq!(canonicalize_to_db_string("garbage"), "garbage");
    }

    #[test]
    fn test_checked_add_signed_returns_new_instant() {
        let dt = UtcDateTime::from_naive_date(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap());
        let added = dt.checked_add_signed(chrono::Duration::days(7)).unwrap();
        assert_eq!(
            added.naive_utc().format("%Y-%m-%d").to_string(),
            "2025-01-08"
        );
    }

    #[test]
    fn test_ical_utc_format() {
        let chrono_dt = Utc.with_ymd_and_hms(2025, 6, 17, 14, 30, 0).unwrap();
        let dt = UtcDateTime::from_chrono_utc(chrono_dt);
        assert_eq!(dt.format_ical_utc(), "20250617T143000Z");
    }

    #[test]
    fn test_ical_date_is_utc_based() {
        // 23:00 UTC must stay on the same UTC date regardless of server timezone.
        let chrono_dt = Utc.with_ymd_and_hms(2025, 6, 17, 23, 0, 0).unwrap();
        let dt = UtcDateTime::from_chrono_utc(chrono_dt);
        assert_eq!(dt.format_ical_date(), "20250617");
    }

    fn ms() -> crate::types::EpisodeStatus {
        crate::types::EpisodeStatus::Missing
    }
    fn ur() -> crate::types::EpisodeStatus {
        crate::types::EpisodeStatus::Unreleased
    }

    #[test]
    fn test_derive_episode_status_missing_when_date_passed() {
        let past = UtcDateTime::from_naive_date(NaiveDate::from_ymd_opt(2020, 1, 1).unwrap());
        let now = UtcDateTime::from_naive_date(NaiveDate::from_ymd_opt(2025, 6, 19).unwrap());
        assert_eq!(derive_episode_status(Some(past), now), ms());
    }

    #[test]
    fn test_derive_episode_status_missing_when_exact_time_passed() {
        let past_time = UtcDateTime::from_naive_utc(
            NaiveDate::from_ymd_opt(2025, 6, 19)
                .unwrap()
                .and_hms_opt(8, 0, 0)
                .unwrap(),
        );
        let now = UtcDateTime::from_naive_utc(
            NaiveDate::from_ymd_opt(2025, 6, 19)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
        );
        assert_eq!(derive_episode_status(Some(past_time), now), ms());
    }

    #[test]
    fn test_derive_episode_status_unreleased_when_future_time_same_day() {
        let future_time = UtcDateTime::from_naive_utc(
            NaiveDate::from_ymd_opt(2025, 6, 19)
                .unwrap()
                .and_hms_opt(20, 0, 0)
                .unwrap(),
        );
        let now = UtcDateTime::from_naive_utc(
            NaiveDate::from_ymd_opt(2025, 6, 19)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
        );
        assert_eq!(derive_episode_status(Some(future_time), now), ur());
    }

    #[test]
    fn test_derive_episode_status_unreleased_when_no_date() {
        let now = UtcDateTime::from_naive_date(NaiveDate::from_ymd_opt(2025, 6, 19).unwrap());
        assert_eq!(derive_episode_status(None, now), ur());
    }

    #[test]
    fn test_derive_episode_status_unreleased_when_future_date() {
        let future = UtcDateTime::from_naive_date(NaiveDate::from_ymd_opt(2030, 1, 1).unwrap());
        let now = UtcDateTime::from_naive_date(NaiveDate::from_ymd_opt(2025, 6, 19).unwrap());
        assert_eq!(derive_episode_status(Some(future), now), ur());
    }

    #[test]
    fn test_release_dates_to_api_converts_all_fields() {
        let dt = NaiveDateTime::parse_from_str("2026-06-18 20:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let dates = ReleaseDates {
            meta_date: Some(dt),
            upload_date: Some(dt),
            est_date: Some(dt),
        };
        let api = dates.to_api();
        assert_eq!(api.meta_date.as_deref(), Some("2026-06-18T20:00:00+00:00"));
        assert_eq!(
            api.upload_date.as_deref(),
            Some("2026-06-18T20:00:00+00:00")
        );
        assert_eq!(api.est_date.as_deref(), Some("2026-06-18T20:00:00+00:00"));
    }

    #[test]
    fn test_release_dates_to_api_preserves_none() {
        let dates = ReleaseDates::<NaiveDateTime> {
            meta_date: None,
            upload_date: None,
            est_date: None,
        };
        let api = dates.to_api();
        assert!(api.meta_date.is_none());
        assert!(api.upload_date.is_none());
        assert!(api.est_date.is_none());
    }

    #[test]
    fn test_release_dates_to_api_mixed_none_and_some() {
        let dt = NaiveDateTime::parse_from_str("2026-06-18 20:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let dates = ReleaseDates {
            meta_date: Some(dt),
            upload_date: None,
            est_date: Some(dt),
        };
        let api = dates.to_api();
        assert!(api.meta_date.is_some());
        assert!(api.upload_date.is_none());
        assert!(api.est_date.is_some());
    }

    #[test]
    fn test_compute_effective_date_with_release_dates_metadata_priority() {
        let meta =
            NaiveDateTime::parse_from_str("2026-06-18 20:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let upload =
            NaiveDateTime::parse_from_str("2026-06-17 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let est =
            NaiveDateTime::parse_from_str("2026-06-19 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let dates = ReleaseDates {
            meta_date: Some(meta),
            upload_date: Some(upload),
            est_date: Some(est),
        };
        let order = vec!["metadata".into(), "source".into(), "estimated".into()];
        let result = compute_effective_date(&dates, &order, true, true, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().naive_utc(), meta);
    }

    #[test]
    fn test_compute_effective_date_with_release_dates_source_fallback() {
        let upload =
            NaiveDateTime::parse_from_str("2026-06-17 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let dates = ReleaseDates {
            meta_date: None,
            upload_date: Some(upload),
            est_date: None,
        };
        let order = vec!["metadata".into(), "source".into(), "estimated".into()];
        let result = compute_effective_date(&dates, &order, true, true, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().naive_utc(), upload);
    }

    #[test]
    fn test_compute_effective_date_with_release_dates_all_none() {
        let dates = ReleaseDates::<NaiveDateTime> {
            meta_date: None,
            upload_date: None,
            est_date: None,
        };
        let order = vec!["metadata".into(), "source".into(), "estimated".into()];
        let result = compute_effective_date(&dates, &order, true, true, true);
        assert!(result.is_none());
    }
}
