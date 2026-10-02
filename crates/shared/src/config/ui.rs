use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// An enum rather than a raw String so invalid values (e.g. "blurple") can't be
// persisted and the UI can branch on the theme.
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq)]
pub struct TableSortState {
    pub column: String,
    pub ascending: bool,
}

impl UIConfig {
    /// Resolve the persisted sort for `table`, falling back to the given default.
    ///
    /// SSoT for the "no explicit sort in the request → use the stored preference"
    /// fallback shared by the paginated list endpoints.
    pub fn table_sort_or(
        &self,
        table: &str,
        default_col: &str,
        default_asc: bool,
    ) -> TableSortState {
        self.table_sorts
            .get(table)
            .cloned()
            .unwrap_or(TableSortState {
                column: default_col.to_string(),
                ascending: default_asc,
            })
    }
}

// A separate module (not inlined into Config): UI preferences are migrated
// independently, stored entirely in the DB (never `config.toml`), and read/written
// by the frontend through the API.
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct UIConfig {
    // Nested because log filtering has its own concerns (levels, pagination, search).
    #[serde(default)]
    pub logs: LogsUIConfig,
    #[serde(default)]
    pub theme: Theme,
    // Separated because release date display has multiple sub-options (sources and
    // their order).
    #[serde(default)]
    pub release_date_display: ReleaseDateDisplayConfig,
    // HashMap (not a fixed struct) because the set of tables is dynamic — plugins
    // can add their own.
    #[serde(default)]
    pub table_sorts: HashMap<String, TableSortState>,
    // View modes are per-page and potentially plugin-defined, so a HashMap fits.
    #[serde(default)]
    pub view_modes: HashMap<String, String>,
    #[serde(default)]
    pub activity: ActivityUIConfig,
    #[serde(default)]
    pub time_format: TimeFormat,
}

// Controls which release date sources are shown and in what order. A series can
// carry a metadata date (TVDB/TMDB), a source date (RSS/Nyaa publish time), and an
// estimated date (organizer scheduler); `order` lets users rank them.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ReleaseDateDisplayConfig {
    #[serde(default = "default_release_date_priority")]
    pub order: Vec<String>,
    // All three default enabled; disabling one is a deliberate choice to hide info.
    #[serde(default = "crate::config::default_true")]
    pub metadata_enabled: bool,
    #[serde(default = "crate::config::default_true")]
    pub source_enabled: bool,
    #[serde(default = "crate::config::default_true")]
    pub estimated_enabled: bool,
}

// Metadata first is authoritative; source second (feed publish time); estimated
// last as a best guess. Putting estimated first would show "not yet released"
// dates as primary even when metadata says the episode aired last week.
fn default_release_date_priority() -> Vec<String> {
    crate::types::ReleaseDateSource::default_order()
        .iter()
        .map(|s| s.as_str().to_string())
        .collect()
}

impl Default for ReleaseDateDisplayConfig {
    fn default() -> Self {
        Self {
            order: default_release_date_priority(),
            metadata_enabled: true,
            source_enabled: true,
            estimated_enabled: true,
        }
    }
}

// Nested because log viewing is cross-cutting: min_level affects the API, the UI,
// and stdout emission, and the surface will likely grow.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LogsUIConfig {
    // "INFO" by default: DEBUG is too verbose in production and ERROR would hide
    // warnings users should see.
    pub min_level: String,
}

impl Default for LogsUIConfig {
    fn default() -> Self {
        Self {
            min_level: "INFO".to_string(),
        }
    }
}

/// 12-hour or 24-hour clock for displayed times.
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TimeFormat {
    #[default]
    Hour12,
    Hour24,
}

// Nested because activity filtering may grow (multi-select, date range, status).
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct ActivityUIConfig {
    // Lowercase type names (e.g. ["download", "import"]); empty means all types.
    #[serde(default)]
    pub filter_types: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_sort_or_prefers_stored_preference() {
        let mut ui = UIConfig::default();
        ui.table_sorts.insert(
            "wanted".to_string(),
            TableSortState {
                column: "series".to_string(),
                ascending: true,
            },
        );
        let s = ui.table_sort_or("wanted", "age", false);
        assert_eq!(s.column, "series");
        assert!(s.ascending);
    }

    #[test]
    fn table_sort_or_falls_back_to_default() {
        let ui = UIConfig::default();
        let s = ui.table_sort_or("wanted", "age", false);
        assert_eq!(s.column, "age");
        assert!(!s.ascending);
    }
}
