use serde::{Deserialize, Serialize};

/// All possible activity event types, shared between the backend logger and frontend display.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ActivityType {
    Download,
    Import,
    Metadata,
    Reassign,
    Assign,
    Analyze,
    Delete,
    Unassign,
}

impl ActivityType {
    /// Lowercase string representation (e.g. `ActivityType::Download` → `"download"`).
    /// SSoT for DB storage and log serialisation.
    pub fn as_str(&self) -> &'static str {
        match self {
            ActivityType::Download => "download",
            ActivityType::Import => "import",
            ActivityType::Metadata => "metadata",
            ActivityType::Reassign => "reassign",
            ActivityType::Assign => "assign",
            ActivityType::Analyze => "analyze",
            ActivityType::Delete => "delete",
            ActivityType::Unassign => "unassign",
        }
    }

    /// Returns `(badge_css_class, label)` for frontend display.
    /// This is the single source of truth for how each activity type is rendered.
    pub fn display_info(&self) -> (&'static str, &'static str) {
        match self {
            ActivityType::Download => ("badge badge-blue", "Download"),
            ActivityType::Import => ("badge badge-teal", "Import"),
            ActivityType::Metadata => ("badge badge-purple", "Metadata"),
            ActivityType::Reassign => ("badge badge-yellow", "Reassign"),
            ActivityType::Assign => ("badge badge-green", "Assign"),
            ActivityType::Analyze => ("badge badge-pink", "Analyze"),
            ActivityType::Delete => ("badge badge-danger", "Delete"),
            ActivityType::Unassign => ("badge badge-orange", "Unassign"),
        }
    }
}

/// A single recorded activity event.
/// Timestamp is stored as an ISO 8601 string for frontend compatibility.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "backend", derive(sqlx::FromRow))]
pub struct ActivityItem {
    pub id: String,
    pub timestamp: String,
    pub activity_type: ActivityType,
    pub title: String,
    pub details: Option<String>,
    pub status: String,
}
