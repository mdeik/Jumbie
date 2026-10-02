use std::str::FromStr;

/// Boolean flags for the three release date sources, used as grouped input
/// to [`select_release_date_source`].
///
/// The same struct is used for both the "has data" availability check and
/// the "is enabled" preference check — the parameter name at the call site
/// disambiguates them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateSourceFlags {
    pub meta: bool,
    pub upload: bool,
    pub est: bool,
}

/// Identifies which release date source was selected as the effective date.
///
/// Each variant maps to a canonical string value used in config serialization
/// and the `select_release_date_source` priority order.  Use [`as_str`] to get
/// the canonical string — never hardcode `"metadata"`, `"source"`, or
/// `"estimated"` elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDateSource {
    MetaDate,
    UploadDate,
    EstDate,
}

impl ReleaseDateSource {
    /// Canonical string representation (SSoT — single definition of the magic
    /// strings that appear in config files, the priority order, and tests).
    pub fn as_str(&self) -> &'static str {
        match self {
            ReleaseDateSource::MetaDate => "metadata",
            ReleaseDateSource::UploadDate => "source",
            ReleaseDateSource::EstDate => "estimated",
        }
    }

    /// All variants in the default priority order (metadata → source → estimated).
    pub const fn default_order() -> [ReleaseDateSource; 3] {
        [
            ReleaseDateSource::MetaDate,
            ReleaseDateSource::UploadDate,
            ReleaseDateSource::EstDate,
        ]
    }
}

impl FromStr for ReleaseDateSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "metadata" => Ok(ReleaseDateSource::MetaDate),
            "source" => Ok(ReleaseDateSource::UploadDate),
            "estimated" => Ok(ReleaseDateSource::EstDate),
            _ => Err(format!("unknown release date source: {s}")),
        }
    }
}

/// Given availability flags and enabled preferences, determine which date source
/// should be used as the "effective" release date.
///
/// SSoT — the single selection algorithm used by both backend and frontend; each side
/// delegates here and maps the result to its own types.
///
/// * `has_data` — which of the three sources have actual data available.
/// * `order` — priority order of sources (values matching [`ReleaseDateSource::as_str`]).
/// * `enabled` — which sources are enabled in the user's preferences.
///
/// # Algorithm
///
/// **Pass 1:** among enabled sources that have data, pick the highest-priority one.
/// **Pass 2:** if none has data, return the highest-priority enabled source (for
/// placeholder display).
pub fn select_release_date_source(
    has_data: &DateSourceFlags,
    order: &[String],
    enabled: &DateSourceFlags,
) -> Option<ReleaseDateSource> {
    let has_any_enabled = enabled.meta || enabled.upload || enabled.est;
    if !has_any_enabled {
        return None;
    }

    // Pass 1: among enabled sources WITH data, pick the highest priority.
    for source in order {
        match source.parse::<ReleaseDateSource>() {
            Ok(ReleaseDateSource::MetaDate) if enabled.meta && has_data.meta => {
                return Some(ReleaseDateSource::MetaDate);
            }
            Ok(ReleaseDateSource::UploadDate) if enabled.upload && has_data.upload => {
                return Some(ReleaseDateSource::UploadDate);
            }
            Ok(ReleaseDateSource::EstDate) if enabled.est && has_data.est => {
                return Some(ReleaseDateSource::EstDate);
            }
            _ => {}
        }
    }

    // Pass 2: no enabled source has data — return the first enabled source.
    for source in order {
        match source.parse::<ReleaseDateSource>() {
            Ok(ReleaseDateSource::MetaDate) if enabled.meta => {
                return Some(ReleaseDateSource::MetaDate);
            }
            Ok(ReleaseDateSource::UploadDate) if enabled.upload => {
                return Some(ReleaseDateSource::UploadDate);
            }
            Ok(ReleaseDateSource::EstDate) if enabled.est => {
                return Some(ReleaseDateSource::EstDate);
            }
            _ => {}
        }
    }

    None
}

/// The three release-date sources for an episode.
///
/// Parametric on `N` so the same struct works for the DB layer
/// (`chrono::NaiveDateTime`) and the API/serialization layer (`String`).
///
/// SSoT: the three date fields are declared here, never as individual flat fields
/// in every struct; a struct carrying release dates embeds `ReleaseDates<T>`.
/// `meta_date` is the provider airdate (TVDB, TMDB), `upload_date` the source feed
/// publish date (RSS, Nyaa), `est_date` the computed/user release estimate.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReleaseDates<N> {
    pub meta_date: Option<N>,
    pub upload_date: Option<N>,
    pub est_date: Option<N>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ReleaseDateSource roundtrip

    #[test]
    fn release_date_source_as_str_roundtrip() {
        for variant in &[
            ReleaseDateSource::MetaDate,
            ReleaseDateSource::UploadDate,
            ReleaseDateSource::EstDate,
        ] {
            let s = variant.as_str();
            let parsed: Option<ReleaseDateSource> = s.parse().ok();
            assert_eq!(parsed, Some(*variant), "roundtrip failed for {s:?}");
        }
    }

    #[test]
    fn release_date_source_from_str_unknown_returns_none() {
        let r1: Option<ReleaseDateSource> = "unknown".parse().ok();
        let r2: Option<ReleaseDateSource> = "".parse().ok();
        assert_eq!(r1, None);
        assert_eq!(r2, None);
    }

    #[test]
    fn release_date_source_default_order_canonical() {
        let strs: Vec<&str> = ReleaseDateSource::default_order()
            .iter()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(strs, vec!["metadata", "source", "estimated"]);
    }

    // select_release_date_source

    fn has(meta: bool, upload: bool, est: bool) -> DateSourceFlags {
        DateSourceFlags { meta, upload, est }
    }

    fn enabled(meta: bool, upload: bool, est: bool) -> DateSourceFlags {
        DateSourceFlags { meta, upload, est }
    }

    fn default_order_strs() -> Vec<String> {
        ReleaseDateSource::default_order()
            .iter()
            .map(|s| s.as_str().to_string())
            .collect()
    }

    fn order_strs(sources: &[ReleaseDateSource]) -> Vec<String> {
        sources.iter().map(|s| s.as_str().to_string()).collect()
    }

    #[test]
    fn selects_metadata_when_highest_priority() {
        let order = default_order_strs();
        let result =
            select_release_date_source(&has(true, true, true), &order, &enabled(true, true, true));
        assert_eq!(result, Some(ReleaseDateSource::MetaDate));
    }

    #[test]
    fn selects_source_when_metadata_disabled() {
        let order = default_order_strs();
        let result =
            select_release_date_source(&has(true, true, true), &order, &enabled(false, true, true));
        assert_eq!(result, Some(ReleaseDateSource::UploadDate));
    }

    #[test]
    fn selects_estimated_when_higher_sources_missing() {
        let order = default_order_strs();
        let result = select_release_date_source(
            &has(false, false, true),
            &order,
            &enabled(true, true, true),
        );
        assert_eq!(result, Some(ReleaseDateSource::EstDate));
    }

    #[test]
    fn skips_disabled_source_even_with_data() {
        let order = vec![
            ReleaseDateSource::MetaDate.as_str().to_string(),
            ReleaseDateSource::UploadDate.as_str().to_string(),
        ];
        let result = select_release_date_source(
            &has(true, true, false),
            &order,
            &enabled(false, true, true),
        );
        assert_eq!(result, Some(ReleaseDateSource::UploadDate));
    }

    #[test]
    fn all_disabled_returns_none() {
        let order = vec![ReleaseDateSource::MetaDate.as_str().to_string()];
        let result = select_release_date_source(
            &has(true, false, false),
            &order,
            &enabled(false, false, false),
        );
        assert_eq!(result, None);
    }

    #[test]
    fn respects_reversed_order() {
        let order = order_strs(&[
            ReleaseDateSource::EstDate,
            ReleaseDateSource::UploadDate,
            ReleaseDateSource::MetaDate,
        ]);
        let result =
            select_release_date_source(&has(true, true, true), &order, &enabled(true, true, true));
        assert_eq!(result, Some(ReleaseDateSource::EstDate));
    }

    #[test]
    fn no_data_shows_first_enabled_source_as_placeholder() {
        let order = default_order_strs();
        let result = select_release_date_source(
            &has(false, false, false),
            &order,
            &enabled(true, true, true),
        );
        assert_eq!(result, Some(ReleaseDateSource::MetaDate));
    }

    #[test]
    fn first_enabled_source_with_data_wins_over_higher_priority_without() {
        let order = default_order_strs();
        let result =
            select_release_date_source(&has(false, true, true), &order, &enabled(true, true, true));
        assert_eq!(result, Some(ReleaseDateSource::UploadDate));
    }

    #[test]
    fn test_release_dates_serialization_roundtrip() {
        let dates = ReleaseDates {
            meta_date: Some("2026-06-18T20:00:00+00:00".to_string()),
            upload_date: None,
            est_date: Some("2026-06-20T00:00:00+00:00".to_string()),
        };
        let json = serde_json::to_string(&dates).unwrap();
        let deserialized: ReleaseDates<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(dates, deserialized);
    }

    #[test]
    fn test_release_dates_json_field_names() {
        let dates = ReleaseDates::<String> {
            meta_date: Some("2026-06-18T20:00:00+00:00".to_string()),
            upload_date: None,
            est_date: None,
        };
        let json = serde_json::to_value(&dates).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("meta_date"));
        assert!(obj.contains_key("upload_date"));
        assert!(obj.contains_key("est_date"));
        assert_eq!(obj["meta_date"], "2026-06-18T20:00:00+00:00");
        assert_eq!(obj["upload_date"], serde_json::Value::Null);
        assert_eq!(obj["est_date"], serde_json::Value::Null);
    }

    #[test]
    fn test_release_dates_default_is_all_none() {
        let dates = ReleaseDates::<String>::default();
        assert!(dates.meta_date.is_none());
        assert!(dates.upload_date.is_none());
        assert!(dates.est_date.is_none());
    }
}
