//! Kept-for-review ("unmatched") files.
//!
//! Files deliberately left in the download directory for manual review — pack
//! overspill that was kept, or files that could not be assigned to an episode.
//! They have no `episodes` row, so this table is their durable record.
//!
//! # Why a dedicated table
//!
//! The file tables are a *scan cache*: `cleanup_missing_files` deletes rows whose
//! `episode_id` doesn't resolve, which would silently drop kept files from the
//! file browser. This table is the SSoT for the review list and carries the
//! series association needed to scope it per series.

use anyhow::Result;

use super::DbManager;

impl DbManager {
    /// Record (or refresh) a file kept for review. Re-recording clears any prior
    /// "missing" marker and preserves an existing `series_id` when `None` is
    /// passed.
    pub async fn upsert_unmatched_file(
        &self,
        file_path: &str,
        series_id: Option<&str>,
        reason: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO unmatched_files (file_path, series_id, reason, last_seen_at)
             VALUES (?, ?, ?, datetime('now'))
             ON CONFLICT(file_path) DO UPDATE SET
               series_id = COALESCE(excluded.series_id, series_id),
               reason = excluded.reason,
               last_seen_at = excluded.last_seen_at,
               missing_since = NULL",
        )
        .bind(file_path)
        .bind(series_id)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Paths kept for review that belong to `series_id`.
    pub async fn get_unmatched_files_for_series(&self, series_id: &str) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar(
            "SELECT file_path FROM unmatched_files WHERE series_id = ? ORDER BY created_at",
        )
        .bind(series_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Remove review records for the given paths (assigned or deleted).
    pub async fn delete_unmatched_files(&self, paths: &[String]) -> Result<()> {
        for path in paths {
            sqlx::query("DELETE FROM unmatched_files WHERE file_path = ?")
                .bind(path)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Remove review records whose file has been missing longer than `grace_days`
    /// (and whose file has since been assigned to an episode). Files that
    /// reappear have their missing marker cleared. Returns the number of rows
    /// deleted.
    pub async fn cleanup_unmatched_files(&self, grace_days: i64) -> Result<usize> {
        // Rows whose file was assigned to an episode after being kept for review
        // are no longer "for review". This sweep catches assignments that don't
        // pass through the manual assign handlers (orphan adoption, scans) and is
        // set-based, so it stays cheap regardless of table size.
        let assigned = sqlx::query(
            "DELETE FROM unmatched_files
             WHERE file_path IN (
                 SELECT fp.file_path FROM episode_files ef
                 JOIN file_paths fp ON fp.id = ef.file_path_id
             )",
        )
        .execute(&self.pool)
        .await?
        .rows_affected() as usize;

        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT file_path, missing_since FROM unmatched_files")
                .fetch_all(&self.pool)
                .await?;
        if rows.is_empty() {
            return Ok(assigned);
        }

        // Existence checks on the blocking pool (stat per row).
        let statuses: Vec<(String, Option<String>, bool)> =
            tokio::task::spawn_blocking(move || {
                rows.into_iter()
                    .map(|(path, missing_since)| {
                        let exists = std::path::Path::new(&path).exists();
                        (path, missing_since, exists)
                    })
                    .collect()
            })
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking for unmatched-file stat failed: {}", e))?;

        let modifier = format!("-{} days", grace_days);
        let mut deleted = assigned;
        for (path, missing_since, exists) in statuses {
            if exists {
                // Reappeared — clear the missing marker if one was set.
                if missing_since.is_some() {
                    sqlx::query(
                        "UPDATE unmatched_files
                         SET missing_since = NULL, last_seen_at = datetime('now')
                         WHERE file_path = ?",
                    )
                    .bind(&path)
                    .execute(&self.pool)
                    .await?;
                }
            } else if missing_since.is_none() {
                // Newly missing — start the grace window.
                sqlx::query(
                    "UPDATE unmatched_files SET missing_since = datetime('now')
                     WHERE file_path = ? AND missing_since IS NULL",
                )
                .bind(&path)
                .execute(&self.pool)
                .await?;
            } else {
                // Missing past the grace period — drop it.
                let res = sqlx::query(
                    "DELETE FROM unmatched_files
                     WHERE file_path = ? AND missing_since < datetime('now', ?)",
                )
                .bind(&path)
                .bind(&modifier)
                .execute(&self.pool)
                .await?;
                deleted += res.rows_affected() as usize;
            }
        }

        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> (DbManager, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();
        (db, tmp)
    }

    /// `series_id` is a real foreign key (ON DELETE CASCADE), so tests must seed
    /// the parent row. A minimal JSON body is enough — the materialized-column
    /// triggers COALESCE missing keys to their defaults.
    async fn seed_series(db: &DbManager, id: &str) {
        sqlx::query("INSERT INTO series_mappings (id, data) VALUES (?, '{}')")
            .bind(id)
            .execute(db.get_pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn upsert_and_get_are_scoped_by_series() {
        let (db, tmp) = test_db().await;
        seed_series(&db, "series-a").await;
        seed_series(&db, "series-b").await;
        let a = tmp.path().join("a.mkv").to_string_lossy().to_string();
        let b = tmp.path().join("b.mkv").to_string_lossy().to_string();

        db.upsert_unmatched_file(&a, Some("series-a"), "unneeded")
            .await
            .unwrap();
        db.upsert_unmatched_file(&b, Some("series-b"), "unmatched")
            .await
            .unwrap();

        assert_eq!(
            db.get_unmatched_files_for_series("series-a").await.unwrap(),
            vec![a.clone()]
        );
        assert_eq!(
            db.get_unmatched_files_for_series("series-b").await.unwrap(),
            vec![b]
        );
        assert!(
            db.get_unmatched_files_for_series("series-c")
                .await
                .unwrap()
                .is_empty()
        );

        // Re-recording preserves the series association when None is passed.
        db.upsert_unmatched_file(&a, None, "unmatched")
            .await
            .unwrap();
        assert_eq!(
            db.get_unmatched_files_for_series("series-a").await.unwrap(),
            vec![a]
        );
    }

    #[tokio::test]
    async fn delete_removes_records() {
        let (db, tmp) = test_db().await;
        seed_series(&db, "s").await;
        let a = tmp.path().join("a.mkv").to_string_lossy().to_string();
        db.upsert_unmatched_file(&a, Some("s"), "unneeded")
            .await
            .unwrap();
        db.delete_unmatched_files(&[a]).await.unwrap();
        assert!(
            db.get_unmatched_files_for_series("s")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn cleanup_keeps_present_and_missing_within_grace() {
        let (db, tmp) = test_db().await;
        seed_series(&db, "s").await;
        let present = tmp.path().join("present.mkv");
        std::fs::write(&present, b"x").unwrap();
        let present = present.to_string_lossy().to_string();
        let missing = tmp.path().join("missing.mkv").to_string_lossy().to_string();

        db.upsert_unmatched_file(&present, Some("s"), "unneeded")
            .await
            .unwrap();
        db.upsert_unmatched_file(&missing, Some("s"), "unneeded")
            .await
            .unwrap();

        // Grace period not elapsed → nothing deleted, missing marker set.
        let deleted = db.cleanup_unmatched_files(30).await.unwrap();
        assert_eq!(deleted, 0);
        let mut remaining = db.get_unmatched_files_for_series("s").await.unwrap();
        remaining.sort();
        let mut expected = vec![present.clone(), missing.clone()];
        expected.sort();
        assert_eq!(remaining, expected);
        let missing_since: Option<String> =
            sqlx::query_scalar("SELECT missing_since FROM unmatched_files WHERE file_path = ?")
                .bind(&missing)
                .fetch_one(db.get_pool())
                .await
                .unwrap();
        assert!(missing_since.is_some(), "missing file must be marked");

        // Backdate the missing marker past the grace period → the still-missing
        // file is dropped, the present one stays.
        sqlx::query(
            "UPDATE unmatched_files SET missing_since = datetime('now', '-40 days') WHERE file_path = ?",
        )
        .bind(&missing)
        .execute(db.get_pool())
        .await
        .unwrap();
        let deleted = db.cleanup_unmatched_files(30).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(
            db.get_unmatched_files_for_series("s").await.unwrap(),
            vec![present]
        );
    }

    #[tokio::test]
    async fn cleanup_clears_marker_when_file_reappears() {
        let (db, tmp) = test_db().await;
        seed_series(&db, "s").await;
        let path = tmp.path().join("flaky.mkv");
        let path_str = path.to_string_lossy().to_string();
        db.upsert_unmatched_file(&path_str, Some("s"), "unneeded")
            .await
            .unwrap();

        // Missing sweep marks it.
        db.cleanup_unmatched_files(30).await.unwrap();
        let marked: Option<String> =
            sqlx::query_scalar("SELECT missing_since FROM unmatched_files WHERE file_path = ?")
                .bind(&path_str)
                .fetch_one(db.get_pool())
                .await
                .unwrap();
        assert!(marked.is_some());

        // Reappears → marker cleared, row retained.
        std::fs::write(&path, b"back").unwrap();
        db.cleanup_unmatched_files(30).await.unwrap();
        let cleared: Option<String> =
            sqlx::query_scalar("SELECT missing_since FROM unmatched_files WHERE file_path = ?")
                .bind(&path_str)
                .fetch_one(db.get_pool())
                .await
                .unwrap();
        assert!(
            cleared.is_none(),
            "marker must be cleared when file returns"
        );
        assert_eq!(
            db.get_unmatched_files_for_series("s").await.unwrap(),
            vec![path_str]
        );
    }

    #[tokio::test]
    async fn cleanup_drops_rows_whose_file_is_now_assigned() {
        let (db, tmp) = test_db().await;
        seed_series(&db, "s").await;
        let assigned = tmp.path().join("assigned.mkv");
        std::fs::write(&assigned, b"x").unwrap();
        let assigned = assigned.to_string_lossy().to_string();
        let kept = tmp.path().join("kept.mkv").to_string_lossy().to_string();

        db.upsert_unmatched_file(&assigned, Some("s"), "unmatched")
            .await
            .unwrap();
        db.upsert_unmatched_file(&kept, Some("s"), "unmatched")
            .await
            .unwrap();

        // The file is adopted/assigned to an episode by a path that does not go
        // through the manual assign handlers (e.g. orphan adoption).
        sqlx::query(
            "INSERT INTO episodes (episode_id, series_id, season, episode, status, monitored)
             VALUES ('s_S01E01', 's', 1, 1, 'downloaded', 1)",
        )
        .execute(db.get_pool())
        .await
        .unwrap();
        db.associate_main_file("s_S01E01", &assigned, None)
            .await
            .unwrap();

        let deleted = db.cleanup_unmatched_files(30).await.unwrap();
        assert_eq!(deleted, 1, "the assigned file's review row must be dropped");
        assert_eq!(
            db.get_unmatched_files_for_series("s").await.unwrap(),
            vec![kept]
        );
    }

    #[tokio::test]
    async fn orphan_query_excludes_review_files() {
        let (db, tmp) = test_db().await;
        seed_series(&db, "s").await;
        let path = tmp.path().join("leftover.mkv");
        std::fs::write(&path, b"x").unwrap();
        let path_str = path.to_string_lossy().to_string();

        // A complete path with no episode association is normally an
        // adoption candidate.
        sqlx::query(
            "INSERT INTO file_paths (file_path, fingerprint, size, state)
             VALUES (?, 'orphan', 1, 'complete')",
        )
        .bind(&path_str)
        .execute(db.get_pool())
        .await
        .unwrap();
        assert_eq!(db.get_orphan_files().await.unwrap(), vec![path_str.clone()]);

        // Recording it for review removes it from adoption...
        db.upsert_unmatched_file(&path_str, Some("s"), "unmatched")
            .await
            .unwrap();
        assert!(
            db.get_orphan_files().await.unwrap().is_empty(),
            "a kept-for-review file must not be offered for adoption"
        );

        // ...until it is assigned and its review row is cleared.
        db.delete_unmatched_files(std::slice::from_ref(&path_str))
            .await
            .unwrap();
        assert_eq!(db.get_orphan_files().await.unwrap(), vec![path_str]);
    }

    #[tokio::test]
    async fn deleting_series_cascades_review_rows() {
        let (db, tmp) = test_db().await;
        seed_series(&db, "gone").await;
        seed_series(&db, "kept").await;
        let a = tmp.path().join("a.mkv").to_string_lossy().to_string();
        let b = tmp.path().join("b.mkv").to_string_lossy().to_string();
        db.upsert_unmatched_file(&a, Some("gone"), "unneeded")
            .await
            .unwrap();
        db.upsert_unmatched_file(&b, Some("kept"), "unneeded")
            .await
            .unwrap();

        db.delete_series_mapping("gone").await.unwrap();

        assert!(
            db.get_unmatched_files_for_series("gone")
                .await
                .unwrap()
                .is_empty(),
            "review rows must not outlive their series"
        );
        assert_eq!(
            db.get_unmatched_files_for_series("kept").await.unwrap(),
            vec![b]
        );
    }
}
