use super::{delete_unit, get, post_unit};
use crate::api_client::ApiError as Error;
use crate::components::common::structs::{AboutInfo, SystemHealth, SystemStatus};
use jumbie_shared::config::TableSortState;
use jumbie_shared::types::LogEntry;

// Query-string helpers (SSoT for list-endpoint parameters). The three list
// fetchers below share the same `search` and `sort` encoding, kept in one place
// so the URL format can't drift between endpoints.

/// Append `&search=<url-encoded>` when a non-empty free-text filter is present.
/// An absent or empty filter adds nothing, so the backend applies its own
/// default (there is no stored `search` preference).
fn append_search(path: &mut String, search: &Option<String>) {
    if let Some(s) = search.as_ref().filter(|s| !s.is_empty()) {
        // Free-text search can contain spaces/`&`; encode it.
        let encoded = js_sys::encode_uri_component(s)
            .as_string()
            .unwrap_or_else(|| s.clone());
        path.push_str(&format!("&search={}", encoded));
    }
}

/// Append `&sort=<column>&order=<asc|desc>` when an explicit sort override is
/// present.  When absent the backend applies the stored preference.
fn append_sort(path: &mut String, sort: &Option<TableSortState>) {
    if let Some(s) = sort {
        path.push_str(&format!(
            "&sort={}&order={}",
            s.column,
            if s.ascending { "asc" } else { "desc" }
        ));
    }
}

pub async fn fetch_health() -> Result<SystemHealth, Error> {
    crate::debug_log!("fetch_health()");
    let result: Result<SystemHealth, Error> = get("system/health").await;
    match &result {
        Ok(h) => crate::debug_log!("fetch_health: status={}", h.status),
        Err(e) => crate::debug_error!("fetch_health failed: {}", e),
    }
    result
}

/// Fetch a page of system logs.
///
/// `min_level`, `search` and `sort` are `None` until the user overrides them;
/// when omitted the backend applies the user's stored preferences (see
/// `resolve_table_sort` / `resolve_log_min_level` on the backend).  `search` is
/// a case-insensitive free-text filter over the message.
pub async fn fetch_logs(
    page: i64,
    limit: i64,
    search: Option<String>,
    sort: Option<TableSortState>,
    min_level: Option<String>,
) -> Result<jumbie_shared::types::PaginatedResponse<LogEntry>, Error> {
    crate::debug_log!(
        "fetch_logs(page={}, limit={}, min_level={:?}, search={:?}, sort={:?})",
        page,
        limit,
        min_level,
        search,
        sort
    );
    let mut path = format!("system/logs?page={}&limit={}", page, limit);
    if let Some(ml) = &min_level {
        path.push_str(&format!("&min_level={}", ml));
    }
    append_search(&mut path, &search);
    append_sort(&mut path, &sort);
    let result: Result<jumbie_shared::types::PaginatedResponse<LogEntry>, Error> = get(&path).await;
    match &result {
        Ok(r) => crate::debug_log!(
            "fetch_logs: got {} entries (total: {})",
            r.items.len(),
            r.total
        ),
        Err(e) => crate::debug_error!("fetch_logs failed: {}", e),
    }
    result
}

/// Fetches the backend's configured log level (e.g. "INFO", "DEBUG", "TRACE").
/// The frontend uses this to limit the level selector in the logs viewer.
pub async fn fetch_log_level() -> Result<String, Error> {
    crate::debug_log!("fetch_log_level()");
    let result: Result<serde_json::Value, Error> = get("system/log-level").await;
    let level = result.map(|v| {
        v.get("level")
            .and_then(|l| l.as_str())
            .unwrap_or("INFO")
            .to_string()
    });
    match &level {
        Ok(l) => crate::debug_log!("fetch_log_level: {}", l),
        Err(e) => crate::debug_error!("fetch_log_level failed: {}", e),
    }
    level
}

/// Fetch the recent activity log with pagination and optional filtering.
///
/// `search`, `sort` and `types` are `None` until the user overrides them; when
/// omitted the backend applies the user's stored preferences.  `types` is a
/// comma-separated list of lowercase activity type names (e.g. `"download,import"`).
pub async fn fetch_activity(
    page: i64,
    limit: i64,
    search: Option<String>,
    sort: Option<TableSortState>,
    types: Option<String>,
) -> Result<jumbie_shared::types::PaginatedResponse<jumbie_shared::types::ActivityItem>, Error> {
    crate::debug_log!(
        "fetch_activity(page={}, limit={}, search={:?}, sort={:?}, types={:?})",
        page,
        limit,
        search,
        sort,
        types
    );
    let mut path = format!("activity?page={}&limit={}", page, limit);
    append_search(&mut path, &search);
    append_sort(&mut path, &sort);
    // Send `types` whenever an override is present — including the empty string,
    // which explicitly means "all types".  Omitting it instead would let the
    // backend fall back to a stored filter the user just cleared (the persist is
    // async, so it may not have landed yet).
    if let Some(t) = &types {
        path.push_str(&format!("&types={}", t));
    }
    let result: Result<
        jumbie_shared::types::PaginatedResponse<jumbie_shared::types::ActivityItem>,
        Error,
    > = get(&path).await;
    match &result {
        Ok(r) => crate::debug_log!(
            "fetch_activity: got {} entries (total: {})",
            r.items.len(),
            r.total
        ),
        Err(e) => crate::debug_error!("fetch_activity failed: {}", e),
    }
    result
}

/// Episodes that are still missing a downloaded file — the "Wanted" view.
///
/// `search` and `sort` are `None` until the user overrides them; when omitted
/// the backend applies the user's stored preference.  `search` matches series
/// title, episode title, and season/episode numbers.
pub async fn fetch_wanted_episodes(
    page: i64,
    limit: i64,
    search: Option<String>,
    sort: Option<TableSortState>,
) -> Result<jumbie_shared::types::PaginatedResponse<jumbie_shared::types::WantedEpisode>, Error> {
    crate::debug_log!(
        "fetch_wanted_episodes(page={}, limit={}, search={:?}, sort={:?})",
        page,
        limit,
        search,
        sort
    );
    let mut path = format!("wanted?page={}&limit={}", page, limit);
    append_search(&mut path, &search);
    append_sort(&mut path, &sort);
    let result: Result<
        jumbie_shared::types::PaginatedResponse<jumbie_shared::types::WantedEpisode>,
        Error,
    > = get(&path).await;
    match &result {
        Ok(r) => crate::debug_log!(
            "fetch_wanted_episodes: got {} entries (total: {})",
            r.items.len(),
            r.total
        ),
        Err(e) => crate::debug_error!("fetch_wanted_episodes failed: {}", e),
    }
    result
}

pub async fn fetch_about() -> Result<AboutInfo, Error> {
    crate::debug_log!("fetch_about()");
    let result: Result<AboutInfo, Error> = get("system/about").await;
    match &result {
        Ok(info) => crate::debug_log!("fetch_about: version={}", info.version),
        Err(e) => crate::debug_error!("fetch_about failed: {}", e),
    }
    result
}

pub async fn fetch_system_status() -> Result<SystemStatus, Error> {
    crate::debug_log!("fetch_system_status()");
    let result: Result<SystemStatus, Error> = get("status").await;
    match &result {
        Ok(s) => crate::debug_log!(
            "fetch_system_status: download_failed={}, rename_failed={}, rename_pop={}, dl_disabled={}, wanted={}",
            s.download_queue_has_failed,
            s.rename_queue_has_failed,
            s.rename_queue_populated,
            s.downloader_disabled,
            s.wanted_has_items,
        ),
        Err(e) => crate::debug_error!("fetch_system_status failed: {}", e),
    }
    result
}

// Ban Management Endpoints

pub async fn fetch_bans() -> Result<Vec<jumbie_shared::types::BanEntry>, Error> {
    crate::debug_log!("fetch_bans()");
    let result: Result<Vec<jumbie_shared::types::BanEntry>, Error> = get("auth/bans").await;
    match &result {
        Ok(bans) => crate::debug_log!("fetch_bans: {} bans", bans.len()),
        Err(e) => crate::debug_error!("fetch_bans failed: {}", e),
    }
    result
}

pub async fn add_ban(ip: String, duration_seconds: Option<u64>) -> Result<(), Error> {
    crate::debug_log!("add_ban({}, {:?})", ip, duration_seconds);
    let result = post_unit(
        "auth/bans",
        &jumbie_shared::types::AddBanPayload {
            ip,
            duration_seconds,
        },
    )
    .await;
    match &result {
        Ok(_) => crate::debug_log!("add_ban succeeded"),
        Err(e) => crate::debug_error!("add_ban failed: {}", e),
    }
    result
}

pub async fn remove_ban(ip: String) -> Result<(), Error> {
    crate::debug_log!("remove_ban({})", ip);
    let result = delete_unit(&format!("auth/bans/{}", ip)).await;
    match &result {
        Ok(_) => crate::debug_log!("remove_ban({}) succeeded", ip),
        Err(e) => crate::debug_error!("remove_ban({}) failed: {}", ip, e),
    }
    result
}

// Remediation

pub async fn remediate(action: String) -> Result<(), Error> {
    crate::debug_log!("remediate({})", action);
    let body = jumbie_shared::types::RemediatePayload { action };
    let result: Result<(), Error> = post_unit("system/remediate", &body).await;
    match &result {
        Ok(_) => crate::debug_log!("remediate({}) succeeded", body.action),
        Err(e) => crate::debug_error!("remediate({}) failed: {}", body.action, e),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sort(column: &str, ascending: bool) -> Option<TableSortState> {
        Some(TableSortState {
            column: column.to_string(),
            ascending,
        })
    }

    #[test]
    fn append_sort_encodes_column_and_direction() {
        let mut p = String::from("x?page=0");
        append_sort(&mut p, &sort("level", true));
        assert_eq!(p, "x?page=0&sort=level&order=asc");

        let mut p = String::from("x?page=0");
        append_sort(&mut p, &sort("timestamp", false));
        assert_eq!(p, "x?page=0&sort=timestamp&order=desc");
    }

    #[test]
    fn append_sort_none_is_noop() {
        let mut p = String::from("x?page=0");
        append_sort(&mut p, &None);
        assert_eq!(p, "x?page=0");
    }

    #[test]
    fn append_search_none_or_empty_is_noop() {
        // Only the include/skip decision is asserted here: the encoding path
        // calls `js_sys::encode_uri_component`, which is wasm-only.  Both cases
        // must leave the path untouched so the backend applies its default.
        let mut p = String::from("x?page=0");
        append_search(&mut p, &None);
        assert_eq!(p, "x?page=0");

        let mut p = String::from("x?page=0");
        append_search(&mut p, &Some(String::new()));
        assert_eq!(p, "x?page=0");
    }
}
