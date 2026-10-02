// Download queue: items matched to a release but not yet sent to a download
// client, items currently downloading, and failed items.
//
// Replacement: a new release for an episode that already has a queued item
// replaces it only when its score is strictly higher.
//
// Multi-episode downloads set `episode_end` to the last episode in the range;
// adding a single-episode download checks whether an existing pack already
// covers that episode (see `get_multiepisode_queue_item_covering`).

use super::{
    DOWNLOAD_QUEUE_BASE, DbManager, IN_FLIGHT_QUEUE_STATUSES, NON_TERMINAL_QUEUE_STATUSES,
};
use anyhow::Result;
use jumbie_shared::types::{AddQueueResult, DownloadQueueItem};

pub struct AddToDownloadQueueParams<'a> {
    pub media_name: &'a str,
    pub media_link: &'a str,
    pub series_title: &'a str,
    pub series_id: &'a str,
    pub seasons: &'a [i32],
    pub episodes: &'a [i32],
    pub episode_id: Option<&'a str>,
    pub score: i32,
    pub is_user_requested: bool,
    pub is_season_pack: bool,
    pub category: &'a str,
    pub multi_targets: Option<&'a str>,
    pub episode_intentions: Option<&'a str>,
    pub source_pub_date: Option<&'a str>,
    pub download_id: &'a str,
    pub scoring_size_bytes: Option<i64>,
    pub scoring_seeders: Option<i32>,
    pub scoring_episode_count: Option<i32>,
    pub submitter: Option<&'a str>,
    pub quality_profile_id: Option<&'a str>,
    pub version: i32,
}

impl DbManager {
    /// Queue a release selected by the system (search result or scanner).
    pub async fn add_to_download_queue(
        &self,
        params: AddToDownloadQueueParams<'_>,
    ) -> Result<AddQueueResult> {
        self.insert_download_queue_item(params, false).await
    }

    /// Queue a release the user explicitly picked (link + download_id). Marks the
    /// row `is_manual`, which drives the `Manual` badge and exempts it from
    /// no-progress autoresolve.
    pub async fn add_manual_download_to_queue(
        &self,
        params: AddToDownloadQueueParams<'_>,
    ) -> Result<AddQueueResult> {
        self.insert_download_queue_item(params, true).await
    }

    /// Single write path for the download queue.
    async fn insert_download_queue_item(
        &self,
        params: AddToDownloadQueueParams<'_>,
        is_manual: bool,
    ) -> Result<AddQueueResult> {
        let AddToDownloadQueueParams {
            media_name,
            media_link,
            series_title,
            series_id,
            seasons,
            episodes,
            episode_id,
            score,
            is_user_requested,
            is_season_pack,
            category,
            multi_targets,
            episode_intentions,
            source_pub_date,
            download_id,
            scoring_size_bytes,
            scoring_seeders,
            scoring_episode_count,
            submitter,
            quality_profile_id,
            version,
        } = params;

        // SSoT: first season/episode from the lists for DB storage / fallback dedup;
        // empty slices → NULL in DB (episodes unknown, resolved by smart-link).
        let primary_season_str: Option<String> = seasons.first().map(|s| format!("{:02}", s));
        let primary_ep = episodes.first().copied();
        with_transaction!(self.pool, |mut tx| {
            // Phase 1: match by exact episode_id; if that misses (e.g. the row has a
            // NULL episode_id via ON DELETE SET NULL), fall back to (series, season, episode).
            let mut existing: Option<DownloadQueueItem> = None;
            if let Some(ep_id) = episode_id {
                existing = sqlx::query_as::<_, DownloadQueueItem>(&format!(
                    "{}WHERE dq.episode_id = ? AND dq.status IN ({}) ",
                    DOWNLOAD_QUEUE_BASE, NON_TERMINAL_QUEUE_STATUSES,
                ))
                .bind(ep_id)
                .fetch_optional(&mut *tx)
                .await?;
            }
            if existing.is_none()
                && !series_id.is_empty()
                && let Some(ref ps) = primary_season_str
                && let Some(ep) = primary_ep
            {
                existing = sqlx::query_as::<_, DownloadQueueItem>(&format!(
                    "{}WHERE dq.series_id = ? \
                     AND dq.season = ? \
                     AND dq.episode = ? \
                     AND dq.status IN ({}) LIMIT 1",
                    DOWNLOAD_QUEUE_BASE, NON_TERMINAL_QUEUE_STATUSES,
                ))
                .bind(series_id)
                .bind(ps)
                .bind(ep)
                .fetch_optional(&mut *tx)
                .await?;
            }

            // Phase 2: if the same magnet is already queued for a *different* target,
            // merge this target into that entry's multi_targets instead of adding a row.
            let magnet_match = if existing.is_none() && !media_link.is_empty() {
                sqlx::query_as::<_, DownloadQueueItem>(&format!(
                    "{}WHERE dq.media_link = ? \
                     AND dq.status IN ({}) LIMIT 1",
                    DOWNLOAD_QUEUE_BASE, NON_TERMINAL_QUEUE_STATUSES,
                ))
                .bind(media_link)
                .fetch_optional(&mut *tx)
                .await?
            } else {
                None
            };

            let mut existing_from_magnet: Option<DownloadQueueItem> = None;
            if let Some(magnet_item) = &magnet_match {
                // Same logical target? Match on episode_id only when BOTH sides have
                // one, else fall back to (series, season, episode) (None matches anything).
                let has_ep_id = magnet_item.episode_id.is_some() && episode_id.is_some();
                let is_same_target = if has_ep_id {
                    magnet_item.episode_id.as_deref() == episode_id
                } else if !magnet_item.series_id.is_empty()
                    && let Some(ref ps) = primary_season_str
                {
                    magnet_item.series_id == series_id
                        && magnet_item.season.as_deref() == Some(ps)
                        && magnet_item.episode == primary_ep
                } else {
                    false
                };

                if is_same_target {
                    existing_from_magnet = Some(magnet_item.clone());
                } else {
                    let new_target = serde_json::json!({
                        "series_id": series_id,
                        "season": primary_season_str
                            .as_deref()
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(1),
                        "episode": primary_ep.unwrap_or(0),
                    });

                    let mut existing_targets: Vec<serde_json::Value> = magnet_item
                        .multi_targets
                        .as_deref()
                        .and_then(|json| serde_json::from_str(json).ok())
                        .unwrap_or_default();
                    existing_targets.push(new_target);

                    let updated_json = serde_json::to_string(&existing_targets).unwrap_or_default();

                    sqlx::query("UPDATE download_queue SET multi_targets = ? WHERE id = ?")
                        .bind(&updated_json)
                        .bind(magnet_item.id)
                        .execute(&mut *tx)
                        .await?;

                    return Ok(AddQueueResult::Merged {
                        existing_id: magnet_item.id,
                        new_targets: existing_targets.len(),
                    });
                }
            }

            // Phase 3: score-based replacement — never bump a better release.
            let score_match = existing.or(existing_from_magnet);
            let mut replaced_item = None;

            if let Some(existing_item) = score_match {
                // Replace only on a strictly higher score. `is_user_requested` is stored
                // for the later organize-time upgrade check, not used here.
                if score > existing_item.score {
                    sqlx::query("DELETE FROM download_queue WHERE id = ?")
                        .bind(existing_item.id)
                        .execute(&mut *tx)
                        .await?;
                    replaced_item = Some(existing_item);
                } else {
                    return Ok(AddQueueResult::Skipped);
                }
            }

            // Phase 4: insert the new row.
            let insert = sqlx::query(
            "INSERT INTO download_queue (media_name, media_link, series_title, series_id, season, episode, episode_end, episode_id, score, is_user_requested, is_manual, is_season_pack, category, multi_targets, episode_intentions, download_id, scoring_size_bytes, scoring_seeders, scoring_episode_count, source_pub_date, submitter, quality_profile_id, version, status)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'Queued')"
        )
        .bind(media_name)
        .bind(media_link)
        .bind(series_title)
        .bind(series_id)
        .bind(&primary_season_str)
        .bind(primary_ep)
        .bind(None::<i32>)
        .bind(episode_id)
        .bind(score)
        .bind(is_user_requested)
        .bind(is_manual)
        .bind(is_season_pack)
        .bind(category)
        .bind(multi_targets)
        .bind(episode_intentions)
                .bind(download_id)
                .bind(scoring_size_bytes)
                .bind(scoring_seeders)
                .bind(scoring_episode_count)
                .bind(source_pub_date)
                .bind(submitter)
                .bind(quality_profile_id)
                .bind(version)
                .execute(&mut *tx)
        .await?;
            let new_id = insert.last_insert_rowid();

            if let Some(item) = replaced_item {
                Ok(AddQueueResult::Replaced(Box::new(item)))
            } else {
                Ok(AddQueueResult::Added { id: new_id })
            }
        })
    }

    /// Per-download metadata snapshot that `organize_completed` writes to
    /// `release_info`. The queue item is the SSoT for a download's attribution:
    /// submitter and quality profile are recorded at queue time and read here.
    pub async fn get_organize_meta(
        &self,
        episode_id: &str,
    ) -> Result<Option<super::OrganizeMetaRow>> {
        let row = sqlx::query_as::<_, super::OrganizeMetaRow>(
            "SELECT dq.media_name, dq.media_link, dq.score, dq.source_pub_date,
                    dq.download_id, dq.submitter, dq.quality_profile_id,
                    dq.scoring_size_bytes, dq.scoring_seeders, dq.scoring_episode_count,
                    COALESCE(dq.version, 1) AS version
             FROM download_queue dq
             WHERE dq.episode_id = ?
             ORDER BY dq.downloaded_at DESC
             LIMIT 1",
        )
        .bind(episode_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Find any active queue item (Queued/Downloading/Failed) that is a multi-episode download
    /// whose episode range includes the given episode number.
    ///
    /// Used to detect when a single-episode download should cancel an existing multi-episode
    /// pack. For example, if "Season 01 Complete" (episode 1-12) is already queued, adding a
    /// download for episode 5 individually would waste bandwidth — the pack already covers it.
    pub async fn get_multiepisode_queue_item_covering(
        &self,
        season: &str,
        ep_num: i32,
    ) -> Result<Option<DownloadQueueItem>> {
        // Best-scoring covering range wins.
        let item = sqlx::query_as::<_, DownloadQueueItem>(&format!(
            "{}{}",
            DOWNLOAD_QUEUE_BASE,
            r#"
            WHERE dq.season = ?
              AND dq.episode_end IS NOT NULL
              AND dq.episode <= ?
              AND dq.episode_end >= ?
              AND dq.status IN ('Queued', 'Downloading', 'Failed')
            ORDER BY score DESC
            LIMIT 1
            "#
        ))
        .bind(season)
        .bind(ep_num)
        .bind(ep_num)
        .fetch_optional(&self.pool)
        .await?;
        Ok(item)
    }

    // Listing: queued items (user-requested and high-score first), downloading
    // items (including 'Paused' so the UI can show them), and full history.

    pub async fn get_queued_items(&self) -> Result<Vec<DownloadQueueItem>> {
        self.fetch_all(&format!(
            "{}{}",
            DOWNLOAD_QUEUE_BASE,
            r#"
            WHERE status = 'Queued'
            ORDER BY is_user_requested DESC, score DESC, downloaded_at ASC
            "#
        ))
        .await
    }

    pub async fn get_downloading_items(&self) -> Result<Vec<DownloadQueueItem>> {
        self.fetch_all(&format!(
            "{}{}",
            DOWNLOAD_QUEUE_BASE,
            r#"
            WHERE status IN ('Downloading', 'Paused')
            "#
        ))
        .await
    }

    /// Items in retry-based content-path resolution.
    /// Returns 'Organizing' items whose next_retry_at has passed (or is NULL).
    pub async fn get_organizing_items(&self) -> Result<Vec<DownloadQueueItem>> {
        self.fetch_all(&format!(
            "{}{}",
            DOWNLOAD_QUEUE_BASE,
            r#"
            WHERE status = 'Organizing'
                          AND (next_retry_at IS NULL OR next_retry_at <= datetime('now'))
            ORDER BY retry_count ASC, downloaded_at ASC
            "#
        ))
        .await
    }

    pub async fn get_download_queue(&self) -> Result<Vec<DownloadQueueItem>> {
        self.fetch_all(&format!(
            "{}{}",
            DOWNLOAD_QUEUE_BASE,
            r#"
            ORDER BY downloaded_at DESC
            "#
        ))
        .await
    }

    // `update_queue_item_status` COALESCEs downloader_id/client_id, so the first
    // client set is preserved across later status updates.

    pub async fn update_queue_item_status(
        &self,
        id: i64,
        status: &str,
        downloader_id: Option<&str>,
        client_id: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE download_queue SET status = ?, \
             downloader_id = COALESCE(?, downloader_id), \
             client_id = COALESCE(?, client_id), error_message = ? WHERE id = ?",
            status,
            downloader_id,
            client_id,
            error_message,
            id
        )
    }

    pub async fn update_download_queue_item(&self, item: &DownloadQueueItem) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE download_queue SET status = ?, downloader_id = ?, client_id = ?, \
             progress = ?, score = ?, is_user_requested = ?, is_season_pack = ?, category = ?, \
             error_message = ? WHERE id = ?",
            &item.status,
            &item.downloader_id,
            &item.client_id,
            item.progress,
            item.score,
            item.is_user_requested,
            item.is_season_pack,
            &item.category,
            &item.error_message,
            item.id
        )
    }

    pub async fn update_queue_item_progress(&self, id: i64, progress: f32) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE download_queue SET progress = ? WHERE id = ?",
            progress,
            id
        )
    }

    /// Update retry tracking for a queue item.
    /// Sets status, retry_count, and schedules the next retry attempt at next_retry_at.
    /// The caller chooses the status — e.g. "Organizing" for content-path resolution
    /// retries, "Queued" for download dispatch retries.
    pub async fn update_queue_item_retry(
        &self,
        id: i64,
        status: &str,
        retry_count: i32,
        next_retry_at: chrono::NaiveDateTime,
    ) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE download_queue SET status = ?, retry_count = ?, \
             next_retry_at = ? WHERE id = ?",
            status,
            retry_count,
            next_retry_at,
            id
        )
    }

    /// Reset retry tracking when an item exits the 'Organizing' state.
    pub async fn reset_queue_item_retry(&self, id: i64) -> Result<()> {
        exec_by_id!(
            self,
            "UPDATE download_queue SET retry_count = 0, next_retry_at = NULL WHERE id = ?",
            id
        )
    }

    /// Record a progress sample and reset the no-progress timer. Called when
    /// progress advanced past the last sample (or on the first sample).
    pub async fn record_download_progress(
        &self,
        id: i64,
        progress: f32,
        at: chrono::NaiveDateTime,
    ) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE download_queue SET last_progress = ?, no_progress_since = ? WHERE id = ?",
            progress,
            at,
            id
        )
    }

    /// Clear progress tracking when an item (re)enters a downloading lifecycle,
    /// so a previous attempt's window cannot make it look instantly stalled.
    pub async fn reset_progress_tracking(&self, id: i64) -> Result<()> {
        exec_by_id!(
            self,
            "UPDATE download_queue SET last_progress = NULL, no_progress_since = NULL \
             WHERE id = ?",
            id
        )
    }

    pub async fn remove_from_download_queue(&self, id: i64) -> Result<()> {
        exec_by_id!(self, "DELETE FROM download_queue WHERE id = ?", id)
    }

    /// Returns the set of episode_ids with active (Queued or Downloading) queue
    /// entries, used to mark episodes as "in_queue" instead of "missing".
    pub async fn get_active_queue_episode_ids(
        &self,
        episode_ids: &[String],
    ) -> Result<std::collections::HashSet<String>> {
        if episode_ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        // Chunk to avoid SQLite's variable number limit (999)
        let mut result = std::collections::HashSet::new();
        for chunk in episode_ids.chunks(450) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "SELECT DISTINCT episode_id FROM download_queue \
                 WHERE episode_id IN ({}) AND status IN ({})",
                placeholders, IN_FLIGHT_QUEUE_STATUSES
            );
            let mut q = sqlx::query_as::<_, (String,)>(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows: Vec<(String,)> = q.fetch_all(&self.pool).await?;
            for (eid,) in rows {
                result.insert(eid);
            }
        }
        Ok(result)
    }

    /// Per-series count of episodes currently in the download queue (in-flight:
    /// Queued or Downloading), regardless of the `monitored` flag.
    ///
    /// The library's yellow "queued" indicator must light up for an episode that is
    /// downloading even when it is unmonitored (e.g. a manual download, or an episode
    /// whose monitoring was turned off after it was queued). `get_wanted_episodes`
    /// cannot serve this — it filters to `monitored = 1`. Episodes excluded from
    /// monitoring by range (`out_of_range`) never count.
    pub async fn get_queued_episode_counts_by_series(
        &self,
    ) -> Result<std::collections::HashMap<String, i32>> {
        let rows = sqlx::query_as::<_, (String, i32)>(&format!(
            "SELECT e.series_id, COUNT(DISTINCT e.episode_id) \
             FROM download_queue dq \
             JOIN episodes e ON e.episode_id = dq.episode_id \
             WHERE dq.status IN ({in_flight}) \
               AND e.status != 'out_of_range' \
             GROUP BY e.series_id",
            in_flight = IN_FLIGHT_QUEUE_STATUSES,
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }

    pub async fn get_queue_item_by_episode_id(
        &self,
        episode_id: &str,
    ) -> Result<Option<DownloadQueueItem>> {
        let item = sqlx::query_as::<_, DownloadQueueItem>(&format!(
            "{}{}",
            DOWNLOAD_QUEUE_BASE,
            r#"
            WHERE episode_id = ?
            ORDER BY downloaded_at DESC
            LIMIT 1
            "#
        ))
        .bind(episode_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(item)
    }

    /// Atomically remove the terminal-status queue item for a given episode_id.
    /// Only targets 'Completed'/'Failed'/'Organizing' items so a newer in-flight
    /// item for the same episode is never removed — the replacement logic only
    /// matches non-terminal statuses, so an unfiltered delete-by-episode would
    /// remove the newest item and leave the old terminal one behind.
    ///
    /// Single DELETE (not SELECT-then-DELETE) to avoid the TOCTOU race where
    /// another thread completes a different item for the same episode in between.
    ///
    /// Returns true if a row was deleted.
    pub async fn remove_completed_queue_item(&self, episode_id: &str) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM download_queue WHERE id = (
                 SELECT id FROM download_queue
                 WHERE episode_id = ? AND status IN ('Completed', 'Failed', 'Organizing')
                 ORDER BY downloaded_at DESC
                 LIMIT 1
             )",
        )
        .bind(episode_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Remove 'Completed' queue items whose episode is already organized — items
    /// missed by remove_completed_queue_item (e.g. a crash between setting
    /// 'Organizing' and the success cleanup).
    ///
    /// Only 'Completed': an 'Organizing' item may still be retrying content-path
    /// resolution (deleting it would strand the download), and a 'Failed' item may
    /// still be retryable by the user.
    pub async fn remove_stale_terminal_queue_items(&self) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM download_queue
            WHERE status = 'Completed'
              AND episode_id IN (
                SELECT episode_id FROM episodes WHERE status = 'organized'
              )
            "#,
        )
        .execute(&self.pool)
        .await?;
        let count = result.rows_affected();
        if count > 0 {
            tracing::info!(
                "Cleaned up {} stale 'Completed' queue items (episode already organized)",
                count
            );
        }
        Ok(count)
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

    async fn seed_downloading_item(db: &DbManager, series_id: &str) -> i64 {
        let mapping = jumbie_shared::types::MappingRule {
            target_title: "Progress Show".to_string(),
            series_id: series_id.to_string(),
            ..Default::default()
        };
        db.upsert_series_mapping(series_id, &mapping).await.unwrap();
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO download_queue \
             (media_name, media_link, series_title, series_id, status, downloaded_at) \
             VALUES ('Ep', 'magnet:x', 'Progress Show', ?, 'Downloading', datetime('now')) \
             RETURNING id",
        )
        .bind(series_id)
        .fetch_one(db.get_pool())
        .await
        .unwrap();
        row.0
    }

    async fn fetch(db: &DbManager, id: i64) -> DownloadQueueItem {
        db.get_download_queue()
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.id == id)
            .unwrap()
    }

    #[tokio::test]
    async fn progress_snapshot_records_and_resets() {
        let (db, _tmp) = test_db().await;
        let id = seed_downloading_item(&db, "series-progress").await;
        let at = chrono::Utc::now().naive_utc();

        db.record_download_progress(id, 0.5, at).await.unwrap();
        let item = fetch(&db, id).await;
        assert_eq!(item.last_progress, Some(0.5));
        assert!(item.no_progress_since.is_some());

        db.reset_progress_tracking(id).await.unwrap();
        let item = fetch(&db, id).await;
        assert_eq!(item.last_progress, None);
        assert_eq!(item.no_progress_since, None);
    }

    fn queue_params<'a>(link: &'a str) -> AddToDownloadQueueParams<'a> {
        AddToDownloadQueueParams {
            media_name: "Origin Test",
            media_link: link,
            series_title: "Origin Test",
            series_id: "",
            seasons: &[1],
            episodes: &[1],
            episode_id: None,
            score: 100,
            is_user_requested: true,
            is_season_pack: false,
            category: "",
            multi_targets: None,
            episode_intentions: None,
            source_pub_date: None,
            download_id: "origin_hash",
            scoring_size_bytes: None,
            scoring_seeders: None,
            scoring_episode_count: None,
            submitter: None,
            quality_profile_id: None,
            version: 1,
        }
    }

    /// `is_manual` is set only by the manual entry point; the system-selected path
    /// must never mark a row manual (it drives the badge and skips autoresolve).
    #[tokio::test]
    async fn manual_flag_is_set_only_by_the_manual_entry_point() {
        let (db, _tmp) = test_db().await;

        let auto = db
            .add_to_download_queue(queue_params("magnet:?xt=urn:btih:auto"))
            .await
            .unwrap();
        assert!(!fetch(&db, auto.queue_id().unwrap()).await.is_manual);

        let manual = db
            .add_manual_download_to_queue(queue_params("magnet:?xt=urn:btih:manual"))
            .await
            .unwrap();
        assert!(fetch(&db, manual.queue_id().unwrap()).await.is_manual);
    }
}
