// SSoT for the activity feed: every activity-relevant event writes a row to
// `activity_log` via `record_activity()`, and the feed API reads only that table.
// `ActivityEvent` is the input struct; `ActivityLogRow` is the DB row.
use chrono::{DateTime, Utc};
use jumbie_shared::types::ActivityType;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// The structured event that callers pass to `record_activity()`.
///
/// Callers set the semantically primary field for their event type:
///   • Download  → `series_title` (title of the show)
///   • Analyze   → `details` (file path or path → ep_id)
///   • Assign    → `details` (file path)
///   • Reassign  → `details` (file path)
///   • Delete    → `details` (filename)
///   • Unassign  → `details` (filename)
///
/// `record_activity` auto-fills the complementary field (`series_title` from
/// `details`, or `details` from `season`/`episode`/`episode_end`), so every
/// row in `activity_log` has both a title and details.
#[derive(Debug, Clone)]
pub struct ActivityEvent {
    pub event_type: ActivityType,
    pub series_title: String,
    pub season: Option<String>,
    pub episode: Option<i32>,
    pub episode_end: Option<i32>,
    pub title: Option<String>,
    pub details: Option<String>,
    pub status: String,
}

/// A row read back from the `activity_log` table.
///
/// `created_at` is `DateTime<Utc>` for sorting/comparison. The API route converts
/// to the shared `ActivityItem` (ISO string) before returning JSON.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ActivityLogRow {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub event_type: String,
    pub series_title: String,
    pub season: Option<String>,
    pub episode: Option<i32>,
    pub episode_end: Option<i32>,
    pub title: Option<String>,
    pub details: Option<String>,
    pub status: String,
}
