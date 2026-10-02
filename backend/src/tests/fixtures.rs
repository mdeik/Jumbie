use jumbie_shared::config::Config;
use jumbie_shared::mapping::{MappingRule, MonitorMode, SeriesSettings};
use std::path::Path;

/// Provides a standard `SeriesSettings` for tests to prevent reconstructing fields.
pub fn default_series_settings() -> SeriesSettings {
    SeriesSettings {
        flatten_season_folders: None, // None = don't flatten = use season folders (default)
        path: Some("TestPath".to_string()),
        monitor_mode: Some(MonitorMode::All),
        ..Default::default()
    }
}

/// Provides a standard `MappingRule` for tests.
pub fn default_mapping() -> MappingRule {
    MappingRule {
        target_title: "Test Series".to_string(),
        name: "test_series".to_string(),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
        qb_category: Some("Series".to_string()),
        settings: default_series_settings(),
        ..Default::default()
    }
}

/// Provides a standard mock `Config` containing only strictly validated fields.
pub fn default_config() -> Config {
    let config_json = serde_json::json!({
        "database": ":memory:",
        "organization": {
            "destination_roots": [{ "path": "/data/media" }],
            "collision_handling": "rename",
            "season_folder_format": "S${season:02}",
            "episode_file_format": "${series} - S${season:02}E${episode:02} - ${title}",
            "season_folder_format_absolute": "S01",
            "episode_file_format_absolute": "${series} - S${season:02}E${episode:02} - ${title}",
            "rename_episodes": true,
            "auto_apply_renames": false
        },
        "sources": {},
        "general": {},
        "proxy": {},
        "auth": {},
        "security": {}
    });
    serde_json::from_value(config_json).expect("Failed to parse test config mock")
}

/// Provides a standard `CreateSeriesRequest` for tests.
pub fn default_create_series_request() -> jumbie_shared::types::CreateSeriesRequest {
    jumbie_shared::types::CreateSeriesRequest {
        path: "Test Series".to_string(),
        series_name: Some("Test Series".to_string()),
        scan_for_existing: Some(false),
        monitor_mode: Some(MonitorMode::All),
        quality_profile: None,
        release_profile: None,
        metadata_ids: Default::default(),
        search_missing_on_add: false,
        // Match the Add Series form's default: root-derived names go through the
        // org policy (sanitize + collision resolve). Tests that want the
        // claim-existing-folder behavior set collision_handling = "overwrite".
        resolve_collisions: true,
        settings: Default::default(),
    }
}

/// Build an organization config rooted at a specific destination path.
/// Used by `file_manager` tests that need a concrete temp directory.
pub fn config_with_dest_root(db_path: &Path, dest_root: &Path) -> Config {
    let config_json = serde_json::json!({
        "database": db_path.to_string_lossy().to_string(),
        "organization": {
            "destination_roots": [{ "path": dest_root.to_string_lossy().to_string() }],
            "collision_handling": "rename",
            "season_folder_format": "S${season:02}",
            "episode_file_format": "${series} - S${season:02}E${episode:02}",
            "rename_episodes": true,
            "auto_apply_renames": false
        },
        "sources": {},
        "general": {},
        "proxy": {},
        "auth": {},
        "security": {}
    });
    serde_json::from_value(config_json).expect("Failed to parse test config")
}

/// A clean `MappingRule` for link_sibling_episodes tests (no path override,
/// so `build_target_path` falls through to `destination_roots` from config).
pub fn link_sibling_mapping() -> MappingRule {
    MappingRule {
        target_title: "Test Series".to_string(),
        name: "test_series".to_string(),
        quality_profile: Some("Any".to_string()),
        release_profile: Some("Any".to_string()),
        ..Default::default()
    }
}

/// Provides a minimal `CreateSeriesRequest` with a custom title.
///
/// WHY relative path: `setup_test_app()` points `destination_roots` at a unique
/// TempDir; a relative path is resolved under it by `validate_path`, avoiding
/// platform-specific absolute-path issues.
pub fn minimal_test_request(title: &str) -> jumbie_shared::types::CreateSeriesRequest {
    jumbie_shared::types::CreateSeriesRequest {
        series_name: Some(title.to_string()),
        path: title.to_string(),
        ..default_create_series_request()
    }
}
