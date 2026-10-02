//! Serde adapters for timestamps that are stored as naive UTC.
//!
//! # API timestamp standard
//!
//! Every timestamp that crosses the API boundary MUST carry an explicit
//! timezone: RFC 3339 with a UTC offset (`2026-06-18T20:00:00+00:00`). Clients
//! therefore never have to guess a zone.
//!
//! The database, however, persists timestamps as naive UTC (SQLite
//! `CURRENT_TIMESTAMP` / `datetime('now')` produce `YYYY-MM-DD HH:MM:SS` with no
//! zone). Types that are used for both DB rows and API responses keep the
//! in-process value as [`chrono::NaiveDateTime`] for sqlx, and use these
//! adapters to convert to/from RFC 3339 only at the serialization boundary.
//!
//! Apply with:
//! ```ignore
//! #[serde(with = "crate::serde_utc::naive_utc")]
//! pub downloaded_at: NaiveDateTime,
//!
//! #[serde(default, with = "crate::serde_utc::option_naive_utc")]
//! pub next_retry_at: Option<NaiveDateTime>,
//! ```

use chrono::NaiveDateTime;
use serde::{Deserialize, Deserializer, Serializer};

/// Serde adapter for a `NaiveDateTime` known to be UTC.
pub mod naive_utc {
    use super::*;

    /// Serialize as RFC 3339 with a UTC offset.
    pub fn serialize<S>(value: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&crate::datetime::naive_utc_to_rfc3339(*value))
    }

    /// Deserialize a timestamp and normalize it to naive UTC.
    ///
    /// Delegates to [`crate::datetime::parse_rfc3339`] — external input must be
    /// RFC 3339 with an explicit offset. Naive/date-only strings are rejected.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<NaiveDateTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        crate::datetime::parse_rfc3339(&raw)
            .map(|dt| dt.naive_utc())
            .map_err(serde::de::Error::custom)
    }
}

/// Serde adapter for an optional `NaiveDateTime` known to be UTC.
pub mod option_naive_utc {
    use super::*;

    /// Serialize as RFC 3339 with a UTC offset, or `null` when absent.
    pub fn serialize<S>(value: &Option<NaiveDateTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(dt) => serializer.serialize_str(&crate::datetime::naive_utc_to_rfc3339(*dt)),
            None => serializer.serialize_none(),
        }
    }

    /// Deserialize an optional timestamp, normalized to naive UTC.
    /// Delegates to [`crate::datetime::parse_rfc3339`] (see `naive_utc::deserialize`).
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<NaiveDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<String>::deserialize(deserializer)? {
            Some(raw) => crate::datetime::parse_rfc3339(&raw)
                .map(|dt| Some(dt.naive_utc()))
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Wrapper {
        #[serde(with = "crate::serde_utc::naive_utc")]
        at: NaiveDateTime,
        #[serde(default, with = "crate::serde_utc::option_naive_utc")]
        maybe: Option<NaiveDateTime>,
    }

    #[test]
    fn serializes_naive_utc_with_offset() {
        let value = Wrapper {
            at: NaiveDateTime::parse_from_str("2026-06-18 20:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
            maybe: None,
        };
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["at"], "2026-06-18T20:00:00+00:00");
        assert!(json["maybe"].is_null());
    }

    #[test]
    fn roundtrips_rfc3339_and_normalizes_to_naive_utc() {
        let json = serde_json::json!({
            "at": "2026-06-18T22:00:00+02:00",
            "maybe": "2026-06-19T00:00:00Z",
        });
        let parsed: Wrapper = serde_json::from_value(json).unwrap();
        assert_eq!(
            parsed.at,
            NaiveDateTime::parse_from_str("2026-06-18 20:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
        );
        assert_eq!(
            parsed.maybe,
            Some(
                NaiveDateTime::parse_from_str("2026-06-19 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
            )
        );
    }

    #[test]
    fn missing_optional_field_defaults_to_none() {
        let parsed: Wrapper = serde_json::from_value(serde_json::json!({
            "at": "2026-06-18T20:00:00Z"
        }))
        .unwrap();
        assert!(parsed.maybe.is_none());
    }

    #[test]
    fn rejects_zone_less_input() {
        // External input must carry an explicit offset (RFC 3339).
        for zone_less in ["2026-06-18 20:00:00", "2026-06-18T20:00:00", "2026-06-19"] {
            let json = serde_json::json!({ "at": zone_less });
            assert!(
                serde_json::from_value::<Wrapper>(json).is_err(),
                "{zone_less} must be rejected (no offset)"
            );
        }
    }
}
