//! Log retrieval — reads log files from disk and serves them with pagination.
//!
//! File-based rather than DB-backed: the tracing framework writes directly to
//! rolling files, so reading them keeps the log system independent of the app DB.

use crate::api::AppState;
use axum::{
    Json,
    extract::{Query, State},
};
use jumbie_shared::types::{LogEntry, PaginatedResponse, PaginationQuery, level_priority};
use std::sync::Arc;
use tracing::debug;

// SSoT: the on-disk log line format lives in `crate::logging` (shared with the
// buffer writer and startup pre-population); re-exported here so the
// `api_routes::system::{parse_log_line, strip_ansi}` paths keep working.
pub use crate::logging::{parse_log_line, strip_ansi};

fn sort_log_entries(entries: &mut [LogEntry], sort_col: &str, sort_asc: bool) {
    match sort_col {
        "level" => {
            entries.sort_by_key(|a| level_priority(&a.level));
        }
        "message" => {
            entries.sort_by(|a, b| a.message.cmp(&b.message));
        }
        _ => {
            entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        }
    }
    if !sort_asc {
        entries.reverse();
    }
}

pub async fn get_logs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PaginationQuery>,
) -> Json<PaginatedResponse<LogEntry>> {
    let page = query.page.unwrap_or(0).max(0);
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);

    // The client omits sort/filter on its first request (it may not yet know the
    // stored values), so stored preferences are applied here and only overridden by
    // explicit request values.
    let ui = state.db.get_ui_preferences().await.unwrap_or_default();
    let min_level = super::resolve_log_min_level(&ui, query.min_level.as_deref());
    let min_priority = level_priority(&min_level);
    let (sort_col, sort_asc) = super::resolve_table_sort(
        &ui,
        "system_logs",
        query.sort.as_deref(),
        query.order.as_deref(),
        "timestamp",
        false,
    );

    // Read from the in-memory buffer (seeded at startup from existing log files,
    // then appended by the tracing layer) — no disk I/O during normal operation.
    let mut entries: Vec<LogEntry> = match state.log_buffer.read() {
        Ok(buf) => buf
            .iter()
            .filter(|e| level_priority(&e.level) >= min_priority)
            .cloned()
            .collect(),
        Err(_) => Vec::new(),
    };
    debug!(num_entries = entries.len(), "Read log entries from buffer");

    // Case-insensitive substring match. No stored default — an empty or absent
    // `search` means "no text filter".
    if let Some(needle) = super::text_needle(query.search.as_deref()) {
        entries.retain(|e| e.message.to_lowercase().contains(&needle));
    }

    sort_log_entries(&mut entries, &sort_col, sort_asc);

    let total = entries.len() as i64;

    let paged_entries: Vec<LogEntry> = entries
        .into_iter()
        .skip((page * limit) as usize)
        .take(limit as usize)
        .collect();

    Json(PaginatedResponse {
        items: paged_entries,
        total,
        page,
        page_size: limit,
    })
}

/// Returns the backend's configured log level.
/// The frontend uses this to limit the level selector to only show levels
/// at or above the backend's actual logging threshold.
pub async fn get_log_level(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    // The stored log_level is a full EnvFilter string like
    // "h2::codec=off,hyper_util::client::legacy::pool=off,info".
    // Extract the base level (last segment after the final comma).
    let level = state
        .log_level
        .rsplit(',')
        .next()
        .unwrap_or(&state.log_level)
        .trim()
        .to_uppercase();
    Json(serde_json::json!({
        "level": level
    }))
}

// Raw log file endpoint — serves the on-disk files directly (merged,
// chronological), bypassing the in-memory buffer. This returns the full ON-DISK
// history (8 MiB budget), useful when debugging an issue older than the buffer.
//
// Only files in the size-rolled scheme (`jumbie.log`, `jumbie.log.N`) are read —
// arbitrary files in the logs directory are never included and callers cannot
// name a file.

#[derive(serde::Deserialize)]
pub struct LogsFileQuery {
    /// Return the last N lines (after the level filter).
    /// Mutually exclusive with `head`. Defaults to 1000 when neither is given.
    pub tail: Option<usize>,
    /// Return the first N lines (after the level filter).
    /// Mutually exclusive with `tail`.
    pub head: Option<usize>,
    /// Minimum level to include, e.g. "debug". Defaults to "info".
    pub level: Option<String>,
}

/// Max lines a single request may return (bounds response size).
const MAX_LOG_FILE_LINES: usize = 100_000;

/// Per-file read cap for tail mode: twice the rotation size. Legitimate files
/// never exceed `ROTATE_BYTES` by more than one write; this also bounds the
/// read even if a giant file was dropped into the logs directory.
const TAIL_READ_CAP: u64 = crate::logging::ROTATE_BYTES * 2;

/// `GET /api/system/logs/file` — merged raw log files with `head`/`tail`/`level`.
pub async fn get_logs_file(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LogsFileQuery>,
) -> Result<axum::response::Response, crate::error::AppError> {
    use axum::http::{StatusCode, header};

    if query.tail.is_some() && query.head.is_some() {
        return Err(crate::error::AppError::BadRequest(
            "Specify either `tail` or `head`, not both".to_string(),
        ));
    }
    let head_mode = query.head.is_some();
    let count = query
        .tail
        .or(query.head)
        .unwrap_or(1000)
        .clamp(1, MAX_LOG_FILE_LINES);

    let level = query.level.as_deref().unwrap_or("INFO").to_uppercase();
    let min_priority = level_priority(&level);
    if min_priority == 0 {
        return Err(crate::error::AppError::BadRequest(format!(
            "Invalid level: {level}"
        )));
    }

    // Current file + numeric archives, oldest → newest (never arbitrary files).
    // SSoT: `crate::logging::log_files` owns the naming scheme.
    let logs_dir = std::path::Path::new(&state.logs_dir);
    let files = crate::logging::log_files(logs_dir);

    let lines: Vec<String> = if head_mode {
        // First N matching lines, oldest file first; stop early.
        let mut out = Vec::new();
        for path in &files {
            if out.len() >= count {
                break;
            }
            out.extend(read_head_lines(path, min_priority, count - out.len()).await);
        }
        out
    } else {
        tail_log_files(&files, min_priority, count).await
    };

    let body = lines.join("\n");
    let body = if body.is_empty() {
        body
    } else {
        format!("{body}\n")
    };
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(axum::body::Body::from(body))
        .expect("static response builder"))
}

/// Whether a raw log line passes a minimum-level filter.
fn line_passes_level(line: &str, min_priority: i32) -> bool {
    let Some(level) = line.split_ascii_whitespace().nth(1) else {
        return false;
    };
    level_priority(level) >= min_priority
}

/// Stream the first `max_lines` matching lines of a file (oldest → newest),
/// stopping as soon as the quota is met. Memory stays bounded even if the
/// file is huge and few lines match.
async fn read_head_lines(
    path: &std::path::Path,
    min_priority: i32,
    max_lines: usize,
) -> Vec<String> {
    use tokio::io::AsyncBufReadExt;

    let Ok(file) = tokio::fs::File::open(path).await else {
        return Vec::new();
    };
    let mut lines = tokio::io::BufReader::new(file).lines();
    let mut out = Vec::new();
    while out.len() < max_lines {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line_passes_level(&line, min_priority) {
                    out.push(line);
                }
            }
            _ => break,
        }
    }
    out
}

/// Collect the last `count` matching lines across `files` (oldest → newest in
/// the slice), returning them in chronological order.
///
/// Scanning starts at the END of the newest file and walks backward, moving to
/// the next-older file only when the quota is not yet met — it never combines
/// everything first. Each file's read is capped at [`TAIL_READ_CAP`] bytes, so
/// even a pathologically large file cannot balloon memory.
async fn tail_log_files(
    files: &[std::path::PathBuf],
    min_priority: i32,
    count: usize,
) -> Vec<String> {
    // Collected newest → oldest as we walk backward; reversed at the end.
    let mut newest_first: Vec<String> = Vec::new();
    for path in files.iter().rev() {
        if newest_first.len() >= count {
            break;
        }
        newest_first
            .extend(read_tail_backward(path, min_priority, count - newest_first.len()).await);
    }
    newest_first.truncate(count);
    newest_first.reverse();
    newest_first
}

/// Return the last `max_lines` matching lines of a file, **newest-first**, by
/// scanning backward from the end and stopping as soon as the quota is met.
/// The read is capped at [`TAIL_READ_CAP`] bytes so a pathologically large
/// file cannot balloon memory.
async fn read_tail_backward(
    path: &std::path::Path,
    min_priority: i32,
    max_lines: usize,
) -> Vec<String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return Vec::new();
    };
    let len = metadata.len();
    let start = len.saturating_sub(TAIL_READ_CAP);

    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return Vec::new();
    };
    if start > 0 && file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return Vec::new();
    }
    let mut reader = tokio::io::BufReader::new(file);
    let mut buf = Vec::new();
    if reader.read_to_end(&mut buf).await.is_err() {
        return Vec::new();
    }

    let text = std::str::from_utf8(&buf).unwrap_or("");
    let lines = if start > 0 {
        // Drop the partial line at the seek boundary.
        let first_nl = text.find('\n').map(|p| p + 1).unwrap_or(0);
        text[first_nl..].lines()
    } else {
        text.lines()
    };

    // Walk newest → oldest, collecting matching lines until the quota is met.
    let mut out = Vec::new();
    for line in lines.rev() {
        if line_passes_level(line, min_priority) {
            out.push(line.to_string());
            if out.len() >= max_lines {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(timestamp: &str, level: &str, message: &str) -> LogEntry {
        LogEntry {
            timestamp: timestamp.to_string(),
            level: level.to_string(),
            message: message.to_string(),
        }
    }

    #[tokio::test]
    async fn test_read_head_lines_filters_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jumbie.log");
        std::fs::write(
            &path,
            "2026-01-01T00:00:00Z  TRACE jumbie::a: t1\n\
             2026-01-01T00:00:01Z  INFO jumbie::a: i1\n\
             2026-01-01T00:00:02Z  DEBUG jumbie::a: d1\n\
             2026-01-01T00:00:03Z  WARN jumbie::a: w1\n\
             2026-01-01T00:00:04Z  INFO jumbie::a: i2\n",
        )
        .unwrap();

        // level=info → TRACE/DEBUG excluded; head=2 → first two matching.
        let lines = read_head_lines(&path, level_priority("INFO"), 2).await;
        assert_eq!(
            lines,
            vec![
                "2026-01-01T00:00:01Z  INFO jumbie::a: i1",
                "2026-01-01T00:00:03Z  WARN jumbie::a: w1"
            ]
        );
    }

    #[tokio::test]
    async fn test_read_tail_backward_returns_last_matching_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jumbie.log");
        std::fs::write(
            &path,
            "2026-01-01T00:00:00Z  TRACE jumbie::a: t1\n\
             2026-01-01T00:00:01Z  INFO jumbie::a: i1\n\
             2026-01-01T00:00:02Z  DEBUG jumbie::a: d1\n\
             2026-01-01T00:00:03Z  WARN jumbie::a: w1\n\
             2026-01-01T00:00:04Z  INFO jumbie::a: i2\n",
        )
        .unwrap();

        // level=info → TRACE/DEBUG excluded; newest-first, capped at 2.
        let lines = read_tail_backward(&path, level_priority("INFO"), 2).await;
        assert_eq!(
            lines,
            vec![
                "2026-01-01T00:00:04Z  INFO jumbie::a: i2",
                "2026-01-01T00:00:03Z  WARN jumbie::a: w1"
            ]
        );
    }

    #[tokio::test]
    async fn test_read_tail_backward_caps_huge_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jumbie.log");
        // A giant file (well past TAIL_READ_CAP) must not be read in full: the
        // result contains only lines from the capped tail window.
        let mut content = String::new();
        for i in 0..200_000 {
            content.push_str(&format!("2026-01-01T00:00:00Z  INFO jumbie::a: line {i}\n"));
        }
        std::fs::write(&path, &content).unwrap();

        let lines = read_tail_backward(&path, level_priority("INFO"), 1000).await;
        assert!(content.len() as u64 > TAIL_READ_CAP);
        assert_eq!(lines.len(), 1000, "stops as soon as the quota is met");
    }

    #[tokio::test]
    async fn test_tail_log_files_spans_files_and_stops_early() {
        let dir = tempfile::tempdir().unwrap();
        // Oldest archive .2 → .1 → current.
        std::fs::write(
            dir.path().join("jumbie.log.2"),
            "2026-01-01T00:00:00Z  INFO jumbie::a: old1\n\
             2026-01-01T00:00:01Z  INFO jumbie::a: old2\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("jumbie.log.1"),
            "2026-01-01T00:00:02Z  INFO jumbie::a: mid1\n\
             2026-01-01T00:00:03Z  INFO jumbie::a: mid2\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("jumbie.log"),
            "2026-01-01T00:00:04Z  INFO jumbie::a: new1\n\
             2026-01-01T00:00:05Z  INFO jumbie::a: new2\n\
             2026-01-01T00:00:06Z  INFO jumbie::a: new3\n",
        )
        .unwrap();

        // files slice is oldest → newest, as the handler builds it.
        let files: Vec<std::path::PathBuf> = vec![
            dir.path().join("jumbie.log.2"),
            dir.path().join("jumbie.log.1"),
            dir.path().join("jumbie.log"),
        ];

        // count=2 → only the current file is consulted; .1/.2 never read.
        let lines = tail_log_files(&files, level_priority("INFO"), 2).await;
        assert_eq!(
            lines,
            vec![
                "2026-01-01T00:00:05Z  INFO jumbie::a: new2",
                "2026-01-01T00:00:06Z  INFO jumbie::a: new3"
            ]
        );

        // count=5 → spans current + .1 (chronological order).
        let lines = tail_log_files(&files, level_priority("INFO"), 5).await;
        assert_eq!(lines.len(), 5);
        assert_eq!(
            lines.first().map(|s| s.as_str()),
            Some("2026-01-01T00:00:02Z  INFO jumbie::a: mid1")
        );
        assert_eq!(
            lines.last().map(|s| s.as_str()),
            Some("2026-01-01T00:00:06Z  INFO jumbie::a: new3")
        );
    }

    #[test]
    fn test_line_passes_level() {
        let line = "2026-01-01T00:00:00Z  INFO jumbie::a: message";
        assert!(line_passes_level(line, level_priority("INFO")));
        assert!(line_passes_level(line, level_priority("TRACE")));
        assert!(!line_passes_level(line, level_priority("WARN")));
        assert!(!line_passes_level("not a log line", level_priority("INFO")));
    }

    #[test]
    fn test_sort_logs_by_timestamp_desc_default() {
        let mut entries = vec![
            make_entry("2024-03-03", "INFO", "third"),
            make_entry("2024-03-01", "ERROR", "first"),
            make_entry("2024-03-02", "WARN", "second"),
        ];
        sort_log_entries(&mut entries, "timestamp", false);
        assert_eq!(entries[0].timestamp, "2024-03-03");
        assert_eq!(entries[1].timestamp, "2024-03-02");
        assert_eq!(entries[2].timestamp, "2024-03-01");
    }

    #[test]
    fn test_sort_logs_by_timestamp_asc() {
        let mut entries = vec![
            make_entry("2024-03-03", "INFO", "third"),
            make_entry("2024-03-01", "ERROR", "first"),
            make_entry("2024-03-02", "WARN", "second"),
        ];
        sort_log_entries(&mut entries, "timestamp", true);
        assert_eq!(entries[0].timestamp, "2024-03-01");
        assert_eq!(entries[1].timestamp, "2024-03-02");
        assert_eq!(entries[2].timestamp, "2024-03-03");
    }

    #[test]
    fn test_sort_logs_by_level_asc() {
        let mut entries = vec![
            make_entry("a", "INFO", "x"),
            make_entry("b", "ERROR", "y"),
            make_entry("c", "WARN", "z"),
        ];
        sort_log_entries(&mut entries, "level", true);
        assert_eq!(entries[0].level, "INFO");
        assert_eq!(entries[1].level, "WARN");
        assert_eq!(entries[2].level, "ERROR");
    }

    #[test]
    fn test_sort_logs_by_level_desc() {
        let mut entries = vec![
            make_entry("a", "INFO", "x"),
            make_entry("b", "ERROR", "y"),
            make_entry("c", "WARN", "z"),
        ];
        sort_log_entries(&mut entries, "level", false);
        assert_eq!(entries[0].level, "ERROR");
        assert_eq!(entries[1].level, "WARN");
        assert_eq!(entries[2].level, "INFO");
    }

    #[test]
    fn test_sort_logs_by_message() {
        let mut entries = vec![
            make_entry("a", "INFO", "zeta"),
            make_entry("b", "ERROR", "alpha"),
            make_entry("c", "WARN", "beta"),
        ];
        sort_log_entries(&mut entries, "message", true);
        assert_eq!(entries[0].message, "alpha");
        assert_eq!(entries[1].message, "beta");
        assert_eq!(entries[2].message, "zeta");
    }
}
