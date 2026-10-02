// Content-keyed file tracking. `file_contents` is the SSoT for a piece of
// content (keyed by `fingerprint`: xxhash, or the identity-fallback hash) and
// `file_paths` holds every path that content has been seen at. One content row
// can back many paths/episodes.
use super::DbManager;
use anyhow::Result;
use std::borrow::Cow;

/// How long a content row is kept after the last scan saw any of its paths.
pub const CONTENT_RETENTION_DAYS: i64 = 30;

/// Size budget for `file_paths` (the per-path scan cache), in estimated row
/// bytes. A *backstop*, not the primary bound: normal growth is capped by the
/// disk-existence sweep in `cleanup_missing_files`. It exists only so a large
/// set of live-but-unassigned rows cannot inflate the DB without limit. A code
/// constant by design — scan-cache budget/age eviction is internal and not
/// user-tunable.
pub const FILE_PATHS_BUDGET_BYTES: u64 = 16 * 1024 * 1024;

/// Per-row byte overhead folded into [`DbManager::file_paths_bytes`]: the numeric
/// columns, SQLite record/btree overhead, and a share of each index entry.
const FILE_PATHS_ROW_OVERHEAD: i64 = 128;

/// Rows deleted per iteration while pruning an over-budget `file_paths`.
const FILE_PATHS_PRUNE_BATCH: i64 = 500;

/// SQL predicate selecting rows the size-cap prune may delete: rows with no
/// `episode_files` association, that are not kept for review in `unmatched_files`
/// (the durable record for `unmatched`/`unneeded` leftovers), and that are NOT
/// auto-adoption candidates (mirroring `get_orphan_files`). Both protected classes
/// are skipped so the cap can never hide a live file — neither from auto-adoption
/// nor from the kept-for-review list.
const FILE_PATHS_EVICTABLE: &str =
    "NOT EXISTS (SELECT 1 FROM episode_files ef WHERE ef.file_path_id = file_paths.id)
     AND NOT EXISTS (SELECT 1 FROM unmatched_files u WHERE u.file_path = file_paths.file_path)
     AND NOT (
         file_paths.state = 'complete'
         OR (file_paths.state = 'pending' AND file_paths.size > 0)
     )";

/// Content key: the fingerprint when present, else a path-scoped synthetic key so
/// a hashless file still gets a stable row. Mirrors the legacy backfill rule.
pub(crate) fn content_key<'a>(fingerprint: &'a str, path: &'a str) -> Cow<'a, str> {
    if fingerprint.is_empty() {
        Cow::Owned(format!("path:{}", path))
    } else {
        Cow::Borrowed(fingerprint)
    }
}

pub struct ContentRecord<'a> {
    pub fingerprint: &'a str,
    pub media_info: Option<&'a str>,
    /// Path this content was seen at; used only on first insert as the origin.
    pub path: &'a str,
}

pub struct PathRecord<'a> {
    pub file_path: &'a str,
    pub fingerprint: &'a str,
    pub inode: i64,
    pub device: i64,
    pub size: i64,
    pub mtime: f64,
    pub state: &'a str,
    pub expected_path: Option<&'a str>,
}

impl DbManager {
    /// Record content seen at `rec.path`. `original_path` and `first_seen_at` are
    /// written on first insert only, so the origin is stable for the content's
    /// lifetime no matter where it later moves or which episode claims it.
    pub async fn upsert_content(&self, rec: ContentRecord<'_>) -> Result<()> {
        let key = content_key(rec.fingerprint, rec.path);
        exec_with_bindings!(
            self,
            "INSERT INTO file_contents
                (fingerprint, media_info, media_info_scan_failed, original_path, first_seen_at, last_seen_at)
             VALUES (?, ?, 0, ?, datetime('now'), datetime('now'))
             ON CONFLICT(fingerprint) DO UPDATE SET
                media_info = COALESCE(excluded.media_info, media_info),
                media_info_scan_failed = CASE
                    WHEN excluded.media_info IS NOT NULL THEN 0
                    ELSE media_info_scan_failed
                END,
                last_seen_at = excluded.last_seen_at",
            key.as_ref(),
            rec.media_info,
            rec.path
        )
    }

    /// Record a path instance. `episode_id` is left untouched on conflict so a
    /// rescan does not drop an existing link; link/unlink manage it explicitly.
    pub async fn upsert_path(&self, rec: PathRecord<'_>) -> Result<()> {
        let key = content_key(rec.fingerprint, rec.file_path);
        exec_with_bindings!(
            self,
            "INSERT INTO file_paths
                (file_path, fingerprint, inode, device, size, mtime, state, expected_path, last_seen_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
             ON CONFLICT(file_path) DO UPDATE SET
                fingerprint = excluded.fingerprint,
                inode = excluded.inode,
                device = excluded.device,
                size = excluded.size,
                mtime = excluded.mtime,
                state = excluded.state,
                expected_path = COALESCE(excluded.expected_path, expected_path),
                last_seen_at = excluded.last_seen_at",
            rec.file_path,
            key.as_ref(),
            rec.inode,
            rec.device,
            rec.size,
            rec.mtime,
            rec.state,
            rec.expected_path
        )
    }

    pub async fn resolve_original_path(&self, fingerprint: &str) -> Result<Option<String>> {
        let row: Option<Option<String>> =
            sqlx::query_scalar("SELECT original_path FROM file_contents WHERE fingerprint = ?")
                .bind(fingerprint)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.flatten())
    }

    /// Drop content rows no path references any more, once unseen for
    /// `CONTENT_RETENTION_DAYS`. A row with any live path instance is never
    /// removed, so pruning one stale path cannot drop shared content.
    pub async fn prune_orphan_contents(&self) -> Result<u64> {
        let window = format!("-{} days", CONTENT_RETENTION_DAYS);

        let orphan_condition = "(last_seen_at IS NULL OR last_seen_at < datetime('now', ?))
               AND NOT EXISTS (
                   SELECT 1 FROM file_paths fp
                   WHERE fp.fingerprint = file_contents.fingerprint
               )";

        let result = sqlx::query(&format!(
            "DELETE FROM file_contents WHERE {orphan_condition}"
        ))
        .bind(&window)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Estimated footprint of `file_paths` in bytes: sum of the variable-length
    /// text columns plus a per-row overhead. Bounded by bytes, not rows, so one
    /// pathologically long path still counts toward the budget.
    pub async fn file_paths_bytes(&self) -> Result<u64> {
        let bytes: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(
                 length(file_path) + length(fingerprint)
                 + length(COALESCE(state, '')) + length(COALESCE(expected_path, ''))
                 + ?
             ), 0)
             FROM file_paths",
        )
        .bind(FILE_PATHS_ROW_OVERHEAD)
        .fetch_one(&self.pool)
        .await?;
        Ok(bytes.max(0) as u64)
    }

    /// Backstop for unbounded `file_paths` growth: delete evictable rows
    /// (unassigned, unreferenced, not kept for review, and not auto-adoption
    /// candidates) oldest-first — by `last_seen_at`, which live files refresh on
    /// every scan — until the estimate is at or below `budget_bytes`. Returns the
    /// count deleted. Assigned rows, kept-for-review rows, and adoption candidates
    /// are never touched, so this can stop short of the budget when everything
    /// remaining is protected.
    pub async fn prune_unassigned_paths_to_budget(&self, budget_bytes: u64) -> Result<u64> {
        let mut deleted: u64 = 0;
        loop {
            let size = self.file_paths_bytes().await?;
            if size <= budget_bytes {
                break;
            }
            let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM file_paths")
                .fetch_one(&self.pool)
                .await?;
            if total == 0 {
                break; // Nothing left — avoid an infinite loop.
            }
            let evictable: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM file_paths WHERE {FILE_PATHS_EVICTABLE}"
            ))
            .fetch_one(&self.pool)
            .await?;
            if evictable == 0 {
                // Everything remaining is assigned or an adoption candidate — the
                // budget cannot be met without hiding live files, so stop.
                break;
            }
            let avg_row_bytes = (size / total as u64).max(1);
            let excess = size - budget_bytes;
            let target = (excess / avg_row_bytes)
                .clamp(1, FILE_PATHS_PRUNE_BATCH as u64)
                .min(evictable as u64) as i64;
            let result = sqlx::query(&format!(
                "DELETE FROM file_paths WHERE file_path IN (
                     SELECT file_path FROM file_paths
                     WHERE {FILE_PATHS_EVICTABLE}
                     ORDER BY last_seen_at IS NULL DESC, last_seen_at ASC, rowid ASC
                     LIMIT ?
                 )"
            ))
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

    async fn test_db() -> (DbManager, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();
        (db, tmp)
    }

    fn content<'a>(fingerprint: &'a str, path: &'a str) -> ContentRecord<'a> {
        ContentRecord {
            fingerprint,
            media_info: None,
            path,
        }
    }

    fn path<'a>(fingerprint: &'a str, file_path: &'a str) -> PathRecord<'a> {
        PathRecord {
            file_path,
            fingerprint,
            inode: 0,
            device: 0,
            size: 1,
            mtime: 0.0,
            state: "complete",
            expected_path: None,
        }
    }

    async fn content_count(db: &DbManager, fingerprint: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM file_contents WHERE fingerprint = ?")
            .bind(fingerprint)
            .fetch_one(db.get_pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn original_path_is_set_once_across_paths() {
        let (db, _tmp) = test_db().await;
        db.upsert_content(content("h1", "/first/a.mkv"))
            .await
            .unwrap();
        db.upsert_path(path("h1", "/first/a.mkv")).await.unwrap();
        // Same content seen later at a new path must not move the origin.
        db.upsert_content(content("h1", "/later/b.mkv"))
            .await
            .unwrap();
        db.upsert_path(path("h1", "/later/b.mkv")).await.unwrap();

        assert_eq!(
            db.resolve_original_path("h1").await.unwrap().as_deref(),
            Some("/first/a.mkv")
        );
    }

    #[tokio::test]
    async fn upsert_path_does_not_drop_associations() {
        let (db, _tmp) = test_db().await;
        db.upsert_path(path("h", "/p.mkv")).await.unwrap();
        seed_episode(&db, "ep-1", "/p.mkv").await;
        db.upsert_path(path("h", "/p.mkv")).await.unwrap();

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM episode_files WHERE episode_id = 'ep-1'")
                .fetch_one(db.get_pool())
                .await
                .unwrap();
        assert_eq!(count, 1, "re-scanning a path must not drop its association");
    }

    #[tokio::test]
    async fn prune_keeps_content_while_any_path_references_it() {
        let (db, _tmp) = test_db().await;
        db.upsert_content(content("h2", "/a.mkv")).await.unwrap();
        db.upsert_path(path("h2", "/a.mkv")).await.unwrap();
        db.upsert_path(path("h2", "/b.mkv")).await.unwrap();
        sqlx::query("UPDATE file_contents SET last_seen_at = datetime('now','-60 days')")
            .execute(db.get_pool())
            .await
            .unwrap();

        // One stale path removed, another still references it -> keep.
        sqlx::query("DELETE FROM file_paths WHERE file_path = '/b.mkv'")
            .execute(db.get_pool())
            .await
            .unwrap();
        assert_eq!(db.prune_orphan_contents().await.unwrap(), 0);
        assert_eq!(content_count(&db, "h2").await, 1);

        // Last reference gone and stale -> drop.
        sqlx::query("DELETE FROM file_paths WHERE file_path = '/a.mkv'")
            .execute(db.get_pool())
            .await
            .unwrap();
        assert_eq!(db.prune_orphan_contents().await.unwrap(), 1);
        assert_eq!(db.resolve_original_path("h2").await.unwrap(), None);
    }

    #[tokio::test]
    async fn prune_keeps_recent_orphan_content() {
        let (db, _tmp) = test_db().await;
        db.upsert_content(content("h3", "/a.mkv")).await.unwrap();
        // No path instances, but just seen -> retained within the window.
        assert_eq!(db.prune_orphan_contents().await.unwrap(), 0);
        assert_eq!(
            db.resolve_original_path("h3").await.unwrap().as_deref(),
            Some("/a.mkv")
        );
    }

    #[tokio::test]
    async fn save_fingerprint_mirrors_into_content_tables() {
        let (db, _tmp) = test_db().await;
        db.save_fingerprint(crate::db::files::SaveFingerprintParams {
            path: "/lib/E01.mkv",
            inode: 5,
            dev: 6,
            size: 100,
            mtime: 1.0,
            quick_hash: "abc",
            state: "organized",
            media_info: None,
        })
        .await
        .unwrap();

        assert_eq!(content_count(&db, "abc").await, 1);
        assert_eq!(
            db.resolve_original_path("abc").await.unwrap().as_deref(),
            Some("/lib/E01.mkv")
        );
        let (fp, state): (String, String) = sqlx::query_as(
            "SELECT fingerprint, state FROM file_paths WHERE file_path = '/lib/E01.mkv'",
        )
        .fetch_one(db.get_pool())
        .await
        .unwrap();
        assert_eq!(fp, "abc");
        assert_eq!(state, "organized");
    }

    #[tokio::test]
    async fn link_and_unlink_mirror_to_episode_files() {
        let (db, _tmp) = test_db().await;
        db.save_fingerprint(crate::db::files::SaveFingerprintParams {
            path: "/lib/E02.mkv",
            inode: 7,
            dev: 8,
            size: 100,
            mtime: 2.0,
            quick_hash: "def",
            state: "organized",
            media_info: None,
        })
        .await
        .unwrap();
        // Episode row with no file yet — a linked video fills the empty slot.
        sqlx::query(
            "INSERT INTO episodes (episode_id, series_id, season, episode, status) \
             VALUES ('ep-2', 's1', 1, 2, 'missing')",
        )
        .execute(db.get_pool())
        .await
        .unwrap();

        db.link_file_episode("/lib/E02.mkv", "ep-2").await.unwrap();
        let linked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = 'ep-2' AND fp.file_path = '/lib/E02.mkv'",
        )
        .fetch_one(db.get_pool())
        .await
        .unwrap();
        assert_eq!(linked, 1, "a video link fills an empty main slot");

        db.unlink_episode_file("ep-2").await.unwrap();
        let unlinked: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM episode_files WHERE episode_id = 'ep-2'")
                .fetch_one(db.get_pool())
                .await
                .unwrap();
        assert_eq!(unlinked, 0);
    }

    #[tokio::test]
    async fn hashless_path_gets_synthetic_content_key() {
        let (db, _tmp) = test_db().await;
        db.upsert_content(ContentRecord {
            fingerprint: "",
            media_info: None,
            path: "/loose.mkv",
        })
        .await
        .unwrap();
        db.upsert_path(path("", "/loose.mkv")).await.unwrap();

        assert_eq!(content_count(&db, "path:/loose.mkv").await, 1);
        let fp: String =
            sqlx::query_scalar("SELECT fingerprint FROM file_paths WHERE file_path = '/loose.mkv'")
                .fetch_one(db.get_pool())
                .await
                .unwrap();
        assert_eq!(fp, "path:/loose.mkv");
    }

    async fn seed_episode(db: &DbManager, episode_id: &str, file_path: &str) {
        sqlx::query(
            "INSERT INTO episodes (episode_id, series_id, season, episode, status)
             VALUES (?, 's1', 1, 1, 'organized')",
        )
        .bind(episode_id)
        .execute(db.get_pool())
        .await
        .unwrap();
        db.associate_main_file(episode_id, file_path, None)
            .await
            .unwrap();
    }

    fn fp<'a>(quick_hash: &'a str, p: &'a str) -> crate::db::files::SaveFingerprintParams<'a> {
        crate::db::files::SaveFingerprintParams {
            path: p,
            inode: 1,
            dev: 1,
            size: 10,
            mtime: 1.0,
            quick_hash,
            state: "organized",
            media_info: None,
        }
    }

    #[tokio::test]
    async fn episode_details_original_path_follows_current_content() {
        let (db, _tmp) = test_db().await;
        seed_episode(&db, "ep1", "/lib/E01.mkv").await;
        db.save_fingerprint(fp("H1", "/lib/E01.mkv")).await.unwrap();

        let row = db.get_episode_by_id("ep1").await.unwrap().unwrap();
        assert_eq!(row.original_path.as_deref(), Some("/lib/E01.mkv"));

        // Reassigned to a different file/content → the original path tracks the
        // episode's *current* content (no set-once anchor).
        db.upsert_content(content("H2", "/other/E01.mkv"))
            .await
            .unwrap();
        db.upsert_path(path("H2", "/other/E01.mkv")).await.unwrap();
        db.associate_main_file("ep1", "/other/E01.mkv", None)
            .await
            .unwrap();

        let row = db.get_episode_by_id("ep1").await.unwrap().unwrap();
        assert_eq!(row.original_path.as_deref(), Some("/other/E01.mkv"));
    }

    #[tokio::test]
    async fn original_path_survives_rename_and_is_shared_across_paths() {
        let (db, _tmp) = test_db().await;
        seed_episode(&db, "ep1", "/lib/E01.mkv").await;
        db.save_fingerprint(fp("H1", "/lib/E01.mkv")).await.unwrap();

        // Jumbie-side move: the path row follows (id-stable), so the association
        // follows too; the content's origin is stable.
        db.move_fingerprint_path("/lib/E01.mkv", "/lib/E01.mkv.renamed")
            .await
            .unwrap();
        let row = db.get_episode_by_id("ep1").await.unwrap().unwrap();
        assert_eq!(row.original_path.as_deref(), Some("/lib/E01.mkv"));

        // A copy at a new path shares the same content and origin.
        db.save_fingerprint(fp("H1", "/backup/E01.mkv"))
            .await
            .unwrap();
        assert_eq!(
            db.resolve_original_path("H1").await.unwrap().as_deref(),
            Some("/lib/E01.mkv")
        );
    }

    #[tokio::test]
    async fn record_hash_only_records_origin_without_media_info() {
        let (db, tmp) = test_db().await;
        let file = tmp.path().join("E01.mkv");
        std::fs::write(&file, b"some video bytes").unwrap();
        let path_str = file.to_string_lossy().to_string();

        let key = db.record_hash_only(&file, "organized").await;
        assert!(!key.is_empty(), "content key must be non-empty");

        let (fp, state): (String, String) =
            sqlx::query_as("SELECT fingerprint, state FROM file_paths WHERE file_path = ?")
                .bind(&path_str)
                .fetch_one(db.get_pool())
                .await
                .unwrap();
        assert_eq!(fp, key);
        assert_eq!(state, "organized");

        let media: Option<String> =
            sqlx::query_scalar("SELECT media_info FROM file_contents WHERE fingerprint = ?")
                .bind(&key)
                .fetch_one(db.get_pool())
                .await
                .unwrap();
        assert!(
            media.is_none(),
            "hash-first leaves ffprobe to the background"
        );

        assert_eq!(
            db.resolve_original_path(&key).await.unwrap().as_deref(),
            Some(path_str.as_str())
        );
    }

    #[tokio::test]
    async fn scan_media_info_only_rehashes_a_changed_file() {
        // The path→fingerprint mapping is only valid while the file's identity is
        // unchanged. An in-place replacement at the same path must be re-hashed so
        // path-keyed readers (episode details: size/release/original path) reflect
        // the new content instead of the previous occupant's fingerprint.
        let (db, tmp) = test_db().await;
        let file = tmp.path().join("E01.mkv");
        std::fs::write(&file, b"original bytes").unwrap();
        let path_str = file.to_string_lossy().to_string();

        let original = db.record_hash_only(&file, "organized").await;
        assert!(!original.is_empty());

        // Replace in place: different size (and mtime) than the stored row.
        std::fs::write(&file, b"replacement bytes of a different length").unwrap();

        let (fp, _media) = db.scan_media_info_only(&file).await;
        assert_ne!(fp, original, "a changed file must be re-hashed");

        let stored: String =
            sqlx::query_scalar("SELECT fingerprint FROM file_paths WHERE file_path = ?")
                .bind(&path_str)
                .fetch_one(db.get_pool())
                .await
                .unwrap();
        assert_eq!(
            stored, fp,
            "the stored row must be rewritten to the new content"
        );
    }

    #[tokio::test]
    async fn scan_media_info_only_reuses_an_unchanged_fingerprint() {
        let (db, tmp) = test_db().await;
        let file = tmp.path().join("E01.mkv");
        std::fs::write(&file, b"some video bytes").unwrap();

        let original = db.record_hash_only(&file, "organized").await;
        assert!(!original.is_empty());

        // No change on disk → the cheap path must reuse the stored fingerprint.
        let (fp, _media) = db.scan_media_info_only(&file).await;
        assert_eq!(fp, original);
    }

    #[tokio::test]
    async fn episode_detail_row_exposes_original_path() {
        let (db, _tmp) = test_db().await;
        db.upsert_content(content("H1", "/orig/E01.mkv"))
            .await
            .unwrap();
        db.upsert_path(path("H1", "/orig/E01.mkv")).await.unwrap();
        seed_episode(&db, "ep1", "/orig/E01.mkv").await;

        // The episode details query (the modal's API source) must expose it —
        // resolved from the joined `file_contents` row for the current file.
        let row = db.get_episode_by_id("ep1").await.unwrap().unwrap();
        assert_eq!(row.original_path.as_deref(), Some("/orig/E01.mkv"));
    }

    #[tokio::test]
    async fn insert_episode_resolves_original_path_from_recorded_fingerprint() {
        let (db, _tmp) = test_db().await;
        // Hash-first: fingerprint recorded before the association exists.
        db.upsert_content(content("H1", "/lib/E01.mkv"))
            .await
            .unwrap();
        db.upsert_path(path("H1", "/lib/E01.mkv")).await.unwrap();

        let no_ids = std::collections::HashMap::new();
        db.insert_episode(crate::db::episodes::InsertEpisodeParams {
            episode_id: "ep1",
            series_id: "s1",
            season: 1,
            episode: 1,
            file_path: Some("/lib/E01.mkv"),
            title: Some(""),
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
            est_date: None,
            metadata_ids: &no_ids,
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: Some(0),
        })
        .await
        .unwrap();

        let row = db.get_episode_by_id("ep1").await.unwrap().unwrap();
        assert_eq!(row.original_path.as_deref(), Some("/lib/E01.mkv"));
        assert_eq!(
            db.resolve_original_path("H1").await.unwrap().as_deref(),
            Some("/lib/E01.mkv")
        );
    }

    fn path_state<'a>(
        fingerprint: &'a str,
        file_path: &'a str,
        state: &'a str,
        size: i64,
    ) -> PathRecord<'a> {
        PathRecord {
            file_path,
            fingerprint,
            inode: 0,
            device: 0,
            size,
            mtime: 0.0,
            state,
            expected_path: None,
        }
    }

    async fn path_count(db: &DbManager) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM file_paths")
            .fetch_one(db.get_pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn prune_unassigned_paths_evicts_oldest_evictable() {
        let (db, _tmp) = test_db().await;
        // 30 evictable (partial) rows; E00 is the oldest by last_seen_at.
        for i in 0..30 {
            let p = format!("/scan/E{:02}.part", i);
            db.upsert_path(path_state("h", &p, "partial", 1))
                .await
                .unwrap();
            sqlx::query(
                "UPDATE file_paths SET last_seen_at = datetime('now', ?) WHERE file_path = ?",
            )
            .bind(format!("-{} days", 60 - i))
            .bind(&p)
            .execute(db.get_pool())
            .await
            .unwrap();
        }

        let deleted = db.prune_unassigned_paths_to_budget(200).await.unwrap();
        assert!(deleted > 0, "over-budget table must shed evictable rows");
        let oldest: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM file_paths WHERE file_path = '/scan/E00.part'",
        )
        .fetch_one(db.get_pool())
        .await
        .unwrap();
        assert_eq!(oldest, 0, "oldest evictable row goes first");
        assert!(db.file_paths_bytes().await.unwrap() <= 200);
    }

    #[tokio::test]
    async fn prune_unassigned_paths_never_evicts_assigned_or_adoption_candidates() {
        let (db, _tmp) = test_db().await;
        // Adoption candidate: unassigned, complete, size > 0, not kept-for-review.
        db.upsert_path(path("h1", "/orphan.mkv")).await.unwrap();
        // Assigned via the association table.
        db.upsert_path(path_state("h2", "/assigned.part", "partial", 1))
            .await
            .unwrap();
        seed_episode(&db, "ep-1", "/assigned.part").await;
        // Referenced by an episode's association.
        db.upsert_path(path_state("h3", "/ref-by-episode.part", "partial", 1))
            .await
            .unwrap();
        seed_episode(&db, "ep2", "/ref-by-episode.part").await;

        let deleted = db.prune_unassigned_paths_to_budget(1).await.unwrap();
        assert_eq!(deleted, 0, "protected rows must never be evicted");
        assert_eq!(path_count(&db).await, 3);
    }

    #[tokio::test]
    async fn prune_unassigned_paths_keeps_kept_for_review_rows() {
        let (db, _tmp) = test_db().await;
        // Kept-for-review leftovers (both `unmatched` and `unneeded` reasons) are
        // protected: `unmatched_files` is their durable record, so the cache rows
        // must not be evicted.
        for (p, reason) in [
            ("/unmatched.mkv", "unmatched"),
            ("/unneeded.mkv", "unneeded"),
        ] {
            db.upsert_path(path("h", p)).await.unwrap();
            sqlx::query("INSERT INTO unmatched_files (file_path, reason) VALUES (?, ?)")
                .bind(p)
                .bind(reason)
                .execute(db.get_pool())
                .await
                .unwrap();
        }

        assert_eq!(db.prune_unassigned_paths_to_budget(1).await.unwrap(), 0);
        assert_eq!(path_count(&db).await, 2);
    }

    #[tokio::test]
    async fn prune_unassigned_paths_noop_under_budget() {
        let (db, _tmp) = test_db().await;
        db.upsert_path(path_state("h", "/a.part", "partial", 1))
            .await
            .unwrap();
        assert_eq!(
            db.prune_unassigned_paths_to_budget(100_000).await.unwrap(),
            0
        );
        assert_eq!(path_count(&db).await, 1);
    }
}
