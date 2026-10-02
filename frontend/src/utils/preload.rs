// Selective preloading by navigation group.

use chrono::Local;
use jumbie_shared::plugin::plugin_type_key;
use std::cell::RefCell;
use std::collections::HashSet;

// Session-level dedup set: tracks which groups have been preloaded so rapid sidebar
// clicks don't re-enter the match arms (allocating strings, computing dates, looping
// over plugins) on every click. preload_api_cache already dedups at the HTTP level,
// but this avoids the Rust-side overhead entirely for repeat visits.
thread_local! {
    static PRELOADED_GROUPS: RefCell<HashSet<String>> =
        RefCell::new(HashSet::new());
}

// Shared helper functions, eliminating duplication between this module (called from
// the sidebar on hover) and `hooks/use_preload.rs` (called on app mount).

/// Preload calendar data for the current window.
pub fn preload_calendar() {
    // Same "today" derivation as `Calendar` — one concept, one spelling.
    let today = Local::now().date_naive();
    let (ws, we) = crate::utils::calc_calendar_window(today);
    let sd = ws.format("%Y-%m-%d").to_string();
    let ed = we.format("%Y-%m-%d").to_string();
    let cache_key = format!("fetch_calendar_{}_{}", sd, ed);
    let req_start = crate::utils::calendar::local_date_to_rfc3339(ws, false);
    let req_end = crate::utils::calendar::local_date_to_rfc3339(we, true);
    let start_enc = js_sys::encode_uri_component(&req_start)
        .as_string()
        .unwrap_or(req_start);
    let end_enc = js_sys::encode_uri_component(&req_end)
        .as_string()
        .unwrap_or(req_end);
    crate::utils::preload_api_cache(cache_key, move || {
        let s = start_enc.clone();
        let e = end_enc.clone();
        async move { crate::api::fetch_calendar(&s, &e).await }
    });
}

// Selective preloading by group name. Preloading all endpoints on app mount would
// generate dozens of API calls for data the user may never look at, so we preload
// only the endpoints needed for a given navigation group. Invoked when the user
// hovers over or clicks a sidebar link. The group names correspond directly to
// sidebar section identifiers.
pub fn preload_group_data(group_id: &str) {
    // Session-level dedup — skip if this group was already preloaded once.
    if !PRELOADED_GROUPS.with(|g| g.borrow_mut().insert(group_id.to_string())) {
        return;
    }
    match group_id {
        "series" => {
            crate::utils::preload_api_cache("fetch_series".to_string(), || {
                crate::api::fetch_series()
            });
            crate::utils::preload_api_cache("fetch_quality_profiles".to_string(), || {
                crate::api::fetch_quality_profiles()
            });
        }
        "wanted" => {
            // Omitting sort/filter params lets the backend apply the stored
            // preferences (see `resolve_table_sort`), so the preloaded page
            // matches the table's first request exactly.
            crate::utils::preload_api_cache("fetch_wanted_episodes:0".to_string(), || {
                crate::api::fetch_wanted_episodes(0, 50, None, None)
            });
        }
        "activity" => {
            crate::utils::preload_api_cache("fetch_activity:0".to_string(), || {
                crate::api::fetch_activity(0, 50, None, None, None)
            });
        }
        "calendar" => {
            preload_calendar();
        }
        "profiles" => {
            crate::utils::preload_api_cache("fetch_qualities".to_string(), || {
                crate::api::fetch_qualities()
            });
            crate::utils::preload_api_cache("fetch_quality_profiles".to_string(), || {
                crate::api::fetch_quality_profiles()
            });
            crate::utils::preload_api_cache("fetch_release_profiles".to_string(), || {
                crate::api::fetch_release_profiles()
            });
        }
        "settings" | "authentication" => {
            crate::utils::preload_api_cache("fetch_available_plugins".to_string(), || {
                crate::api::fetch_available_plugins()
            });
            crate::utils::preload_api_cache("fetch_plugins_cfg".to_string(), || {
                crate::api::fetch_plugins_cfg()
            });
            crate::utils::preload_api_cache("fetch_config".to_string(), || {
                crate::api::fetch_config()
            });
            crate::utils::preload_api_cache("fetch_health".to_string(), || {
                crate::api::fetch_health()
            });

            // Preload plugin schemas from cached available plugins so that
            // clicking "Edit" on any plugin is instant.
            if let Some(plugins) = crate::utils::read_cache::<
                Vec<jumbie_shared::plugin::PluginInstanceInfo>,
            >("fetch_available_plugins")
            {
                for plugin in plugins {
                    // Use plugin_id (e.g. "jumbie.tvmaze") when available, falling back
                    // to display_name for backwards compatibility. The backend accepts
                    // both forms and extracts the short name.
                    let plugin_key = plugin
                        .plugin_id
                        .as_deref()
                        .unwrap_or(&plugin.display_name)
                        .to_lowercase();
                    let plugin_type = plugin_type_key(&plugin_key).to_string();
                    let cache_key = format!("fetch_plugin_schema_{}", plugin_type);
                    crate::utils::preload_api_cache(cache_key, move || {
                        let pid = plugin_type.clone();
                        async move { crate::api::fetch_plugin_schema(&pid).await }
                    });
                }
            }
        }
        "management" => {
            crate::utils::preload_api_cache("fetch_config".to_string(), || {
                crate::api::fetch_config()
            });
            crate::utils::preload_api_cache("fetch_rename_queue".to_string(), || {
                crate::api::fetch_rename_queue()
            });
            crate::utils::preload_api_cache("fetch_download_queue".to_string(), || {
                crate::api::fetch_download_queue()
            });
            crate::utils::preload_api_cache("fetch_organized_series".to_string(), || {
                crate::api::fetch_organized_series()
            });
        }
        "system" => {
            crate::utils::preload_api_cache("fetch_health".to_string(), || {
                crate::api::fetch_health()
            });
            crate::utils::preload_api_cache("fetch_plugin_status".to_string(), || {
                crate::api::fetch_plugin_status()
            });
            crate::utils::preload_api_cache("fetch_logs:0".to_string(), || {
                crate::api::fetch_logs(0, 100, None, None, None)
            });
            crate::utils::preload_api_cache("fetch_about".to_string(), || {
                crate::api::fetch_about()
            });
        }
        _ => {}
    }
}

/// Fetch quality profiles on component mount using the standard two-phase
/// cache-and-fetch pattern.
pub fn use_quality_profiles<S>(setter: S)
where
    S: Fn(std::collections::HashMap<String, jumbie_shared::scoring::QualityProfile>)
        + Clone
        + 'static,
{
    crate::utils::use_api_cache(
        "fetch_quality_profiles".to_string(),
        || crate::api::fetch_quality_profiles(),
        setter,
    );
}
