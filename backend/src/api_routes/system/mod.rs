//! System API route handlers — split into focused sub-modules.
//!
//! All public items are re-exported here so callers keep using
//! `crate::api_routes::system::*`; sub-modules stay `pub(crate)` behind this
//! stable interface, following the same pattern as `series/mod.rs`.

mod activity;
mod auto_search;
mod download_queue;
mod health;
mod logs;
mod media_info_scan;
pub(crate) mod organized_series;
mod rename_queue;
pub(crate) mod rename_queue_notifier;
mod search;
pub mod search_core;
mod status;

// Re-exports

pub use activity::{get_activity, get_wanted};

pub use auto_search::{
    auto_search_season, get_auto_season_status, parse_episode_numbers_from_title,
};

pub use download_queue::{
    delete_download_queue, get_queue, pause_download_queue, remove_from_queue,
    resume_download_queue, retry_download_queue,
};

pub use health::{
    AboutInfo, PermissionCheck, StorageHealth, SystemHealth, get_about, get_health,
    get_memory_stats, get_plugin_metrics, ping, remediate,
};

pub use jumbie_shared::types::LogEntry;
pub use logs::{get_log_level, get_logs, get_logs_file, parse_log_line, strip_ansi};

pub use organized_series::{
    batch_edit_organized_series, batch_move_series, batch_move_series_preview, batch_move_status,
    get_active_operations, get_organized_series, toggle_library_visibility, toggle_series_monitor,
};

pub use rename_queue::{get_rename_queue, get_rename_queue_detail, simulate_resolve_collision};

pub use media_info_scan::{get_media_info_scan_count_for_series, get_media_info_scan_counts};

pub use search::{auto_season_search, search_media, validate_path_endpoint};

pub use status::get_system_status;

/// Resolve the effective sort for a paginated system list.
///
/// Priority (SSoT for all list endpoints):
///   1. An explicit `sort`/`order` from the request — the user changed the sort.
///   2. The user's persisted preference for `table` (`ui_preferences.table_sorts`).
///   3. The endpoint's `default_col` / `default_asc`.
///
/// The client omits `sort`/`order` on its first request (before the preferences
/// are known), so step 2 is what makes the persisted sort take effect without a
/// second round-trip.
pub(crate) fn resolve_table_sort(
    ui: &jumbie_shared::config::UIConfig,
    table: &str,
    sort: Option<&str>,
    order: Option<&str>,
    default_col: &str,
    default_asc: bool,
) -> (String, bool) {
    match sort {
        Some(col) => (
            col.to_string(),
            order.map(|o| o == "asc").unwrap_or(default_asc),
        ),
        None => {
            let pref = ui.table_sort_or(table, default_col, default_asc);
            (pref.column, pref.ascending)
        }
    }
}

/// Resolve the effective minimum log level.
///
/// Priority (SSoT): explicit `min_level` from the request → the user's stored
/// `ui_preferences.logs.min_level` → the literal default.  The client omits the
/// parameter on its first request, so step 2 is what makes the stored filter
/// take effect without a second round-trip.
pub(crate) fn resolve_log_min_level(
    ui: &jumbie_shared::config::UIConfig,
    min_level: Option<&str>,
) -> String {
    match min_level {
        Some(l) => l.to_string(),
        None => ui.logs.min_level.clone(),
    }
    .to_uppercase()
}

/// Normalise an optional free-text query parameter into a lowercase needle for
/// case-insensitive substring matching.  Returns `None` when absent or blank.
/// SSoT for the shared `search` parameter across the list endpoints.
pub(crate) fn text_needle(search: Option<&str>) -> Option<String> {
    search
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
}

/// Resolve the effective activity type filter.
///
/// Priority (SSoT): explicit `types` from the request → the user's stored
/// `ui_preferences.activity.filter_types`.  Returns `None` when the resolved
/// value is empty ("show all types") — this also covers an explicit empty
/// `types=`, which must override a stored filter during the async persist.
pub(crate) fn resolve_activity_types(
    ui: &jumbie_shared::config::UIConfig,
    types: Option<&str>,
) -> Option<Vec<String>> {
    let raw = match types {
        Some(t) => t.to_string(),
        None => ui.activity.filter_types.join(","),
    };
    if raw.trim().is_empty() {
        None
    } else {
        Some(
            raw.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_activity_types, resolve_log_min_level, resolve_table_sort};
    use jumbie_shared::config::{TableSortState, UIConfig};

    #[test]
    fn pagination_query_deserializes_all_optional_fields() {
        use axum::extract::Query;
        use axum::http::Uri;
        use jumbie_shared::types::PaginationQuery;
        let uri: Uri = "/?page=1&limit=10&sort=level&order=asc&search=foo&min_level=WARN&types=download,import"
            .parse()
            .unwrap();
        let Query(q) = Query::<PaginationQuery>::try_from_uri(&uri).unwrap();
        assert_eq!(q.page, Some(1));
        assert_eq!(q.limit, Some(10));
        assert_eq!(q.sort.as_deref(), Some("level"));
        assert_eq!(q.order.as_deref(), Some("asc"));
        assert_eq!(q.search.as_deref(), Some("foo"));
        assert_eq!(q.min_level.as_deref(), Some("WARN"));
        assert_eq!(q.types.as_deref(), Some("download,import"));
    }

    #[test]
    fn pagination_query_deserializes_when_filters_omitted() {
        use axum::extract::Query;
        use axum::http::Uri;
        use jumbie_shared::types::PaginationQuery;
        let uri: Uri = "/?page=0&limit=100".parse().unwrap();
        let Query(q) = Query::<PaginationQuery>::try_from_uri(&uri).unwrap();
        assert_eq!(q.page, Some(0));
        assert_eq!(q.limit, Some(100));
        assert!(q.sort.is_none());
        assert!(q.search.is_none());
        assert!(q.min_level.is_none());
        assert!(q.types.is_none());
    }

    #[test]
    fn log_min_level_falls_back_to_stored_preference() {
        let mut ui = UIConfig::default();
        ui.logs.min_level = "warn".to_string();
        // Omitted → stored preference (normalised to uppercase).
        assert_eq!(resolve_log_min_level(&ui, None), "WARN");
        // Explicit value wins.
        assert_eq!(resolve_log_min_level(&ui, Some("debug")), "DEBUG");
    }

    #[test]
    fn activity_types_fall_back_to_stored_preference() {
        let mut ui = UIConfig::default();
        ui.activity.filter_types = vec!["download".to_string(), "import".to_string()];
        assert_eq!(
            resolve_activity_types(&ui, None),
            Some(vec!["download".to_string(), "import".to_string()])
        );
        // Explicit empty overrides the stored filter → show all.
        assert_eq!(resolve_activity_types(&ui, Some("")), None);
        // Explicit value wins (lower-cased).
        assert_eq!(
            resolve_activity_types(&ui, Some("Metadata")),
            Some(vec!["metadata".to_string()])
        );
    }

    fn ui_with_sort(table: &str, column: &str, ascending: bool) -> UIConfig {
        let mut ui = UIConfig::default();
        ui.table_sorts.insert(
            table.to_string(),
            TableSortState {
                column: column.to_string(),
                ascending,
            },
        );
        ui
    }

    #[test]
    fn explicit_sort_wins_over_preference() {
        let ui = ui_with_sort("system_logs", "level", true);
        let (col, asc) = resolve_table_sort(
            &ui,
            "system_logs",
            Some("message"),
            Some("asc"),
            "timestamp",
            false,
        );
        assert_eq!(col, "message");
        assert!(asc);
    }

    #[test]
    fn omitted_sort_uses_stored_preference() {
        let ui = ui_with_sort("system_logs", "level", true);
        let (col, asc) = resolve_table_sort(&ui, "system_logs", None, None, "timestamp", false);
        assert_eq!(col, "level");
        assert!(asc);
    }

    #[test]
    fn omitted_sort_without_preference_uses_endpoint_default() {
        let ui = UIConfig::default();
        let (col, asc) = resolve_table_sort(&ui, "system_logs", None, None, "timestamp", false);
        assert_eq!(col, "timestamp");
        assert!(!asc);
    }

    #[test]
    fn explicit_sort_without_order_uses_default_direction() {
        let ui = UIConfig::default();
        let (col, asc) =
            resolve_table_sort(&ui, "system_logs", Some("level"), None, "timestamp", false);
        assert_eq!(col, "level");
        assert!(!asc);
    }
}
