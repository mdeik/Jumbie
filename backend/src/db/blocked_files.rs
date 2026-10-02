//! Durable "do not auto-adopt this file" markers.
//!
//! Created when the user unassigns a wrong file, consulted by the scanner's
//! filename-derived discovery so a rescan cannot re-adopt it, cleared by an
//! explicit (re)assignment, and expired by a stale-entry sweep.
//!
//! # Keying
//!
//! The key is `(series, normalized file name, size)` with an optional content
//! hash. Names are normalized by the shared SSoT
//! [`jumbie_shared::formatting::normalize_file_name`], so a one-character rename
//! yields a different key and therefore unblocks the file.

use anyhow::Result;
use jumbie_shared::formatting::normalize_file_name;

use super::DbManager;

/// The identity of a blocked file — the fields that decide a match.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BlockedFile {
    pub file_name: String,
    pub size: i64,
    pub quick_hash: Option<String>,
}

/// Does `block` cover a candidate with these attributes?
///
/// SSoT for the matching rule (used by the scanner's discovery gate and by
/// block-clearing on explicit assignment): exact normalized name, then — when
/// *both* the stored and the candidate hash are known — require equal hashes;
/// otherwise fall back to equal size. A differing size/hash means a *new* file,
/// which is allowed back in.
///
/// `candidate_name` must already be normalized.
pub fn blocked_file_matches(
    block: &BlockedFile,
    candidate_name: &str,
    candidate_size: i64,
    candidate_hash: Option<&str>,
) -> bool {
    if block.file_name != candidate_name {
        return false;
    }
    match (block.quick_hash.as_deref(), candidate_hash) {
        (Some(bh), Some(ch)) => bh == ch,
        _ => block.size == candidate_size,
    }
}

/// The SSoT gate for the discovery paths (scanner walks, orphan adoption).
///
/// Is the candidate file blocked by any of the series' `blocks`? All blocks
/// sharing the name are consulted (the name repeats across sizes); the content
/// hash is fetched at most once, and only when a name-matching block carries
/// one, so a walk stays cheap. On a match, the block's `last_seen_at` is bumped
/// so the stale sweep does not expire a file that is still present.
///
/// Callers still own *loading* the series' blocks (and may cache them).
pub async fn is_file_blocked(
    db: &DbManager,
    series_id: &str,
    blocks: &[BlockedFile],
    file_name: &str,
    size: i64,
    path: &str,
) -> bool {
    if blocks.is_empty() {
        return false;
    }
    let normalized = normalize_file_name(file_name);
    let name_matches: Vec<&BlockedFile> = blocks
        .iter()
        .filter(|b| b.file_name == normalized)
        .collect();
    if name_matches.is_empty() {
        return false;
    }

    let candidate_hash = if name_matches.iter().any(|b| b.quick_hash.is_some()) {
        db.path_file_identity(path).await.and_then(|(_, hash)| hash)
    } else {
        None
    };

    let matched = name_matches
        .iter()
        .any(|b| blocked_file_matches(b, &normalized, size, candidate_hash.as_deref()));
    if matched {
        // A match implies equal size (hash match or size match), so the
        // (series, name, size) identity is the matched block's.
        let _ = db
            .touch_blocked_file_seen(series_id, &normalized, size)
            .await;
    }
    matched
}

impl DbManager {
    /// The size + content hash of `path`, from its fingerprint row when known,
    /// else by a filesystem stat for the size only. `None` when neither is
    /// available (e.g. the file is gone and was never fingerprinted).
    pub async fn path_file_identity(&self, path: &str) -> Option<(i64, Option<String>)> {
        if let Ok(Some((size, hash))) = sqlx::query_as::<_, (Option<i64>, Option<String>)>(
            "SELECT size, fingerprint FROM file_paths WHERE file_path = ?",
        )
        .bind(path)
        .fetch_optional(&self.pool)
        .await
            && let Some(size) = size
        {
            return Some((size, hash));
        }
        let size = std::fs::metadata(path).ok().map(|m| m.len() as i64)?;
        Some((size, None))
    }

    /// Record (or refresh) a file as blocked. `file_name` is normalized here.
    pub async fn block_file(
        &self,
        series_id: &str,
        file_name: &str,
        size: i64,
        quick_hash: Option<&str>,
    ) -> Result<()> {
        let name = normalize_file_name(file_name);
        sqlx::query(
            "INSERT INTO blocked_files (series_id, file_name, size, quick_hash, last_seen_at)
             VALUES (?, ?, ?, ?, datetime('now'))
             ON CONFLICT(series_id, file_name, size) DO UPDATE SET
               quick_hash = COALESCE(excluded.quick_hash, quick_hash),
               last_seen_at = excluded.last_seen_at",
        )
        .bind(series_id)
        .bind(&name)
        .bind(size)
        .bind(quick_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All blocked files for a series.
    pub async fn get_blocked_files(&self, series_id: &str) -> Result<Vec<BlockedFile>> {
        let rows = sqlx::query_as::<_, BlockedFile>(
            "SELECT file_name, size, quick_hash FROM blocked_files WHERE series_id = ?",
        )
        .bind(series_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Remove blocks that a specific file matches (explicit intent wins).
    /// `file_name` is normalized here.
    pub async fn clear_blocked_files_for(
        &self,
        series_id: &str,
        file_name: &str,
        size: i64,
        quick_hash: Option<&str>,
    ) -> Result<u64> {
        let name = normalize_file_name(file_name);
        let candidates = sqlx::query_as::<_, BlockedFile>(
            "SELECT file_name, size, quick_hash FROM blocked_files \
             WHERE series_id = ? AND file_name = ?",
        )
        .bind(series_id)
        .bind(&name)
        .fetch_all(&self.pool)
        .await?;

        let mut deleted = 0u64;
        for row in candidates {
            if blocked_file_matches(&row, &name, size, quick_hash) {
                let res = sqlx::query(
                    "DELETE FROM blocked_files WHERE series_id = ? AND file_name = ? AND size = ?",
                )
                .bind(series_id)
                .bind(&name)
                .bind(row.size)
                .execute(&self.pool)
                .await?;
                deleted += res.rows_affected();
            }
        }
        Ok(deleted)
    }

    /// Clear any block matching the file at `path` (explicit-assignment hook).
    pub async fn clear_blocked_files_for_path(&self, series_id: &str, path: &str) -> Result<u64> {
        let name = match std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
        {
            Some(n) if !n.is_empty() => n,
            _ => return Ok(0),
        };
        let Some((size, hash)) = self.path_file_identity(path).await else {
            return Ok(0);
        };
        self.clear_blocked_files_for(series_id, name, size, hash.as_deref())
            .await
    }

    /// Bump `last_seen_at` for the block identified by `(series, name, size)`, so
    /// the stale sweep does not expire a file that is still on disk and matched.
    pub async fn touch_blocked_file_seen(
        &self,
        series_id: &str,
        file_name: &str,
        size: i64,
    ) -> Result<()> {
        let name = normalize_file_name(file_name);
        sqlx::query(
            "UPDATE blocked_files SET last_seen_at = datetime('now') \
             WHERE series_id = ? AND file_name = ? AND size = ?",
        )
        .bind(series_id)
        .bind(&name)
        .bind(size)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete blocks not seen for `grace_days` — their file is gone, renamed, or
    /// replaced. Returns the number of rows deleted.
    pub async fn cleanup_stale_blocked_files(&self, grace_days: i64) -> Result<usize> {
        let modifier = format!("-{} days", grace_days);
        let res = sqlx::query("DELETE FROM blocked_files WHERE last_seen_at < datetime('now', ?)")
            .bind(&modifier)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() as usize)
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

    /// `series_id` is a real FK (ON DELETE CASCADE), so tests seed the parent.
    async fn seed_series(db: &DbManager, id: &str) {
        sqlx::query("INSERT INTO series_mappings (id, data) VALUES (?, '{}')")
            .bind(id)
            .execute(db.get_pool())
            .await
            .unwrap();
    }

    /// Does the loaded block list cover a candidate with these attributes?
    fn is_blocked(blocks: &[BlockedFile], name: &str, size: i64, hash: Option<&str>) -> bool {
        let n = normalize_file_name(name);
        blocks
            .iter()
            .any(|b| blocked_file_matches(b, &n, size, hash))
    }

    #[tokio::test]
    async fn block_matches_by_normalized_name_and_size() {
        let (db, _tmp) = test_db().await;
        seed_series(&db, "s").await;
        db.block_file("s", "Show.S01E05.mkv", 100, None)
            .await
            .unwrap();
        let blocks = db.get_blocked_files("s").await.unwrap();

        assert!(
            is_blocked(&blocks, "show.s01e05.MKV", 100, None),
            "name normalized"
        );
        assert!(
            is_blocked(&blocks, "  Show.S01E05.mkv  ", 100, None),
            "trimmed"
        );
        assert!(
            !is_blocked(&blocks, "show.s01e05.mkv", 101, None),
            "different size = new file"
        );
        assert!(
            !is_blocked(&blocks, "show.s01e06.mkv", 100, None),
            "renamed = unblocked"
        );
    }

    #[tokio::test]
    async fn blocks_are_scoped_per_series() {
        let (db, _tmp) = test_db().await;
        seed_series(&db, "a").await;
        seed_series(&db, "b").await;
        db.block_file("a", "Show.S01E05.mkv", 100, None)
            .await
            .unwrap();
        assert!(db.get_blocked_files("b").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn hash_decides_when_both_known_else_size() {
        let (db, _tmp) = test_db().await;
        seed_series(&db, "s").await;
        db.block_file("s", "x.mkv", 100, Some("hashA"))
            .await
            .unwrap();
        let blocks = db.get_blocked_files("s").await.unwrap();

        assert!(
            !is_blocked(&blocks, "x.mkv", 100, Some("hashB")),
            "same name+size but different content = new file"
        );
        assert!(is_blocked(&blocks, "x.mkv", 100, Some("hashA")));
        assert!(
            is_blocked(&blocks, "x.mkv", 100, None),
            "unknown hash falls back to size"
        );
    }

    #[tokio::test]
    async fn clear_removes_only_matching_blocks() {
        let (db, _tmp) = test_db().await;
        seed_series(&db, "s").await;
        db.block_file("s", "x.mkv", 100, None).await.unwrap();

        assert_eq!(
            db.clear_blocked_files_for("s", "x.mkv", 999, None)
                .await
                .unwrap(),
            0,
            "a non-matching size must not clear the block"
        );
        assert_eq!(db.get_blocked_files("s").await.unwrap().len(), 1);

        assert_eq!(
            db.clear_blocked_files_for("s", "x.mkv", 100, None)
                .await
                .unwrap(),
            1
        );
        assert!(db.get_blocked_files("s").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn clear_for_path_uses_fingerprint_identity() {
        let (db, tmp) = test_db().await;
        seed_series(&db, "s").await;
        let path = tmp.path().join("x.mkv");
        std::fs::write(&path, b"0123456789").unwrap();
        let path_str = path.to_string_lossy().to_string();
        let size = std::fs::metadata(&path).unwrap().len() as i64;
        db.block_file("s", "x.mkv", size, None).await.unwrap();

        assert_eq!(
            db.clear_blocked_files_for_path("s", &path_str)
                .await
                .unwrap(),
            1
        );
        assert!(db.get_blocked_files("s").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_removes_only_stale_blocks() {
        let (db, _tmp) = test_db().await;
        seed_series(&db, "s").await;
        db.block_file("s", "old.mkv", 1, None).await.unwrap();
        db.block_file("s", "new.mkv", 2, None).await.unwrap();
        sqlx::query(
            "UPDATE blocked_files SET last_seen_at = datetime('now','-10 days') \
             WHERE file_name = 'old.mkv'",
        )
        .execute(db.get_pool())
        .await
        .unwrap();

        assert_eq!(db.cleanup_stale_blocked_files(7).await.unwrap(), 1);
        let remaining = db.get_blocked_files("s").await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].file_name, "new.mkv");
    }

    #[tokio::test]
    async fn touching_a_block_keeps_it_alive() {
        let (db, _tmp) = test_db().await;
        seed_series(&db, "s").await;
        db.block_file("s", "x.mkv", 5, None).await.unwrap();
        sqlx::query(
            "UPDATE blocked_files SET last_seen_at = datetime('now','-30 days') \
             WHERE file_name = 'x.mkv'",
        )
        .execute(db.get_pool())
        .await
        .unwrap();

        db.touch_blocked_file_seen("s", "x.mkv", 5).await.unwrap();
        assert_eq!(db.cleanup_stale_blocked_files(7).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn deleting_series_cascades_blocks() {
        let (db, tmp) = test_db().await;
        seed_series(&db, "gone").await;
        seed_series(&db, "kept").await;
        let _ = tmp;
        db.block_file("gone", "a.mkv", 1, None).await.unwrap();
        db.block_file("kept", "b.mkv", 1, None).await.unwrap();

        db.delete_series_mapping("gone").await.unwrap();

        assert!(db.get_blocked_files("gone").await.unwrap().is_empty());
        assert_eq!(db.get_blocked_files("kept").await.unwrap().len(), 1);
    }
}
