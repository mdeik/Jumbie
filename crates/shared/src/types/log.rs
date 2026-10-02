//! Shared log types — SSoT for `LogEntry`, log-level priority ordering, and the
//! canonical set of valid log levels.
//!
//! The backend serialises `LogEntry` (file-parsed logs → JSON API) and the frontend
//! deserialises it, so defining it once prevents drift. `level_priority` and
//! `LOG_LEVELS` are shared so the backend's server-side filtering and the frontend's
//! min-level dropdown cannot drift.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

/// A single log entry from the backend's rolling log files.
///
/// Serialize + Deserialize are derived so the backend can emit parsed entries as
/// JSON and the frontend can deserialise them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
}

/// Canonical ordered list of log levels from lowest to highest severity.
///
/// SSoT: every place iterating, filtering, or displaying log levels uses this.
/// Adding a new level is a one-line change here — the backend filter, frontend
/// dropdown, and priority function all derive from it.
pub const LOG_LEVELS: &[&str] = &["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];

/// Returns a numeric priority for a log level (higher = more severe).
///
/// Used by the backend to skip entries below the min-level filter and by the
/// frontend to decide which levels to offer. Unknown levels get priority 0 so they
/// are always excluded when a min-level filter is active.
pub fn level_priority(level: &str) -> i32 {
    match level {
        "ERROR" => 5,
        "WARN" => 4,
        "INFO" => 3,
        "DEBUG" => 2,
        "TRACE" => 1,
        _ => 0,
    }
}

impl LogEntry {
    /// Estimated in-memory footprint of this entry (heap strings + allocation
    /// overhead). Used by the byte-bounded buffer so memory stays fixed even
    /// when individual log lines are pathologically large.
    pub fn estimated_bytes(&self) -> usize {
        self.timestamp.len() + self.level.len() + self.message.len() + 128
    }

    /// Validate this log entry before inserting it into the cache.
    ///
    /// Returns `true` when the entry has a non-empty timestamp, a recognised log
    /// level, and a non-empty message. Filtering here (rather than letting
    /// `level_priority == 0` reject downstream) keeps invalid entries from wasting
    /// buffer capacity and polluting the API response.
    pub fn validate(&self) -> bool {
        if self.timestamp.is_empty() {
            return false;
        }
        if level_priority(&self.level) == 0 {
            return false;
        }
        if self.message.is_empty() {
            return false;
        }
        true
    }
}

/// Default byte budget for the in-memory log ring buffer.
///
/// The buffer holds the newest ~4 MiB of parsed log entries, evicting the
/// oldest as new entries arrive. Bounding by **bytes** (not entry count) keeps
/// memory fixed even if a single log line is megabytes long or the user drops
/// a giant file into the logs directory.
pub const LOG_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Byte-bounded ring of parsed log entries shared between the tracing layer
/// (writes on every event) and the logs API handler (reads on every request).
///
/// Pre-populated at startup from the size-rolled log files (newest-first, up to the
/// byte budget); at runtime a custom `MakeWriter` parses each formatted line and
/// pushes it (zero disk reads); `get_logs` then reads and filters in memory. `push`
/// evicts from the front while the estimated footprint exceeds [`LOG_BUFFER_BYTES`].
#[derive(Debug)]
pub struct LogRing {
    entries: VecDeque<LogEntry>,
    bytes: usize,
    budget: usize,
}

impl LogRing {
    /// Create a ring with the default byte budget ([`LOG_BUFFER_BYTES`]).
    pub fn new() -> Self {
        Self::with_budget(LOG_BUFFER_BYTES)
    }

    /// Create a ring with a custom byte budget (used by tests).
    pub fn with_budget(budget: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            budget,
        }
    }

    /// Append an entry, evicting the oldest entries while over budget.
    pub fn push(&mut self, entry: LogEntry) {
        self.bytes += entry.estimated_bytes();
        self.entries.push_back(entry);
        while self.bytes > self.budget && !self.entries.is_empty() {
            let dropped = self.entries.pop_front().expect("non-empty");
            self.bytes = self.bytes.saturating_sub(dropped.estimated_bytes());
        }
    }

    /// Iterate entries oldest → newest.
    pub fn iter(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    /// Number of entries currently held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ring is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Current estimated byte footprint.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Default for LogRing {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe handle to the in-memory log ring, shared between the tracing
/// layer (writes on every event) and the logs API handler (reads on request).
pub type LogBuffer = Arc<RwLock<LogRing>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_levels_are_ordered_by_severity() {
        // Each known level must have a strictly increasing priority
        let mut prev = 0;
        for level in LOG_LEVELS {
            let p = level_priority(level);
            assert!(
                p > prev,
                "{} should be higher priority than previous",
                level
            );
            prev = p;
        }
        assert_eq!(prev, 5, "ERROR should be the highest priority (5)");
    }

    #[test]
    fn test_unknown_level_is_zero() {
        assert_eq!(level_priority("CRITICAL"), 0);
        assert_eq!(level_priority(""), 0);
        assert_eq!(level_priority("DEBUG "), 0); // leading/trailing space not allowed
    }

    #[test]
    fn test_validate_valid_entry() {
        let entry = LogEntry {
            timestamp: "2026-06-19T08:10:27.123Z".to_string(),
            level: "INFO".to_string(),
            message: "jumbie: starting up".to_string(),
        };
        assert!(entry.validate());
    }

    #[test]
    fn test_validate_empty_timestamp_fails() {
        let entry = LogEntry {
            timestamp: "".to_string(),
            level: "INFO".to_string(),
            message: "something happened".to_string(),
        };
        assert!(!entry.validate());
    }

    #[test]
    fn test_validate_unknown_level_fails() {
        let entry = LogEntry {
            timestamp: "2026-06-19T08:10:27.123Z".to_string(),
            level: "CRITICAL".to_string(),
            message: "something happened".to_string(),
        };
        assert!(!entry.validate());
    }

    #[test]
    fn test_validate_empty_message_fails() {
        let entry = LogEntry {
            timestamp: "2026-06-19T08:10:27.123Z".to_string(),
            level: "WARN".to_string(),
            message: "".to_string(),
        };
        assert!(!entry.validate());
    }

    #[test]
    fn test_validate_all_known_levels() {
        for level in LOG_LEVELS {
            let entry = LogEntry {
                timestamp: "2026-06-19T08:10:27.123Z".to_string(),
                level: level.to_string(),
                message: "test message".to_string(),
            };
            assert!(entry.validate(), "known level '{}' should pass", level);
        }
    }

    #[test]
    fn test_log_entry_round_trip() {
        let entry = LogEntry {
            timestamp: "2026-06-19T08:10:27.123Z".to_string(),
            level: "INFO".to_string(),
            message: "jumbie: starting up".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.timestamp, entry.timestamp);
        assert_eq!(deserialized.level, entry.level);
        assert_eq!(deserialized.message, entry.message);
    }

    #[test]
    fn test_log_ring_bounded_by_bytes_not_entries() {
        // A tiny budget with a single giant entry must still stay under budget
        // (the giant entry itself is dropped rather than retained).
        let mut ring = LogRing::with_budget(200);
        ring.push(LogEntry {
            timestamp: "t".to_string(),
            level: "INFO".to_string(),
            message: "x".repeat(10_000),
        });
        assert_eq!(ring.len(), 0, "oversized entry is dropped");
        assert!(ring.bytes() <= 200);
    }

    #[test]
    fn test_log_ring_evicts_oldest_first() {
        let mut ring = LogRing::with_budget(500);
        for i in 0..100 {
            ring.push(LogEntry {
                timestamp: format!("t{i}"),
                level: "INFO".to_string(),
                message: "m".to_string(),
            });
        }
        assert!(ring.bytes() <= 500);
        assert!(ring.len() < 100, "oldest entries evicted");
        // Remaining entries are the newest, in order.
        let timestamps: Vec<&str> = ring.iter().map(|e| e.timestamp.as_str()).collect();
        assert!(
            timestamps.windows(2).all(|w| w[0] < w[1]),
            "entries remain in chronological order"
        );
        assert_eq!(
            timestamps.last().copied(),
            Some("t99"),
            "newest entry retained"
        );
    }

    #[test]
    fn test_log_ring_empty_and_iter() {
        let ring = LogRing::new();
        assert!(ring.is_empty());
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.iter().count(), 0);
    }
}
