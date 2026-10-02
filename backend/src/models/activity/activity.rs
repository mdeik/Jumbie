// The backend maps activity timestamps to `DateTime<Utc>` for chrono sorting; the
// shared crate's `ActivityItem` uses a plain `String` because the frontend renders the
// ISO string directly and must not pull chrono into its dependency tree.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

// Re-export the canonical shared enum for `crate::models::activity::ActivityType`.
pub use jumbie_shared::types::ActivityType;

/// Backend-specific activity item with a typed `DateTime<Utc>` timestamp.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ActivityItem {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub activity_type: ActivityType,
    pub title: String,
    pub details: Option<String>,
    pub status: String,
}
