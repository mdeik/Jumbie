use std::collections::HashMap;

use crate::config::ui::{TableSortState, Theme, UIConfig};
use crate::scoring::{Quality, QualityProfile, ReleaseProfile};

/// Default qualities to seed into the database when it's first created.
///
/// Keys are UUID-like so a quality keeps a stable identifier even if renamed in
/// the UI. Tags (not regex) are used because they are human-readable and match
/// how users think about qualities.
pub fn default_qualities() -> HashMap<String, Quality> {
    let mut q = HashMap::new();

    q.insert(
        "quality-4k-uuid-00000000000".to_string(),
        Quality {
            name: "4k".to_string(),
            tags: vec!["4k".into(), "2160p".into(), "2160".into()],
        },
    );

    q.insert(
        "quality-1080p-uuid-00000000".to_string(),
        Quality {
            name: "1080p".to_string(),
            tags: vec![
                "1080p".into(),
                "1920x1080".into(),
                "1080".into(),
                "fhd".into(),
            ],
        },
    );

    q.insert(
        "quality-720p-uuid-000000000".to_string(),
        Quality {
            name: "720p".to_string(),
            tags: vec!["720p".into(), "1280x720".into(), "720".into(), "hd".into()],
        },
    );

    q.insert(
        "quality-sd-uuid-00000000000".to_string(),
        Quality {
            name: "SD".to_string(),
            tags: vec!["480p".into(), "480".into(), "sd".into()],
        },
    );

    q.insert(
        "quality-bd-uuid-00000000000".to_string(),
        Quality {
            name: "Blu-ray".to_string(),
            tags: vec!["blu-ray".into(), "bluray".into(), "bd".into()],
        },
    );

    q.insert(
        "quality-web-uuid-00000000000".to_string(),
        Quality {
            name: "WEB".to_string(),
            tags: vec!["web-rip".into(), "web".into()],
        },
    );

    q.insert(
        "quality-dvd-uuid-00000000000".to_string(),
        Quality {
            name: "DVD".to_string(),
            tags: vec!["dvdrip".into(), "dvd".into()],
        },
    );

    q
}

/// Default quality profiles to seed into the database.
///
/// "High Definition" is 720p+ with upgrades allowed. SD is excluded: including it
/// would let an SD release satisfy the profile before a 720p+ one arrives, causing
/// a wasteful extra download under `upgrades_allowed`.
pub fn default_quality_profiles() -> HashMap<String, QualityProfile> {
    let mut p = HashMap::new();

    p.insert(
        "high-def-profile-uuid-000000".to_string(),
        QualityProfile {
            name: "High Definition".to_string(),
            qualities: vec![
                "quality-4k-uuid-00000000000".into(),
                "quality-1080p-uuid-00000000".into(),
                "quality-720p-uuid-000000000".into(),
            ],
            upgrade_only_qualities: vec![],
        },
    );

    p
}

/// Default release profiles to seed into the database.
///
/// "Default Weights" is the only seed: sensible scores (BluRay > WebDL > 1080p >
/// 720p) with `min_score` 10, so anything with a positive score is acceptable.
/// A lower `min_score` would let through releases with no positive terms (score 0).
pub fn default_release_profiles() -> HashMap<String, ReleaseProfile> {
    let mut p = HashMap::new();

    let mut terms = HashMap::new();
    terms.insert("1080p".to_string(), 10);
    terms.insert("720p".to_string(), 5);
    terms.insert("BluRay".to_string(), 15);
    terms.insert("WebDL".to_string(), 12);

    p.insert(
        "default-weights-uuid-0000000".to_string(),
        ReleaseProfile {
            name: "Default Weights".to_string(),
            case_insensitive_terms: false,
            case_insensitive_submitters: false,
            min_score: 10,
            terms,
            ..Default::default()
        },
    );

    p
}

/// Default UI preferences to seed into the database.
///
/// `table_sorts` covers the three most-visited views so they don't fall back to
/// arbitrary insertion order. `Theme::Auto` is the least opinionated default.
pub fn default_ui_preferences() -> UIConfig {
    UIConfig {
        table_sorts: HashMap::from([
            (
                "series_library".to_string(),
                TableSortState {
                    column: "title".to_string(),
                    ascending: true,
                },
            ),
            (
                "system_rename_queue".to_string(),
                TableSortState {
                    column: "series".to_string(),
                    ascending: true,
                },
            ),
            (
                "system_managed_folders".to_string(),
                TableSortState {
                    column: "name".to_string(),
                    ascending: true,
                },
            ),
        ]),
        theme: Theme::Auto,
        ..Default::default()
    }
}
