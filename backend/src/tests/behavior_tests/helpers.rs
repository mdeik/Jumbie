use std::collections::HashMap;

use jumbie_shared::mapping::{MappingRule, SeasonOverride, SeriesSettings};

/// Build a `SeasonOverride` with only the fields relevant to these tests,
/// filling the rest with sensible defaults.
pub fn make_season_override(
    season: &str,
    alias_season_number: Option<u32>,
    episode_offset: Option<i32>,
    aliases: Vec<String>,
) -> SeasonOverride {
    SeasonOverride {
        season: season.to_string(),
        episode_start: None,
        episode_end: None,
        cell_count: None,
        episode_offset,
        alias_season_number,
        search_format: None,
        aliases,
        reg_patterns: vec![],
    }
}

/// Build a `MappingRule` pre-populated with a `SeriesSettings` whose `season`
/// field is set to `season_overrides`. All other fields are at their default
/// values so each test only overrides what it cares about.
pub fn make_mapping(
    target_title: &str,
    season_overrides: HashMap<String, SeasonOverride>,
) -> MappingRule {
    MappingRule {
        target_title: target_title.to_string(),
        name: target_title.to_lowercase().replace(' ', "_"),
        quality_profile: None,
        release_profile: None,
        qb_category: None,
        filters: None,
        scoring: None,
        series_id: String::new(),
        hidden_in_library: false,
        settings: SeriesSettings {
            aliases: vec![],
            absolute_numbering: Some(false),
            season: season_overrides,
            season_absolute: HashMap::new(),
            reg_patterns: vec![],
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            path: None,
            monitor_mode: None,
            metadata_ids: HashMap::new(),
            metadata_last_synced_at: HashMap::new(),
            last_known_dir_mtimes: HashMap::new(),
        },
    }
}
