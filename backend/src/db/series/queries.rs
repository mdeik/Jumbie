// Series query operations: mappings, stats, episode details, metadata seasons,
// fetch status.

use crate::db::DbManager;
use crate::db::{CalendarEpisodeRow, EpisodeDetailRow, SeriesStatRow};
use anyhow::Result;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

impl DbManager {
    // Series mappings are JSON blobs in a simple key-value table (id, data);
    // MappingRule evolves frequently, so a blob avoids a migration per field.
    // fetch_json_map deserializes all rows into a HashMap in one round trip.

    pub async fn get_all_series_mappings(
        &self,
    ) -> Result<std::collections::HashMap<String, jumbie_shared::types::MappingRule>> {
        let current_gen = self.mappings_cache_gen.load(Ordering::Acquire);

        // Fast path: try the cache before acquiring the DB lock.
        {
            let cache = self.mappings_cache.lock().await;
            if let Some((cached_gen, ref cached_mappings)) = *cache
                && cached_gen == current_gen
            {
                // Cache hit — cheap clone of the Arc-inner HashMap.
                return Ok(Arc::clone(cached_mappings).as_ref().clone());
            }
        }

        // Cache miss or stale — load everything from the database.
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, data FROM series_mappings")
            .fetch_all(&self.pool)
            .await?;
        let mut map = std::collections::HashMap::new();
        for (id, data) in rows {
            if let Ok(mut mapping) =
                serde_json::from_str::<jumbie_shared::types::MappingRule>(&data)
            {
                // Backfill legacy mappings that have empty series_id.
                // Persist the new UUID so the fix is permanent — future
                // lookups won't need to regenerate.
                if mapping.ensure_series_id() {
                    self.upsert_series_mapping(&id, &mapping).await?;
                }
                map.insert(id, mapping);
            }
        }

        // Read the current generation *after* any fix-up upserts above,
        // which may have incremented it.  This ensures the cached
        // generation matches post-fix-up state.
        let final_gen = self.mappings_cache_gen.load(Ordering::Acquire);
        let map_arc = Arc::new(map);
        let result_map = Arc::clone(&map_arc).as_ref().clone();
        let mut cache = self.mappings_cache.lock().await;
        *cache = Some((final_gen, map_arc));
        Ok(result_map)
    }

    /// Lightweight alternative to `get_all_series_mappings` that returns only
    /// series IDs without loading or deserializing any JSON data.
    ///
    /// Used by the Release Date Estimator to iterate over all series without
    /// the overhead of parsing every MappingRule blob.  This also avoids the
    /// side-effect backfill that `get_all_series_mappings` performs.
    pub async fn get_all_series_ids(&self) -> Result<Vec<String>> {
        let ids = sqlx::query_scalar::<_, String>("SELECT id FROM series_mappings")
            .fetch_all(&self.pool)
            .await?;
        Ok(ids)
    }

    pub async fn get_series_mapping(
        &self,
        id: &str,
    ) -> Result<Option<jumbie_shared::types::MappingRule>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT data FROM series_mappings WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((data,)) = row {
            let mut mapping: jumbie_shared::types::MappingRule = match serde_json::from_str(&data) {
                Ok(m) => m,
                Err(_) => return Ok(None),
            };
            // Backfill legacy mapping with empty series_id.
            // Persist the new UUID so the fix is permanent.
            if mapping.ensure_series_id() {
                self.upsert_series_mapping(id, &mapping).await?;
            }
            Ok(Some(mapping))
        } else {
            Ok(None)
        }
    }

    /// Batch variant of [`get_series_mapping`]: fetches multiple mappings in one
    /// round-trip, chunked to avoid SQLite's parameter limit.
    pub async fn get_series_mappings_batch(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, jumbie_shared::types::MappingRule>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        const CHUNK: usize = 500;
        let mut out = std::collections::HashMap::new();

        for chunk in ids.chunks(CHUNK) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id, data FROM series_mappings WHERE id IN ({})",
                placeholders
            );
            let mut q = sqlx::query_as::<_, (String, String)>(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows: Vec<(String, String)> = q.fetch_all(&self.pool).await?;

            for (id, data) in rows {
                if let Ok(mut mapping) =
                    serde_json::from_str::<jumbie_shared::types::MappingRule>(&data)
                {
                    // Backfill legacy mappings (same as get_series_mapping / get_all_series_mappings)
                    if mapping.ensure_series_id() {
                        self.upsert_series_mapping(&id, &mapping).await?;
                    }
                    out.insert(id, mapping);
                }
            }
        }
        Ok(out)
    }

    /// Lookup by UUID first, then fall back to scanning name/target_title/aliases.
    /// The slow path is O(n) but only used for human-readable series names.
    pub async fn get_mapping_by_key(
        &self,
        series_key: &str,
    ) -> Result<Option<(String, jumbie_shared::types::MappingRule)>> {
        if let Some(rule) = self.get_series_mapping(series_key).await? {
            return Ok(Some((series_key.to_string(), rule)));
        }
        let all = self.get_all_series_mappings().await?;

        // SSoT: delegate to the same lookup logic used by process_entry_with_mappings.
        // The in-memory matching logic lives in lookup_mapping_in — any change to
        // alias/name matching semantics happens in one place.
        Ok(crate::source_processor::lookup_mapping_in(series_key, &all))
    }

    // Series stats. `wanted_missing_sql` is a broad SQL-level filter matching the
    // pattern in `get_wanted_episodes()`: it ORs the date sources against
    // `datetime('now')`. It already respects episode_end because should_monitor_episode()
    // sets out-of-range episodes monitored = 0. The precise status is computed in Rust
    // (`derive_episode_status`). Shared by get_series_stats and *_with_seasons.
    fn wanted_missing_sql() -> String {
        format!(
            r#"CASE WHEN e.monitored = 1
                                              AND (
                                                   (e.meta_date IS NOT NULL AND e.meta_date <= datetime('now'))
                                                OR (e.est_date IS NOT NULL AND e.est_date <= datetime('now'))
                                              )
                                              AND NOT (e.status IN ('downloaded', 'organized') OR {}) THEN 1 ELSE 0 END"#,
            crate::db::EPISODE_ASSIGNED_PREDICATE,
        )
    }

    pub async fn get_series_stats(&self) -> Result<Vec<(String, String, i64, i64, i64)>> {
        let sql = format!(
            "SELECT e.series_id, COALESCE(sm.target_title, ''),
                    SUM(CASE WHEN e.status IN ('downloaded', 'organized') OR {assigned} THEN 1 ELSE 0 END),
                    COALESCE(sm.total_size, 0),
                    SUM({wanted})
             FROM episodes e
             LEFT JOIN series_mappings sm ON e.series_id = sm.id
             LEFT JOIN season_overrides so
                   ON e.series_id = so.series_id
                  AND e.season = so.season
                  AND e.numbering_mode = so.numbering_mode
             WHERE e.episode_cell_type IS NOT 0
               AND (so.episode_start IS NULL OR so.episode_start <= e.episode)
               AND (so.episode_end   IS NULL OR so.episode_end   >= e.episode)
             GROUP BY e.series_id",
            assigned = crate::db::EPISODE_ASSIGNED_PREDICATE,
            wanted = Self::wanted_missing_sql(),
        );
        let rows = sqlx::query_as::<_, (String, String, i64, i64, i64)>(&sql)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    pub async fn get_series_stats_with_seasons(&self) -> Result<Vec<SeriesStatRow>> {
        let sql = format!(
            "SELECT e.series_id, COALESCE(sm.target_title, ''),
                    SUM(CASE WHEN e.status IN ('downloaded', 'organized') OR {assigned} THEN 1 ELSE 0 END),
                    COUNT(e.episode_id),
                    COALESCE(sm.total_size, 0),
                    SUM({wanted}),
                    GROUP_CONCAT(DISTINCT e.season)
             FROM episodes e
             LEFT JOIN series_mappings sm ON e.series_id = sm.id
             LEFT JOIN season_overrides so
                   ON e.series_id = so.series_id
                  AND e.season = so.season
                  AND e.numbering_mode = so.numbering_mode
             WHERE e.episode_cell_type IS NOT 0
               AND (so.episode_start IS NULL OR so.episode_start <= e.episode)
               AND (so.episode_end   IS NULL OR so.episode_end   >= e.episode)
             GROUP BY e.series_id",
            assigned = crate::db::EPISODE_ASSIGNED_PREDICATE,
            wanted = Self::wanted_missing_sql(),
        );
        let rows = sqlx::query_as::<_, (String, String, i64, i64, i64, i64, Option<String>)>(&sql)
            .fetch_all(&self.pool)
            .await?;

        let result = rows
            .into_iter()
            .map(
                |(
                    series_id,
                    title,
                    downloaded_count,
                    total_count,
                    size,
                    monitored_missing_count,
                    seasons_str,
                )| {
                    let mut seasons: Vec<String> = seasons_str
                        .map(|s| s.split(',').map(|ss| ss.to_string()).collect())
                        .unwrap_or_default();
                    seasons.sort();
                    SeriesStatRow {
                        series_id,
                        series_title: title,
                        downloaded_count,
                        total_count,
                        size,
                        monitored_missing_count,
                        seasons,
                    }
                },
            )
            .collect();

        Ok(result)
    }

    pub async fn get_series_seasons(&self, series_id: &str) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar::<_, i32>(
            "SELECT DISTINCT season FROM episodes WHERE series_id = ? ORDER BY season",
        )
        .bind(series_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|s| s.to_string()).collect())
    }

    /// Batch variant of get_series_seasons, returning a map keyed by series_id.
    pub async fn get_series_seasons_batch(
        &self,
        series_ids: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<String>>> {
        if series_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        const CHUNK: usize = 500;
        let mut out: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for chunk in series_ids.chunks(CHUNK) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT e.series_id, e.season
                 FROM episodes e
                 WHERE e.series_id IN ({})
                 GROUP BY e.series_id, e.season
                 ORDER BY e.series_id, e.season",
                placeholders
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for row in rows {
                let sid: String = row.get("series_id");
                let season: i32 = row.get("season");
                out.entry(sid).or_default().push(season.to_string());
            }
        }
        Ok(out)
    }

    // Fetches all episode rows for a series in the given numbering mode; the
    // numbering_mode column (0=normal, 1=absolute) is index-friendly and unambiguous.
    pub async fn get_series_episodes_details(
        &self,
        series_id: &str,
        absolute: bool,
    ) -> Result<Vec<EpisodeDetailRow>> {
        let rows = sqlx::query_as::<_, EpisodeDetailRow>(&format!(
            "SELECT e.episode_id, e.season, e.episode, e.status, f.file_path, \
                    rm.release_title AS release_title, \
                    COALESCE(f.size, 0) as size, e.title, \
                    e.quality_profile_id, \
                    {submitter} AS submitter, \
                    c.media_info, c.fingerprint AS quick_hash, \
                    e.created_at, e.file_acquired_at, e.monitored, e.meta_date, \
                    {upload_date} AS upload_date, e.est_date, e.metadata_ids, \
                    e.description, e.runtime, e.image_url, \
                    rm.download_id AS download_id, rm.score AS score, e.numbering_mode, e.metadata_source, \
                    rm.download_link AS download_link, e.series_id, e.monitor_override, \
                    {original_path} AS original_path \
                             FROM episodes e \
                             {join} \
                             WHERE e.series_id = ? AND e.numbering_mode = ? \
             ORDER BY e.season, e.episode",
            submitter = crate::db::SUBMITTER_EXPR,
            join = crate::db::EPISODE_FILE_JOIN,
            original_path = crate::db::ORIGINAL_PATH_EXPR,
            upload_date = crate::db::UPLOAD_DATE_EXPR,
        ))
        .bind(series_id)
        .bind(absolute as i32)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Batch variant of get_series_episodes_details. All series_ids use the SAME
    /// numbering mode; callers needing mixed modes call this twice.
    pub async fn get_series_episodes_details_batch(
        &self,
        series_ids: &[String],
        absolute: bool,
    ) -> Result<std::collections::HashMap<String, Vec<EpisodeDetailRow>>> {
        if series_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let numbering_mode = absolute as i32;
        const CHUNK: usize = 500;
        let mut out: std::collections::HashMap<String, Vec<EpisodeDetailRow>> =
            std::collections::HashMap::new();
        for chunk in series_ids.chunks(CHUNK) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "SELECT e.series_id,
                    e.episode_id,
                    e.season,
                    e.episode,
                    e.status,
                    f.file_path,
                    rm.release_title AS release_title,
                    COALESCE(f.size, 0) as size,
                    e.title,
                    e.quality_profile_id,
                    {submitter} AS submitter,
                    c.media_info,
                    c.fingerprint AS quick_hash,
                    e.created_at,
                    e.file_acquired_at,
                    e.monitored,
                    e.meta_date,
                    {upload_date} AS upload_date,
                    e.est_date,
                    e.metadata_ids,
                    e.description,
                    e.runtime,
                    e.image_url,
                    rm.download_id AS download_id,
                    rm.score AS score,
                    e.numbering_mode,
                    e.metadata_source,
                    rm.download_link AS download_link,
                    e.series_id,
                    e.monitor_override,
                    {original_path} AS original_path
             FROM episodes e
             {join}
             WHERE e.series_id IN ({episode_ids}) AND e.numbering_mode = ?
             ORDER BY e.series_id, e.season, e.episode",
                episode_ids = placeholders,
                submitter = crate::db::SUBMITTER_EXPR,
                join = crate::db::EPISODE_FILE_JOIN,
                original_path = crate::db::ORIGINAL_PATH_EXPR,
                upload_date = crate::db::UPLOAD_DATE_EXPR,
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            q = q.bind(numbering_mode);
            let rows = q.fetch_all(&self.pool).await?;
            for row in rows {
                let series_id: String = row.get("series_id");
                let detail = EpisodeDetailRow {
                    episode_id: row.get("episode_id"),
                    season: row.get("season"),
                    episode: row.get("episode"),
                    status: row.get("status"),
                    file_path: row.get("file_path"),
                    release_title: row.get("release_title"),
                    size: row.get("size"),
                    title: row.get("title"),
                    quality_profile_id: row.get("quality_profile_id"),
                    submitter: row.get("submitter"),
                    media_info: row.get("media_info"),
                    quick_hash: row.get("quick_hash"),
                    created_at: row.get("created_at"),
                    file_acquired_at: row.get("file_acquired_at"),
                    monitored: row.get("monitored"),
                    meta_date: row.get("meta_date"),
                    metadata_ids: row.get("metadata_ids"),
                    description: row.get("description"),
                    runtime: row.get("runtime"),
                    image_url: row.get("image_url"),
                    upload_date: row.get("upload_date"),
                    est_date: row.get("est_date"),
                    download_id: row.get("download_id"),
                    score: row.get("score"),
                    numbering_mode: row.get("numbering_mode"),
                    metadata_source: row.get("metadata_source"),
                    download_link: row.get("download_link"),
                    series_id: row.get("series_id"),
                    monitor_override: row.get("monitor_override"),
                    original_path: row.get("original_path"),
                };
                out.entry(series_id).or_default().push(detail);
            }
        }
        Ok(out)
    }

    // Rename-plan (lean) queries. CONTRACT: these populate EXACTLY the fields the
    // plan pipeline reads (episode_id, season, episode, file_path, title, submitter,
    // media_info, created_at, meta_date, metadata_source, series_id); every other
    // EpisodeDetailRow field is default. Enforced in both directions by the drift
    // test `rename_plan_query_populates_exactly_the_contract_fields`.
    pub async fn get_rename_plan_episodes(
        &self,
        series_id: &str,
        absolute: bool,
    ) -> Result<Vec<EpisodeDetailRow>> {
        let rows = sqlx::query_as::<_, EpisodeDetailRow>(&format!(
            "SELECT e.episode_id, e.season, e.episode, f.file_path, e.title, \
                {submitter} AS submitter, \
                c.media_info, e.created_at, e.meta_date, e.metadata_source, \
                e.series_id \
             FROM episodes e \
             {join} \
             WHERE e.series_id = ? AND e.numbering_mode = ? \
             ORDER BY e.season, e.episode",
            submitter = crate::db::SUBMITTER_EXPR,
            join = crate::db::EPISODE_FILE_JOIN,
        ))
        .bind(series_id)
        .bind(absolute as i32)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Batch variant of [`get_rename_plan_episodes`] — same contract, all series
    /// share the same numbering mode; callers with mixed modes call this twice.
    /// Uses the same chunked-IN pattern as the fat batch query to stay under
    /// SQLite's parameter limit.
    pub async fn get_rename_plan_episodes_batch(
        &self,
        series_ids: &[String],
        absolute: bool,
    ) -> Result<std::collections::HashMap<String, Vec<EpisodeDetailRow>>> {
        if series_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let numbering_mode = absolute as i32;
        const CHUNK: usize = 500;
        let mut out: std::collections::HashMap<String, Vec<EpisodeDetailRow>> =
            std::collections::HashMap::new();
        for chunk in series_ids.chunks(CHUNK) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "SELECT e.series_id, \
                    e.episode_id, e.season, e.episode, f.file_path, e.title, \
                    {submitter} AS submitter, \
                    c.media_info, e.created_at, e.meta_date, e.metadata_source \
                 FROM episodes e \
                 {join} \
                 WHERE e.series_id IN ({episode_ids}) AND e.numbering_mode = ? \
                 ORDER BY e.series_id, e.season, e.episode",
                episode_ids = placeholders,
                submitter = crate::db::SUBMITTER_EXPR,
                join = crate::db::EPISODE_FILE_JOIN,
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            q = q.bind(numbering_mode);
            let rows = q.fetch_all(&self.pool).await?;
            for row in rows {
                let series_id: String = row.get("series_id");
                let detail = EpisodeDetailRow {
                    episode_id: row.get("episode_id"),
                    season: row.get("season"),
                    episode: row.get("episode"),
                    file_path: row.get("file_path"),
                    title: row.get("title"),
                    submitter: row.get("submitter"),
                    media_info: row.get("media_info"),
                    created_at: row.get("created_at"),
                    meta_date: row.get("meta_date"),
                    metadata_source: row.get("metadata_source"),
                    series_id: row.get("series_id"),
                    // Fields omitted by contract → defaults (see struct docs)
                    status: None,
                    release_title: None,
                    size: 0,
                    quality_profile_id: None,
                    quick_hash: None,
                    original_path: None,
                    file_acquired_at: None,
                    monitored: false,
                    upload_date: None,
                    est_date: None,
                    metadata_ids: None,
                    description: None,
                    runtime: None,
                    image_url: None,
                    download_id: None,
                    score: None,
                    numbering_mode: 0,
                    download_link: None,
                    monitor_override: false,
                };
                out.entry(series_id).or_default().push(detail);
            }
        }
        Ok(out)
    }

    // Calendar Episodes.
    //
    // Range selection is a raw-date SUPERSET: an episode is a candidate when ANY of
    // `meta_date`, `upload_date` or `est_date` falls in the window. The effective date
    // is resolved per the user's current date-source configuration (priority + enabled
    // flags) and may be any of the three. Filtering by a resolved effective date in SQL
    // would wrongly exclude an episode whose currently selected source is out of range
    // while another possible source is in range. So the superset is intentional;
    // `get_calendar` applies the user's preference to the returned rows.
    //
    // Ordering is a stable chronological approximation over the raw dates; the display
    // uses the resolved effective date.
    pub async fn get_calendar_episodes(
        &self,
        start_date: chrono::NaiveDateTime,
        end_date: chrono::NaiveDateTime,
        global_absolute: bool,
    ) -> Result<Vec<CalendarEpisodeRow>> {
        let rows = sqlx::query_as::<_, CalendarEpisodeRow>(&format!(
            "SELECT e.series_id,
                    COALESCE(sm.target_title, '') AS series_title,
                    e.episode_id, e.season, e.episode, e.title,
                    e.meta_date AS meta_date,
                    {upload_date} AS upload_date,
                    e.est_date,
                    e.status, f.file_path, e.monitored,
                    e.numbering_mode,
                    e.runtime,
                    c.media_info
             FROM episodes e
             LEFT JOIN series_mappings sm ON e.series_id = sm.id
             {join}
             WHERE (
                       (e.meta_date >= ? AND e.meta_date <= ?)
                    OR ({upload_date} >= ? AND {upload_date} <= ?)
                    OR (e.est_date >= ? AND e.est_date <= ?)
                   )
               -- SSoT: the series tristate lives in the JSON `data` column; the
               -- materialized absolute_numbering column cannot distinguish None
               -- from Some(false), so None (use global) falls back to the global
               -- default passed in here.
               AND e.numbering_mode = CASE
                       WHEN json_extract(sm.data, '$.settings.absolute_numbering') = 'true' THEN 1
                       WHEN json_extract(sm.data, '$.settings.absolute_numbering') = 'false' THEN 0
                       ELSE ? END
             ORDER BY COALESCE(e.meta_date, upload_date, e.est_date) ASC, series_title ASC",
            join = crate::db::EPISODE_FILE_JOIN,
            upload_date = crate::db::UPLOAD_DATE_EXPR,
        ))
        .bind(start_date)
        .bind(end_date)
        .bind(start_date)
        .bind(end_date)
        .bind(start_date)
        .bind(end_date)
        .bind(global_absolute as i32)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_series_episode_count(&self, series_id: &str) -> Result<usize> {
        let count: i64 = self
            .fetch_scalar_bound(
                "SELECT COUNT(*) FROM episodes WHERE series_id = ?",
                series_id.to_string(),
            )
            .await?
            .unwrap_or(0);

        Ok(count as usize)
    }

    // Metadata Season Cache Query
    // Metadata season cache: (season_number, episode_count) tuples for a specific
    // provider + mode. The opposite-mode variant is a fallback for when a user has
    // metadata in one mode but the UI requests the other (e.g. after toggling
    // absolute numbering) — episode counts are usually the same between modes.
    pub async fn get_metadata_season_cache(
        &self,
        plugin_series_id: &str,
        plugin_id: &str,
        instance_id: &str,
        ordering_mode: &str,
    ) -> anyhow::Result<Vec<(String, i32)>> {
        let rows = sqlx::query_as::<_, (String, i32)>(
            "SELECT season_number, episode_count FROM metadata_season_cache
             WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ? AND ordering_mode = ?
             ORDER BY season_number",
        )
        .bind(plugin_series_id)
        .bind(plugin_id)
        .bind(instance_id)
        .bind(ordering_mode)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Batch-fetches metadata season cache for multiple (metadata_id, plugin_id, instance_id, ordering_mode)
    /// tuples in a single SQL query. Returns a HashMap keyed by metadata_id, where each
    /// value is a Vec of (season_number, episode_count).
    pub async fn get_metadata_season_cache_batch(
        &self,
        queries: &[(String, String, String, String)],
    ) -> anyhow::Result<std::collections::HashMap<String, Vec<(String, i32)>>> {
        if queries.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        // Build a WHERE clause with one set of positional ? params per query tuple.
        let clauses: Vec<&str> = std::iter::repeat_n(
            "(metadata_id = ? AND plugin_id = ? AND instance_id = ? AND ordering_mode = ?)",
            queries.len(),
        )
        .collect();

        let sql = format!(
            "SELECT metadata_id, season_number, episode_count FROM metadata_season_cache \
             WHERE {} ORDER BY metadata_id, season_number",
            clauses.join(" OR ")
        );

        let mut query = sqlx::query_as::<_, (String, String, i32)>(&sql);
        for (metadata_id, plugin_id, instance_id, ordering_mode) in queries {
            query = query
                .bind(metadata_id)
                .bind(plugin_id)
                .bind(instance_id)
                .bind(ordering_mode);
        }

        let rows = query.fetch_all(&self.pool).await?;

        let mut result: std::collections::HashMap<String, Vec<(String, i32)>> =
            std::collections::HashMap::new();
        for (metadata_id, season_number, episode_count) in rows {
            result
                .entry(metadata_id)
                .or_default()
                .push((season_number, episode_count));
        }
        Ok(result)
    }

    // Metadata fetch log: tracks whether a fetch has been attempted for a given
    // (provider, mode), so the fetch loop doesn't repeatedly retry a show that a
    // provider doesn't have (or whose ID is wrong).
    pub async fn has_fetch_been_attempted(
        &self,
        plugin_series_id: &str,
        plugin_id: &str,
        instance_id: &str,
        ordering_mode: &str,
    ) -> bool {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM metadata_fetch_log
             WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ? AND ordering_mode = ?",
        )
        .bind(plugin_series_id)
        .bind(plugin_id)
        .bind(instance_id)
        .bind(ordering_mode)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0)
            > 0
    }

    /// Fetch per-season organized and expected episode counts for all series at once.
    ///
    /// Returns `series_id → [SeasonEpisodeCount]` (season-ascending). Callers that
    /// only need series-wide totals sum the seasons, so the totals and the
    /// per-season breakdown can never disagree (SSoT).
    ///
    /// `organized` = episodes that own a main file, across the whole season (files
    /// outside the configured range still count, they are real downloads).
    /// `expected` = in-range episodes present (out-of-range episodes and
    /// `cell_count` placeholder cells excluded); callers overlay each season's
    /// configured `cell_count` via [`SeriesSettings::expected_episode_count`] to get
    /// the final expected total. A NULL season (legacy/absolute rows) is counted as
    /// season 1, matching the display default used elsewhere.
    pub async fn get_series_season_completion_counts(
        &self,
    ) -> Result<HashMap<String, Vec<jumbie_shared::types::SeasonEpisodeCount>>> {
        let rows = sqlx::query_as::<_, (String, i32, i32, i32)>(&format!(
            "SELECT e.series_id, COALESCE(e.season, 1) AS season, \
                    COUNT(CASE WHEN {assigned} THEN 1 END) AS organized_count, \
                    COUNT(CASE WHEN (so.episode_start IS NULL OR so.episode_start <= e.episode) \
                                AND (so.episode_end IS NULL OR so.episode_end >= e.episode) \
                                AND e.episode_cell_type IS NOT 0 \
                          THEN 1 END) AS expected_count \
             FROM episodes e \
             LEFT JOIN season_overrides so \
                   ON e.series_id = so.series_id \
                  AND e.season = so.season \
                  AND e.numbering_mode = so.numbering_mode \
             WHERE e.series_id IS NOT NULL AND e.series_id != '' \
             GROUP BY e.series_id, COALESCE(e.season, 1) \
             ORDER BY e.series_id, season",
            assigned = crate::db::EPISODE_ASSIGNED_PREDICATE,
        ))
        .fetch_all(&self.pool)
        .await?;

        let mut out: HashMap<String, Vec<jumbie_shared::types::SeasonEpisodeCount>> =
            HashMap::new();
        for (series_id, season, organized, expected) in rows {
            out.entry(series_id)
                .or_default()
                .push(jumbie_shared::types::SeasonEpisodeCount {
                    season,
                    organized,
                    expected,
                });
        }
        Ok(out)
    }

    /// Returns episodes with release_title and scoring inputs for a set of series,
    /// used by the rescore to recalculate scores when release profiles change.
    pub async fn get_episodes_for_rescore(
        &self,
        series_ids: &[String],
    ) -> Result<Vec<crate::db::EpisodeRescoreRow>> {
        if series_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = series_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        // Uses the same SSoT join as every other episode query, so a multipart
        // episode (whose parts live in `episode_files`/`file_paths`) is rescored from
        // its primary part rather than silently skipped.
        let sql = format!(
            "SELECT e.episode_id, e.series_id, rm.release_title, rm.submitter,
                    rm.scoring_size_bytes,
                    rm.scoring_seeders,
                    rm.scoring_episode_count,
                    {upload_date} AS scoring_upload_date
             FROM episodes e
             {join}
             WHERE e.series_id IN ({episode_ids}) AND rm.release_title IS NOT NULL AND rm.release_title != ''",
            join = crate::db::EPISODE_FILE_JOIN,
            upload_date = crate::db::UPLOAD_DATE_EXPR,
            episode_ids = placeholders,
        );
        let mut q = sqlx::query_as::<_, crate::db::EpisodeRescoreRow>(&sql);
        for id in series_ids {
            q = q.bind(id);
        }
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows)
    }
}
