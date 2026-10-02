// Read-only episode queries (is_*, get_*, count_*, link_*).

use super::merge_activity_rows;
use crate::datetime::{ReleaseDatesExt, UtcDateTime, derive_episode_status};
use crate::db::DbManager;
use crate::db::EpisodeDetailRow;
use crate::models::activity::{ActivityItem, ActivityType};
use anyhow::Result;
use jumbie_shared::formatting::{LabelStyle, fmt_episode, fmt_season_episode};
use jumbie_shared::types::EpisodeStatus;

/// SSoT: the SQL broad-filter for "episode has a release date that is not in
/// the future" — any of the three date columns being `<= now` qualifies.
///
/// Shared by every wanted/search query (`get_wanted_episodes`,
/// `has_wanted_episodes_by_preference`, `get_auto_search_candidates`, and
/// `get_monitored_missing_for_series`) so the definition of "released" lives
/// in ONE place.  Each call site binds `now` twice, in column order
/// (meta_date, est_date). A wanted episode has no file, so it has no source
/// (`upload_date`) date — that is a file-scoped value (see `release_info`).
const RELEASED_DATE_FILTER: &str = "(e.meta_date IS NOT NULL AND e.meta_date <= ?) \
     OR (e.est_date IS NOT NULL AND e.est_date <= ?)";

/// Auxiliary file kind derived from its extension. The association stores only
/// `main`/`auxiliary`; a sidecar's own type is a pure function of its path.
fn aux_kind(path: &str) -> jumbie_shared::media_format::FileKind {
    jumbie_shared::media_format::file_kind_for_path(std::path::Path::new(path))
        .unwrap_or(jumbie_shared::media_format::FileKind::Subtitle)
}

impl DbManager {
    /// `assigned`: the episode owns at least one `main` file (single, multipart part,
    /// or a shared multi-episode file). Pure DB state; it says nothing about the file
    /// being present on disk (that is `disk_present`, resolved at read time).
    pub async fn is_episode_assigned(&self, episode_id: &str) -> Result<bool> {
        let assigned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM episode_files WHERE episode_id = ? AND kind = 'main')",
        )
        .bind(episode_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(assigned)
    }

    /// Returns `true` if no episode currently holds this path as a main file with a
    /// non-missing status. Used during file-move collision resolution: if the
    /// occupant at `dst` is unassigned (its file was cleared by a reassign), we can
    /// rename the occupant out of the way instead of suffixing the incoming file.
    pub async fn is_path_unassigned(&self, path: &std::path::Path) -> Result<bool> {
        let path_str = path.to_string_lossy().to_string();
        let assigned: i64 = sqlx::query_scalar(
            "SELECT COUNT(1) FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             JOIN episodes e ON e.episode_id = ef.episode_id \
             WHERE fp.file_path = ? AND e.status != 'missing'",
        )
        .bind(&path_str)
        .fetch_one(&self.pool)
        .await?;
        Ok(assigned == 0)
    }

    /// The episode's primary main-file path — its non-part file if present, else its
    /// lowest-numbered part. `None` when the episode owns no main file. SSoT for
    /// "the episode's file path" (mirrors [`crate::db::EPISODE_FILE_JOIN`]).
    pub async fn get_episode_file_path(&self, episode_id: &str) -> Result<Option<String>> {
        let sql = format!(
            "SELECT fp.file_path FROM episodes e \
             JOIN file_paths fp ON fp.id = {primary} \
             WHERE e.episode_id = ?",
            primary = crate::db::EPISODE_PRIMARY_FILE_ID,
        );
        let path: Option<String> = sqlx::query_scalar(&sql)
            .bind(episode_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(path)
    }

    /// Map of `episode_id → file_path` for every episode of `series_id` that
    /// currently has a main file, using its **primary** file. One query per series
    /// replaces the per-file lookup in smart-link's permissive path (which uses it
    /// to detect already-present episodes without a round-trip per file).
    pub async fn get_episode_file_paths_for_series(
        &self,
        series_id: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let sql = format!(
            "SELECT e.episode_id, fp.file_path FROM episodes e \
             JOIN file_paths fp ON fp.id = {primary} \
             WHERE e.series_id = ?",
            primary = crate::db::EPISODE_PRIMARY_FILE_ID,
        );
        let rows: Vec<(String, String)> = sqlx::query_as(&sql)
            .bind(series_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().collect())
    }

    pub async fn is_episode_monitored(&self, episode_id: &str) -> Result<bool> {
        let monitored: Option<bool> = self
            .fetch_scalar_bound(
                "SELECT monitored FROM episodes WHERE episode_id = ?",
                episode_id.to_string(),
            )
            .await?;
        Ok(monitored.unwrap_or(true))
    }

    // Batch status query: returns (is_downloaded, is_monitored) per episode_id for
    // the "wanted" view/search results without N+1 queries, chunked at 500 to stay
    // under SQLite's variable limit (999).
    pub async fn get_episodes_status_batch(
        &self,
        episode_ids: &[String],
    ) -> Result<std::collections::HashMap<String, (bool, bool)>> {
        if episode_ids.is_empty() {
            return Ok(Default::default());
        }

        let mut map = std::collections::HashMap::new();

        // One query per chunk: a scalar subquery resolves the primary path and a
        // count of main associations marks downloaded even for a multipart episode.
        for chunk in episode_ids.chunks(500) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "SELECT e.episode_id, e.status, \
                 (SELECT fp.file_path FROM file_paths fp WHERE fp.id = {primary}) as file_path, \
                 e.monitored, \
                 (SELECT COUNT(*) FROM episode_files ef \
                  WHERE ef.episode_id = e.episode_id AND ef.kind = 'main') as part_count \
                 FROM episodes e WHERE e.episode_id IN ({})",
                placeholders,
                primary = crate::db::EPISODE_PRIMARY_FILE_ID,
            );
            let mut q =
                sqlx::query_as::<_, (String, Option<String>, Option<String>, bool, i32)>(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for (id, status, file_path, monitored, part_count) in rows {
                let downloaded = part_count > 0
                    || status.as_deref() == Some(EpisodeStatus::Downloaded.as_str())
                    || status.as_deref() == Some(EpisodeStatus::Organized.as_str())
                    || file_path.map(|p| !p.is_empty()).unwrap_or(false);
                map.insert(id, (downloaded, monitored));
            }
        }

        Ok(map)
    }

    // Orphan files: `file_paths` rows with no episode association (finished but
    // never matched to an episode). The UI offers these for manual assignment.
    //
    // Files recorded in `unmatched_files` (kept-for-review) are excluded — they are
    // deliberately held back, and `unmatched_files` is the SSoT for that decision.

    pub async fn get_orphan_files(&self) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar(
            "SELECT fp.file_path FROM file_paths fp
             WHERE (fp.state = 'complete' OR (fp.state = 'pending' AND fp.size > 0))
             AND NOT EXISTS (
                 SELECT 1 FROM episode_files ef WHERE ef.file_path_id = fp.id
             )
             AND NOT EXISTS (
                 SELECT 1 FROM unmatched_files u WHERE u.file_path = fp.file_path
             )",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Link a tracked path to an episode. The kind is derived from the file
    /// extension: a subtitle/nfo sidecar is recorded as auxiliary and never
    /// stamps the episode's origin, and an nfo is limited to one per episode.
    pub async fn link_file_episode(&self, path: &str, episode_id: &str) -> Result<()> {
        let kind = jumbie_shared::media_format::file_kind_for_path(std::path::Path::new(path))
            .unwrap_or(jumbie_shared::media_format::FileKind::Video);
        self.link_file_episode_as(path, episode_id, kind).await
    }

    /// Link with an explicit kind (the scanner classifies before linking).
    /// Auxiliary kinds attach as episode-scoped sidecars. A `video` link becomes the
    /// episode's main file when it has none; otherwise it is recorded as a `linked`
    /// attachment (e.g. a pending download detected before organize) that never
    /// displaces the main file and is excluded from episode-media queries. A tagged
    /// file whose language tag the episode already holds is a duplicate and is not
    /// attached (one file per language tag). The file type is derived from the
    /// extension.
    ///
    /// Auxiliary associations are **episode-scoped**, never part-scoped: a sidecar
    /// belongs to the episode as a whole, not to a specific multipart piece.
    pub async fn link_file_episode_as(
        &self,
        path: &str,
        episode_id: &str,
        kind: jumbie_shared::media_format::FileKind,
    ) -> Result<()> {
        if kind.is_auxiliary() {
            return self.associate_auxiliary_file(episode_id, path).await;
        }

        // One file per language tag: never attach a second file into a language slot
        // the episode already holds (a duplicate variant).
        if jumbie_shared::parsing::has_language_tag(path)
            && self.has_same_slot_occupant(episode_id, path).await?
        {
            return Ok(());
        }

        let file_path_id = self.ensure_path_row(path).await?;
        let has_main: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM episode_files \
             WHERE episode_id = ? AND kind = 'main')",
        )
        .bind(episode_id)
        .fetch_one(&self.pool)
        .await?;
        let assoc_kind = if has_main { "linked" } else { "main" };

        sqlx::query(
            "INSERT INTO episode_files (episode_id, file_path_id, kind, part_number) \
             VALUES (?, ?, ?, NULL) \
             ON CONFLICT(episode_id, file_path_id) DO UPDATE SET \
                 kind = CASE WHEN episode_files.kind = 'main' THEN 'main' ELSE excluded.kind END",
        )
        .bind(episode_id)
        .bind(file_path_id)
        .bind(assoc_kind)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Sidecars attached to one episode, in natural path order.
    pub async fn get_auxiliary_files_for_episode(
        &self,
        episode_id: &str,
    ) -> Result<Vec<jumbie_shared::types::AuxiliaryFile>> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT fp.file_path FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind = 'auxiliary'",
        )
        .bind(episode_id)
        .fetch_all(&self.pool)
        .await?;
        let mut files: Vec<jumbie_shared::types::AuxiliaryFile> = rows
            .into_iter()
            .map(|path| jumbie_shared::types::AuxiliaryFile {
                kind: aux_kind(&path),
                path,
            })
            .collect();
        files.sort_by(|a, b| jumbie_shared::formatting::natural_cmp(&a.path, &b.path));
        Ok(files)
    }

    /// `linked` (non-primary) video attachments for a series, as
    /// `(episode_id, file_path)`. These are language/version variants kept alongside
    /// an episode's main file, and pre-organize pending downloads.
    pub async fn get_linked_files_for_series(
        &self,
        series_id: &str,
    ) -> Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT ef.episode_id, fp.file_path
             FROM episode_files ef
             JOIN file_paths fp ON fp.id = ef.file_path_id
             JOIN episodes e ON e.episode_id = ef.episode_id
             WHERE e.series_id = ? AND ef.kind = 'linked'",
        )
        .bind(series_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Sidecars for a whole series keyed by episode_id, for the episode list view.
    pub async fn get_auxiliary_files_for_series(
        &self,
        series_id: &str,
    ) -> Result<std::collections::HashMap<String, Vec<jumbie_shared::types::AuxiliaryFile>>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT ef.episode_id, fp.file_path
             FROM episode_files ef
             JOIN file_paths fp ON fp.id = ef.file_path_id
             JOIN episodes e ON e.episode_id = ef.episode_id
             WHERE e.series_id = ? AND ef.kind = 'auxiliary'",
        )
        .bind(series_id)
        .fetch_all(&self.pool)
        .await?;
        let mut map: std::collections::HashMap<String, Vec<jumbie_shared::types::AuxiliaryFile>> =
            std::collections::HashMap::new();
        for (episode_id, path) in rows {
            map.entry(episode_id)
                .or_default()
                .push(jumbie_shared::types::AuxiliaryFile {
                    kind: aux_kind(&path),
                    path,
                });
        }
        for files in map.values_mut() {
            files.sort_by(|a, b| jumbie_shared::formatting::natural_cmp(&a.path, &b.path));
        }
        Ok(map)
    }

    // link_files_by_prefix matches by directory prefix (not exact path) so a folder
    // of files (season pack) links to the same episode. Each matched path is
    // classified by extension so a sidecar inside the folder records its kind.
    pub async fn link_files_by_prefix(&self, prefix: &str, episode_id: &str) -> Result<()> {
        let dir_prefix = crate::db::sql_child_path_like_pattern(prefix);
        let exact = crate::db::normalize_sql_path_prefix(prefix);
        let paths: Vec<String> = sqlx::query_scalar(
            "SELECT file_path FROM file_paths \
             WHERE replace(file_path, '\\', '/') LIKE ? OR replace(file_path, '\\', '/') = ?",
        )
        .bind(&dir_prefix)
        .bind(&exact)
        .fetch_all(&self.pool)
        .await?;

        for path in paths {
            let kind = jumbie_shared::media_format::file_kind_for_path(std::path::Path::new(&path))
                .unwrap_or(jumbie_shared::media_format::FileKind::Video);
            self.link_file_episode_as(&path, episode_id, kind).await?;
        }
        Ok(())
    }

    pub async fn get_episode_by_path(&self, file_path: &str) -> Result<Option<String>> {
        let episode_id: Option<String> = sqlx::query_scalar(
            "SELECT ef.episode_id FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE fp.file_path = ? AND ef.kind = 'main' LIMIT 1",
        )
        .bind(file_path)
        .fetch_optional(&self.pool)
        .await?;
        Ok(episode_id)
    }

    pub async fn get_series_id_by_file_path(&self, file_path: &str) -> Result<Option<String>> {
        let series_id: Option<String> = sqlx::query_scalar(
            "SELECT e.series_id FROM episodes e \
             JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main' \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE fp.file_path = ? LIMIT 1",
        )
        .bind(file_path)
        .fetch_optional(&self.pool)
        .await?;
        Ok(series_id)
    }

    pub async fn get_episode_title(&self, episode_id: &str) -> Result<Option<String>> {
        let ep = self.get_episode_by_id(episode_id).await?;
        Ok(ep.and_then(|e| e.title))
    }

    /// SSoT: delegates to [`Self::get_episode_by_id`], which resolves submitter via
    /// the shared `SUBMITTER_EXPR` — the same implementation every detail query uses.
    pub async fn get_episode_submitter(&self, episode_id: &str) -> Result<Option<String>> {
        let ep = self.get_episode_by_id(episode_id).await?;
        Ok(ep.and_then(|e| e.submitter))
    }

    pub async fn get_episode_title_by_number(
        &self,
        series_id: &str,
        season: i32,
        episode: i32,
    ) -> Result<Option<String>> {
        let title: Option<String> = sqlx::query_scalar(
            "SELECT title FROM episodes WHERE series_id = ? AND season = ? AND episode = ?",
        )
        .bind(series_id)
        .bind(season)
        .bind(episode)
        .fetch_optional(&self.pool)
        .await?;
        Ok(title)
    }

    // Activity feed: reads `activity_log`, the single source of truth, into which
    // every user-facing event (download, import, assign, delete, …) writes at event
    // time via `DbManager::record_activity()`.
    pub async fn get_recent_activity(&self, limit: i64) -> Result<Vec<ActivityItem>> {
        let rows = sqlx::query_as::<_, crate::models::activity::ActivityLogRow>(
            "SELECT id, created_at, event_type, series_title, season, episode, episode_end,
                            title, details, status
                     FROM activity_log
                     ORDER BY created_at DESC
                     LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let merged = merge_activity_rows(rows);

        let items = merged
            .into_iter()
            .map(|r| {
                let at = match r.event_type.as_str() {
                    "download" => ActivityType::Download,
                    "import" => ActivityType::Import,
                    "metadata" => ActivityType::Metadata,
                    "reassign" => ActivityType::Reassign,
                    "assign" => ActivityType::Assign,
                    "analyze" => ActivityType::Analyze,
                    "delete" => ActivityType::Delete,
                    "unassign" => ActivityType::Unassign,
                    other => {
                        tracing::debug!("Unknown event_type in activity_log: {}", other);
                        ActivityType::Download
                    }
                };
                // Build "SxxEyy" string when season/episode are present.
                // Uses the shared fmt_season_episode — SSoT for episode formatting.
                let fmt_ep = |season: &Option<String>, episode: Option<i32>| -> String {
                    episode
                        .map(|ep| match season {
                            Some(s) if !s.is_empty() => {
                                let s_int = s.parse::<i32>().unwrap_or(1);
                                fmt_season_episode(s_int, ep, None, LabelStyle::Short)
                            }
                            _ => fmt_episode(ep, LabelStyle::Short),
                        })
                        .unwrap_or_default()
                };

                let (title, details) = match at {
                    ActivityType::Download => {
                        // Title = "Series - SxxEyy", details = release name (title column)
                        let ep_str = fmt_ep(&r.season, r.episode);
                        let t = if ep_str.is_empty() {
                            r.series_title.clone()
                        } else {
                            format!("{} - {}", r.series_title, ep_str)
                        };
                        (t, r.title.clone())
                    }
                    ActivityType::Assign | ActivityType::Reassign => {
                        // Title = "Series - SxxEyy", details = file path
                        let ep_str = fmt_ep(&r.season, r.episode);
                        let t = if ep_str.is_empty() {
                            r.series_title.clone()
                        } else {
                            format!("{} - {}", r.series_title, ep_str)
                        };
                        (t, r.details.clone())
                    }
                    ActivityType::Delete | ActivityType::Unassign => {
                        // Title = "Series - SxxEyy" when season/episode available (enriched call sites),
                        // otherwise fall back to filename; details = file path
                        let ep_str = fmt_ep(&r.season, r.episode);
                        let t = if ep_str.is_empty() {
                            r.series_title.clone()
                        } else {
                            format!("{} - {}", r.series_title, ep_str)
                        };
                        (t, r.details.clone())
                    }
                    ActivityType::Import => {
                        // Title = "Series - SxxEyy" (scanner enriches with season/episode),
                        // details = file path
                        let ep_str = fmt_ep(&r.season, r.episode);
                        let t = if ep_str.is_empty() {
                            r.series_title.clone()
                        } else {
                            format!("{} - {}", r.series_title, ep_str)
                        };
                        (t, r.details.clone())
                    }
                    ActivityType::Metadata => {
                        // Title = series name, details = consolidated episode range
                        (r.series_title.clone(), r.details.clone())
                    }
                    ActivityType::Analyze => {
                        // Title = "Series - SxxEyy" when enriched (episodes.rs),
                        // details = media info summary or file path
                        let ep_str = fmt_ep(&r.season, r.episode);
                        let t = if ep_str.is_empty() {
                            r.series_title.clone()
                        } else {
                            format!("{} - {}", r.series_title, ep_str)
                        };
                        (t, r.details.clone())
                    }
                };
                ActivityItem {
                    id: format!("log_{}", r.id),
                    timestamp: r.created_at,
                    activity_type: at,
                    title,
                    details,
                    status: r.status,
                }
            })
            .collect();

        Ok(items)
    }

    // Wanted episodes: monitored episodes with a past release date (meta_date,
    // upload_date, or est_date) that are not yet downloaded. Drives the UI "Wanted"
    // view and the auto-search loop; excludes "out_of_range" episodes.
    //
    // The effective public date honours the user's release-date priority/enabled
    // flags via the shared `compute_effective_date` helper (same as the calendar).
    /// Loads release date display preferences for effective-date computation.
    /// SSoT: shared by all wanted-episode queries instead of re-extracting the
    /// 4 fields from `get_ui_preferences()` at each call site.
    async fn load_release_date_prefs(&self) -> crate::datetime::ReleaseDatePrefs {
        let ui_prefs = self.get_ui_preferences().await.unwrap_or_default();
        crate::datetime::ReleaseDatePrefs {
            order: ui_prefs.release_date_display.order,
            metadata_enabled: ui_prefs.release_date_display.metadata_enabled,
            source_enabled: ui_prefs.release_date_display.source_enabled,
            estimated_enabled: ui_prefs.release_date_display.estimated_enabled,
        }
    }

    pub async fn get_wanted_episodes(&self) -> Result<Vec<jumbie_shared::types::WantedEpisode>> {
        let now = chrono::Utc::now().naive_utc();
        let prefs = self.load_release_date_prefs().await;

        type WantedEpisodeRow = (
            String,
            String,
            String,
            Option<i32>,
            i32,
            Option<String>,
            Option<chrono::NaiveDateTime>,
            Option<chrono::NaiveDateTime>,
            String,
        );
        let rows: Vec<WantedEpisodeRow> = sqlx::query_as(
            &format!(
                "SELECT COALESCE(sm.target_title, '') AS series_title, e.series_id, e.episode_id, e.season, e.episode, e.title,
                       e.meta_date, e.est_date,
                       CASE
                         WHEN EXISTS (
                           SELECT 1 FROM download_queue dq
                           WHERE dq.episode_id = e.episode_id
                             AND dq.status IN ({})
                         ) THEN 'in_queue'
                         ELSE e.status
                       END AS status
                FROM episodes e
                LEFT JOIN series_mappings sm ON e.series_id = sm.id
                WHERE e.status NOT IN ('downloaded', 'organized', 'out_of_range')
                  AND e.monitored = 1
                  AND {unassigned}
                  AND ({})
                ORDER BY COALESCE(e.meta_date, e.est_date) DESC",
                crate::db::IN_FLIGHT_QUEUE_STATUSES,
                RELEASED_DATE_FILTER,
                unassigned = crate::db::EPISODE_UNASSIGNED_PREDICATE,
            )
        )
        .bind(now)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;

        let mut wanted = Vec::new();
        for (
            series_title,
            series_id,
            episode_id,
            season,
            episode,
            title,
            meta_date,
            est_date,
            mut status,
        ) in rows
        {
            // SSoT: compute effective date using the shared ReleaseDatePrefs helper.
            // A wanted episode has no file, hence no source (`upload_date`) date.
            let dates: jumbie_shared::types::ReleaseDates<chrono::NaiveDateTime> =
                jumbie_shared::types::ReleaseDates {
                    meta_date,
                    upload_date: None,
                    est_date,
                };
            let Some(effective) = prefs.effective_date(meta_date, None, est_date) else {
                continue;
            };

            // Derive status using the SSoT helper.  If SQL already flagged the
            // episode as "in_queue", keep that; otherwise resolve "missing" vs
            // "unreleased" from the effective date.
            if status != EpisodeStatus::InQueue.as_str() {
                status = derive_episode_status(Some(effective), UtcDateTime::now()).to_string();
            }

            wanted.push(jumbie_shared::types::WantedEpisode {
                series_id,
                series_title,
                episode_id,
                season: season.map(|s| s.to_string()),
                episode,
                title,
                eff_date: effective.to_rfc3339_utc(),
                dates: dates.to_api(),
                status,
            });
        }

        Ok(wanted)
    }

    /// Boolean check: are there any wanted episodes whose effective date (per the
    /// user's preferences) is past? Stops scanning on the first match, for the sidebar.
    ///
    /// SSoT: shares the same SQL broad-filter + effective-date logic as
    /// `get_wanted_episodes()`.
    pub async fn has_wanted_episodes_by_preference(&self) -> Result<bool> {
        let prefs = self.load_release_date_prefs().await;

        if !prefs.metadata_enabled && !prefs.source_enabled && !prefs.estimated_enabled {
            return Ok(false);
        }

        let now = chrono::Utc::now().naive_utc();

        // Fetch only the date columns — we don't need full WantedEpisode structs.
        // A LIMIT of 100 is a safety cap; we stop early on the first match.
        let rows: Vec<(Option<chrono::NaiveDateTime>, Option<chrono::NaiveDateTime>)> =
            sqlx::query_as(&format!(
                "SELECT e.meta_date, e.est_date
             FROM episodes e
             LEFT JOIN series_mappings sm ON e.series_id = sm.id
             WHERE e.status NOT IN ('downloaded', 'organized', 'out_of_range')
               AND e.monitored = 1
               AND {unassigned}
               AND ({})
             LIMIT 100",
                RELEASED_DATE_FILTER,
                unassigned = crate::db::EPISODE_UNASSIGNED_PREDICATE,
            ))
            .bind(now)
            .bind(now)
            .fetch_all(&self.pool)
            .await?;

        for (meta_date, est_date) in rows {
            if let Some(d) = prefs.effective_date(meta_date, None, est_date)
                && d.naive_utc() <= now
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Fetches wanted episodes whose effective date is past the `min_wait` threshold,
    /// optionally bounded by `max_age`, returning pre-sorted `(series_id, season,
    /// episode)` tuples for the background auto-search task.
    ///
    /// Uses the same effective-date logic as `get_wanted_episodes()`. Separate from it
    /// to avoid parsing full WantedEpisode structs on every poll cycle.
    pub async fn get_auto_search_candidates(
        &self,
        cutoff_youngest: chrono::NaiveDateTime,
        cutoff_oldest: Option<chrono::NaiveDateTime>,
    ) -> Result<Vec<(String, Option<i32>, i32)>> {
        let prefs = self.load_release_date_prefs().await;
        let now = chrono::Utc::now().naive_utc();

        // Broad SQL filter matching `get_wanted_episodes`: any date column in the past.
        type WantedEpRow = (
            String,
            Option<i32>,
            i32,
            Option<chrono::NaiveDateTime>,
            Option<chrono::NaiveDateTime>,
        );
        let rows: Vec<WantedEpRow> = sqlx::query_as(&format!(
            "SELECT e.series_id, e.season, e.episode,
                        e.meta_date, e.est_date
                 FROM episodes e
                 WHERE e.status NOT IN ('downloaded', 'organized', 'out_of_range')
                   AND e.monitored = 1
                   AND ({})
                   AND NOT EXISTS (
                     SELECT 1 FROM download_queue dq
                     WHERE dq.episode_id = e.episode_id
                       AND dq.status IN ({})
                   )
                 ORDER BY e.series_id, e.season, e.episode",
            RELEASED_DATE_FILTER,
            crate::db::IN_FLIGHT_QUEUE_STATUSES,
        ))
        .bind(now)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;

        let mut candidates = Vec::with_capacity(rows.len());
        for (series_id, season, episode, meta_date, est_date) in rows {
            if let Some(effective) = prefs.effective_date(meta_date, None, est_date) {
                let effective_naive = effective.naive_utc();
                if effective_naive <= cutoff_youngest {
                    if let Some(oldest) = cutoff_oldest
                        && effective_naive < oldest
                    {
                        continue;
                    }
                    candidates.push((series_id, season, episode));
                }
            }
        }

        Ok(candidates)
    }

    // Max Released Episode
    // Highest (season, episode) pair with a meta_date that is downloaded/organized,
    // used by Future monitor mode. Counting only downloaded episodes means pre-populated
    // metadata rows don't suppress future detection.
    pub async fn get_max_released_episode(&self, series_id: &str) -> Result<Option<(i32, i32)>> {
        let row: Option<(i32, i32)> = sqlx::query_as(
            "SELECT season as season_num, episode
             FROM episodes
             WHERE series_id = ?
               AND meta_date IS NOT NULL
               AND status IN ('downloaded', 'organized')
             ORDER BY season_num DESC, episode DESC
             LIMIT 1",
        )
        .bind(series_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    // Episode Count Per Season
    // Number of episode rows for a series/season, used by season-pack scoring to
    // derive the adjusted_end range instead of defaulting to 12.
    pub async fn count_episodes_for_season(&self, series_id: &str, season: i32) -> Result<i32> {
        let count: Option<(i32,)> =
            sqlx::query_as("SELECT COUNT(*) FROM episodes WHERE series_id = ? AND season = ?")
                .bind(series_id)
                .bind(season)
                .fetch_optional(&self.pool)
                .await?;
        Ok(count.map(|c| c.0).unwrap_or(0))
    }

    // Downloaded Episodes for Series
    // All downloaded episodes for a series, used by organize/rename to enumerate
    // files that need moving.
    pub async fn get_downloaded_episodes_for_series(
        &self,
        series_id: &str,
    ) -> anyhow::Result<Vec<(String, String, Option<String>, Option<String>, String, i32)>> {
        let sql = format!(
            "SELECT e.episode_id, fp.file_path, e.title, e.description, e.season, e.episode \
             FROM episodes e \
             JOIN file_paths fp ON fp.id = {primary} \
             WHERE e.series_id = ?",
            primary = crate::db::EPISODE_PRIMARY_FILE_ID,
        );
        let rows =
            sqlx::query_as::<_, (String, String, Option<String>, Option<String>, String, i32)>(
                &sql,
            )
            .bind(series_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    /// Count episodes whose stored `file_path` starts with the given prefix.
    /// Used by the root-removal migration to determine whether a legacy series
    /// has data tied to a specific root (even if the directory was deleted).
    pub async fn count_episodes_with_path_prefix(
        &self,
        series_id: &str,
        prefix: &str,
    ) -> Result<usize> {
        // Escape LIKE wildcards to prevent path characters from matching
        // unintended rows. SQLite's ESCAPE clause defines the escape char.
        let pattern = format!(
            "{}%",
            prefix
                .replace("\\", "\\\\")
                .replace("%", "\\%")
                .replace("_", "\\_")
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM episodes e
             WHERE e.series_id = ? AND EXISTS (
                 SELECT 1 FROM episode_files ef
                 JOIN file_paths fp ON fp.id = ef.file_path_id
                 WHERE ef.episode_id = e.episode_id AND ef.kind = 'main'
                   AND fp.file_path LIKE ? ESCAPE '\\')",
        )
        .bind(series_id)
        .bind(pattern)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as usize)
    }

    // Episodes for Series Season
    // Looks up the series mapping to resolve target_title, then fetches all episode
    // rows for that season. Uses an explicit column list (not SELECT *) because `size`
    // and `media_info` live in file_paths/file_contents.
    pub async fn get_episodes_for_series_season(
        &self,
        series_id: &str,
        season: i32,
        absolute: bool,
    ) -> Result<Vec<EpisodeDetailRow>> {
        let rows = sqlx::query_as::<_, EpisodeDetailRow>(&format!(
            "SELECT e.episode_id, e.season, e.episode, e.status, f.file_path, \
            rm.release_title AS release_title, \
            COALESCE(f.size, 0) as size, e.title, \
                        e.quality_profile_id, \
            {submitter} AS submitter, c.media_info, c.fingerprint AS quick_hash, \
            e.created_at, e.file_acquired_at, e.monitored, e.meta_date, \
            {upload_date} AS upload_date, e.est_date, e.metadata_ids, \
            e.description, e.runtime, e.image_url, \
            rm.download_id AS download_id, rm.score AS score, e.numbering_mode, e.metadata_source, \
            rm.download_link AS download_link, e.series_id, e.monitor_override, \
            {original_path} AS original_path \
            FROM episodes e \
             {join} \
             WHERE e.series_id = ? AND e.season = ? AND e.numbering_mode = ?",
            submitter = crate::db::SUBMITTER_EXPR,
            join = crate::db::EPISODE_FILE_JOIN,
            original_path = crate::db::ORIGINAL_PATH_EXPR,
            upload_date = crate::db::UPLOAD_DATE_EXPR,
        ))
        .bind(series_id)
        .bind(season)
        .bind(absolute as i32)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // Single Episode Lookup
    // Fetches a complete EpisodeDetailRow by episode_id. The fingerprint LEFT JOIN is
    // needed because size and media_info live in file_paths/file_contents.
    pub async fn get_episode_by_id(&self, episode_id: &str) -> Result<Option<EpisodeDetailRow>> {
        // Built with format! so the JOIN/submitter fragments stay SSoT'd in
        // db::EPISODE_FILE_JOIN / db::SUBMITTER_EXPR (same as every other
        // episode query). Uses sqlx directly because the bound-query helpers
        // require a &'static str.
        let sql = format!(
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
             WHERE e.episode_id = ?",
            submitter = crate::db::SUBMITTER_EXPR,
            join = crate::db::EPISODE_FILE_JOIN,
            original_path = crate::db::ORIGINAL_PATH_EXPR,
            upload_date = crate::db::UPLOAD_DATE_EXPR,
        );
        let row = sqlx::query_as::<_, EpisodeDetailRow>(&sql)
            .bind(episode_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    /// Batch query returning (episode_id → series_id) for the given episode IDs.
    /// Uses the `series_id` column directly from the episodes table, avoiding
    /// any name/title-based resolution that could be ambiguous.
    pub async fn get_episodes_series_ids(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, String>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        const CHUNK: usize = 500;
        let mut out = std::collections::HashMap::new();
        for chunk in ids.chunks(CHUNK) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "SELECT e.episode_id, e.series_id FROM episodes e WHERE e.episode_id IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for row in rows {
                let ep_id: String = sqlx::Row::get(&row, 0);
                let sid: String = sqlx::Row::get(&row, 1);
                out.insert(ep_id, sid);
            }
        }
        Ok(out)
    }

    /// Lightweight batch query — returns (episode_id, metadata_ids_json, metadata_source)
    /// for a list of episode IDs. Only fetches the fields needed for metadata resolution,
    /// avoiding the expensive file join used by `get_episode_by_id`.
    pub async fn get_episodes_metadata_batch(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, (Option<String>, Option<String>)>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        const CHUNK: usize = 500;
        let mut out = std::collections::HashMap::new();
        for chunk in ids.chunks(CHUNK) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "SELECT e.episode_id, e.metadata_ids, e.metadata_source \
                 FROM episodes e WHERE e.episode_id IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for row in rows {
                let ep_id: String = sqlx::Row::get(&row, 0);
                let metadata_ids: Option<String> = sqlx::Row::get(&row, 1);
                let metadata_source: Option<String> = sqlx::Row::get(&row, 2);
                out.insert(ep_id, (metadata_ids, metadata_source));
            }
        }
        Ok(out)
    }

    /// Fetch all monitored, missing, and released episodes for one series as
    /// `(season, episode_number)` pairs. Used by `SearchQueue::submit_monitored_missing`.
    ///
    /// SSoT: "released" uses the same definition as `get_auto_search_candidates`
    /// (shared `RELEASED_DATE_FILTER`, then `prefs.effective_date` refinement).
    pub async fn get_monitored_missing_for_series(
        &self,
        series_id: &str,
    ) -> Result<Vec<(i32, i32)>> {
        let id = series_id.to_string();
        let now = chrono::Utc::now().naive_utc();
        let prefs = self.load_release_date_prefs().await;

        type MissingRow = (
            i32,
            i32,
            Option<chrono::NaiveDateTime>,
            Option<chrono::NaiveDateTime>,
        );
        let rows: Vec<MissingRow> = sqlx::query_as(&format!(
            "SELECT e.season, e.episode, e.meta_date, e.est_date
             FROM episodes e
             WHERE e.series_id = ?
               AND e.monitored = 1
               AND e.status NOT IN ('downloaded', 'organized', 'out_of_range')
               AND e.monitored = 1
               AND {unassigned}
               AND ({})
             ORDER BY e.season, e.episode",
            RELEASED_DATE_FILTER,
            unassigned = crate::db::EPISODE_UNASSIGNED_PREDICATE,
        ))
        .bind(&id)
        .bind(now)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (season, episode, meta_date, est_date) in rows {
            if let Some(effective) = prefs.effective_date(meta_date, None, est_date)
                && effective.naive_utc() <= now
            {
                out.push((season, episode));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::episodes::crud::InsertEpisodeParams;

    /// Contract enforcement for `get_rename_plan_episodes(_batch)`: the lean
    /// query must populate EXACTLY the fields the rename-plan pipeline reads
    /// (see the query doc comment) and nothing else — even when the underlying
    /// tables have data for the excluded columns.
    ///
    /// If someone changes the projection in either direction (adds a column
    /// that isn't read, or forgets one that is), this test fails loudly.
    #[tokio::test]
    async fn rename_plan_query_populates_exactly_the_contract_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();

        let mapping = jumbie_shared::types::MappingRule {
            target_title: "Contract Show".into(),
            ..Default::default()
        };
        db.upsert_series_mapping("series-contract", &mapping)
            .await
            .unwrap();

        let metadata_ids = std::collections::HashMap::new();
        db.insert_episode(InsertEpisodeParams {
            episode_id: "ep-1",
            series_id: "series-contract",
            season: 1,
            episode: 1,
            file_path: Some("/media/Contract.Show.S01E01.mkv"),
            title: Some("Pilot"),
            quality_profile_id: Some("qp-1"),
            status: "aired",
            meta_date: Some(
                chrono::NaiveDate::from_ymd_opt(2026, 1, 5)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            ),
            est_date: Some(
                chrono::NaiveDate::from_ymd_opt(2026, 1, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            ),
            metadata_ids: &metadata_ids,
            description: Some("Must NOT be fetched by the lean query"),
            runtime: Some(45),
            image_url: Some("https://example.com/poster.jpg"),
            metadata_source: Some("provider"),
            numbering_mode: Some(0),
        })
        .await
        .unwrap();

        // Content row (media_info) and a size/state update on the path row the
        // association already created — the lean query must not read the excluded
        // columns.
        sqlx::query(
            "INSERT INTO file_contents (fingerprint, media_info)
             VALUES (?, ?)",
        )
        .bind("hash-1")
        .bind(r#"{"has_chapters":false,"width":1920,"height":1080}"#)
        .execute(&db.pool)
        .await
        .unwrap();

        sqlx::query(
            "UPDATE file_paths SET fingerprint = ?, state = 'complete', size = ? \
             WHERE file_path = ?",
        )
        .bind("hash-1")
        .bind(5_000_000_000i64)
        .bind("/media/Contract.Show.S01E01.mkv")
        .execute(&db.pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO release_info (quick_hash, release_title, submitter, download_link, score, download_id)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("hash-1")
        .bind("Contract.Show.S01E01.1080p.WEB-DL-GROUP")
        .bind("GROUP")
        .bind("magnet:?xt=urn:btih:abc123")
        .bind(99)
        .bind("dl-1")
        .execute(&db.pool)
        .await
        .unwrap();

        let rows = db
            .get_rename_plan_episodes_batch(&["series-contract".to_string()], false)
            .await
            .unwrap();
        let eps = rows.get("series-contract").expect("series present");
        assert_eq!(eps.len(), 1);
        let row = &eps[0];

        // Required contract fields — must be populated
        assert_eq!(row.episode_id, "ep-1");
        assert_eq!(row.season, Some(1));
        assert_eq!(row.episode, 1);
        assert_eq!(
            row.file_path.as_deref(),
            Some("/media/Contract.Show.S01E01.mkv")
        );
        assert_eq!(row.title.as_deref(), Some("Pilot"));
        assert_eq!(row.submitter.as_deref(), Some("GROUP"));
        assert_eq!(
            row.media_info.as_deref(),
            Some(r#"{"has_chapters":false,"width":1920,"height":1080}"#)
        );
        assert!(row.created_at.is_some(), "created_at must be populated");
        assert!(row.meta_date.is_some(), "meta_date must be populated");
        assert_eq!(row.metadata_source.as_deref(), Some("provider"));
        assert_eq!(row.series_id, "series-contract");

        // Excluded fields — must be None/default even though the underlying
        // tables have data for them.
        assert_eq!(row.status, None);
        assert_eq!(row.release_title, None);
        assert_eq!(row.size, 0);
        assert_eq!(row.quality_profile_id, None);
        assert_eq!(row.quick_hash, None);
        assert_eq!(row.file_acquired_at, None);
        assert!(!row.monitored);
        assert_eq!(row.upload_date, None);
        assert_eq!(row.est_date, None);
        assert_eq!(row.metadata_ids, None);
        assert_eq!(row.description, None);
        assert_eq!(row.runtime, None);
        assert_eq!(row.image_url, None);
        assert_eq!(row.download_id, None);
        assert_eq!(row.score, None);
        assert_eq!(row.numbering_mode, 0);
        assert_eq!(row.download_link, None);
        assert!(!row.monitor_override);

        // Single-series variant must behave identically.
        let single = db
            .get_rename_plan_episodes("series-contract", false)
            .await
            .unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].episode_id, "ep-1");
        assert_eq!(single[0].description, None);
        assert_eq!(
            single[0].media_info.as_deref(),
            Some(r#"{"has_chapters":false,"width":1920,"height":1080}"#)
        );
    }

    /// `get_monitored_missing_for_series` must use the SAME definition of
    /// "released" as the background wanted-task: the broad SQL date filter
    /// plus the user's release-date display preferences.  An episode whose
    /// PREFERRED date source (metadata-first by default) is in the future must
    /// not be searched even when another date column is in the past.
    #[tokio::test]
    async fn monitored_missing_respects_release_date_preferences() {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();

        let mapping = jumbie_shared::types::MappingRule {
            target_title: "Prefs Show".into(),
            ..Default::default()
        };
        db.upsert_series_mapping("prefs-series", &mapping)
            .await
            .unwrap();

        let metadata_ids = std::collections::HashMap::new();
        let past = chrono::NaiveDate::from_ymd_opt(2024, 1, 5)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let future = chrono::NaiveDate::from_ymd_opt(2999, 1, 5)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();

        // E01: released (past meta_date) → must qualify.
        db.insert_episode(InsertEpisodeParams {
            episode_id: "prefs-ep-1",
            series_id: "prefs-series",
            season: 1,
            episode: 1,
            status: "unreleased",
            meta_date: Some(past),
            est_date: None,
            metadata_ids: &metadata_ids,
            numbering_mode: Some(0),
            ..InsertEpisodeParams::dummy("prefs-ep-1", "prefs-series", 1, 1, &metadata_ids)
        })
        .await
        .unwrap();

        // E02: future meta_date + past upload_date → the broad SQL filter
        // matches (upload_date ≤ now), but the effective date (meta_date,
        // metadata-first default prefs) is in the future → must be excluded.
        db.insert_episode(InsertEpisodeParams {
            episode_id: "prefs-ep-2",
            series_id: "prefs-series",
            season: 1,
            episode: 2,
            status: "unreleased",
            meta_date: Some(future),
            est_date: None,
            metadata_ids: &metadata_ids,
            numbering_mode: Some(0),
            ..InsertEpisodeParams::dummy("prefs-ep-2", "prefs-series", 1, 2, &metadata_ids)
        })
        .await
        .unwrap();

        let missing = db
            .get_monitored_missing_for_series("prefs-series")
            .await
            .unwrap();
        assert_eq!(
            missing,
            vec![(1, 1)],
            "only the episode whose preferred date is past may be searched"
        );
    }

    async fn aux_db() -> (DbManager, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();
        let mapping = jumbie_shared::types::MappingRule {
            target_title: "Aux Show".into(),
            ..Default::default()
        };
        db.upsert_series_mapping("aux-series", &mapping)
            .await
            .unwrap();
        let metadata_ids = std::collections::HashMap::new();
        db.insert_episode(InsertEpisodeParams::dummy(
            "aux_ep",
            "aux-series",
            1,
            1,
            &metadata_ids,
        ))
        .await
        .unwrap();
        db.associate_main_file("aux_ep", "/lib/ep.mkv", None)
            .await
            .unwrap();
        (db, tmp)
    }

    /// The episode's primary main-file path, mirroring the read-model precedence.
    async fn primary_path(db: &DbManager, episode_id: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT fp.file_path FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind = 'main' \
             ORDER BY (ef.part_number IS NOT NULL), ef.part_number LIMIT 1",
        )
        .bind(episode_id)
        .fetch_optional(&db.pool)
        .await
        .unwrap()
    }

    async fn insert_path(db: &DbManager, path: &str) {
        sqlx::query(
            "INSERT INTO file_paths (file_path, fingerprint, state) VALUES (?, 'fp', 'complete')",
        )
        .bind(path)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    /// Parent episode for multipart/aux tests: `file_path = NULL` (an episode whose
    /// content is spread across parts, or has only sidecars).
    async fn parts_episode_db() -> (DbManager, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();
        let mapping = jumbie_shared::types::MappingRule {
            target_title: "Parts Show".into(),
            series_id: "parts-series".into(),
            ..Default::default()
        };
        db.upsert_series_mapping("parts-series", &mapping)
            .await
            .unwrap();
        let metadata_ids = std::collections::HashMap::new();
        db.insert_episode(InsertEpisodeParams {
            episode_id: "parts_ep",
            series_id: "parts-series",
            season: 1,
            episode: 1,
            file_path: None,
            title: None,
            quality_profile_id: None,
            status: "organized",
            meta_date: None,
            est_date: None,
            metadata_ids: &metadata_ids,
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        })
        .await
        .unwrap();
        (db, tmp)
    }

    /// Record a path + content, optionally carrying a `release_info.upload_date`.
    async fn insert_dated_path(db: &DbManager, path: &str, hash: &str, date: Option<&str>) {
        use crate::db::fingerprints::{ContentRecord, PathRecord};
        db.upsert_content(ContentRecord {
            fingerprint: hash,
            media_info: None,
            path,
        })
        .await
        .unwrap();
        db.upsert_path(PathRecord {
            file_path: path,
            fingerprint: hash,
            inode: 0,
            device: 0,
            size: 1,
            mtime: 0.0,
            state: "organized",
            expected_path: None,
        })
        .await
        .unwrap();
        if let Some(d) = date {
            sqlx::query("INSERT INTO release_info (quick_hash, upload_date) VALUES (?, ?)")
                .bind(hash)
                .bind(d)
                .execute(&db.pool)
                .await
                .unwrap();
        }
    }

    fn source_dt(day: u32) -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 6, day)
            .unwrap()
            .and_hms_opt(20, 0, 0)
            .unwrap()
    }

    /// A multipart episode's file date comes from its parts (which live in
    /// `episode_files`, not on the episode row).
    #[tokio::test]
    async fn multipart_episode_resolves_upload_date_from_its_parts() {
        let (db, _tmp) = parts_episode_db().await;
        insert_dated_path(
            &db,
            "/lib/Show.S01E01.pt1.mkv",
            "p1h",
            Some("2026-06-18 20:00:00"),
        )
        .await;
        insert_dated_path(
            &db,
            "/lib/Show.S01E01.pt2.mkv",
            "p2h",
            Some("2026-06-18 20:00:00"),
        )
        .await;
        db.upsert_episode_part("parts_ep", 1, "/lib/Show.S01E01.pt1.mkv", None)
            .await
            .unwrap();
        db.upsert_episode_part("parts_ep", 2, "/lib/Show.S01E01.pt2.mkv", None)
            .await
            .unwrap();

        let row = db.get_episode_by_id("parts_ep").await.unwrap().unwrap();
        assert_eq!(
            row.upload_date,
            Some(source_dt(18)),
            "a multipart episode's file date must come from a part"
        );
    }

    /// When only some parts carry a date, the dated part is used — never a
    /// sibling's absence.
    #[tokio::test]
    async fn multipart_episode_prefers_a_dated_part() {
        let (db, _tmp) = parts_episode_db().await;
        insert_dated_path(&db, "/lib/Show.S01E01.pt1.mkv", "p1h", None).await;
        insert_dated_path(
            &db,
            "/lib/Show.S01E01.pt2.mkv",
            "p2h",
            Some("2026-06-19 20:00:00"),
        )
        .await;
        db.upsert_episode_part("parts_ep", 1, "/lib/Show.S01E01.pt1.mkv", None)
            .await
            .unwrap();
        db.upsert_episode_part("parts_ep", 2, "/lib/Show.S01E01.pt2.mkv", None)
            .await
            .unwrap();

        let row = db.get_episode_by_id("parts_ep").await.unwrap().unwrap();
        assert_eq!(
            row.upload_date,
            Some(source_dt(19)),
            "a dated part must win over an undated sibling part"
        );
    }

    /// Auxiliary sidecars never supply an episode's file date, even when their path
    /// sorts ahead of the video parts.
    #[tokio::test]
    async fn auxiliary_file_never_supplies_the_episode_file_date() {
        let (db, _tmp) = parts_episode_db().await;
        // "Show.S01E01.en.srt" sorts before "Show.S01E01.pt1.mkv".
        insert_dated_path(&db, "/lib/Show.S01E01.en.srt", "srth", None).await;
        db.associate_auxiliary_file("parts_ep", "/lib/Show.S01E01.en.srt")
            .await
            .unwrap();
        insert_dated_path(
            &db,
            "/lib/Show.S01E01.pt1.mkv",
            "p1h",
            Some("2026-06-18 20:00:00"),
        )
        .await;
        db.upsert_episode_part("parts_ep", 1, "/lib/Show.S01E01.pt1.mkv", None)
            .await
            .unwrap();

        let row = db.get_episode_by_id("parts_ep").await.unwrap().unwrap();
        assert_eq!(
            row.upload_date,
            Some(source_dt(18)),
            "an auxiliary sidecar must never supply the episode's file date"
        );
    }

    /// Disowning an episode drops its associations, so the episode-details join
    /// cannot resurrect the lost file's date afterwards.
    #[tokio::test]
    async fn disown_drops_the_association_so_no_stale_date_survives() {
        let (db, _tmp) = parts_episode_db().await;
        insert_dated_path(
            &db,
            "/lib/Show.S01E01.mkv",
            "vh",
            Some("2026-06-18 20:00:00"),
        )
        .await;
        db.associate_main_file("parts_ep", "/lib/Show.S01E01.mkv", None)
            .await
            .unwrap();

        let before = db.get_episode_by_id("parts_ep").await.unwrap().unwrap();
        assert_eq!(before.upload_date, Some(source_dt(18)));

        db.clear_episode_ownership(&["parts_ep".to_string()])
            .await
            .unwrap();

        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM episode_files WHERE episode_id = 'parts_ep'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(remaining, 0, "disown must drop every association");

        let after = db.get_episode_by_id("parts_ep").await.unwrap().unwrap();
        assert_eq!(
            after.upload_date, None,
            "a disowned episode must not resurrect the lost file's date"
        );
    }

    /// A multipart episode's non-date file fields (release_title/submitter/size)
    /// resolve from its primary part once the part is linked — so scanned and
    /// downloaded multiparts render identically.
    #[tokio::test]
    async fn multipart_episode_resolves_file_fields_from_its_primary_part() {
        let (db, _tmp) = parts_episode_db().await;
        insert_dated_path(
            &db,
            "/lib/Show.S01E01.pt1.mkv",
            "p1h",
            Some("2026-06-18 20:00:00"),
        )
        .await;
        insert_dated_path(
            &db,
            "/lib/Show.S01E01.pt2.mkv",
            "p2h",
            Some("2026-06-18 20:00:00"),
        )
        .await;
        sqlx::query(
            "UPDATE release_info SET release_title = 'Show.S01E01', submitter = 'Grp' \
             WHERE quick_hash = 'p1h'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE file_paths SET size = 111 \
             WHERE file_path = '/lib/Show.S01E01.pt1.mkv'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE file_paths SET size = 222 \
             WHERE file_path = '/lib/Show.S01E01.pt2.mkv'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        db.upsert_episode_part("parts_ep", 1, "/lib/Show.S01E01.pt1.mkv", Some(111))
            .await
            .unwrap();
        db.upsert_episode_part("parts_ep", 2, "/lib/Show.S01E01.pt2.mkv", Some(222))
            .await
            .unwrap();

        let row = db.get_episode_by_id("parts_ep").await.unwrap().unwrap();
        assert_eq!(row.release_title.as_deref(), Some("Show.S01E01"));
        assert_eq!(row.submitter.as_deref(), Some("Grp"));
        assert!(row.size > 0, "size must resolve from the primary part");
    }

    /// The rescore query must see a multipart episode through its part, not skip it
    /// because it has no single-file association.
    #[tokio::test]
    async fn rescore_sees_multipart_episodes_through_their_parts() {
        let (db, _tmp) = parts_episode_db().await;
        insert_dated_path(
            &db,
            "/lib/Show.S01E01.pt1.mkv",
            "p1h",
            Some("2026-06-18 20:00:00"),
        )
        .await;
        sqlx::query(
            "UPDATE release_info SET release_title = 'Show.S01E01', \
             scoring_size_bytes = 123, scoring_seeders = 9 WHERE quick_hash = 'p1h'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        db.upsert_episode_part("parts_ep", 1, "/lib/Show.S01E01.pt1.mkv", None)
            .await
            .unwrap();

        let rows = db
            .get_episodes_for_rescore(&["parts-series".to_string()])
            .await
            .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "a multipart episode must be rescored via its part"
        );
        assert_eq!(rows[0].episode_id, "parts_ep");
        assert_eq!(rows[0].release_title.as_deref(), Some("Show.S01E01"));
        assert_eq!(rows[0].scoring_size_bytes, Some(123));
    }

    /// Score / release / hash lookups resolve a multipart episode through its parts
    /// (`episode_files`), not a `episode_files` link.
    #[tokio::test]
    async fn multipart_episode_file_lookups_use_its_parts() {
        let (db, _tmp) = parts_episode_db().await;
        insert_dated_path(
            &db,
            "/lib/Show.S01E01.pt1.mkv",
            "look-h",
            Some("2026-06-18 20:00:00"),
        )
        .await;
        sqlx::query(
            "UPDATE release_info SET release_title = 'R', submitter = 'G', score = 42, \
             download_id = 'dl-1', version = 2 WHERE quick_hash = 'look-h'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        db.upsert_episode_part("parts_ep", 1, "/lib/Show.S01E01.pt1.mkv", None)
            .await
            .unwrap();

        assert_eq!(db.get_episode_score("parts_ep").await.unwrap(), Some(42));
        let (score, submitter, _version, title) = db
            .get_episode_release_info("parts_ep")
            .await
            .unwrap()
            .expect("release info should resolve from the part");
        assert_eq!(score, 42);
        assert_eq!(submitter.as_deref(), Some("G"));
        assert_eq!(title.as_deref(), Some("R"));
        assert_eq!(
            db.get_episode_hash("parts_ep").await.unwrap().as_deref(),
            Some("dl-1")
        );
    }

    /// A part's media info resolves from `file_contents` (content-keyed), the single
    /// store — not a per-part copy.
    #[tokio::test]
    async fn part_media_info_comes_from_file_contents() {
        let (db, _tmp) = parts_episode_db().await;
        insert_dated_path(&db, "/lib/Show.S01E01.pt1.mkv", "mi-h", None).await;
        sqlx::query(
            "UPDATE file_contents SET media_info = '{\"width\":1920}' WHERE fingerprint = 'mi-h'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        db.upsert_episode_part("parts_ep", 1, "/lib/Show.S01E01.pt1.mkv", None)
            .await
            .unwrap();
        db.update_episode_part_fingerprint("parts_ep", 1, "mi-h")
            .await
            .unwrap();

        let parts = db.get_episode_parts("parts_ep").await.unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].media_info.as_deref(), Some("{\"width\":1920}"));
    }

    /// A sidecar links as auxiliary: it never becomes the episode's playable file
    /// and never stamps the episode's origin fingerprint.
    #[tokio::test]
    async fn auxiliary_link_marks_kind_and_preserves_episode_file() {
        let (db, _tmp) = aux_db().await;
        insert_path(&db, "/dl/ep.srt").await;

        db.link_file_episode("/dl/ep.srt", "aux_ep").await.unwrap();

        let aux = db.get_auxiliary_files_for_episode("aux_ep").await.unwrap();
        assert_eq!(aux.len(), 1);
        assert_eq!(aux[0].kind, jumbie_shared::media_format::FileKind::Subtitle);

        let file_path = primary_path(&db, "aux_ep").await;
        assert_eq!(
            file_path.as_deref(),
            Some("/lib/ep.mkv"),
            "an auxiliary file must not replace the episode file"
        );
    }

    /// At most one nfo stays linked per episode; a newer nfo replaces the old one.
    #[tokio::test]
    async fn one_nfo_per_episode_is_enforced() {
        let (db, _tmp) = aux_db().await;
        insert_path(&db, "/dl/a.nfo").await;
        insert_path(&db, "/dl/b.nfo").await;

        db.link_file_episode("/dl/a.nfo", "aux_ep").await.unwrap();
        db.link_file_episode("/dl/b.nfo", "aux_ep").await.unwrap();

        let aux = db.get_auxiliary_files_for_episode("aux_ep").await.unwrap();
        assert_eq!(aux.len(), 1, "only one nfo may stay linked");
        assert_eq!(aux[0].kind, jumbie_shared::media_format::FileKind::Nfo);
        assert_eq!(aux[0].path, "/dl/b.nfo", "the newest nfo wins");
    }

    /// Subtitles are unlimited.
    #[tokio::test]
    async fn multiple_subtitles_are_allowed() {
        let (db, _tmp) = aux_db().await;
        insert_path(&db, "/dl/ep.en.srt").await;
        insert_path(&db, "/dl/ep.fr.ass").await;

        db.link_file_episode("/dl/ep.en.srt", "aux_ep")
            .await
            .unwrap();
        db.link_file_episode("/dl/ep.fr.ass", "aux_ep")
            .await
            .unwrap();

        let aux = db.get_auxiliary_files_for_episode("aux_ep").await.unwrap();
        assert_eq!(aux.len(), 2);
    }

    /// Manually assigning a subtitle attaches it as auxiliary instead of overwriting
    /// the episode's playable file.
    #[tokio::test]
    async fn manual_assign_of_auxiliary_never_replaces_the_episode_file() {
        let (db, _tmp) = aux_db().await;
        insert_path(&db, "/dl/extra.srt").await;

        let cleared = db
            .assign_file_to_episode(crate::db::episodes::assign::AssignFileToEpisodeParams {
                episode_id: "aux_ep",
                series_id: "aux-series",
                series_title: "Aux Show",
                season: 1,
                episode: 1,
                file_path: "/dl/extra.srt",
                episode_title: "",
                all_target_episode_ids: None,
                numbering_mode: 0,
                only_unassigned: false,
            })
            .await
            .unwrap();
        assert!(cleared.is_empty(), "aux assignment clears no episode paths");

        let file_path = primary_path(&db, "aux_ep").await;
        assert_eq!(file_path.as_deref(), Some("/lib/ep.mkv"));
        let aux = db.get_auxiliary_files_for_episode("aux_ep").await.unwrap();
        assert_eq!(aux.len(), 1);
        assert_eq!(aux[0].path, "/dl/extra.srt");
    }

    /// A deliberate manual assignment of a sidecar materializes the episode cell, so
    /// an episode that exists only to hold sidecars can be created by hand. The
    /// scanner never creates a cell for a sidecar.
    #[tokio::test]
    async fn manual_assign_of_auxiliary_creates_the_cell() {
        let (db, _tmp) = aux_db().await;
        insert_path(&db, "/dl/only.srt").await;

        let new_episode = "aux-series_S01E07";
        assert!(!db.episode_exists(new_episode).await.unwrap());

        let cleared = db
            .assign_file_to_episode(crate::db::episodes::assign::AssignFileToEpisodeParams {
                episode_id: new_episode,
                series_id: "aux-series",
                series_title: "Aux Show",
                season: 1,
                episode: 7,
                file_path: "/dl/only.srt",
                episode_title: "",
                all_target_episode_ids: None,
                numbering_mode: 0,
                only_unassigned: false,
            })
            .await
            .unwrap();
        assert!(cleared.is_empty(), "aux assignment clears no episode paths");

        assert!(
            db.episode_exists(new_episode).await.unwrap(),
            "manual assignment must create the cell"
        );
        let aux = db
            .get_auxiliary_files_for_episode(new_episode)
            .await
            .unwrap();
        assert_eq!(aux.len(), 1);
        assert_eq!(aux[0].path, "/dl/only.srt");
    }

    /// Folder linking classifies each matched path by extension, so sidecars
    /// inside a season-pack folder record `subtitle`/`nfo` (not `video`).
    #[tokio::test]
    async fn link_files_by_prefix_classifies_sidecars() {
        let (db, _tmp) = aux_db().await;
        insert_path(&db, "/dl/pack/Show.S01E01.mkv").await;
        insert_path(&db, "/dl/pack/Show.S01E01.en.srt").await;
        insert_path(&db, "/dl/pack/Show.S01E01.nfo").await;

        db.link_files_by_prefix("/dl/pack", "aux_ep").await.unwrap();

        let aux = db.get_auxiliary_files_for_episode("aux_ep").await.unwrap();
        let kinds: std::collections::HashSet<_> = aux.iter().map(|a| a.kind).collect();
        assert_eq!(aux.len(), 2, "subtitle + nfo linked as auxiliary");
        assert!(kinds.contains(&jumbie_shared::media_format::FileKind::Subtitle));
        assert!(kinds.contains(&jumbie_shared::media_format::FileKind::Nfo));

        // The pack's video is not an auxiliary association.
        assert!(
            aux.iter().all(|a| a.path != "/dl/pack/Show.S01E01.mkv"),
            "the video must not be linked as auxiliary"
        );
    }

    #[tokio::test]
    async fn link_files_by_prefix_matches_windows_backslashes() {
        let (db, _tmp) = aux_db().await;
        insert_path(&db, r"C:\dl\pack\Show.S01E01.mkv").await;
        insert_path(&db, r"C:\dl\pack\Show.S01E01.en.srt").await;
        insert_path(&db, r"C:\dl\pack\Show.S01E01.nfo").await;
        insert_path(&db, r"C:\dl\other\Show.S01E02.mkv").await;

        db.link_files_by_prefix(r"C:\dl\pack", "aux_ep").await.unwrap();

        let aux = db.get_auxiliary_files_for_episode("aux_ep").await.unwrap();
        assert_eq!(aux.len(), 2, "subtitle + nfo linked as auxiliary");
        assert!(aux.iter().any(|a| a.path == r"C:\dl\pack\Show.S01E01.en.srt"));
        assert!(aux.iter().any(|a| a.path == r"C:\dl\pack\Show.S01E01.nfo"));

        let linked: Vec<String> = sqlx::query_scalar(
            "SELECT fp.file_path FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind = 'linked'",
        )
        .bind("aux_ep")
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(linked, vec![r"C:\dl\pack\Show.S01E01.mkv".to_string()]);
    }

    /// Sidecars list in natural path order, with no kind grouping (a subtitle and
    /// an nfo interleave by name).
    #[tokio::test]
    async fn auxiliary_files_are_natural_sorted() {
        let (db, _tmp) = aux_db().await;
        insert_path(&db, "/dl/c.nfo").await;
        insert_path(&db, "/dl/ep.10.srt").await;
        insert_path(&db, "/dl/ep.2.srt").await;

        db.link_file_episode("/dl/c.nfo", "aux_ep").await.unwrap();
        db.link_file_episode("/dl/ep.10.srt", "aux_ep")
            .await
            .unwrap();
        db.link_file_episode("/dl/ep.2.srt", "aux_ep")
            .await
            .unwrap();

        let aux = db.get_auxiliary_files_for_episode("aux_ep").await.unwrap();
        let paths: Vec<&str> = aux.iter().map(|a| a.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["/dl/c.nfo", "/dl/ep.2.srt", "/dl/ep.10.srt"],
            "natural path order, no kind grouping"
        );
    }
}
