use crate::datetime::UtcDateTime;
use std::collections::HashMap;

// Metadata provider modules. tvmaze / tvdb are external API clients implementing
// the MetadataPlugin trait; each has its own ID format (slug vs numeric) and auth
// pattern, so they live in separate modules despite the shared trait.
pub mod tvdb;
pub mod tvmaze;

// Normalised Metadata Model — every provider maps its own schema into this common
// representation, keeping the rest of the system provider-agnostic.
//
// Seasons and episodes are separate collections because episodes carry per-item
// data (title, description, meta_date) for display and filtering while seasons
// carry aggregate data (episode_count) for pack-scoring and statistics; neither
// can be derived from the other without extra API calls.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SeriesMetadata {
    pub episodes: Vec<EpisodeMetadata>,
    pub seasons: Vec<SeasonMetadata>,
}

/// Metadata about the series itself (title, overview, etc.) fetched from
/// a metadata provider. Used to update the series title from an online source.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SeriesMetadataInfo {
    /// The canonical title of the series.
    pub name: String,
    /// Optional overview / description.
    pub overview: Option<String>,
    /// The original country of the series (ISO 3166-1 alpha-3, e.g. "jpn", "usa").
    /// Used by TVDB for timezone-aware date interpretation.
    pub original_country: Option<String>,
    /// A map of language → aliases, if the provider returns them.
    /// Keys are language codes (e.g. "eng"), values are alias lists.
    #[serde(default)]
    pub aliases: HashMap<String, Vec<String>>,
    /// URL to the series poster/cover art image.
    pub image_url: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EpisodeMetadata {
    pub unique_id: String,
    pub season: i32,
    pub episode: i32,
    pub title: String,
    // Optional fields: the provider may not return every field for every episode.
    // The DB stores these as NULL and the UI gracefully degrades.
    pub description: Option<String>,
    pub runtime: Option<i32>,
    pub image_url: Option<String>,
    pub meta_date: Option<UtcDateTime>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SeasonMetadata {
    pub season: i32,
    // i32 (not u32) because SQLite has no unsigned integer support — this avoids
    // a per-value conversion; negative values are caught at validation time.
    pub episode_count: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_json() -> serde_json::Value {
        serde_json::json!({
            "unique_id": "1",
            "season": 1,
            "episode": 1,
            "title": "Pilot",
            "description": null,
            "runtime": null,
            "image_url": null,
            "meta_date": null
        })
    }

    /// The documented plugin contract sends `meta_date` as an RFC 3339 / ISO 8601
    /// string. Regression guard: `UtcDateTime` deserializes from a string (it used
    /// to expect an internal `{naive, origin}` object, so a spec-conformant
    /// plugin would fail to deserialize).
    #[test]
    fn episode_metadata_deserializes_meta_date_string() {
        let mut json = base_json();
        json["meta_date"] = serde_json::json!("2026-06-18T20:00:00+00:00");
        let ep: EpisodeMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(ep.meta_date.unwrap().to_db_string(), "2026-06-18 20:00:00");
    }

    #[test]
    fn episode_metadata_accepts_null_and_rejects_zone_less_meta_date() {
        let ep: EpisodeMetadata = serde_json::from_value(base_json()).unwrap();
        assert!(ep.meta_date.is_none());

        // Plugins are responsible for providing an explicit offset.
        for bad in ["2026-06-18", "2026-06-18T20:00:00", "2026-06-18 20:00:00"] {
            let mut json = base_json();
            json["meta_date"] = serde_json::json!(bad);
            assert!(
                serde_json::from_value::<EpisodeMetadata>(json).is_err(),
                "{bad} must be rejected (no offset)"
            );
        }
    }
}
