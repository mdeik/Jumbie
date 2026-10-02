// The journal is the write-ahead log for file operations (rename, move, delete):
// each operation is recorded BEFORE it happens so a crash mid-operation can be
// detected on restart (leftover pending entries). Distinct from the retry queue,
// which holds operations that FAILED deterministically and should be retried with
// backoff.
use super::DbManager;
use anyhow::Result;
use jumbie_shared::types::RetryItem;

impl DbManager {
    pub async fn log_journal_start(
        &self,
        event_type: &str,
        src: &str,
        dest: Option<&str>,
    ) -> Result<i64> {
        let id = sqlx::query_scalar(
            "INSERT INTO file_event_log (event_type, source_path, destination_path, status) VALUES (?, ?, ?, 'pending') RETURNING id"
        )
        .bind(event_type)
        .bind(src)
        .bind(dest)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    // Journal ids are SQLite ROWIDs (i64), not UUIDs: entries are ephemeral,
    // written and resolved within a session, so the smaller id is enough.
    pub async fn complete_journal_entry(&self, id: i64) -> Result<()> {
        exec_by_id!(
            self,
            "UPDATE file_event_log SET status = 'completed' WHERE id = ?",
            id
        )
    }

    pub async fn get_pending_journal_entries(
        &self,
    ) -> Result<Vec<(i64, String, String, Option<String>)>> {
        // Returns (id, event_type, source_path, destination_path)
        let rows = sqlx::query_as::<_, (i64, String, String, Option<String>)>(
            "SELECT id, event_type, source_path, destination_path FROM file_event_log WHERE status = 'pending'"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // Exponential backoff: 2^retry_count minutes (0 → 1, 1 → 2, 2 → 4, …).
    // Deliberately uncapped: if an operation reaches max_retries it is a permanent
    // failure rather than transient contention.
    pub async fn schedule_retry(
        &self,
        operation: &str,
        src: &str,
        dest: Option<&str>,
        ep_id: Option<&str>,
        error: &str,
        retry_count: i32,
    ) -> Result<()> {
        let backoff_minutes = 2_i64.pow(retry_count as u32);
        let next_retry =
            chrono::Utc::now().naive_utc() + chrono::Duration::minutes(backoff_minutes);

        exec_with_bindings!(self,
            "INSERT INTO retry_queue (operation, source_path, destination_path, episode_id, error_message, retry_count, next_retry_at, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'pending')",
            operation, src, dest, ep_id, error, retry_count, next_retry
        )
    }

    pub async fn get_pending_retries(&self) -> Result<Vec<RetryItem>> {
        let now = chrono::Utc::now().naive_utc();
        let rows = sqlx::query_as::<_, RetryItem>(
        "SELECT id, operation, source_path, destination_path, episode_id, error_message, retry_count, max_retries, next_retry_at, status
         FROM retry_queue
         WHERE status = 'pending' AND next_retry_at <= ?"
    )
    .bind(now)
    .fetch_all(&self.pool)
    .await?;
        Ok(rows)
    }

    pub async fn update_retry_status(
        &self,
        id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE retry_queue SET status = ?, error_message = COALESCE(?, error_message), last_attempt_at = CURRENT_TIMESTAMP WHERE id = ?",
            status,
            error,
            id
        )
    }

    /// Delete completed file_event_log entries older than 7 days.
    /// After crash recovery, 'pending' entries are the only ones that matter;
    /// 'completed' entries are dead data that serves no purpose.
    pub async fn cleanup_completed_journal_entries(&self) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM file_event_log \
             WHERE status = 'completed' \
               AND created_at < datetime('now', '-7 days')",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Delete retry_queue entries with terminal (non-pending) status whose
    /// last_attempt_at is older than 7 days.
    ///
    /// Keeps recently-failed entries so the user can inspect or retry them.
    /// Terminal retries that succeeded or exhausted max_retries are dead data.
    pub async fn cleanup_terminal_retry_entries(&self) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM retry_queue \
             WHERE status NOT IN ('pending') \
               AND last_attempt_at IS NOT NULL \
               AND last_attempt_at < datetime('now', '-7 days')",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}
