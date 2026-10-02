use anyhow::Result;
use sqlx::ConnectOptions;
use sqlx::sqlite::{Sqlite, SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

#[macro_use]
mod macros;

// Builds a comma-separated list of `?` placeholders for SQL `IN (...)` clauses.
// sqlx binds must be known at compile time, so a Vec cannot be bound to `IN (?)`;
// this constructs n placeholders to bind individually.
pub(crate) fn sql_in_placeholders(count: usize) -> String {
    (0..count).map(|_| "?").collect::<Vec<_>>().join(",")
}


/// Normalizes a path string or prefix in Rust for SQL prefix / LIKE matching.
/// Trims trailing slashes (both `/` and `\`) and replaces all `\` with `/`.
pub(crate) fn normalize_sql_path_prefix(path: &str) -> String {
    path.trim_end_matches(['/', '\\']).replace('\\', "/")
}

/// Builds a SQL LIKE pattern matching any file or folder strictly inside the given directory prefix.
/// Normalizes both Windows backslashes and Unix slashes so the pattern is always `{prefix}/%`.
pub(crate) fn sql_child_path_like_pattern(dir_prefix: &str) -> String {
    format!("{}/%", normalize_sql_path_prefix(dir_prefix))
}

pub mod auth;
pub mod automatic_profiles;
pub mod config;
pub mod download_queue;
pub mod episodes;
pub mod files;
pub mod fingerprints;
pub mod ownership;

pub mod activity;
pub mod autoresolve;
pub mod blocked_files;
pub mod journal;
pub mod metadata_cache;
pub mod plugins;
pub mod scoring;
pub mod series;
pub mod unmatched_files;

// Download queue base query: resolves series_title from series_mappings (the
// trigger-populated `target_title`) so a series rename shows in the queue without
// backfilling rows; falls back to the stored series_title when no mapping exists.
const DOWNLOAD_QUEUE_BASE: &str = "SELECT dq.id, dq.downloaded_at, dq.media_name, dq.media_link, \
    COALESCE(sm.target_title, dq.series_title) AS series_title, \
    dq.season, dq.episode, dq.episode_end, \
    COALESCE(dq.episode_id, '') AS episode_id, dq.score, dq.is_user_requested, dq.is_manual, \
    dq.multi_targets, dq.status, \
    dq.download_id, \
    dq.downloader_id, dq.client_id, dq.progress, dq.error_message, \
    COALESCE(dq.series_id, '') AS series_id, dq.is_season_pack, dq.category, \
    dq.retry_count, dq.next_retry_at, \
    dq.last_progress, dq.no_progress_since, \
    dq.episode_intentions, \
    dq.scoring_size_bytes, dq.scoring_seeders, dq.scoring_episode_count, \
    dq.submitter, \
    dq.quality_profile_id, \
    COALESCE(dq.version, 1) AS version \
FROM download_queue dq \
LEFT JOIN series_mappings sm ON dq.series_id = sm.id ";

/// SSoT: in-flight queue statuses (actively downloading or awaiting dispatch).
/// These block auto-search. `Failed` is deliberately excluded: it should not
/// block finding a NEW release, but it is still non-terminal for dedup.
pub(crate) const IN_FLIGHT_QUEUE_STATUSES: &str = "'Queued', 'Downloading'";

/// SSoT: non-terminal queue statuses (still tracked: queued, downloading, or
/// failed). Terminal are `'Completed'` and `'Organizing'`.
pub(crate) const NON_TERMINAL_QUEUE_STATUSES: &str = "'Queued', 'Downloading', 'Failed'";

/// SSoT: the file→fingerprint→release JOIN chain shared by every episode
/// detail/rename-plan/calendar query. `file_paths` (f) resolves the episode's
/// **primary** main file — its non-part association if present, else the
/// lowest-numbered part (see [`EPISODE_PRIMARY_FILE_ID`]) — and `file_contents`
/// (c) supplies media_info/fingerprint. Auxiliary associations are never selected.
pub(crate) const EPISODE_FILE_JOIN: &str = "LEFT JOIN file_paths f ON f.id = (SELECT ef.file_path_id FROM episode_files ef WHERE ef.episode_id = e.episode_id AND ef.kind = 'main' ORDER BY (ef.part_number IS NOT NULL), ef.part_number, ef.file_path_id LIMIT 1) LEFT JOIN file_contents c ON c.fingerprint = f.fingerprint LEFT JOIN release_info rm ON c.fingerprint = rm.quick_hash";

/// SSoT: the `file_paths.id` of an episode's primary main file, in the same
/// precedence as [`EPISODE_FILE_JOIN`] (non-part first, then lowest part number).
pub(crate) const EPISODE_PRIMARY_FILE_ID: &str = "(SELECT ef.file_path_id FROM episode_files ef WHERE ef.episode_id = e.episode_id AND ef.kind = 'main' ORDER BY (ef.part_number IS NOT NULL), ef.part_number, ef.file_path_id LIMIT 1)";

/// SSoT: an episode's original content path — where the content its **current**
/// file holds was first seen. Resolved from the joined `file_contents` row (`c`)
/// for the episode's current file, so it follows reassignments and renames
/// (the same row that supplies media_info/fingerprint/release_info). NULL when
/// the file has no tracked content.
pub(crate) const ORIGINAL_PATH_EXPR: &str = "c.original_path";

/// SSoT: submitter resolution — release_info submitter first, download_queue
/// fallback. NULLIF guards against an empty-string submitter shadowing the fallback.
pub(crate) const SUBMITTER_EXPR: &str = "COALESCE(NULLIF(rm.submitter, ''), (SELECT dq.submitter FROM download_queue dq WHERE dq.episode_id = e.episode_id AND dq.submitter IS NOT NULL LIMIT 1))";

/// SSoT: the episode's file (Source Feed) date, resolved from the content of the
/// episode's **main** files.
///
/// The joined primary file (`rm.upload_date`) is preferred. A multipart episode's
/// primary file is its lowest part, but that part may carry no date — so the
/// expression then falls back to the first main association, in the same
/// precedence order, that actually has a date. Auxiliary sidecars are never
/// considered (see [`EPISODE_FILE_JOIN`]).
///
/// Callers select it as `{UPLOAD_DATE_EXPR} AS upload_date` (the query must expose
/// the `e` and `rm` aliases, i.e. use [`EPISODE_FILE_JOIN`]).
pub(crate) const UPLOAD_DATE_EXPR: &str = "COALESCE(rm.upload_date, (SELECT ri.upload_date FROM episode_files ef JOIN file_paths fp ON fp.id = ef.file_path_id JOIN file_contents fc ON fc.fingerprint = fp.fingerprint JOIN release_info ri ON ri.quick_hash = fc.fingerprint WHERE ef.episode_id = e.episode_id AND ef.kind = 'main' AND ri.upload_date IS NOT NULL ORDER BY (ef.part_number IS NOT NULL), ef.part_number, ef.file_path_id LIMIT 1))";

/// SSoT: predicate over `file_paths fp` selecting an episode's **main** files
/// (its single file, its multipart files, or a shared multi-episode file).
/// Used by every query that enumerates or writes per-episode file data
/// (rescore, download finalize, episode hash/release/score). Auxiliary files are
/// excluded. Callers bind the episode id ONCE.
pub(crate) const EPISODE_FILES_PREDICATE: &str = "fp.id IN (SELECT ef.file_path_id FROM episode_files ef WHERE ef.episode_id = ? AND ef.kind = 'main')";

/// SSoT: `e` owns at least one main file. This is an association/DB-state
/// predicate only — it does not imply the file exists on disk (`disk_present` is
/// resolved at read time).
pub(crate) const EPISODE_ASSIGNED_PREDICATE: &str = "EXISTS (SELECT 1 FROM episode_files ef WHERE ef.episode_id = e.episode_id AND ef.kind = 'main')";

/// SSoT: `e` owns no main file.
pub(crate) const EPISODE_UNASSIGNED_PREDICATE: &str = "NOT EXISTS (SELECT 1 FROM episode_files ef WHERE ef.episode_id = e.episode_id AND ef.kind = 'main')";

#[derive(Debug, Clone)]
pub struct DatabaseStats {
    pub total_size_bytes: i64,
    // Space occupied by freelist pages — pages that were allocated, then freed
    // (e.g. by DELETEs), but not yet reclaimed. SQLite reuses freelist pages for
    // future inserts before growing the file, so this is *reclaimable* space that
    // VACUUM can return to the OS, not inherently wasteful data.
    pub wasted_bytes: i64,
    pub page_count: i64,
    pub freelist_count: i64,
}

#[derive(Debug, Clone)]
pub struct SeriesStatRow {
    pub series_id: String,
    pub series_title: String,
    pub downloaded_count: i64,
    pub total_count: i64,
    pub size: i64,
    pub monitored_missing_count: i64,
    pub seasons: Vec<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CompletedFileRow {
    pub file_path: String,
    pub episode_id: String,
    pub download_id: String,
    /// The episode's current file_path in the DB (may differ from file_path
    /// if the scanner claimed the episode after the download finished).
    pub episode_file_path: Option<String>,
    pub episode_status: String,
    pub episode_score: Option<i32>,
    pub series_id: String,
}

// The fingerprint is a content-based hash (expensive to compute or recompute).
// These three fields act as a fast staleness check: if inode, mtime, and size
// all match a previous scan, the file is almost certainly unmodified, so the
// cached fingerprint can be trusted without re-hashing the entire file.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FingerprintMeta {
    pub inode: i64,
    pub device: i64,
    pub mtime: f64,
    pub size: i64,
}

// `None` does not mean "unknown"; it means "not yet applicable at this stage" (e.g.
// file_path=None + monitored=true = waiting for download).
//
// Fields marked `#[sqlx(default)]` may be omitted by the LEAN rename-plan query
// (`get_rename_plan_episodes`), which populates only the contract fields enforced by
// the drift test in db/episodes/queries.rs — a new required column must be added to
// that query AND the test's "required" list.
#[derive(Debug, Clone, Hash, sqlx::FromRow)]
pub struct EpisodeDetailRow {
    pub episode_id: String,
    pub season: Option<i32>,
    pub episode: i32,
    #[sqlx(default)]
    pub status: Option<String>,
    pub file_path: Option<String>,
    #[sqlx(default)]
    pub release_title: Option<String>,
    #[sqlx(default)]
    pub size: i64,
    pub title: Option<String>,
    #[sqlx(default)]
    pub quality_profile_id: Option<String>,
    pub submitter: Option<String>,
    pub media_info: Option<String>,
    #[sqlx(default)]
    pub quick_hash: Option<String>,
    pub created_at: Option<chrono::NaiveDateTime>,
    #[sqlx(default)]
    pub file_acquired_at: Option<chrono::NaiveDateTime>,
    #[sqlx(default)]
    pub monitored: bool,
    pub meta_date: Option<chrono::NaiveDateTime>,
    #[sqlx(default)]
    pub upload_date: Option<chrono::NaiveDateTime>,
    #[sqlx(default)]
    pub est_date: Option<chrono::NaiveDateTime>,
    #[sqlx(default)]
    pub metadata_ids: Option<String>,
    #[sqlx(default)]
    pub description: Option<String>,
    #[sqlx(default)]
    pub runtime: Option<i32>,
    #[sqlx(default)]
    pub image_url: Option<String>,
    #[sqlx(default)]
    pub download_id: Option<String>,
    #[sqlx(default)]
    pub score: Option<i32>,
    #[sqlx(default)]
    pub numbering_mode: i32,
    pub metadata_source: Option<String>,
    pub series_id: String,
    #[sqlx(default)]
    /// Download link (magnet/URL) used to acquire this episode.
    pub download_link: Option<String>,
    /// Whether the user explicitly toggled this episode's `monitored` flag
    /// (as opposed to it being derived from the series' monitor mode).
    /// Defaults to `false` for queries that don't SELECT this column.
    #[sqlx(default)]
    pub monitor_override: bool,
    /// The original path of this episode's origin content. Populated by
    /// `ORIGINAL_PATH_EXPR`; defaults to None for lean queries.
    #[sqlx(default)]
    pub original_path: Option<String>,
}

/// Per-download metadata snapshot read from the queue item at organize time and
/// written to `release_info` (keyed by the content quick_hash).  The queue item
/// is the SSoT for a download's attribution — submitter and quality profile are
/// recorded at queue time, before the mapping or release state can change.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OrganizeMetaRow {
    pub media_name: String,
    pub media_link: String,
    pub score: i32,
    pub source_pub_date: Option<String>,
    pub download_id: Option<String>,
    pub submitter: Option<String>,
    pub quality_profile_id: Option<String>,
    pub scoring_size_bytes: Option<i64>,
    pub scoring_seeders: Option<i32>,
    pub scoring_episode_count: Option<i32>,
    pub version: i32,
}

/// Lightweight row for the rescore path — only columns needed to recalculate
/// `episodes.score` when a release profile changes.
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct EpisodeRescoreRow {
    pub episode_id: String,
    pub series_id: String,
    pub release_title: Option<String>,
    pub submitter: Option<String>,
    pub scoring_size_bytes: Option<i64>,
    pub scoring_seeders: Option<i32>,
    pub scoring_upload_date: Option<String>,
    pub scoring_episode_count: Option<i32>,
}

// `status` reflects the episode's airing/production lifecycle (e.g. "unaired" / "aired"),
// while `file_path` reflects physical availability on disk. These are orthogonal:
// an episode can be "aired" but missing from disk (not yet downloaded), or
// "unaired" but already downloaded (pre-release). Neither field subsumes the other.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CalendarEpisodeRow {
    pub series_id: String,
    pub series_title: String,
    pub episode_id: String,
    pub season: Option<i32>,
    pub episode: i32,
    pub title: Option<String>,
    pub meta_date: Option<chrono::NaiveDateTime>,
    pub upload_date: Option<chrono::NaiveDateTime>,
    pub est_date: Option<chrono::NaiveDateTime>,
    pub status: Option<String>,
    pub file_path: Option<String>,
    pub monitored: Option<bool>,
    pub numbering_mode: i32,
    pub runtime: Option<i32>,
    pub media_info: Option<String>,
}

// SQLite has no unsigned integer type — all INTEGER columns store signed 64-bit
// values. Using u32 would introduce silent truncation risk and require runtime
// validation on every interaction. i32 keeps the schema portable and avoids
// friction with SQLite's type system; negative values are simply invalid by
// convention enforced at the application layer.
#[derive(Debug, Clone, Hash, sqlx::FromRow)]
pub struct EpisodePartRow {
    pub part_number: i32,
    pub file_path: String,
    pub size: Option<i64>,
    pub fingerprint: Option<String>,
    pub media_info: Option<String>,
    /// Origin path of this part's own content fingerprint (each part is its own
    /// file with its own fingerprint). Defaults to None for lean queries.
    #[sqlx(default)]
    pub original_path: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EpisodeMediaInfoRow {
    pub file_path: Option<String>,
    pub submitter: Option<String>,
    pub download_id: Option<String>,
    pub quick_hash: Option<String>,
    #[sqlx(rename = "media_info")]
    pub media_info_json: Option<String>,
}

/// One row from automatic_profile_media_scans — the SSoT for scoring data.
/// Stored independently of episodes/file_paths so that deleting an
/// episode does NOT affect automatic profile scoring.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaScanRow {
    pub id: String,
    pub submitter: String,
    pub filename: String,
    pub series_id: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub media_info: String,
}

/// Provenance marker for episode rows created by the cell system rather than
/// by the scanner, downloads, or metadata provider.
///
/// Stored as INTEGER in SQLite: NULL = standard, 0 = user_defined, 1 = provider.
/// This is the **single source of truth** for cell state transitions.
///
/// Lifecycle:
///   (doesn't exist)  ──[cell_count config]──→  UserDefined
///   UserDefined      ──[provider fills in]──→  Provider  (or NULL via ON CONFLICT)
///   Provider         ──[stays provider]──→     (traceability only; behaves like NULL)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeCellType {
    /// Created from season `cell_count` config; no provider data yet.
    UserDefined = 0,
    /// Originally a user-defined cell; provider metadata has since filled it.
    Provider = 1,
}

impl EpisodeCellType {
    pub const fn to_db_int(self) -> i32 {
        self as i32
    }
}

type MappingsCacheEntry = (
    u64,
    Arc<std::collections::HashMap<String, jumbie_shared::types::MappingRule>>,
);

pub struct DbManager {
    pool: SqlitePool,

    // Generation-counter cache for series mappings: incremented on every upsert/
    // delete so `get_all_series_mappings()` can serve cached data without
    // deserializing every MappingRule blob on each access.
    mappings_cache_gen: AtomicU64,
    // None = never loaded or stale; the stored generation must match for a hit.
    mappings_cache: tokio::sync::Mutex<Option<MappingsCacheEntry>>,

    // Cached UI-preferences singleton (read on nearly every paginated request),
    // refreshed on `save_ui_preferences`. None = not yet loaded.
    ui_prefs_cache: tokio::sync::RwLock<Option<jumbie_shared::config::UIConfig>>,
}

impl DbManager {
    async fn upsert_json_record<T: serde::Serialize>(
        &self,
        table: &str,
        id: &str,
        data: &T,
    ) -> Result<()> {
        let json_data = serde_json::to_string(data)?;
        let query = format!(
            "INSERT INTO {} (id, data) VALUES (?, ?) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            table
        );
        sqlx::query(&query)
            .bind(id)
            .bind(json_data)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn save_json_map<T: serde::Serialize>(
        &self,
        table: &str,
        map: &std::collections::HashMap<String, T>,
    ) -> Result<()> {
        with_transaction!(self.pool, |mut tx| {
            sqlx::query(&format!("DELETE FROM {}", table))
                .execute(&mut *tx)
                .await?;
            for (id, item) in map {
                let json_data = serde_json::to_string(item)?;
                sqlx::query(&format!("INSERT INTO {} (id, data) VALUES (?, ?)", table))
                    .bind(id)
                    .bind(json_data)
                    .execute(&mut *tx)
                    .await?;
            }
            Ok(())
        })
    }

    async fn fetch_optional_bound<T, B>(&self, query: &'static str, bind: B) -> Result<Option<T>>
    where
        T: for<'r> sqlx::FromRow<'r, SqliteRow> + Send + Unpin,
        B: sqlx::Encode<'static, Sqlite> + sqlx::Type<Sqlite> + Send + 'static,
    {
        Ok(sqlx::query_as::<_, T>(query)
            .bind(bind)
            .fetch_optional(&self.pool)
            .await?)
    }

    async fn fetch_all_bound<T, B>(&self, query: &'static str, bind: B) -> Result<Vec<T>>
    where
        T: for<'r> sqlx::FromRow<'r, SqliteRow> + Send + Unpin,
        B: sqlx::Encode<'static, Sqlite> + sqlx::Type<Sqlite> + Send + 'static,
    {
        Ok(sqlx::query_as::<_, T>(query)
            .bind(bind)
            .fetch_all(&self.pool)
            .await?)
    }

    async fn fetch_all<T>(&self, query: &str) -> Result<Vec<T>>
    where
        T: for<'r> sqlx::FromRow<'r, SqliteRow> + Send + Unpin,
    {
        Ok(sqlx::query_as::<_, T>(query).fetch_all(&self.pool).await?)
    }

    async fn fetch_scalar_bound<T, B>(
        &self,
        query: &'static str,
        bind: B,
    ) -> Result<Option<T>, sqlx::Error>
    where
        T: for<'r> sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite> + Send + Unpin,
        B: sqlx::Encode<'static, Sqlite> + sqlx::Type<Sqlite> + Send + 'static,
    {
        sqlx::query_scalar::<_, T>(query)
            .bind(bind)
            .fetch_optional(&self.pool)
            .await
    }

    async fn tx_execute_bound<B>(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        query: &'static str,
        bind: B,
    ) -> Result<(), sqlx::Error>
    where
        B: sqlx::Encode<'static, Sqlite> + sqlx::Type<Sqlite> + Send + 'static,
    {
        sqlx::query(query).bind(bind).execute(&mut **tx).await?;
        Ok(())
    }

    pub fn get_pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent()
            && !parent.exists()
        {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create database directory '{}': {}",
                    parent.display(),
                    e
                )
            })?;
        }

        // SqliteConnectOptions avoids URL parsing issues with absolute paths; enable
        // foreign keys for integrity.
        let connect_options = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            // Generous busy timeout (sqlx's default is only 5s): under parallel
            // test/CI load, a short write transaction can legitimately wait for
            // the WAL write lock longer than that and must not fail with
            // SQLITE_BUSY — the lock is held by another quick write, not by a
            // deadlock. Write transactions in this codebase are short (media
            // extraction runs OUTSIDE transactions), so 30s of busy-waiting is
            // effectively unbounded contention coverage.
            .busy_timeout(std::time::Duration::from_secs(30))
            // Emit query logs at TRACE instead of the default DEBUG so they only appear
            // when JUMBIE_LOG_LEVEL=trace. The global log level is the single source of truth.
            .log_statements(log::LevelFilter::Trace);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(connect_options)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to connect to database at '{}': {}",
                    db_path.display(),
                    e
                )
            })?;

        let manager = Self {
            pool,
            mappings_cache_gen: AtomicU64::new(0),
            mappings_cache: tokio::sync::Mutex::new(None),
            ui_prefs_cache: tokio::sync::RwLock::new(None),
        };

        manager.run_migrations().await?;

        Ok(manager)
    }

    pub async fn run_migrations(&self) -> Result<()> {
        tracing::info!("Running database migrations...");

        // Consolidated single-migration schema; `CREATE TABLE IF NOT EXISTS` inside
        // makes re-application on the upgrade path a safe no-op.
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to run database migration: {}", e))?;

        // Data migration: rename use_season_folders → flatten_season_folders.
        // The semantics are inverted, so existing values must be flipped.
        self.migrate_flatten_season_folders().await?;

        // Data migration: destination_roots strings → objects with
        // `include_subdirs_in_managed` (see `DestinationRoot`).
        self.migrate_destination_roots_to_objects().await?;

        Ok(())
    }

    /// One-time data migration: renames `use_season_folders` → `flatten_season_folders`,
    /// inverting the boolean (old `true` = use folders, new `true` = flatten).
    async fn migrate_flatten_season_folders(&self) -> Result<()> {
        let marker = "data_migration_flatten_season_folders_v1";
        if self.get_system_state(marker).await?.is_some() {
            return Ok(()); // already applied
        }

        let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, data FROM series_mappings")
            .fetch_all(&self.pool)
            .await?;

        let mut updated = 0usize;
        for (id, data) in &rows {
            if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(data)
                && let Some(obj) = val.as_object_mut()
                && let Some(old) = obj.remove("use_season_folders")
            {
                let flipped = match old {
                    serde_json::Value::Bool(b) => serde_json::Value::Bool(!b),
                    _ => old,
                };
                // Only insert if the new key isn't already present
                if !obj.contains_key("flatten_season_folders") {
                    obj.insert("flatten_season_folders".to_string(), flipped);
                }
                let new_data = serde_json::to_string(&val)?;
                sqlx::query("UPDATE series_mappings SET data = ? WHERE id = ?")
                    .bind(&new_data)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                updated += 1;
            }
        }

        if updated > 0 {
            tracing::info!(
                "Migrated {} series_mappings: use_season_folders → flatten_season_folders (value inverted)",
                updated,
            );
        }

        self.set_system_state(marker, "done").await?;
        Ok(())
    }

    /// One-time data migration: `organization.destination_roots` changed from an
    /// array of bare path strings to an array of objects carrying
    /// `include_subdirs_in_managed`. Rewrite each stored string into the object
    /// form (flag = true, matching the previous always-include behavior) so
    /// installs created before per-root settings keep loading.
    async fn migrate_destination_roots_to_objects(&self) -> Result<()> {
        let marker = "data_migration_destination_roots_objects_v1";
        if self.get_system_state(marker).await?.is_some() {
            return Ok(()); // already applied
        }

        let row: Option<(String,)> =
            sqlx::query_as("SELECT data FROM config_defaults WHERE key = 'organization'")
                .fetch_optional(&self.pool)
                .await?;

        let mut migrated = 0usize;
        if let Some((data,)) = row
            && let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&data)
            && let Some(roots) = val
                .get_mut("destination_roots")
                .and_then(|v| v.as_array_mut())
        {
            for entry in roots.iter_mut() {
                if let Some(path) = entry.as_str().map(str::to_string) {
                    *entry = serde_json::json!({
                        "path": path,
                        "include_subdirs_in_managed": true,
                    });
                    migrated += 1;
                }
            }

            if migrated > 0 {
                let new_data = serde_json::to_string(&val)?;
                sqlx::query("UPDATE config_defaults SET data = ? WHERE key = 'organization'")
                    .bind(&new_data)
                    .execute(&self.pool)
                    .await?;
            }
        }

        if migrated > 0 {
            tracing::info!(
                "Migrated {} destination_roots entries to object form",
                migrated,
            );
        }

        self.set_system_state(marker, "done").await?;
        Ok(())
    }

    /// Perform VACUUM to reclaim unused space
    pub async fn vacuum(&self) -> Result<()> {
        exec!(self, "VACUUM")
    }

    /// Perform ANALYZE to update query planner statistics
    pub async fn analyze(&self) -> Result<()> {
        exec!(self, "ANALYZE")
    }

    pub async fn stats(&self) -> Result<DatabaseStats> {
        let stats: (i64, i64, i64, i64) =
            sqlx::query_as("SELECT page_count, freelist_count, page_size, page_count * page_size FROM pragma_page_count, pragma_freelist_count, pragma_page_size")
                .fetch_one(&self.pool)
                .await?;

        Ok(DatabaseStats {
            total_size_bytes: stats.3,
            wasted_bytes: stats.1 * stats.2,
            page_count: stats.0,
            freelist_count: stats.1,
        })
    }

    /// Generic helper to fetch key-value pairs where the value is JSON-encoded
    async fn fetch_json_map<K, V>(&self, query: &str) -> Result<std::collections::HashMap<K, V>>
    where
        K: std::hash::Hash
            + std::cmp::Eq
            + Send
            + Unpin
            + for<'r> sqlx::Decode<'r, sqlx::Sqlite>
            + sqlx::Type<sqlx::Sqlite>,
        V: serde::de::DeserializeOwned,
    {
        let rows: Vec<(K, String)> = sqlx::query_as(query).fetch_all(&self.pool).await?;
        let mut map = std::collections::HashMap::new();
        for (key, data) in rows {
            if let Ok(val) = serde_json::from_str(&data) {
                map.insert(key, val);
            }
        }
        Ok(map)
    }

    /// Create a backup of the database using VACUUM INTO
    pub async fn backup(&self, backup_path: &Path) -> Result<()> {
        let backup_str = backup_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid backup path"))?;

        // Bound via parameter to prevent SQL injection.
        exec_by_id!(self, "VACUUM INTO ?", backup_str)
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    use sqlx::sqlite::SqliteConnection;
    use std::time::Duration as StdDuration;

    /// Regression test for the macOS-only flake in
    /// `test_batch_assign_files_sequential`, which failed with
    /// `error returned from database: (code: 517) database is locked`
    /// (SQLITE_BUSY_SNAPSHOT).
    ///
    /// The handler reads (SELECT) before it writes (UPDATE) inside a
    /// transaction. With a deferred `BEGIN`, the read lock was taken first;
    /// a concurrent writer could commit in between and invalidate the read
    /// snapshot, making the later write fail with code 517 — an error
    /// `busy_timeout` cannot retry. `with_transaction!` must therefore take
    /// the write lock up front via `BEGIN IMMEDIATE`.
    ///
    /// This asserts that structural property directly: once the transaction
    /// has performed its first read, a separate connection can no longer
    /// acquire the write lock (it fails fast with SQLITE_BUSY instead of
    /// committing and invalidating our snapshot).
    #[tokio::test]
    async fn with_transaction_holds_write_lock_while_reading() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = DbManager::new(&db_path).await.unwrap();
        let pool = db.get_pool();

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS lock_probe (id INTEGER PRIMARY KEY, v INTEGER NOT NULL)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO lock_probe (id, v) VALUES (1, 0)")
            .execute(pool)
            .await
            .unwrap();

        // Independent connection with a short busy timeout so the probe fails
        // fast rather than waiting out the 30s production timeout.
        let mut writer: SqliteConnection = SqliteConnectOptions::new()
            .filename(&db_path)
            .busy_timeout(StdDuration::from_millis(100))
            .connect()
            .await
            .unwrap();

        let res: Result<(), sqlx::Error> = async {
            with_transaction!(pool, |mut tx| {
                // Read first: with a deferred BEGIN this is what created the
                // snapshot that could later go stale.
                let _: (i64,) = sqlx::query_as("SELECT v FROM lock_probe WHERE id = 1")
                    .fetch_one(&mut *tx)
                    .await?;

                // The immediate write lock is already held, so this concurrent
                // write must be rejected rather than commit and invalidate us.
                let concurrent_write =
                    sqlx::query("UPDATE lock_probe SET v = v + 1 WHERE id = 1")
                        .execute(&mut writer)
                        .await;
                assert!(
                    concurrent_write.is_err(),
                    "concurrent writer should be blocked by the immediate write lock; \
                     a deferred BEGIN would let it commit and later yield SQLITE_BUSY_SNAPSHOT (517)"
                );

                sqlx::query("UPDATE lock_probe SET v = v + 1 WHERE id = 1")
                    .execute(&mut *tx)
                    .await?;
                Ok(())
            })
        }
        .await;

        res.expect("with_transaction should succeed");

        let v: (i64,) = sqlx::query_as("SELECT v FROM lock_probe WHERE id = 1")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(v.0, 1);
    }

    #[test]
    fn test_normalize_sql_path_prefix() {
        assert_eq!(normalize_sql_path_prefix("/data/tv/Show/"), "/data/tv/Show");
        assert_eq!(normalize_sql_path_prefix("/data/tv/Show"), "/data/tv/Show");
        assert_eq!(
            normalize_sql_path_prefix(r"C:\media\tv\Show\"),
            "C:/media/tv/Show"
        );
        assert_eq!(
            normalize_sql_path_prefix(r"C:\media\tv\Show"),
            "C:/media/tv/Show"
        );
        assert_eq!(
            normalize_sql_path_prefix(r"C:/media\tv/Show/\"),
            "C:/media/tv/Show"
        );
    }

    #[test]
    fn test_sql_child_path_like_pattern() {
        assert_eq!(
            sql_child_path_like_pattern("/data/tv/Show/"),
            "/data/tv/Show/%"
        );
        assert_eq!(
            sql_child_path_like_pattern(r"C:\media\tv\Show\"),
            "C:/media/tv/Show/%"
        );
        assert_eq!(
            sql_child_path_like_pattern(r"C:\media\tv\Show"),
            "C:/media/tv/Show/%"
        );
    }
}

