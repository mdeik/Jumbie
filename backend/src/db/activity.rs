// Activity log: single source of truth for the activity feed. Every user-facing
// event writes one row to `activity_log` via `record_activity()`; the feed reads
// from this table exclusively.
//
// Callers set the semantically primary field for their event type (series_title
// for download, details for file-path events). `record_activity` fills in the
// complementary field so every row has both.
//
// Derivation rules (change here to tweak how title/details are derived):
//   series_title empty + details = file path  → extract filename as series_title
//   details empty + episode present           → build episode range from season/episode/episode_end
//
// Separate from `file_event_log` (a crash-recovery write-ahead log) so the two
// concerns and their schemas stay distinct.

use super::DbManager;
use crate::models::activity::ActivityEvent;
use anyhow::Result;
use jumbie_shared::formatting::{LabelStyle, fmt_episode, fmt_season_episode};

/// Size budget for `activity_log` (~100 KiB of estimated row bytes). Retention
/// is size-based — oldest rows are pruned while the estimate exceeds the budget,
/// never by age. ~100 KiB ≈ a few hundred typical events, not an archive.
pub const ACTIVITY_LOG_BUDGET_BYTES: u64 = 100 * 1024;

/// Per-row byte overhead folded into [`activity_log_bytes`]: the fixed columns
/// (event_type, series_title, status), the numeric episode columns, SQLite's
/// record/btree overhead, and a share of the index entry.
const ACTIVITY_ROW_OVERHEAD: i64 = 96;

/// Rows deleted per iteration while pruning an over-budget activity table.
const PRUNE_BATCH: i64 = 500;

impl DbManager {
    /// Record a single activity event — the only writer to `activity_log`. The
    /// `CURRENT_TIMESTAMP` default sets the time at insert, so it reflects when
    /// the event happened. Auto-fills complementary title/details; see module
    /// docs for the derivation rules.
    pub async fn record_activity(&self, mut event: ActivityEvent) -> Result<()> {
        // Derive series_title from details, splitting off any " -> ep_id" suffix.
        if event.series_title.is_empty()
            && let Some(ref details) = event.details
        {
            let path_part = details.split(" -> ").next().unwrap_or(details);
            if let Some(name) = std::path::Path::new(path_part).file_name() {
                event.series_title = name.to_string_lossy().to_string();
            }
        }

        if event.details.is_none()
            && let Some(ep) = event.episode
        {
            event.details = Some(match &event.season {
                Some(s) if !s.is_empty() => {
                    let s_int = s.parse::<i32>().unwrap_or(1);
                    fmt_season_episode(s_int, ep, event.episode_end, LabelStyle::Short)
                }
                _ => {
                    if let Some(ep_end) = event.episode_end.filter(|e| *e > ep) {
                        format!(
                            "{}-{}",
                            fmt_episode(ep, LabelStyle::Short),
                            fmt_episode(ep_end, LabelStyle::Short)
                        )
                    } else {
                        fmt_episode(ep, LabelStyle::Short)
                    }
                }
            });
        }

        sqlx::query(
            "INSERT INTO activity_log (event_type, series_title, season, episode, episode_end, title, details, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event.event_type.as_str())
        .bind(&event.series_title)
        .bind(&event.season)
        .bind(event.episode)
        .bind(event.episode_end)
        .bind(&event.title)
        .bind(&event.details)
        .bind(&event.status)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Estimated footprint of `activity_log` in bytes: sum of variable-length
    /// text columns plus a per-row overhead. Bounded by bytes, not rows, so one
    /// pathologically large `details` string still triggers pruning.
    pub async fn activity_log_bytes(&self) -> Result<u64> {
        let bytes: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(
                 length(event_type) + length(series_title)
                 + length(COALESCE(season, ''))
                 + length(COALESCE(title, '')) + length(COALESCE(details, ''))
                 + ?
             ), 0)
             FROM activity_log",
        )
        .bind(ACTIVITY_ROW_OVERHEAD)
        .fetch_one(&self.pool)
        .await?;
        Ok(bytes.max(0) as u64)
    }

    /// Delete oldest rows until the estimated size is at or below `budget_bytes`,
    /// returning the count deleted. Each pass deletes only the byte excess
    /// (clamped to [`PRUNE_BATCH`]), so a small overrun prunes a few rows, not the
    /// whole table; an oversized row is dropped whole. Oldest-first by `id`, which
    /// is monotonic with insert order (`INTEGER PRIMARY KEY AUTOINCREMENT`).
    pub async fn prune_activity_log_to_budget(&self, budget_bytes: u64) -> Result<u64> {
        let mut deleted: u64 = 0;
        loop {
            let size = self.activity_log_bytes().await?;
            if size <= budget_bytes {
                break;
            }
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM activity_log")
                .fetch_one(&self.pool)
                .await?;
            if count == 0 {
                break; // Nothing left to delete — avoid an infinite loop.
            }
            let avg_row_bytes = (size / count as u64).max(1);
            let excess = size - budget_bytes;
            let target = (excess / avg_row_bytes).clamp(1, PRUNE_BATCH as u64) as i64;
            let result = sqlx::query(
                "DELETE FROM activity_log
                 WHERE id IN (SELECT id FROM activity_log ORDER BY id ASC LIMIT ?)",
            )
            .bind(target)
            .execute(&self.pool)
            .await?;
            let n = result.rows_affected();
            if n == 0 {
                break;
            }
            deleted += n as u64;
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::activity::ActivityEvent;
    use jumbie_shared::types::ActivityType;

    async fn setup() -> (DbManager, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();
        (db, tmp)
    }

    async fn insert(db: &DbManager, series: &str, details: &str) {
        db.record_activity(ActivityEvent {
            event_type: ActivityType::Download,
            series_title: series.to_string(),
            season: None,
            episode: Some(1),
            episode_end: None,
            title: None,
            details: Some(details.to_string()),
            status: "Success".to_string(),
        })
        .await
        .unwrap();
    }

    async fn row_ids(db: &DbManager) -> Vec<i64> {
        sqlx::query_scalar("SELECT id FROM activity_log ORDER BY id ASC")
            .fetch_all(&db.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn activity_log_bytes_tracks_row_text() {
        let (db, _tmp) = setup().await;
        assert_eq!(db.activity_log_bytes().await.unwrap(), 0);

        insert(&db, "Show", "details").await;
        let one = db.activity_log_bytes().await.unwrap();
        assert!(one > 0);

        insert(&db, "Show", "details").await;
        let two = db.activity_log_bytes().await.unwrap();
        // Two byte-identical rows → exactly double the footprint.
        assert_eq!(two, one * 2);
    }

    #[tokio::test]
    async fn prune_activity_log_to_budget_evicts_oldest_rows() {
        let (db, _tmp) = setup().await;
        // 30 rows × ~350 B ≈ 10 KB — comfortably over a 2 KB budget.
        for i in 0..30 {
            insert(&db, &format!("Show {i}"), &"x".repeat(250)).await;
        }
        let before = db.activity_log_bytes().await.unwrap();
        let ids_before = row_ids(&db).await;
        assert!(before > 2000);
        assert_eq!(ids_before.len(), 30);

        let deleted = db.prune_activity_log_to_budget(2000).await.unwrap();
        assert!(deleted > 0);

        let after = db.activity_log_bytes().await.unwrap();
        assert!(after <= 2000, "size {after} still exceeds budget 2000");
        let ids_after = row_ids(&db).await;
        assert!(!ids_after.is_empty());
        // Only the newest rows survive — ids_after is a suffix of ids_before.
        assert_eq!(
            &ids_before[ids_before.len() - ids_after.len()..],
            &ids_after[..]
        );
    }

    #[tokio::test]
    async fn prune_activity_log_to_budget_drops_oversized_rows() {
        let (db, _tmp) = setup().await;
        // A single row larger than the whole budget must be dropped entirely.
        insert(&db, "Big", &"y".repeat(50_000)).await;
        let deleted = db.prune_activity_log_to_budget(1000).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(db.activity_log_bytes().await.unwrap(), 0);
        assert!(row_ids(&db).await.is_empty());
    }

    #[tokio::test]
    async fn prune_activity_log_to_budget_noop_when_under_budget() {
        let (db, _tmp) = setup().await;
        insert(&db, "Small", "tiny").await;
        let deleted = db.prune_activity_log_to_budget(100_000).await.unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(row_ids(&db).await.len(), 1);
    }
}
