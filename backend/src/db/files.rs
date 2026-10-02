// File tracking is split across three related tables: `episode_files` is the
// single episode↔file association (`main` for a playable file, `auxiliary` for a
// sidecar), `file_paths` stores every known path's identity (inode, mtime, state)
// to detect renames, duplicates, and modifications, and `episodes` owns episode
// metadata only. Content-level data (media_info, origin, retention) lives in
// `file_contents`, keyed by fingerprint, so copies/paths of the same content share
// one row.
use super::DbManager;
use super::fingerprints::{ContentRecord, PathRecord};
use super::{CompletedFileRow, EpisodeMediaInfoRow, EpisodePartRow, FingerprintMeta};
use anyhow::Result;
use sqlx::Row;

pub struct SaveFingerprintParams<'a> {
    pub path: &'a str,
    pub inode: u64,
    pub dev: u64,
    pub size: u64,
    pub mtime: f64,
    pub quick_hash: &'a str,
    pub state: &'a str,
    pub media_info: Option<&'a jumbie_shared::types::MediaInfo>,
}

impl DbManager {
    /// Register `file_path` at `part_number` for `episode_id` (multipart
    /// association). Replaces whatever occupied that part slot.
    pub async fn upsert_episode_part(
        &self,
        episode_id: &str,
        part_number: u32,
        file_path: &str,
        size: Option<i64>,
    ) -> Result<()> {
        Self::upsert_episode_part_tx(
            &mut *self.pool.acquire().await?,
            episode_id,
            part_number,
            file_path,
            size,
        )
        .await
    }

    /// [`Self::upsert_episode_part`] on an open connection.
    pub(crate) async fn upsert_episode_part_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        part_number: u32,
        file_path: &str,
        size: Option<i64>,
    ) -> Result<()> {
        let file_path_id = Self::ensure_path_row_tx(conn, file_path).await?;
        if let Some(size) = size {
            sqlx::query("UPDATE file_paths SET size = ? WHERE id = ?")
                .bind(size)
                .bind(file_path_id)
                .execute(&mut *conn)
                .await?;
        }
        Self::associate_main_file_tx(conn, episode_id, file_path, Some(part_number as i32)).await
    }

    pub async fn get_episode_parts(&self, episode_id: &str) -> Result<Vec<EpisodePartRow>> {
        // Part content (media_info, origin) is content-keyed in `file_contents`, so it
        // lives in exactly one place and is shared by every copy/part with the same
        // fingerprint — episode_files stores only the association and path.
        let rows = self
            .fetch_all_bound(
                "SELECT ef.part_number, fp.file_path, fp.size, fp.fingerprint,
                        fc.media_info, fc.original_path
             FROM episode_files ef
             JOIN file_paths fp ON fp.id = ef.file_path_id
             LEFT JOIN file_contents fc ON fc.fingerprint = fp.fingerprint
             WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number IS NOT NULL
             ORDER BY ef.part_number ASC",
                episode_id.to_string(),
            )
            .await?;
        Ok(rows)
    }

    pub async fn update_episode_part_fingerprint(
        &self,
        episode_id: &str,
        part_number: u32,
        fingerprint: &str,
    ) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE file_paths SET fingerprint = ? WHERE id = \
             (SELECT file_path_id FROM episode_files \
              WHERE episode_id = ? AND kind = 'main' AND part_number = ?)",
            fingerprint,
            episode_id,
            part_number as i64
        )
    }

    pub async fn get_parts_for_series(
        &self,
        episode_ids: &[String],
    ) -> Result<Vec<(String, EpisodePartRow)>> {
        if episode_ids.is_empty() {
            return Ok(vec![]);
        }
        // SQLite chunk limit
        const CHUNK: usize = 500;
        let mut out = Vec::new();
        for chunk in episode_ids.chunks(CHUNK) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "SELECT ef.episode_id, ef.part_number, fp.file_path, fp.size, fp.fingerprint,
                        fc.media_info, fc.original_path
                 FROM episode_files ef
                 JOIN file_paths fp ON fp.id = ef.file_path_id
                 LEFT JOIN file_contents fc ON fc.fingerprint = fp.fingerprint
                 WHERE ef.kind = 'main' AND ef.part_number IS NOT NULL
                   AND ef.episode_id IN ({})
                 ORDER BY ef.episode_id, ef.part_number",
                placeholders
            );
            let mut q = sqlx::query_as::<
                _,
                (
                    String,
                    i32,
                    String,
                    Option<i64>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ),
            >(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for (ep_id, part_number, file_path, size, fingerprint, media_info, original_path) in
                rows
            {
                out.push((
                    ep_id,
                    EpisodePartRow {
                        part_number,
                        file_path,
                        size,
                        fingerprint,
                        media_info,
                        original_path,
                    },
                ));
            }
        }
        Ok(out)
    }

    /// Run by the background Stale File Cleanup task: checks every `episode_files`
    /// main association against disk and removes stale DB references. A non-part
    /// association is only cleaned when unseen for 30+ days (`last_seen_at` tracks
    /// the last scan that found it); a missing part is reaped immediately.
    /// Returns (affected (episode_id, series_id), total_count).
    pub async fn cleanup_missing_files(&self) -> Result<(Vec<(String, String)>, usize)> {
        let mut total: usize = 0;
        let mut affected: Vec<(String, String)> = Vec::new();

        // (file_path, episode_id, series_id, part_number, stale).
        let rows: Vec<(String, String, String, Option<i32>, bool)> = self
            .fetch_all(
                "SELECT DISTINCT fp.file_path, e.episode_id, e.series_id, ef.part_number,
                        (fp.last_seen_at IS NULL OR fp.last_seen_at < datetime('now', '-30 days')) AS stale
                 FROM episode_files ef
                 JOIN file_paths fp ON fp.id = ef.file_path_id
                 JOIN episodes e ON e.episode_id = ef.episode_id
                 WHERE ef.kind = 'main'",
            )
            .await?;

        // Batch-check existence on blocking thread pool to avoid stalling the
        // async runtime with N sequential stat() syscalls.
        let checked: Vec<(String, String, String, Option<i32>, bool, bool)> =
            tokio::task::spawn_blocking(move || {
                rows.into_iter()
                    .map(|(path, ep, sid, part, stale)| {
                        let exists = std::path::Path::new(&path).exists();
                        (path, ep, sid, part, stale, exists)
                    })
                    .collect()
            })
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking for episode stat failed: {}", e))?;

        // A path is stale when it is gone and (it is a part, or it has not been
        // seen for the 30-day grace window).
        let mut stale_paths: Vec<String> = Vec::new();
        for (path, episode_id, series_id, part_number, stale, exists) in &checked {
            if !exists && (part_number.is_some() || *stale) {
                affected.push((episode_id.clone(), series_id.clone()));
                if !stale_paths.contains(path) {
                    stale_paths.push(path.clone());
                }
            }
        }

        for file_path in &stale_paths {
            tracing::debug!(
                "Stale file cleanup: {} missing on disk, disowning its episodes",
                file_path
            );
            self.disown_path(file_path).await?;

            // Delete the file_paths row too, once nothing references it. The
            // content row is left for the retention sweep.
            let _ = sqlx::query(
                "DELETE FROM file_paths
                 WHERE file_path = ?
                   AND NOT EXISTS (SELECT 1 FROM episode_files ef
                                   WHERE ef.file_path_id = file_paths.id)",
            )
            .bind(file_path)
            .execute(&self.pool)
            .await;

            total += 1;
        }

        // Unassigned scan-cache rows whose file is gone. The branches above only
        // cover rows *linked* to an episode; a path scanned while unassigned
        // (a download never adopted, a source file since deleted) has no such link
        // and was otherwise never reaped, so `file_paths` grew without bound. Gate
        // on the same 30-day window as the episode sweep so a transient move
        // (atomic delete+recreate) can't drop a row for a live file.
        let stale_unassigned: Vec<String> = sqlx::query_scalar(
            "SELECT fp.file_path FROM file_paths fp
             WHERE NOT EXISTS (SELECT 1 FROM episode_files ef WHERE ef.file_path_id = fp.id)
               AND (fp.last_seen_at IS NULL OR fp.last_seen_at < datetime('now', '-30 days'))",
        )
        .fetch_all(&self.pool)
        .await?;

        let missing_unassigned: Vec<String> = tokio::task::spawn_blocking(move || {
            stale_unassigned
                .into_iter()
                .filter(|path| !std::path::Path::new(path).exists())
                .collect()
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking for unassigned stat failed: {}", e))?;

        if !missing_unassigned.is_empty() {
            tracing::debug!(
                "Stale file cleanup: found {} unassigned file_paths rows gone from disk, deleting",
                missing_unassigned.len()
            );
            for path in &missing_unassigned {
                let _ = sqlx::query("DELETE FROM file_paths WHERE file_path = ?")
                    .bind(path)
                    .execute(&self.pool)
                    .await;
            }
            total += missing_unassigned.len();
        }

        // Kept-for-review files: drop rows assigned to an episode, and rows whose
        // file has been missing past the same 30-day grace period.
        match self.cleanup_unmatched_files(30).await {
            Ok(n) if n > 0 => {
                tracing::debug!(
                    "Stale file cleanup: removed {} kept-for-review file rows",
                    n
                );
                total += n;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("Stale file cleanup: unmatched-file cleanup failed: {}", e),
        }

        // Content rows whose last path just disappeared are kept for the retention
        // window, then dropped (origin dies with the row).
        match self.prune_orphan_contents().await {
            Ok(n) if n > 0 => {
                tracing::debug!(
                    "Stale file cleanup: removed {} unreferenced content rows",
                    n
                );
                total += n as usize;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("Stale file cleanup: content retention failed: {}", e),
        }

        // release_info is keyed by content hash and is retained exactly as long as
        // the matching `file_contents` row. Unassign/delete never remove it — only
        // the age-gated content retention above does, once the content has aged out.
        match self.prune_orphan_release_info().await {
            Ok(n) if n > 0 => {
                tracing::debug!(
                    "Stale file cleanup: removed {} release_info rows (content aged out)",
                    n
                );
                total += n as usize;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("Stale file cleanup: release_info retention failed: {}", e),
        }

        Ok((affected, total))
    }

    pub async fn get_episode_hash(&self, episode_id: &str) -> Result<Option<String>> {
        // The download client's torrent hash: stored on the queue item at queue
        // time, and on release_info after organize.  Try the queue first
        // (pre-organize), then release_info (post-organize).
        let from_queue: Option<String> = sqlx::query_scalar(
            "SELECT download_id FROM download_queue WHERE episode_id = ? AND download_id IS NOT NULL LIMIT 1",
        )
        .bind(episode_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(hash) = from_queue
            && !hash.is_empty()
        {
            return Ok(Some(hash));
        }

        let from_rm: Option<String> = sqlx::query_scalar(&format!(
            "SELECT rm.download_id FROM file_paths fp \
             LEFT JOIN release_info rm ON rm.quick_hash = fp.fingerprint \
             WHERE {pred} AND rm.download_id IS NOT NULL \
             LIMIT 1",
            pred = crate::db::EPISODE_FILES_PREDICATE,
        ))
        .bind(episode_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(from_rm)
    }

    pub async fn get_episode_quality(&self, _episode_id: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// Returns (score, submitter, version, release_title) for the release
    /// associated with an episode via its file_paths fingerprint.
    /// Returns the defaults (0, None, 1, None) when no release is found.
    pub async fn get_episode_release_info(
        &self,
        episode_id: &str,
    ) -> Result<Option<(i32, Option<String>, i32, Option<String>)>> {
        let row = sqlx::query(&format!(
            "SELECT COALESCE(rm.score, 0), rm.submitter, COALESCE(rm.version, 1), rm.release_title \
             FROM file_paths fp \
             LEFT JOIN release_info rm ON rm.quick_hash = fp.fingerprint \
             WHERE {pred} \
             LIMIT 1",
            pred = crate::db::EPISODE_FILES_PREDICATE,
        ))
        .bind(episode_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            let score: i32 = r.get(0);
            let submitter: Option<String> = r.get(1);
            let version: i32 = r.get(2);
            let release_title: Option<String> = r.get(3);
            (score, submitter, version, release_title)
        }))
    }

    pub async fn get_episode_score(&self, episode_id: &str) -> Result<Option<i32>> {
        let score: Option<i32> = sqlx::query_scalar::<_, i32>(&format!(
            "SELECT rm.score FROM file_paths fp \
             LEFT JOIN release_info rm ON rm.quick_hash = fp.fingerprint \
             WHERE {pred} AND rm.score IS NOT NULL \
             LIMIT 1",
            pred = crate::db::EPISODE_FILES_PREDICATE,
        ))
        .bind(episode_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(score)
    }

    // Fingerprint schemes: (inode, device, mtime, size) is the primary change
    // detector (stat is cheap and catches most renames/copies without hashing);
    // `fingerprint` (xxhash, when available) handles renames where the inode
    // changes. `state` transitions pending → complete → organized, and `last_seen_at`
    // is updated on every scan so deleted-outside-app entries can be pruned.
    pub async fn get_fingerprint_meta(&self, path: &str) -> Result<Option<FingerprintMeta>> {
        let row: Option<FingerprintMeta> = self
            .fetch_optional_bound(
                "SELECT inode, device, mtime, size FROM file_paths WHERE file_path = ?",
                path.to_string(),
            )
            .await?;
        Ok(row)
    }

    /// Save (or update) a path's fingerprint.
    ///
    /// `quick_hash` is the SSoT for content identity: the xxhash when available,
    /// else an identity fallback ("{inode}-{size}-{mtime}"). It is the content key
    /// in `file_contents`; content-dup lookups query it directly.
    pub async fn save_fingerprint(&self, params: SaveFingerprintParams<'_>) -> Result<()> {
        let media_info_json = params
            .media_info
            .map(|info| serde_json::to_string(info).unwrap_or_default());

        self.upsert_content(ContentRecord {
            fingerprint: params.quick_hash,
            media_info: media_info_json.as_deref(),
            path: params.path,
        })
        .await?;
        self.upsert_path(PathRecord {
            file_path: params.path,
            fingerprint: params.quick_hash,
            inode: params.inode as i64,
            device: params.dev as i64,
            size: params.size as i64,
            mtime: params.mtime,
            state: params.state,
            expected_path: None,
        })
        .await?;

        Ok(())
    }

    // Release metadata is normalized into `release_info`, keyed by content hash
    // (quick_hash), so multiple copies of the same file share release info.

    /// Upsert release info by content hash.
    ///
    /// Precedence is deliberately asymmetric:
    ///  - `submitter` is first-write-wins (`COALESCE(submitter, excluded.submitter)`):
    ///    the acquisition's group must not be replaced by a value re-derived from a
    ///    renamed file on a later scan.
    ///  - `release_title` / `download_link` are last-non-NULL-wins, so an
    ///    authoritative later write can refresh them (the release title feeds
    ///    rescoring).
    pub async fn set_release_info(
        &self,
        quick_hash: &str,
        release_title: Option<&str>,
        download_link: Option<&str>,
        submitter: Option<&str>,
    ) -> Result<()> {
        if quick_hash.is_empty() || quick_hash.contains('-') {
            // Identity fallback hashes ("{inode}-{size}-{mtime}") are never valid
            // for content-based dedup.  Skipping is consistent with get_media_info_by_quick_hash.
            return Ok(());
        }
        exec_with_bindings!(
            self,
            "INSERT INTO release_info (quick_hash, release_title, download_link, submitter)
                         VALUES (?, ?, ?, ?)
                         ON CONFLICT(quick_hash) DO UPDATE SET
                         release_title = COALESCE(excluded.release_title, release_title),
                         download_link = COALESCE(excluded.download_link, download_link),
                         submitter = COALESCE(submitter, excluded.submitter)",
            quick_hash,
            release_title,
            download_link,
            submitter
        )
    }

    /// Write release info using a file_path (resolves quick_hash via subquery),
    /// avoiding an extra SELECT round-trip when the caller only has the path. The
    /// subquery filters out identity-fallback hashes (containing dashes) to match
    /// `set_release_info`'s early-return guard. Same asymmetric precedence as
    /// `set_release_info` (see there).
    pub async fn set_release_info_by_path(
        &self,
        file_path: &str,
        release_title: Option<&str>,
        download_link: Option<&str>,
        submitter: Option<&str>,
    ) -> Result<()> {
        exec_with_bindings!(
            self,
            "INSERT INTO release_info (quick_hash, release_title, download_link, submitter)
                         SELECT fp.fingerprint, ?1, ?2, ?3
                         FROM file_paths fp
                         WHERE fp.file_path = ?4
                           AND fp.fingerprint IS NOT NULL
                           AND fp.fingerprint NOT LIKE '%-%-%'
                         ON CONFLICT(quick_hash) DO UPDATE SET
                         release_title = COALESCE(excluded.release_title, release_title),
                         download_link = COALESCE(excluded.download_link, download_link),
                         submitter = COALESCE(excluded.submitter, submitter)",
            release_title,
            download_link,
            submitter,
            file_path
        )
    }

    /// Record the file's date (a scanner mtime fallback) at content level, keyed by
    /// fingerprint. Only fills an EMPTY `release_info.upload_date`: an authoritative
    /// source-feed date (written by organize) always wins. Identity-fallback hashes
    /// are skipped, like `set_release_info_by_path`.
    pub async fn set_release_upload_date_fallback_by_path(
        &self,
        file_path: &str,
        date: Option<chrono::NaiveDateTime>,
    ) -> Result<()> {
        let Some(date) = date else {
            return Ok(());
        };
        exec_with_bindings!(
            self,
            "INSERT INTO release_info (quick_hash, upload_date)
                         SELECT fp.fingerprint, ?1
                         FROM file_paths fp
                         WHERE fp.file_path = ?2
                           AND fp.fingerprint IS NOT NULL
                           AND fp.fingerprint NOT LIKE '%-%-%'
                         ON CONFLICT(quick_hash) DO UPDATE SET
                         upload_date = COALESCE(release_info.upload_date, excluded.upload_date)",
            date,
            file_path
        )
    }

    /// Write the organize-time metadata snapshot to `release_info` (keyed by the
    /// content quick_hash), called by `organize_completed` after the move.
    ///
    /// Only `score` and `version` are authoritative overwrites (organize selected
    /// the release). Every other field is COALESCEd so a partial snapshot cannot
    /// erase attribution, `download_id`, the source feed date, or the scoring
    /// inputs already recorded by `finalize_download`.
    pub async fn upsert_organize_release_meta(
        &self,
        quick_hash: &str,
        meta: &super::OrganizeMetaRow,
        upload_date: Option<chrono::NaiveDateTime>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO release_info (quick_hash, release_title, download_link, submitter, score, download_id, upload_date, scoring_size_bytes, scoring_seeders, scoring_episode_count, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(quick_hash) DO UPDATE SET
             release_title = COALESCE(excluded.release_title, release_info.release_title),
             download_link = COALESCE(excluded.download_link, release_info.download_link),
             submitter = COALESCE(excluded.submitter, release_info.submitter),
             score = excluded.score,
             download_id = COALESCE(excluded.download_id, release_info.download_id),
             upload_date = COALESCE(excluded.upload_date, release_info.upload_date),
             scoring_size_bytes = COALESCE(excluded.scoring_size_bytes, release_info.scoring_size_bytes),
             scoring_seeders = COALESCE(excluded.scoring_seeders, release_info.scoring_seeders),
             scoring_episode_count = COALESCE(excluded.scoring_episode_count, release_info.scoring_episode_count),
             version = excluded.version",
        )
        .bind(quick_hash)
        .bind(&meta.media_name)
        .bind(&meta.media_link)
        .bind(meta.submitter.as_deref())
        .bind(meta.score)
        .bind(meta.download_id.as_deref())
        .bind(upload_date)
        .bind(meta.scoring_size_bytes)
        .bind(meta.scoring_seeders)
        .bind(meta.scoring_episode_count)
        .bind(meta.version)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Drop `release_info` rows whose content row is gone. `release_info` is keyed
    /// by content hash and retained exactly as long as its `file_contents` row:
    /// unassign/delete never remove it — only the age-gated content retention
    /// (or a size cap) does. Returns rows removed.
    pub async fn prune_orphan_release_info(&self) -> Result<u64> {
        let res = sqlx::query(
            "DELETE FROM release_info
             WHERE NOT EXISTS (
                 SELECT 1 FROM file_contents fc WHERE fc.fingerprint = release_info.quick_hash
             )",
        )
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn get_files_without_media_info(&self, limit: i64) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT fp.file_path
             FROM file_paths fp
             INNER JOIN file_contents fc ON fc.fingerprint = fp.fingerprint
             WHERE fc.media_info IS NULL AND fc.media_info_scan_failed = 0
               AND EXISTS (SELECT 1 FROM episode_files ef
                           WHERE ef.file_path_id = fp.id AND ef.kind = 'main')
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn mark_media_info_scan_failed(&self, path: &str) -> Result<()> {
        sqlx::query(
            "UPDATE file_contents SET media_info_scan_failed = 1
             WHERE fingerprint IN (SELECT fingerprint FROM file_paths WHERE file_path = ?)",
        )
        .bind(path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Batch-check which file paths have `media_info_scan_failed` set to 1.
    /// Used by the rename queue's proactive scan to avoid re-submitting files
    /// whose media info extraction permanently failed (e.g. corrupted file).
    pub async fn get_media_info_scan_failed_paths(
        &self,
        paths: &[String],
    ) -> Result<std::collections::HashSet<String>> {
        if paths.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        use std::collections::HashSet;
        const CHUNK: usize = 500;
        let mut failed: HashSet<String> = HashSet::new();
        for chunk in paths.chunks(CHUNK) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "SELECT fp.file_path FROM file_paths fp
                 INNER JOIN file_contents fc ON fc.fingerprint = fp.fingerprint
                 WHERE fc.media_info_scan_failed = 1 AND fp.file_path IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&sql);
            for p in chunk {
                q = q.bind(p);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for row in rows {
                if let Ok(path) = row.try_get::<String, _>("file_path") {
                    failed.insert(path);
                }
            }
        }
        Ok(failed)
    }

    pub async fn update_media_info(
        &self,
        path: &str,
        media_info: Option<&jumbie_shared::types::MediaInfo>,
    ) -> Result<()> {
        let media_info_json =
            media_info.map(|info| serde_json::to_string(info).unwrap_or_default());

        sqlx::query(
            "UPDATE file_contents SET media_info = ?, media_info_scan_failed = 0
             WHERE fingerprint IN (SELECT fingerprint FROM file_paths WHERE file_path = ?)",
        )
        .bind(media_info_json.as_deref())
        .bind(path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_all_episodes_with_media_info(&self) -> Result<Vec<EpisodeMediaInfoRow>> {
        let sql = format!(
            "SELECT f.file_path, rm.submitter, rm.download_id AS download_id, \
             c.fingerprint AS quick_hash, c.media_info \
             FROM episodes e {join} \
             WHERE c.media_info IS NOT NULL AND rm.submitter IS NOT NULL",
            join = crate::db::EPISODE_FILE_JOIN,
        );
        let rows = sqlx::query_as::<_, EpisodeMediaInfoRow>(&sql)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    pub async fn delete_fingerprint(&self, path: &str) -> Result<()> {
        sqlx::query("DELETE FROM file_paths WHERE file_path = ?")
            .bind(path)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_file_state(&self, path: &str) -> Result<Option<String>> {
        let state: Option<String> = self
            .fetch_scalar_bound(
                "SELECT state FROM file_paths WHERE file_path = ?",
                path.to_string(),
            )
            .await?;
        Ok(state)
    }

    /// Point `episode_id` at `new_path` as its primary main file and mark it
    /// `organized`. Replaces any existing non-part main file.
    pub async fn update_file_path(&self, episode_id: &str, new_path: &str) -> Result<()> {
        sqlx::query(
            "UPDATE episodes SET status = 'organized', \
             file_acquired_at = COALESCE(file_acquired_at, CURRENT_TIMESTAMP) \
             WHERE episode_id = ?",
        )
        .bind(episode_id)
        .execute(&self.pool)
        .await?;
        self.associate_main_file(episode_id, new_path, None).await
    }

    /// Returns (episode_id, season, episode, title) for all episodes sharing the same
    /// file_path, excluding the specified episode_id, that are not yet organized.
    /// Used by the sibling-linking logic to hard-link a season pack file to every
    /// episode that references it before the source is removed.
    pub async fn get_episodes_by_source_path(
        &self,
        file_path: &str,
        exclude_episode_id: &str,
    ) -> Result<
        Vec<(
            String,
            Option<String>,
            i32,
            Option<String>,
            Option<String>,
            Option<String>,
        )>,
    > {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                i32,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT e.episode_id, e.season, e.episode, e.title, rm.submitter
             FROM episodes e
             JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main'
             JOIN file_paths fp ON fp.id = ef.file_path_id
             LEFT JOIN release_info rm ON rm.quick_hash = fp.fingerprint
             WHERE fp.file_path = ?
             AND e.episode_id != ?
             AND (e.status IS NULL OR e.status != 'organized')",
        )
        .bind(file_path)
        .bind(exclude_episode_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_completed_files(&self) -> Result<Vec<CompletedFileRow>> {
        // Returns normal unorganized downloads, plus the race case where the scanner
        // claimed the episode with a DIFFERENT file between download completion and
        // organize_completed (episode_file_path != the fingerprint's file_path). The
        // caller detects that mismatch and cleans up instead of moving the file.
        // Episodes whose primary file already matches are excluded to avoid a duplicate move.
        let sql = format!(
            "SELECT f.file_path, e.episode_id, rm.download_id AS download_id, \
                    pf.file_path AS episode_file_path, e.status AS episode_status, \
                    COALESCE(rm.score, 0) AS episode_score, COALESCE(e.series_id, '') AS series_id \
             FROM file_paths f \
             JOIN episode_files ef ON ef.file_path_id = f.id AND ef.kind IN ('main', 'linked') \
             JOIN episodes e ON e.episode_id = ef.episode_id \
             LEFT JOIN file_paths pf ON pf.id = {primary} \
             LEFT JOIN release_info rm ON rm.quick_hash = f.fingerprint \
             WHERE (f.state = 'complete' OR (f.state = 'pending' AND f.size > 0)) \
               AND (e.status != 'organized' OR (e.status = 'organized' \
                    AND (pf.id IS NULL OR pf.id != f.id)))",
            primary = crate::db::EPISODE_PRIMARY_FILE_ID,
        );
        let rows = sqlx::query_as::<_, CompletedFileRow>(&sql)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    /// Returns (file_path, episode_id) for all files in the `_unmatched` subfolder
    /// of a given series, identified by episode_id prefix (e.g. `"{series_id}_"`,
    /// where `series_id` is the UUID used to build episode IDs).
    pub async fn get_unmatched_fingerprints_for_series(
        &self,
        episode_id_prefix: &str,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT fp.file_path, ef.episode_id
             FROM file_paths fp
             JOIN episode_files ef ON ef.file_path_id = fp.id
             WHERE ef.episode_id LIKE ? AND replace(fp.file_path, '\\', '/') LIKE '%/_unmatched/%'",
        )
        .bind(format!("{}%", episode_id_prefix))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Updates the file_path of a fingerprint record (atomic; file_path has a unique index).
    pub async fn move_fingerprint_path(
        &self,
        old_path: &str,
        new_path: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE file_paths SET file_path = ? WHERE file_path = ?")
            .bind(new_path)
            .bind(old_path)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Look up media info for any fingerprint whose quick_hash (xxhash) matches and
    /// whose media_info is not NULL — lets a copy of an already-fingerprinted file
    /// (new inode, identical content) skip ffprobe. `quick_hash` is the SSoT for
    /// content identity; there is no separate content_hash column.
    ///
    /// Returns the media_info JSON string if a match is found.
    pub async fn get_media_info_by_quick_hash(&self, quick_hash: &str) -> Result<Option<String>> {
        if quick_hash.is_empty() || quick_hash.contains('-') {
            // Identity fallback hashes contain dashes (e.g. "{inode}-{size}-{mtime}")
            // and are never valid for content-based dedup since they encode the
            // file's path identity, not its content.
            return Ok(None);
        }
        let media_info: Option<String> = sqlx::query_scalar(
            "SELECT media_info FROM file_contents \
             WHERE fingerprint = ? AND media_info IS NOT NULL \
             LIMIT 1",
        )
        .bind(quick_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(media_info)
    }

    /// True when `file_path` has a tracked, unchanged fingerprint **with media
    /// info**. Used to skip re-scanning a file whose identity (inode/device/size/
    /// mtime) still matches the stored row. Delegates to the SSoT identity check.
    pub async fn has_existing_fingerprint(&self, file_path: &str) -> Result<bool> {
        Ok(self
            .stored_fingerprint_if_unchanged(std::path::Path::new(file_path), true)
            .await
            .is_some())
    }

    /// Calculates hash, extracts media info, and saves fingerprint to DB.
    /// Returns (xxhash, final_hash_fallback, media_info).
    ///
    /// Dedup: if the computed xxhash matches an existing row that already has
    /// media_info, ffprobe is skipped (the media info is reused for a bit-identical
    /// copy with a new inode).
    pub async fn update_file_fingerprint(
        &self,
        path: &std::path::Path,
        status: &str,
    ) -> (String, String, Option<jumbie_shared::types::MediaInfo>) {
        let metadata = match tokio::fs::metadata(path).await {
            Ok(m) => m,
            Err(_) => return (String::new(), String::new(), None),
        };
        let (inode, dev, mtime) = crate::platform::file_identity(&metadata);
        let size = metadata.len();

        let path_str = path.to_string_lossy().to_string();

        // Fast path: reuse the existing fingerprint when the file is unchanged
        // (same identity and media_info) — the common case in organize_completed.
        // Delegates to the SSoT identity check.
        if let Some((fingerprint, Some(media_info_json))) =
            self.stored_fingerprint_if_unchanged(path, true).await
            && let Ok(media_info) =
                serde_json::from_str::<jumbie_shared::types::MediaInfo>(&media_info_json)
        {
            let _ = self
                .save_fingerprint(SaveFingerprintParams {
                    path: &path_str,
                    inode,
                    dev,
                    size,
                    mtime,
                    quick_hash: &fingerprint,
                    state: status,
                    media_info: Some(&media_info),
                })
                .await;
            // Identity-fallback keys (with dashes) are not content hashes.
            let hash_val = if fingerprint.contains('-') {
                String::new()
            } else {
                fingerprint.clone()
            };
            return (hash_val, fingerprint, Some(media_info));
        }

        let hash_val = crate::utils::media_info::calculate_xxhash(path)
            .await
            .unwrap_or_default();

        // Quick-hash dedup: a copied file gets a new inode but the same xxhash, so
        // reuse the original row's media_info instead of re-extracting.
        let hash = if !hash_val.is_empty() {
            hash_val.clone()
        } else {
            format!("{}-{}-{}", inode, size, mtime)
        };

        let media_info = if !hash_val.is_empty() {
            match self.get_media_info_by_quick_hash(&hash).await {
                Ok(Some(json_str)) => serde_json::from_str(&json_str).ok(),
                _ => None,
            }
        } else {
            None
        };

        // Only fall back to ffprobe if no cached media_info was found.
        let media_info = if media_info.is_some() {
            media_info
        } else {
            crate::utils::media_info::extract_media_info(path)
                .await
                .ok()
        };

        let _ = self
            .save_fingerprint(SaveFingerprintParams {
                path: &path_str,
                inode,
                dev,
                size,
                mtime,
                quick_hash: &hash,
                state: status,
                media_info: media_info.as_ref(),
            })
            .await;

        (hash_val, hash, media_info)
    }

    /// SSoT: scan a single file for fingerprints + media info, marking it as
    /// permanently failed if extraction fails. Shared by the proactive scanner
    /// (before rename-queue computation) and the background scanner.
    ///
    /// Returns (xxhash, final_hash_fallback, media_info).
    pub async fn scan_file_fingerprint(
        &self,
        path: &std::path::Path,
        status: &str,
    ) -> (String, String, Option<jumbie_shared::types::MediaInfo>) {
        let (hash_val, hash, media_info) = self.update_file_fingerprint(path, status).await;
        if media_info.is_none() {
            let _ = self
                .mark_media_info_scan_failed(&path.to_string_lossy())
                .await;
        }
        (hash_val, hash, media_info)
    }

    /// Hash-first: compute the content fingerprint (xxhash) for `path` and record
    /// content + path without running ffprobe, so auto-assignment can key on the
    /// fingerprint. Returns the content fingerprint (identity fallback when the
    /// content hash is unavailable). Media info is filled later by
    /// [`Self::scan_media_info_only`].
    pub async fn record_hash_only(&self, path: &std::path::Path, state: &str) -> String {
        let hash = crate::utils::media_info::calculate_xxhash(path)
            .await
            .unwrap_or_default();
        let path_str = path.to_string_lossy().to_string();

        let (inode, dev, mtime, size) = match std::fs::metadata(path) {
            Ok(m) => {
                let (i, d, mt) = crate::platform::file_identity(&m);
                (i, d, mt, m.len())
            }
            Err(_) => return hash,
        };

        let _ = self
            .upsert_content(ContentRecord {
                fingerprint: &hash,
                media_info: None,
                path: &path_str,
            })
            .await;
        let _ = self
            .upsert_path(PathRecord {
                file_path: &path_str,
                fingerprint: &hash,
                inode: inode as i64,
                device: dev as i64,
                size: size as i64,
                mtime,
                state,
                expected_path: None,
            })
            .await;

        crate::db::fingerprints::content_key(&hash, &path_str).into_owned()
    }

    /// Extract and store ffprobe media info for an already-fingerprinted path,
    /// reusing cached info from another copy of the same content when available.
    /// Returns `(fingerprint, media_info)`.
    pub async fn scan_media_info_only(
        &self,
        path: &std::path::Path,
    ) -> (String, Option<jumbie_shared::types::MediaInfo>) {
        let path_str = path.to_string_lossy().to_string();

        // Resolve the fingerprint that actually describes the file now at `path`.
        // `file_paths.fingerprint` is keyed by path and is only trustworthy while the
        // file's identity is unchanged; if the row no longer matches the file on disk
        // (an in-place replacement, or a file moved onto a path another content used
        // to occupy), re-hash so path-keyed readers resolve the current content
        // instead of re-stamping the previous occupant's fingerprint.
        let fingerprint = match self.stored_fingerprint_if_unchanged(path, false).await {
            Some((fp, _)) => fp,
            None => self.record_hash_only(path, "organized").await,
        };
        if fingerprint.is_empty() {
            return (fingerprint, None);
        }

        if let Ok(Some(json)) = self.get_media_info_by_quick_hash(&fingerprint).await
            && let Ok(info) = serde_json::from_str::<jumbie_shared::types::MediaInfo>(&json)
        {
            let _ = self.update_media_info(&path_str, Some(&info)).await;
            return (fingerprint, Some(info));
        }

        match crate::utils::media_info::extract_media_info(path).await {
            Ok(info) => {
                let _ = self.update_media_info(&path_str, Some(&info)).await;
                (fingerprint, Some(info))
            }
            Err(_) => {
                let _ = self.mark_media_info_scan_failed(&path_str).await;
                (fingerprint, None)
            }
        }
    }

    /// The stored fingerprint for `path`, but only when the file on disk still
    /// matches the stored identity (inode, device, size, mtime). `None` means the
    /// row is missing or stale, so the caller must (re)hash before trusting a
    /// path→fingerprint lookup.
    ///
    /// When `require_media_info` is set, the row's content must also have scanned
    /// media info (callers that skip ffprobe only when it is available); the returned
    /// tuple's second element is that media-info JSON.
    ///
    /// SSoT for "is this path's fingerprint still valid" — shared by the media-info
    /// scan (`scan_media_info_only`), the rescan re-hash decision (`ensure_hashed`),
    /// `has_existing_fingerprint`, and `update_file_fingerprint`.
    pub(crate) async fn stored_fingerprint_if_unchanged(
        &self,
        path: &std::path::Path,
        require_media_info: bool,
    ) -> Option<(String, Option<String>)> {
        let metadata = std::fs::metadata(path).ok()?;
        let (inode, dev, mtime) = crate::platform::file_identity(&metadata);
        let size = metadata.len();

        let row: Option<(String, i64, i64, f64, i64, Option<String>)> = sqlx::query_as(
            "SELECT fp.fingerprint, fp.inode, fp.device, fp.mtime, fp.size, fc.media_info \
             FROM file_paths fp \
             LEFT JOIN file_contents fc ON fc.fingerprint = fp.fingerprint \
             WHERE fp.file_path = ?",
        )
        .bind(path.to_string_lossy().to_string())
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        let (fingerprint, stored_inode, stored_dev, stored_mtime, stored_size, media_info) = row?;
        let unchanged = stored_inode == inode as i64
            && stored_dev == dev as i64
            && (stored_mtime - mtime).abs() < 0.001
            && stored_size == size as i64;
        if !unchanged {
            return None;
        }
        if require_media_info && media_info.is_none() {
            return None;
        }
        Some((fingerprint, media_info))
    }

    /// After a series directory is moved on disk, update all DB paths that pointed
    /// inside the old directory so they reflect the new location. Files whose
    /// `file_path` was outside the old directory are left untouched.
    ///
    /// Associations are keyed by `file_paths.id`, so a move re-points them to the
    /// renamed row instead of recreating it — no association is ever dropped by a
    /// move.
    ///
    /// # Tables updated (all in one transaction):
    /// - `file_paths.file_path` / `expected_path`
    /// - `episode_files.file_path_id` (re-pointed to the renamed rows)
    /// - `retry_queue.source_path`, `retry_queue.destination_path`
    pub async fn update_series_paths(
        &self,
        old_prefix: &std::path::Path,
        new_prefix: &std::path::Path,
    ) -> Result<()> {
        // A move to the same location is a no-op (rewriting and deleting the same
        // rows would otherwise wipe them).
        if old_prefix == new_prefix {
            return Ok(());
        }

        let old_str = old_prefix.to_string_lossy();
        let new_str = new_prefix.to_string_lossy();

        // Path-safe: only match files physically inside old_prefix (with separator).
        // This avoids matching paths like "/data/tv/My Showcase/S01/ep.mkv" when
        // old_prefix is "/data/tv/My Show".
        let old_exact = super::normalize_sql_path_prefix(&old_str);
        let old_like = super::sql_child_path_like_pattern(&old_str);
        let new_exact = super::normalize_sql_path_prefix(&new_str);
        let new_like = super::sql_child_path_like_pattern(&new_str);

        // Number of characters to skip when computing the path suffix.
        let prefix_len = old_str.len() as i32;

        with_transaction!(self.pool, |mut tx| {
            // Stale rows left at the target path by a prior scan carry content that is
            // no longer there; drop the unassociated ones so the moved (authoritative)
            // row wins. A target row an episode still references is kept and shared.
            sqlx::query(
                "DELETE FROM file_paths
                 WHERE (replace(file_path, '\\', '/') = ?1 OR replace(file_path, '\\', '/') LIKE ?2)
                   AND NOT EXISTS (SELECT 1 FROM episode_files ef
                                   WHERE ef.file_path_id = file_paths.id)",
            )
            .bind(&new_exact)
            .bind(&new_like)
            .execute(&mut *tx)
            .await?;

            // Create the renamed rows first (id-stable when the target path is new). A
            // collision (two old paths → one new) maps both onto the existing target
            // row, which the re-point below then shares.
            sqlx::query(
                "INSERT OR IGNORE INTO file_paths
                 (file_path, fingerprint, inode, device, size, mtime,
                  state, expected_path, last_seen_at)
                 SELECT
                   ?1 || SUBSTR(fp.file_path, ?2),
                   fp.fingerprint, fp.inode, fp.device, fp.size, fp.mtime,
                   fp.state,
                   CASE WHEN fp.expected_path IS NOT NULL
                     THEN ?1 || SUBSTR(fp.expected_path, ?2)
                     ELSE NULL
                   END,
                   fp.last_seen_at
                 FROM file_paths fp
                 WHERE replace(fp.file_path, '\\', '/') = ?3 OR replace(fp.file_path, '\\', '/') LIKE ?4",
            )
            .bind(new_str.as_ref())
            .bind(prefix_len + 1) // SUBSTR is 1-indexed; skip past old_prefix + separator
            .bind(&old_exact)
            .bind(&old_like)
            .execute(&mut *tx)
            .await?;

            // Re-point every association on a moved row at the renamed row.
            sqlx::query(
                "UPDATE episode_files
                 SET file_path_id = (
                     SELECT n.id FROM file_paths n
                     WHERE n.file_path = ?1 || SUBSTR(
                         (SELECT o.file_path FROM file_paths o
                          WHERE o.id = episode_files.file_path_id), ?2))
                 WHERE file_path_id IN (
                     SELECT o.id FROM file_paths o
                     WHERE replace(o.file_path, '\\', '/') = ?3 OR replace(o.file_path, '\\', '/') LIKE ?4)",
            )
            .bind(new_str.as_ref())
            .bind(prefix_len + 1)
            .bind(&old_exact)
            .bind(&old_like)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "DELETE FROM file_paths
                 WHERE replace(file_path, '\\', '/') = ?1 OR replace(file_path, '\\', '/') LIKE ?2",
            )
            .bind(&old_exact)
            .bind(&old_like)
            .execute(&mut *tx)
            .await?;

            // retry_queue.source_path / destination_path
            sqlx::query(
                "UPDATE retry_queue
                 SET source_path = ?1 || SUBSTR(source_path, ?3)
                 WHERE replace(source_path, '\\', '/') = ?4 OR replace(source_path, '\\', '/') LIKE ?2",
            )
            .bind(new_str.as_ref())
            .bind(&old_like)
            .bind(prefix_len + 1)
            .bind(&old_exact)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "UPDATE retry_queue
                 SET destination_path = ?1 || SUBSTR(destination_path, ?3)
                 WHERE destination_path IS NOT NULL
                   AND (replace(destination_path, '\\', '/') = ?4 OR replace(destination_path, '\\', '/') LIKE ?2)",
            )
            .bind(new_str.as_ref())
            .bind(&old_like)
            .bind(prefix_len + 1)
            .bind(&old_exact)
            .execute(&mut *tx)
            .await?;

            Ok(())
        })
    }

    /// Clear paths pointing inside the old series directory when files are deleted
    /// or left in place (DoNothing) — unlike `update_series_paths`, paths are not
    /// rewritten to the new location.
    ///
    /// # Tables updated (all in one transaction):
    /// - `episode_files` — episodes that lose their only main file are disowned
    /// - `file_paths`, `retry_queue` — rows with a path under `old_prefix` are deleted
    pub async fn clear_series_paths(&self, old_prefix: &std::path::Path) -> Result<()> {
        let old_str = old_prefix.to_string_lossy();
        let old_exact = super::normalize_sql_path_prefix(&old_str);
        let old_like = super::sql_child_path_like_pattern(&old_str);

        with_transaction!(self.pool, |mut tx| {
            // Episodes associated with a path under the prefix — captured before the
            // delete so ones left empty can be disowned consistently.
            let episode_ids: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT ef.episode_id FROM episode_files ef
                 JOIN file_paths fp ON fp.id = ef.file_path_id
                 WHERE replace(fp.file_path, '\\', '/') = ?1 OR replace(fp.file_path, '\\', '/') LIKE ?2",
            )
            .bind(&old_exact)
            .bind(&old_like)
            .fetch_all(&mut *tx)
            .await?;

            // Deleting the paths cascades their `episode_files` rows (FK).
            sqlx::query(
                "DELETE FROM file_paths
                 WHERE replace(file_path, '\\', '/') = ?1 OR replace(file_path, '\\', '/') LIKE ?2",
            )
            .bind(&old_exact)
            .bind(&old_like)
            .execute(&mut *tx)
            .await?;

            // Disown episodes left with no main file (their only file was under the
            // prefix).
            if !episode_ids.is_empty() {
                let ph = crate::db::sql_in_placeholders(episode_ids.len());
                let sql = format!(
                    "SELECT e.episode_id FROM episodes e WHERE e.episode_id IN ({ph}) \
                     AND NOT EXISTS (SELECT 1 FROM episode_files ef \
                                     WHERE ef.episode_id = e.episode_id AND ef.kind = 'main')"
                );
                let mut q = sqlx::query_scalar::<_, String>(&sql);
                for id in &episode_ids {
                    q = q.bind(id);
                }
                let emptied = q.fetch_all(&mut *tx).await?;
                crate::db::ownership::disown_episodes_tx(&mut tx, &emptied).await?;
            }

            // retry_queue — column is NOT NULL and the entry is meaningless without a path.
            sqlx::query(
                "DELETE FROM retry_queue
                 WHERE replace(source_path, '\\', '/') = ?1 OR replace(source_path, '\\', '/') LIKE ?2",
            )
            .bind(&old_exact)
            .bind(&old_like)
            .execute(&mut *tx)
            .await?;

            Ok(())
        })
    }

    /// Clears the primary file association for downloaded episodes whose file no
    /// longer exists on disk.
    ///
    /// Returns the number of episodes that were cleared.
    ///
    /// # Tables updated (all in one transaction):
    /// - `episode_files` — the affected episodes are disowned (all their associations)
    /// - `file_paths` — the gone path rows are deleted
    pub async fn clear_not_found_episode_files(
        &self,
        series_id: &str,
    ) -> Result<(Vec<String>, usize)> {
        #[derive(sqlx::FromRow)]
        struct EpisodePath {
            episode_id: String,
            file_path: String,
        }
        let episode_paths: Vec<EpisodePath> = sqlx::query_as(
            "SELECT DISTINCT e.episode_id, fp.file_path
             FROM episodes e
             JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main'
             JOIN file_paths fp ON fp.id = ef.file_path_id
             WHERE e.series_id = ? AND ef.part_number IS NULL",
        )
        .bind(series_id)
        .fetch_all(&self.pool)
        .await?;

        let mut to_clear: Vec<String> = Vec::new();
        for row in &episode_paths {
            if !std::path::Path::new(&row.file_path).exists() {
                to_clear.push(row.episode_id.clone());
            }
        }

        if to_clear.is_empty() {
            return Ok((vec![], 0));
        }

        let count = to_clear.len();
        let affected = to_clear.clone();

        with_transaction!(self.pool, |mut tx| {
            // Canonical disowned state for every episode whose file is gone,
            // including its part rows.
            crate::db::ownership::disown_episodes_tx(&mut tx, &to_clear).await?;

            for row in &episode_paths {
                if !to_clear.contains(&row.episode_id) {
                    continue;
                }
                sqlx::query("DELETE FROM file_paths WHERE file_path = ?")
                    .bind(&row.file_path)
                    .execute(&mut *tx)
                    .await?;
            }

            Ok::<_, anyhow::Error>(())
        })?;

        Ok((affected, count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fingerprints::{ContentRecord, PathRecord};

    async fn test_db() -> (DbManager, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();
        (db, tmp)
    }

    async fn upsert(db: &DbManager, file_path: &str, fingerprint: &str) {
        db.upsert_path(PathRecord {
            file_path,
            fingerprint,
            inode: 0,
            device: 0,
            size: 1,
            mtime: 0.0,
            state: "partial",
            expected_path: None,
        })
        .await
        .unwrap();
    }

    async fn has_row(db: &DbManager, path: &str) -> bool {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM file_paths WHERE file_path = ?")
            .bind(path)
            .fetch_one(db.get_pool())
            .await
            .unwrap();
        n > 0
    }

    #[tokio::test]
    async fn cleanup_missing_files_reaps_unassigned_rows_gone_from_disk() {
        let (db, tmp) = test_db().await;

        // Unassigned path whose file still exists -> kept, even when unseen.
        let live = tmp.path().join("live.part");
        std::fs::write(&live, b"x").unwrap();
        let live_str = live.to_string_lossy().to_string();
        upsert(&db, &live_str, "hl").await;

        // Unassigned path whose file is gone -> reaped.
        let gone_str = tmp.path().join("gone.part").to_string_lossy().to_string();
        upsert(&db, &gone_str, "hg").await;

        // Both past the 30-day grace so only disk existence decides.
        sqlx::query("UPDATE file_paths SET last_seen_at = datetime('now','-60 days')")
            .execute(db.get_pool())
            .await
            .unwrap();

        let (affected, count) = db.cleanup_missing_files().await.unwrap();
        assert!(affected.is_empty(), "cache reaping affects no episode");
        assert_eq!(count, 1);
        assert!(has_row(&db, &live_str).await, "on-disk path must be kept");
        assert!(
            !has_row(&db, &gone_str).await,
            "missing path must be reaped"
        );
    }

    #[tokio::test]
    async fn cleanup_missing_files_keeps_recent_unassigned_rows() {
        let (db, tmp) = test_db().await;

        // Missing file but seen just now -> within the grace window, kept.
        let gone_str = tmp.path().join("gone.part").to_string_lossy().to_string();
        upsert(&db, &gone_str, "h").await;

        let (_, count) = db.cleanup_missing_files().await.unwrap();
        assert_eq!(count, 0);
        assert!(has_row(&db, &gone_str).await);
    }

    async fn release_info_count(db: &DbManager, quick_hash: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM release_info WHERE quick_hash = ?")
            .bind(quick_hash)
            .fetch_one(db.get_pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn release_info_survives_delete_until_its_content_ages_out() {
        let (db, _tmp) = test_db().await;

        // Content + release_info but no live path row — exactly the state left by
        // an unassign/delete. release_info must NOT be removed by that act.
        db.upsert_content(ContentRecord {
            fingerprint: "h1",
            media_info: None,
            path: "/lib/E01.mkv",
        })
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO release_info (quick_hash, release_title, upload_date) \
             VALUES ('h1', 'Show S01E01', '2026-06-18 20:00:00')",
        )
        .execute(db.get_pool())
        .await
        .unwrap();

        // Recent content → release_info preserved.
        db.cleanup_missing_files().await.unwrap();
        assert_eq!(
            release_info_count(&db, "h1").await,
            1,
            "release_info must survive unassign/delete while its content is live"
        );

        // Once the content ages out (the only age-based eviction), release_info goes too.
        sqlx::query(
            "UPDATE file_contents SET last_seen_at = datetime('now','-60 days') WHERE fingerprint = 'h1'",
        )
        .execute(db.get_pool())
        .await
        .unwrap();
        db.cleanup_missing_files().await.unwrap();
        assert_eq!(
            release_info_count(&db, "h1").await,
            0,
            "release_info clears with its aged-out content row"
        );
    }

    #[tokio::test]
    async fn get_unmatched_fingerprints_matches_windows_and_unix_paths() {
        let (db, _tmp) = test_db().await;
        let meta = std::collections::HashMap::new();
        db.insert_episode(crate::db::episodes::InsertEpisodeParams::dummy(
            "series1_ABS0001",
            "series1",
            1,
            1,
            &meta,
        ))
        .await
        .unwrap();
        db.insert_episode(crate::db::episodes::InsertEpisodeParams::dummy(
            "series1_ABS0002",
            "series1",
            1,
            2,
            &meta,
        ))
        .await
        .unwrap();

        // One Windows backslash path, one Unix forward slash path
        let win_path = r"C:\Media\Show\_unmatched\Some.Release.ABS0001.mkv";
        let unix_path = "/media/show/_unmatched/Some.Release.ABS0002.mkv";

        db.associate_main_file("series1_ABS0001", win_path, None)
            .await
            .unwrap();
        db.associate_main_file("series1_ABS0002", unix_path, None)
            .await
            .unwrap();

        let rows = db
            .get_unmatched_fingerprints_for_series("series1_")
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|(p, id)| p == win_path && id == "series1_ABS0001"));
        assert!(rows.iter().any(|(p, id)| p == unix_path && id == "series1_ABS0002"));
    }
}

