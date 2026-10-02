use crate::api::AppState;
use crate::models::activity::ActivityItem;
use axum::{
    Json,
    extract::{Query, State},
};
use jumbie_shared::types::{PaginatedResponse, PaginationQuery};
use std::sync::Arc;

fn sort_activity_items(items: &mut [ActivityItem], sort_col: &str, sort_asc: bool) {
    match sort_col {
        "timestamp" => {
            items.sort_by_key(|a| a.timestamp);
        }
        "type" => {
            items.sort_by(|a, b| {
                format!("{:?}", a.activity_type).cmp(&format!("{:?}", b.activity_type))
            });
        }
        "title" => {
            items.sort_by(|a, b| a.title.cmp(&b.title));
        }
        "status" => {
            items.sort_by(|a, b| a.status.cmp(&b.status));
        }
        _ => {
            items.sort_by_key(|a| a.timestamp);
        }
    }
    if !sort_asc {
        items.reverse();
    }
}

pub async fn get_activity(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PaginationQuery>,
) -> Json<PaginatedResponse<ActivityItem>> {
    tracing::debug!(
        "get_activity called: page={}, limit={}, sort={:?}, types={:?}",
        query.page.unwrap_or(0),
        query.limit.unwrap_or(50),
        query.sort,
        query.types
    );
    let page = query.page.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);

    // The client omits sort/filter on its first request, so stored preferences
    // are applied here and only overridden by explicit request values.
    let ui = state.db.get_ui_preferences().await.unwrap_or_default();
    let (sort_col, sort_asc) = super::resolve_table_sort(
        &ui,
        "activity",
        query.sort.as_deref(),
        query.order.as_deref(),
        "timestamp",
        false,
    );

    // Comma-separated lowercase activity type names; empty/absent (after the
    // stored-preference fallback) means "show all".
    let type_filter = super::resolve_activity_types(&ui, query.types.as_deref());

    match state.db.get_recent_activity(1000).await {
        Ok(mut items) => {
            if let Some(ref filter) = type_filter {
                items.retain(|item| filter.iter().any(|t| t == item.activity_type.as_str()));
            }
            if let Some(needle) = super::text_needle(query.search.as_deref()) {
                items.retain(|item| {
                    item.title.to_lowercase().contains(&needle)
                        || item
                            .details
                            .as_deref()
                            .is_some_and(|d| d.to_lowercase().contains(&needle))
                        || item.status.to_lowercase().contains(&needle)
                });
            }
            sort_activity_items(&mut items, &sort_col, sort_asc);
            paginate_response(items, page, limit)
        }
        Err(e) => {
            tracing::error!("Failed to fetch activity: {}", e);
            paginate_response(Vec::new(), page, limit)
        }
    }
}

fn sort_wanted_episodes(
    items: &mut [jumbie_shared::types::WantedEpisode],
    sort_col: &str,
    sort_asc: bool,
) {
    match sort_col {
        "series" => {
            items.sort_by(|a, b| {
                a.series_title
                    .to_lowercase()
                    .cmp(&b.series_title.to_lowercase())
            });
        }
        "episode" => {
            items.sort_by(|a, b| a.season.cmp(&b.season).then(a.episode.cmp(&b.episode)));
        }
        "age" => {
            items.sort_by(|a, b| {
                a.eff_date.cmp(&b.eff_date).then(
                    a.series_title
                        .to_lowercase()
                        .cmp(&b.series_title.to_lowercase()),
                )
            });
        }
        "status" => {
            items.sort_by(|a, b| a.status.cmp(&b.status));
        }
        _ => {
            items.sort_by(|a, b| {
                a.series_title
                    .to_lowercase()
                    .cmp(&b.series_title.to_lowercase())
            });
        }
    }
    if !sort_asc {
        items.reverse();
    }
}

/// Match a wanted episode against a normalized (lowercase) search needle.
///
/// Covers the fields a user is likely to search by: series title, episode
/// title, and the season/episode numbers.  Numeric queries accept the plain
/// number as well as an `s`/`e` prefix (e.g. "3", "03", "e3", "s3").
fn wanted_episode_matches(item: &jumbie_shared::types::WantedEpisode, needle: &str) -> bool {
    if item.series_title.to_lowercase().contains(needle) {
        return true;
    }
    if item
        .title
        .as_deref()
        .is_some_and(|t| t.to_lowercase().contains(needle))
    {
        return true;
    }
    if let Ok(n) = needle.trim_start_matches(['s', 'e']).parse::<i32>() {
        if item.episode == n {
            return true;
        }
        let season_num = item
            .season
            .as_deref()
            .and_then(|s| s.trim_start_matches(['s', 'S']).parse::<i32>().ok());
        if season_num == Some(n) {
            return true;
        }
    }
    false
}

pub async fn get_wanted(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PaginationQuery>,
) -> Json<PaginatedResponse<jumbie_shared::types::WantedEpisode>> {
    tracing::debug!(
        "get_wanted called: page={}, limit={}, sort={:?}",
        query.page.unwrap_or(0),
        query.limit.unwrap_or(50),
        query.sort
    );
    let page = query.page.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);

    // Default to the stored sort when the request omits it.
    let ui = state.db.get_ui_preferences().await.unwrap_or_default();
    let (sort_col, sort_asc) = super::resolve_table_sort(
        &ui,
        "wanted",
        query.sort.as_deref(),
        query.order.as_deref(),
        "age",
        false,
    );

    match state.db.get_wanted_episodes().await {
        Ok(mut items) => {
            if let Some(needle) = super::text_needle(query.search.as_deref()) {
                items.retain(|item| wanted_episode_matches(item, &needle));
            }
            sort_wanted_episodes(&mut items, &sort_col, sort_asc);
            paginate_response(items, page, limit)
        }
        Err(e) => {
            tracing::error!("Failed to fetch wanted episodes: {}", e);
            paginate_response(Vec::new(), page, limit)
        }
    }
}

fn paginate_response<T>(items: Vec<T>, page: i64, limit: i64) -> Json<PaginatedResponse<T>> {
    let total = items.len() as i64;
    let paginated_items = items
        .into_iter()
        .skip((page * limit) as usize)
        .take(limit as usize)
        .collect();
    Json(PaginatedResponse {
        items: paginated_items,
        total,
        page,
        page_size: limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::activity::ActivityType;
    use chrono::TimeZone;

    fn make_item(
        id: &str,
        ts_secs: i64,
        act_type: &str,
        title: &str,
        status: &str,
    ) -> ActivityItem {
        let act = match act_type {
            "download" => ActivityType::Download,
            "metadata" => ActivityType::Metadata,
            "import" => ActivityType::Import,
            "reassign" => ActivityType::Reassign,
            "assign" => ActivityType::Assign,
            "analyze" => ActivityType::Analyze,
            "delete" => ActivityType::Delete,
            "unassign" => ActivityType::Unassign,
            _ => ActivityType::Import,
        };
        ActivityItem {
            id: id.to_string(),
            timestamp: chrono::Utc.timestamp_opt(ts_secs, 0).unwrap(),
            activity_type: act,
            title: title.to_string(),
            details: None,
            status: status.to_string(),
        }
    }

    #[test]
    fn test_sort_activity_by_timestamp_desc_default() {
        let mut items = vec![
            make_item("1", 300, "download", "Beta", "Success"),
            make_item("2", 100, "metadata", "Alpha", "Failed"),
            make_item("3", 200, "import", "Gamma", "Success"),
        ];
        sort_activity_items(&mut items, "timestamp", false);
        assert_eq!(items[0].id, "1");
        assert_eq!(items[1].id, "3");
        assert_eq!(items[2].id, "2");
    }

    #[test]
    fn test_sort_activity_by_timestamp_asc() {
        let mut items = vec![
            make_item("1", 300, "download", "Beta", "Success"),
            make_item("2", 100, "metadata", "Alpha", "Failed"),
            make_item("3", 200, "import", "Gamma", "Success"),
        ];
        sort_activity_items(&mut items, "timestamp", true);
        assert_eq!(items[0].id, "2");
        assert_eq!(items[1].id, "3");
        assert_eq!(items[2].id, "1");
    }

    #[test]
    fn test_sort_activity_by_title_asc() {
        let mut items = vec![
            make_item("1", 100, "download", "Zeta", "Success"),
            make_item("2", 200, "metadata", "Alpha", "Success"),
            make_item("3", 300, "import", "Beta", "Failed"),
        ];
        sort_activity_items(&mut items, "title", true);
        assert_eq!(items[0].title, "Alpha");
        assert_eq!(items[1].title, "Beta");
        assert_eq!(items[2].title, "Zeta");
    }

    #[test]
    fn test_sort_activity_by_title_desc() {
        let mut items = vec![
            make_item("1", 100, "download", "Alpha", "Success"),
            make_item("2", 200, "metadata", "Zeta", "Success"),
            make_item("3", 300, "import", "Beta", "Failed"),
        ];
        sort_activity_items(&mut items, "title", false);
        assert_eq!(items[0].title, "Zeta");
        assert_eq!(items[1].title, "Beta");
        assert_eq!(items[2].title, "Alpha");
    }

    #[test]
    fn test_sort_activity_by_status() {
        let mut items = vec![
            make_item("1", 100, "download", "A", "Success"),
            make_item("2", 200, "metadata", "B", "Failed"),
            make_item("3", 300, "import", "C", "Pending"),
        ];
        sort_activity_items(&mut items, "status", true);
        assert_eq!(items[0].status, "Failed");
        assert_eq!(items[1].status, "Pending");
        assert_eq!(items[2].status, "Success");
    }

    #[test]
    fn test_sort_activity_by_type() {
        let mut items = vec![
            make_item("1", 100, "delete", "A", "Success"),
            make_item("2", 200, "assign", "B", "Success"),
            make_item("3", 300, "import", "C", "Success"),
        ];
        sort_activity_items(&mut items, "type", true);
        // alphabetical: Assign < Delete < Import
        assert_eq!(items[0].activity_type, ActivityType::Assign);
        assert_eq!(items[1].activity_type, ActivityType::Delete);
        assert_eq!(items[2].activity_type, ActivityType::Import);
    }

    fn make_wanted(
        series: &str,
        season: Option<&str>,
        episode: i32,
        title: Option<&str>,
    ) -> jumbie_shared::types::WantedEpisode {
        jumbie_shared::types::WantedEpisode {
            series_id: "sid".to_string(),
            series_title: series.to_string(),
            episode_id: "eid".to_string(),
            season: season.map(String::from),
            episode,
            title: title.map(String::from),
            eff_date: String::new(),
            dates: jumbie_shared::types::ReleaseDates {
                meta_date: None,
                upload_date: None,
                est_date: None,
            },
            status: "missing".to_string(),
        }
    }

    #[test]
    fn test_wanted_search_matches_series_and_episode_title() {
        let item = make_wanted("Breaking Bad", Some("1"), 3, Some("Pilot"));
        assert!(wanted_episode_matches(&item, "breaking"));
        assert!(wanted_episode_matches(&item, "pilot"));
        assert!(!wanted_episode_matches(&item, "better call saul"));
    }

    #[test]
    fn test_wanted_search_matches_episode_number() {
        let item = make_wanted("Breaking Bad", Some("5"), 3, None);
        assert!(wanted_episode_matches(&item, "3"));
        assert!(wanted_episode_matches(&item, "03"));
        assert!(wanted_episode_matches(&item, "e3"));
        assert!(!wanted_episode_matches(&item, "4"));
    }

    #[test]
    fn test_wanted_search_matches_season_number() {
        let item = make_wanted("Breaking Bad", Some("5"), 3, None);
        assert!(wanted_episode_matches(&item, "5"));
        assert!(wanted_episode_matches(&item, "s5"));
        assert!(!wanted_episode_matches(&item, "6"));
    }
}
