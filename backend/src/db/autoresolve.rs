// Autoresolve accounting: the releases excluded from auto-search after a stalled
// download was removed, and the per-target attempt budget that bounds how many
// alternatives are tried.
//
// A stalled release is the highest-scored one, so without a rejection a re-search
// would pick it again and stall in a loop; rejections expire after the rejection
// window so a transient bad swarm is not banned forever.
// The attempt budget is separate (a shorter window) and is cleared when a download
// for the same episode succeeds.

use super::DbManager;
use anyhow::Result;
use std::collections::HashMap;

/// A single active rejection: why a release was rejected and when it becomes
/// eligible for auto-search again.
#[derive(Debug, Clone)]
pub struct Rejection {
    pub reason: String,
    /// Naive UTC, mirroring the DB's canonical timestamp format.
    pub expires_at: chrono::NaiveDateTime,
}

/// The set of releases currently rejected, indexed by link and download id.
/// Carries the reason and expiry so manual search can explain a rejection.
#[derive(Debug, Default, Clone)]
pub struct RejectedDownloads {
    by_link: HashMap<String, Rejection>,
    by_download_id: HashMap<String, Rejection>,
}

impl RejectedDownloads {
    /// Record an active rejection. Both the link and (when present) the download
    /// id resolve to it, mirroring how `get_rejected_downloads` indexes rows.
    pub(crate) fn insert(
        &mut self,
        media_link: &str,
        download_id: Option<&str>,
        rejection: Rejection,
    ) {
        if let Some(id) = download_id.filter(|d| !d.is_empty()) {
            self.by_download_id
                .insert(id.to_string(), rejection.clone());
        }
        self.by_link.insert(media_link.to_string(), rejection);
    }

    /// True when a candidate matching either identifier is rejected.
    pub fn contains(&self, link: Option<&str>, download_id: Option<&str>) -> bool {
        self.lookup(link, download_id).is_some()
    }

    /// The rejection a candidate matches. A link match takes precedence over a
    /// download-id match, mirroring the stored precedence order.
    pub fn lookup(&self, link: Option<&str>, download_id: Option<&str>) -> Option<&Rejection> {
        link.and_then(|l| self.by_link.get(l))
            .or_else(|| download_id.and_then(|d| self.by_download_id.get(d)))
    }

    pub fn is_empty(&self) -> bool {
        self.by_link.is_empty() && self.by_download_id.is_empty()
    }
}

/// The series/season/episode a rejection belongs to. Any field may be NULL for a
/// release that could not be scoped to a single target.
#[derive(Debug, Clone, Copy, Default)]
pub struct RejectionTarget<'a> {
    pub series_id: Option<&'a str>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
}

impl DbManager {
    /// Reject a release until `expires_at`. Upsert so a re-stall refreshes the
    /// window instead of failing the insert. `target` records which series/season/
    /// episode the release was tried for.
    pub async fn reject_download(
        &self,
        media_link: &str,
        download_id: Option<&str>,
        reason: &str,
        target: RejectionTarget<'_>,
        expires_at: chrono::NaiveDateTime,
    ) -> Result<()> {
        exec_with_bindings!(
            self,
            "INSERT INTO rejected_downloads \
             (media_link, download_id, series_id, season, episode, reason, expires_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(media_link) DO UPDATE SET \
             download_id = excluded.download_id, series_id = excluded.series_id, \
             season = excluded.season, episode = excluded.episode, \
             reason = excluded.reason, expires_at = excluded.expires_at",
            media_link,
            download_id,
            target.series_id,
            target.season,
            target.episode,
            reason,
            expires_at
        )
    }

    /// Rejected releases that have not yet expired.
    pub async fn get_rejected_downloads(&self) -> Result<RejectedDownloads> {
        #[derive(sqlx::FromRow)]
        struct RejectedRow {
            media_link: String,
            download_id: Option<String>,
            reason: String,
            expires_at: chrono::NaiveDateTime,
        }

        let rows: Vec<RejectedRow> = sqlx::query_as(
            "SELECT media_link, download_id, reason, expires_at FROM rejected_downloads \
             WHERE expires_at > datetime('now')",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut rejected = RejectedDownloads::default();
        for row in rows {
            rejected.insert(
                &row.media_link,
                row.download_id.as_deref(),
                Rejection {
                    reason: row.reason,
                    expires_at: row.expires_at,
                },
            );
        }
        Ok(rejected)
    }

    /// Remove expired rejections. Returns the number removed.
    pub async fn prune_expired_rejections(&self) -> Result<u64> {
        let result =
            sqlx::query("DELETE FROM rejected_downloads WHERE expires_at <= datetime('now')")
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected())
    }

    // ── Autoresolve attempt budget ────────────────────────────────────────
    //
    // Attempts are tracked within a sliding `budget_hours` window anchored on the
    // last attempt. Hitting the limit switches the reset to the (shorter)
    // `hit_cooldown_hours`, so a failed episode is retried soon after instead of
    // waiting out the full window.

    /// Attempts already spent on a target, or 0 once the applicable window has
    /// elapsed (the budget window normally, the hit cooldown after a failure).
    pub async fn get_autoresolve_attempts(
        &self,
        target: RejectionTarget<'_>,
        budget_hours: i64,
        hit_cooldown_hours: i64,
    ) -> Result<i64> {
        let budget = format!("-{budget_hours} hours");
        let hit = format!("-{hit_cooldown_hours} hours");
        let attempts: Option<i64> = sqlx::query_scalar(
            "SELECT CASE \
                 WHEN (exhausted_at IS NOT NULL AND exhausted_at > datetime('now', ?)) \
                   OR (exhausted_at IS NULL AND last_attempt_at > datetime('now', ?)) \
                 THEN attempts ELSE 0 END \
             FROM autoresolve_attempts \
             WHERE series_id IS ? AND season IS ? AND episode IS ?",
        )
        .bind(&hit)
        .bind(&budget)
        .bind(target.series_id)
        .bind(target.season)
        .bind(target.episode)
        .fetch_optional(&self.pool)
        .await?;
        Ok(attempts.unwrap_or(0))
    }

    /// Record one alternative attempt for a target, moving the window anchor to
    /// now. When the applicable window has elapsed the count restarts at 1 and the
    /// exhaustion cooldown is cleared.
    pub async fn increment_autoresolve_attempts(
        &self,
        target: RejectionTarget<'_>,
        budget_hours: i64,
        hit_cooldown_hours: i64,
    ) -> Result<()> {
        let budget = format!("-{budget_hours} hours");
        let hit = format!("-{hit_cooldown_hours} hours");
        exec_with_bindings!(
            self,
            "INSERT INTO autoresolve_attempts \
             (series_id, season, episode, attempts, last_attempt_at, exhausted_at) \
             VALUES (?, ?, ?, 1, datetime('now'), NULL) \
             ON CONFLICT(series_id, season, episode) DO UPDATE SET \
             attempts = CASE \
                 WHEN (exhausted_at IS NOT NULL AND exhausted_at > datetime('now', ?)) \
                   OR (exhausted_at IS NULL AND last_attempt_at > datetime('now', ?)) \
                 THEN attempts + 1 ELSE 1 END, \
             exhausted_at = CASE \
                 WHEN (exhausted_at IS NOT NULL AND exhausted_at > datetime('now', ?)) \
                   OR (exhausted_at IS NULL AND last_attempt_at > datetime('now', ?)) \
                 THEN exhausted_at ELSE NULL END, \
             last_attempt_at = datetime('now')",
            target.series_id,
            target.season,
            target.episode,
            &hit,
            &budget,
            &hit,
            &budget
        )
    }

    /// Mark that the budget was hit (the item was failed) so its cooldown is
    /// enforced before the count can reset.
    pub async fn mark_autoresolve_exhausted(&self, target: RejectionTarget<'_>) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE autoresolve_attempts SET exhausted_at = datetime('now') \
             WHERE series_id IS ? AND season IS ? AND episode IS ?",
            target.series_id,
            target.season,
            target.episode
        )
    }

    /// Clear the budget for the given episodes of a series/season. Called when a
    /// download succeeds, proving alternatives work, so a future stall starts
    /// fresh.
    pub async fn reset_autoresolve_attempts(
        &self,
        series_id: &str,
        season: i32,
        episodes: &[i32],
    ) -> Result<()> {
        if episodes.is_empty() {
            return Ok(());
        }
        let placeholders = super::sql_in_placeholders(episodes.len());
        let sql = format!(
            "DELETE FROM autoresolve_attempts \
             WHERE series_id = ? AND season = ? AND episode IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql).bind(series_id).bind(season);
        for episode in episodes {
            query = query.bind(*episode);
        }
        query.execute(&self.pool).await?;
        Ok(())
    }

    /// Remove attempt rows whose applicable window has elapsed. Returns the
    /// number removed.
    pub async fn prune_expired_autoresolve_attempts(
        &self,
        budget_hours: i64,
        hit_cooldown_hours: i64,
    ) -> Result<u64> {
        let budget = format!("-{budget_hours} hours");
        let hit = format!("-{hit_cooldown_hours} hours");
        let result = sqlx::query(
            "DELETE FROM autoresolve_attempts \
             WHERE (exhausted_at IS NOT NULL AND exhausted_at <= datetime('now', ?)) \
                OR (exhausted_at IS NULL AND last_attempt_at <= datetime('now', ?))",
        )
        .bind(&hit)
        .bind(&budget)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
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

    fn hours_from_now(hours: i64) -> chrono::NaiveDateTime {
        chrono::Utc::now().naive_utc() + chrono::Duration::hours(hours)
    }

    fn target(episode: i32) -> RejectionTarget<'static> {
        RejectionTarget {
            series_id: Some("series-1"),
            season: Some(1),
            episode: Some(episode),
        }
    }

    #[tokio::test]
    async fn reject_then_get_matches_link_and_download_id() {
        let (db, _tmp) = test_db().await;
        db.reject_download(
            "magnet:a",
            Some("hash-a"),
            "no progress",
            target(1),
            hours_from_now(48),
        )
        .await
        .unwrap();

        let rejected = db.get_rejected_downloads().await.unwrap();
        assert!(rejected.contains(Some("magnet:a"), None));
        assert!(rejected.contains(None, Some("hash-a")));
        assert!(!rejected.contains(Some("magnet:b"), Some("hash-b")));
    }

    #[tokio::test]
    async fn expired_rejection_is_excluded_and_pruned() {
        let (db, _tmp) = test_db().await;
        db.reject_download(
            "magnet:old",
            None,
            "no progress",
            target(1),
            hours_from_now(-1),
        )
        .await
        .unwrap();

        assert!(db.get_rejected_downloads().await.unwrap().is_empty());
        assert_eq!(db.prune_expired_rejections().await.unwrap(), 1);

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM rejected_downloads")
            .fetch_one(db.get_pool())
            .await
            .unwrap();
        assert_eq!(count.0, 0);
    }

    #[tokio::test]
    async fn re_rejecting_refreshes_instead_of_duplicating() {
        let (db, _tmp) = test_db().await;
        db.reject_download(
            "magnet:a",
            Some("hash-1"),
            "first",
            target(1),
            hours_from_now(1),
        )
        .await
        .unwrap();
        db.reject_download(
            "magnet:a",
            Some("hash-2"),
            "second",
            target(1),
            hours_from_now(48),
        )
        .await
        .unwrap();

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM rejected_downloads")
            .fetch_one(db.get_pool())
            .await
            .unwrap();
        assert_eq!(count.0, 1);

        let rejected = db.get_rejected_downloads().await.unwrap();
        assert!(rejected.contains(None, Some("hash-2")));
        assert!(!rejected.contains(None, Some("hash-1")));
    }

    #[tokio::test]
    async fn attempt_budget_counts_within_window_and_expires() {
        let (db, _tmp) = test_db().await;
        let t = target(1);
        assert_eq!(db.get_autoresolve_attempts(t, 12, 2).await.unwrap(), 0);

        db.increment_autoresolve_attempts(t, 12, 2).await.unwrap();
        db.increment_autoresolve_attempts(t, 12, 2).await.unwrap();
        assert_eq!(db.get_autoresolve_attempts(t, 12, 2).await.unwrap(), 2);

        // Elapse the window: the count reads as 0, and the next attempt restarts at 1.
        sqlx::query(
            "UPDATE autoresolve_attempts SET last_attempt_at = datetime('now', '-13 hours')",
        )
        .execute(db.get_pool())
        .await
        .unwrap();
        assert_eq!(db.get_autoresolve_attempts(t, 12, 2).await.unwrap(), 0);
        db.increment_autoresolve_attempts(t, 12, 2).await.unwrap();
        assert_eq!(db.get_autoresolve_attempts(t, 12, 2).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn hit_cooldown_governs_the_reset() {
        let (db, _tmp) = test_db().await;
        let t = target(1);
        db.increment_autoresolve_attempts(t, 12, 2).await.unwrap();
        db.increment_autoresolve_attempts(t, 12, 2).await.unwrap();

        // Budget hit: the count is held for the cooldown even though the 12h
        // attempt window is still open.
        db.mark_autoresolve_exhausted(t).await.unwrap();
        assert_eq!(db.get_autoresolve_attempts(t, 12, 2).await.unwrap(), 2);

        // Once the cooldown elapses the count resets, well before the 12h window.
        sqlx::query("UPDATE autoresolve_attempts SET exhausted_at = datetime('now', '-3 hours')")
            .execute(db.get_pool())
            .await
            .unwrap();
        assert_eq!(db.get_autoresolve_attempts(t, 12, 2).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn reset_autoresolve_attempts_clears_only_named_episodes() {
        let (db, _tmp) = test_db().await;
        db.increment_autoresolve_attempts(target(1), 12, 2)
            .await
            .unwrap();
        db.increment_autoresolve_attempts(target(2), 12, 2)
            .await
            .unwrap();
        db.increment_autoresolve_attempts(target(2), 12, 2)
            .await
            .unwrap();

        db.reset_autoresolve_attempts("series-1", 1, &[1])
            .await
            .unwrap();
        assert_eq!(
            db.get_autoresolve_attempts(target(1), 12, 2).await.unwrap(),
            0
        );
        assert_eq!(
            db.get_autoresolve_attempts(target(2), 12, 2).await.unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn prune_removes_only_elapsed_attempt_windows() {
        let (db, _tmp) = test_db().await;
        db.increment_autoresolve_attempts(target(1), 12, 2)
            .await
            .unwrap();
        db.increment_autoresolve_attempts(target(2), 12, 2)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE autoresolve_attempts SET last_attempt_at = datetime('now', '-13 hours') \
             WHERE episode = 1",
        )
        .execute(db.get_pool())
        .await
        .unwrap();

        assert_eq!(
            db.prune_expired_autoresolve_attempts(12, 2).await.unwrap(),
            1
        );
        let remaining: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM autoresolve_attempts")
            .fetch_one(db.get_pool())
            .await
            .unwrap();
        assert_eq!(remaining.0, 1);
    }
}
