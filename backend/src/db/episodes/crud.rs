// Episode CRUD Operations
// Episode CRUD: InsertEpisodeParams plus all insert/update/delete methods.

use crate::datetime::UtcDateTime;
use crate::db::DbManager;
use anyhow::Result;
use jumbie_shared::formatting::{fmt_absolute_episode_id, fmt_episode_id_num};
use jumbie_shared::types::EpisodeStatus;

/// Parameter struct for `insert_episode` to avoid 19 positional parameters.
/// Adding a field here means the compiler tells you about every place that
/// constructs the struct — no more silent mismatches from reordered args.
pub struct InsertEpisodeParams<'a> {
    pub episode_id: &'a str,
    pub series_id: &'a str,
    pub season: i32,
    pub episode: i32,
    pub file_path: Option<&'a str>,
    pub title: Option<&'a str>,
    pub quality_profile_id: Option<&'a str>,
    pub status: &'a str,
    pub meta_date: Option<chrono::NaiveDateTime>,
    pub est_date: Option<chrono::NaiveDateTime>,
    pub metadata_ids: &'a std::collections::HashMap<String, String>,
    pub description: Option<&'a str>,
    pub runtime: Option<i32>,
    pub image_url: Option<&'a str>,
    pub metadata_source: Option<&'a str>,
    /// Numbering mode: 0 = normal, 1 = absolute. Defaults to 0.
    pub numbering_mode: Option<i32>,
}

impl<'a> InsertEpisodeParams<'a> {
    /// Creates a dummy episode with required fields populated, and all optional fields set to None.
    /// Useful in tests to avoid boilerplate, while allowing targeted overrides using struct update syntax.
    pub fn dummy(
        episode_id: &'a str,
        series_id: &'a str,
        season: i32,
        episode: i32,
        metadata_ids: &'a std::collections::HashMap<String, String>,
    ) -> Self {
        Self {
            episode_id,
            series_id,
            season,
            episode,
            file_path: None,
            title: None,
            quality_profile_id: None,
            status: EpisodeStatus::Missing.as_str(),
            meta_date: None,
            est_date: None,
            metadata_ids,
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: None,
        }
    }
}

/// Parameters for inserting a parent episode row for part files.
pub struct InsertParentEpisodeParams<'a> {
    pub episode_id: &'a str,
    pub series_id: &'a str,
    pub season: i32,
    pub episode: i32,
    pub title: &'a str,
    pub numbering_mode: i32,
}

/// Parameters for saving custom metadata on an episode.
pub struct SaveCustomMetadataParams<'a> {
    pub episode_id: &'a str,
    pub series_id: &'a str,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub runtime: Option<i32>,
    pub image_url: Option<&'a str>,
    /// Canonical UTC timestamp (date-only input → 00:00:00 UTC).
    /// Typed (not `&str`) so a date-only string can never reach the column.
    pub meta_date: Option<UtcDateTime>,
}

impl DbManager {
    // The SINGLE function for inserting/updating episode rows. All paths converge
    // here: scanner, download pipeline, smart-link, metadata refresh.
    //
    // COALESCE preserves existing values for NULL params, so a metadata refresh with
    // only title+meta_date won't overwrite quality, file_path, or download_id set by
    // the download pipeline. `numbering_mode` defaults to 0 (normal).
    //
    // When the episode already holds a live main file, an incoming path does not
    // displace it; a language-variant pair is still reconciled (see
    // `reconcile_ingested_slot`).
    pub async fn insert_episode(&self, params: InsertEpisodeParams<'_>) -> Result<String> {
        use jumbie_shared::validation::{validate_episode_number, validate_season_number};
        validate_season_number(&params.season.to_string())
            .map_err(|e| anyhow::anyhow!("Invalid season {}: {}", params.season, e))?;
        validate_episode_number(params.episode)
            .map_err(|e| anyhow::anyhow!("Invalid episode {}: {}", params.episode, e))?;

        let meta_json =
            serde_json::to_string(params.metadata_ids).unwrap_or_else(|_| "{}".to_string());

        // Normalize placeholder defaults (Some(""), Some("Unknown"), Some(0)) to NULL so
        // COALESCE on the UPDATE path preserves existing metadata. Encoding this once
        // here makes every caller automatically safe.
        let title: Option<&str> = params.title.filter(|t| !t.is_empty());

        let numbering_mode = params.numbering_mode.unwrap_or(0);

        let existing: Option<(i64, String, String)> =
            sqlx::query_as("SELECT id, episode_id, status FROM episodes WHERE episode_id = ?")
                .bind(params.episode_id)
                .fetch_optional(&self.pool)
                .await?;

        let episode_id = if let Some((_id, existing_episode_id, existing_status)) = existing {
            // The episode's current non-part main file (if any) decides whether to
            // preserve or adopt the incoming path. Preserve while the file is still
            // alive; if it is gone, let the new path through immediately — otherwise
            // the episode stays stranded on a dead path for 30 days until
            // cleanup_missing_files fires. SSoT guard for all callers.
            let existing_path: Option<String> = sqlx::query_scalar(
                "SELECT fp.file_path FROM episode_files ef \
                 JOIN file_paths fp ON fp.id = ef.file_path_id \
                 WHERE ef.episode_id = ? AND ef.kind = 'main' \
                   AND ef.part_number IS NULL LIMIT 1",
            )
            .bind(&existing_episode_id)
            .fetch_optional(&self.pool)
            .await?;

            let preserve_existing = existing_path
                .as_ref()
                .is_some_and(|p| std::path::Path::new(p).exists());

            // `None` means "do not adopt a new path" (keep the current association).
            let effective_path: Option<&str> = if preserve_existing {
                None
            } else {
                params.file_path.filter(|p| !p.is_empty())
            };
            let status = if preserve_existing {
                &existing_status
            } else {
                params.status
            };

            sqlx::query(
                "UPDATE episodes SET
                 status = ?,
                 title = COALESCE(?, title),
                 quality_profile_id = COALESCE(?, quality_profile_id),
                 numbering_mode = COALESCE(?, numbering_mode),
                 meta_date = COALESCE(?, meta_date),
                 est_date = COALESCE(est_date, ?),
                 metadata_ids = COALESCE(?, metadata_ids),
                 description = COALESCE(?, description),
                 runtime = COALESCE(?, runtime),
                 image_url = COALESCE(?, image_url),
                 metadata_source = COALESCE(?, metadata_source),
                 file_acquired_at = CASE
                     WHEN ? IS NOT NULL THEN COALESCE(file_acquired_at, CURRENT_TIMESTAMP)
                     ELSE file_acquired_at
                 END
                 WHERE episode_id = ?",
            )
            .bind(status)
            .bind(title)
            .bind(params.quality_profile_id)
            .bind(numbering_mode)
            .bind(params.meta_date)
            .bind(params.est_date)
            .bind(&meta_json)
            .bind(params.description)
            .bind(params.runtime)
            .bind(params.image_url)
            .bind(params.metadata_source)
            .bind(effective_path)
            .bind(&existing_episode_id)
            .execute(&self.pool)
            .await?;

            if let Some(p) = effective_path {
                // The episode's main file is gone: adopt the incoming path, unless it
                // duplicates a language slot the episode still holds.
                let ingest = self
                    .reconcile_ingested_slot(&existing_episode_id, p, None, None)
                    .await?;
                if ingest.outcome.caller_writes_slot() {
                    self.associate_main_file(&existing_episode_id, p, None)
                        .await?;
                }
            } else if let Some(p) = params.file_path.filter(|p| !p.is_empty()) {
                // A live main file is preserved (a different release must not
                // displace it), but a language-variant pair is reconciled rather than
                // dropped.
                self.reconcile_ingested_slot(&existing_episode_id, p, None, None)
                    .await?;
            }
            existing_episode_id
        } else {
            sqlx::query(
                "INSERT INTO episodes (episode_id, series_id, season, episode, status, title, quality_profile_id, numbering_mode, monitored, meta_date, est_date, metadata_ids, description, runtime, image_url, metadata_source, file_acquired_at)
                                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
                                     CASE WHEN ? IS NOT NULL THEN CURRENT_TIMESTAMP ELSE NULL END)"
                )
                .bind(params.episode_id)
                    .bind(params.series_id)
                .bind(params.season)
                .bind(params.episode)
                .bind(params.status)
                .bind(title)
                .bind(params.quality_profile_id)
                .bind(numbering_mode)
                .bind(true)
                .bind(params.meta_date)
                .bind(params.est_date)
                .bind(&meta_json)
                .bind(params.description)
                .bind(params.runtime)
                .bind(params.image_url)
                .bind(params.metadata_source)
                .bind(params.file_path)
                .execute(&self.pool)
                .await?;

            if let Some(p) = params.file_path.filter(|p| !p.is_empty()) {
                self.associate_main_file(params.episode_id, p, None).await?;
            }
            params.episode_id.to_string()
        };

        Ok(episode_id)
    }

    /// Set the episode's status (update if present, minimal insert if not).
    ///
    /// SSoT for episode status transitions — the ONLY function that should touch
    /// `status` on the episode row, so callers only intending a status change cannot
    /// accidentally write other metadata fields.
    pub async fn set_episode_status(
        &self,
        episode_id: &str,
        series_id: &str,
        season: i32,
        episode: i32,
        status: &str,
    ) -> Result<String> {
        use jumbie_shared::validation::{validate_episode_number, validate_season_number};
        validate_season_number(&season.to_string())
            .map_err(|e| anyhow::anyhow!("Invalid season {}: {}", season, e))?;
        validate_episode_number(episode)
            .map_err(|e| anyhow::anyhow!("Invalid episode {}: {}", episode, e))?;

        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM episodes WHERE episode_id = ?")
                .bind(episode_id)
                .fetch_optional(&self.pool)
                .await?;

        if existing.is_some() {
            sqlx::query("UPDATE episodes SET status = ? WHERE episode_id = ?")
                .bind(status)
                .bind(episode_id)
                .execute(&self.pool)
                .await?;
            Ok(episode_id.to_string())
        } else {
            sqlx::query(
                "INSERT INTO episodes \
                 (episode_id, series_id, season, episode, status, \
                  numbering_mode, monitored) \
                 VALUES (?, ?, ?, ?, ?, 0, 1)",
            )
            .bind(episode_id)
            .bind(series_id)
            .bind(season)
            .bind(episode)
            .bind(status)
            .execute(&self.pool)
            .await?;
            Ok(episode_id.to_string())
        }
    }

    /// Insert a parent episode row for part files, doing nothing if it already exists.
    /// Delegates to `insert_episode` after a pre-check to preserve existing rows
    /// (ON CONFLICT DO NOTHING semantics for the parent placeholder use case).
    pub async fn insert_parent_episode(&self, params: InsertParentEpisodeParams<'_>) -> Result<()> {
        let InsertParentEpisodeParams {
            episode_id,
            series_id,
            season,
            episode,
            title,
            numbering_mode,
        } = params;

        let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM episodes WHERE episode_id = ?")
            .bind(episode_id)
            .fetch_optional(&self.pool)
            .await?;
        if exists.is_some() {
            return Ok(());
        }

        self.insert_episode(InsertEpisodeParams {
            episode_id,
            series_id,
            season,
            episode,
            file_path: None,
            title: Some(title),
            quality_profile_id: None,
            status: EpisodeStatus::Organized.as_str(),
            meta_date: None,
            est_date: None,
            metadata_ids: &std::collections::HashMap::new(),
            description: None,
            runtime: None,
            image_url: None,
            metadata_source: None,
            numbering_mode: Some(numbering_mode),
        })
        .await?;
        Ok(())
    }

    // Fingerprint Updates
    // Episode cell lifecycle: `episode_cell_type` marks provenance (NULL = standard,
    // 0 = UserDefined, 1 = Provider). ensure_episode_cells is the single place
    // UserDefined rows are created AND pruned, reading cell_count from the mapping's
    // season overrides (SSoT) in one transaction.

    /// Ensure DB rows exist for all configured episode cells (slots 1..cell_count for
    /// seasons with `cell_count` set), pruning UserDefined rows outside the current
    /// range (or all of them for a season with no cell_count).
    ///
    /// Called BEFORE `fill_missing_episodes`: once cells are real rows they take part
    /// in release-date estimation, metadata sync, and the calendar view.
    pub async fn ensure_episode_cells(
        &self,
        mapping: &jumbie_shared::types::MappingRule,
        normalized_seasons: &[String],
        global_absolute: bool,
    ) -> Result<()> {
        let series_id = &mapping.series_id;
        let active_mode = mapping.settings.active_mode(global_absolute);
        let absolute = active_mode.is_absolute();

        // Collect expected episode_ids per season that has cell_count set.
        // These define which cells to keep for the CURRENT mode.
        // episode_ids encode the mode (ABS vs SXXEXX) via fmt_episode_id,
        // so cells from the other mode have different IDs and are naturally
        // excluded from both the expected set and the mode-scoped DELETE.
        let mut expected_ids: Vec<String> = Vec::new();
        let mut has_cells = false;

        for raw_season in normalized_seasons {
            // Season resolution is mode-aware: absolute numbering is canonically
            // season 1 (see ABSOLUTE_SEASON_NUM) whatever the label says, while a
            // normal-mode label must be numeric. An unparseable normal-mode season
            // is skipped rather than coerced into some neighbouring season.
            let season_num = match jumbie_shared::mapping::resolve_season_num(raw_season, absolute)
            {
                Ok(n) => n,
                Err(e) => {
                    tracing::debug!(
                        "ensure_episode_cells: skipping season for series {} ({}): {}",
                        mapping.target_title,
                        series_id,
                        e
                    );
                    continue;
                }
            };

            let override_rule = mapping
                .settings
                .find_season_override(raw_season, global_absolute);

            let cell_count = override_rule.and_then(|o| o.cell_count);

            match cell_count {
                Some(n) if n > 0 => {
                    has_cells = true;
                    for i in 1..=n {
                        let eid = if absolute {
                            fmt_absolute_episode_id(i, series_id)
                        } else {
                            fmt_episode_id_num(season_num, i, series_id)
                        };
                        expected_ids.push(eid);
                    }
                }
                _ => {
                    // cell_count is None or 0 — no cells for this season.
                    // UserDefined rows for this season will be pruned below.
                }
            }
        }

        with_transaction!(self.pool, |mut tx| {
            let numbering_mode = absolute as i32;

            // A UserDefined cell is only a placeholder, safe to prune. A cell that
            // owns a file is not: dropping it would cascade its `episode_files` row
            // away and orphan the `file_paths` entry. `has_cells`/`expected_ids`
            // select the candidates; the ownership predicate spares the real ones.
            let prunable = format!(
                "DELETE FROM episodes AS e \
                 WHERE e.series_id = ? AND e.episode_cell_type = 0 \
                 AND e.numbering_mode = ? AND {unassigned}",
                unassigned = crate::db::EPISODE_UNASSIGNED_PREDICATE,
            );

            if !has_cells {
                // No season has cell_count set — prune ALL prunable cells for the
                // CURRENT mode only. Other mode's cells survive.
                sqlx::query(&prunable)
                    .bind(series_id)
                    .bind(numbering_mode)
                    .execute(&mut *tx)
                    .await?;
                return Ok(());
            }

            if expected_ids.is_empty() {
                return Ok(());
            }

            // Prune: delete UserDefined rows for the CURRENT mode whose
            // episode_id is outside the expected set.  The mode filter
            // ensures we never touch cells created under the other mode.
            let delete_sql = format!(
                "{prunable} AND e.episode_id NOT IN ({})",
                crate::db::sql_in_placeholders(expected_ids.len()),
            );
            let mut del_q = sqlx::query(&delete_sql)
                .bind(series_id)
                .bind(numbering_mode);
            for eid in &expected_ids {
                del_q = del_q.bind(eid);
            }
            del_q.execute(&mut *tx).await?;

            // Load occupied episode_ids AFTER the prune, so newly freed
            // slots are available for re-insertion with current format.
            // Only check current-mode rows to avoid blocking on other-mode
            // cells that may share (season, episode) pairs.
            let occupied_ids: std::collections::HashSet<String> = sqlx::query_scalar(
                "SELECT episode_id FROM episodes WHERE series_id = ? \
                     AND numbering_mode = ?",
            )
            .bind(series_id)
            .bind(numbering_mode)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();

            for raw_season in normalized_seasons {
                // See the pass above: mode-aware resolution — absolute is season
                // 1, a normal-mode label must parse or the season is skipped.
                let Ok(season_num) =
                    jumbie_shared::mapping::resolve_season_num(raw_season, absolute)
                else {
                    continue;
                };

                let override_rule = mapping
                    .settings
                    .find_season_override(raw_season, global_absolute);

                let cell_count = override_rule.and_then(|o| o.cell_count);
                let configured_start = override_rule.and_then(|o| o.episode_start).unwrap_or(1);
                let configured_end = override_rule
                    .and_then(|o| o.episode_end)
                    .unwrap_or(i32::MAX);

                if let Some(n) = cell_count
                    && n > 0
                {
                    for i in 1..=n {
                        let eid = if absolute {
                            fmt_absolute_episode_id(i, series_id)
                        } else {
                            fmt_episode_id_num(season_num, i, series_id)
                        };

                        // Skip if this episode_id already exists in the
                        // current mode (from a prior ensure_episode_cells
                        // or metadata fetch).
                        if occupied_ids.contains(&eid) {
                            continue;
                        }

                        let is_out_of_range = i < configured_start || i > configured_end;

                        sqlx::query(
                            "INSERT INTO episodes (
                                    episode_id, series_id, season, episode,
                                    status, monitored, episode_cell_type, numbering_mode
                                ) VALUES (?, ?, ?, ?, 'unreleased', ?, 0, ?)
                                ON CONFLICT(episode_id) DO NOTHING",
                        )
                        .bind(&eid)
                        .bind(series_id)
                        // Same resolved number the `eid` above was built from, so
                        // the row's season can never disagree with its own ID.
                        .bind(season_num)
                        .bind(i)
                        .bind(!is_out_of_range)
                        .bind(numbering_mode)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            }

            Ok(())
        })
    }

    /// Whether an `episodes` row (a "cell") exists for `episode_id`. The scanner
    /// uses this to attach sidecars only to cells that already exist — a cell is
    /// created by a video/part file, a user-defined cell, or a provider episode,
    /// never by a sidecar alone.
    pub async fn episode_exists(&self, episode_id: &str) -> Result<bool> {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM episodes WHERE episode_id = ?)")
                .bind(episode_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(exists)
    }

    /// Insert a minimal episode row (status `missing`) if none exists. Satisfies the
    /// `episode_files` foreign key when a file is deliberately attached to an episode
    /// whose cell does not exist yet (orphan adoption, or a sidecar assigned by hand).
    /// The scanner never calls this for sidecars — it only attaches to existing cells.
    pub async fn ensure_episode_row(
        &self,
        episode_id: &str,
        series_id: &str,
        season: i32,
        episode: i32,
        numbering_mode: i32,
    ) -> Result<()> {
        Self::ensure_episode_row_tx(
            &mut *self.pool.acquire().await?,
            episode_id,
            series_id,
            season,
            episode,
            numbering_mode,
        )
        .await
    }

    /// [`Self::ensure_episode_row`] on an open connection.
    pub(crate) async fn ensure_episode_row_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        series_id: &str,
        season: i32,
        episode: i32,
        numbering_mode: i32,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO episodes \
             (episode_id, series_id, season, episode, status, numbering_mode, monitored) \
             VALUES (?, ?, ?, ?, 'missing', ?, 1) \
             ON CONFLICT(episode_id) DO NOTHING",
        )
        .bind(episode_id)
        .bind(series_id)
        .bind(season)
        .bind(episode)
        .bind(numbering_mode)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    // File (Un)linking
    // unlink_episode_file applies the SSoT disowned state (manual unassign, or the
    // organize pipeline clearing an old path). relink_episode_file points the
    // episode at a new path after the move and marks it 'downloaded'.

    pub async fn unlink_episode_file(&self, episode_id: &str) -> anyhow::Result<()> {
        // SSoT disowned state (clears the association, status, dates).
        self.clear_episode_ownership(&[episode_id.to_string()])
            .await?;
        Ok(())
    }

    pub async fn relink_episode_file(
        &self,
        episode_id: &str,
        file_path: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE episodes SET status = 'downloaded' WHERE episode_id = ?")
            .bind(episode_id)
            .execute(&self.pool)
            .await?;
        self.associate_main_file(episode_id, file_path, None)
            .await?;
        Ok(())
    }

    // Custom Episode Metadata
    // Custom/cleared/provider metadata source states — the SSoT for modifying
    // `metadata_source` and the metadata fields outside the batch sync path.

    /// Save custom metadata for a single episode. Updates the provided fields
    /// and sets `metadata_source = 'custom'` so polling does not overwrite them.
    pub async fn save_custom_episode_metadata(
        &self,
        params: SaveCustomMetadataParams<'_>,
    ) -> Result<bool> {
        let SaveCustomMetadataParams {
            episode_id,
            series_id,
            title,
            description,
            runtime,
            image_url,
            meta_date,
        } = params;

        // Only update fields that were explicitly provided — NULL means
        // "keep the existing value" (COALESCE).  This prevents a title-only
        // edit from nulling out meta_date or other non-provided fields.
        let rows = sqlx::query(
            "UPDATE episodes SET
                title = ?,
                description = COALESCE(?, description),
                runtime = COALESCE(?, runtime),
                image_url = COALESCE(?, image_url),
                meta_date = COALESCE(?, meta_date),
                metadata_source = 'custom'
             WHERE episode_id = ? AND series_id = ?",
        )
        .bind(title)
        .bind(description)
        .bind(runtime)
        .bind(image_url)
        .bind(meta_date)
        .bind(episode_id)
        .bind(series_id)
        .execute(&self.pool)
        .await?;
        Ok(rows.rows_affected() > 0)
    }

    /// Clear metadata for a single episode. Nulls out all metadata fields
    /// and sets `metadata_source = 'cleared'` so polling does not re-populate.
    pub async fn clear_episode_metadata(&self, episode_id: &str, series_id: &str) -> Result<bool> {
        let rows = sqlx::query(
            "UPDATE episodes SET
                title = NULL,
                description = NULL,
                runtime = NULL,
                image_url = NULL,
                meta_date = NULL,
                metadata_source = 'cleared'
             WHERE episode_id = ? AND series_id = ?",
        )
        .bind(episode_id)
        .bind(series_id)
        .execute(&self.pool)
        .await?;
        Ok(rows.rows_affected() > 0)
    }

    /// Reset a single episode back to provider state. Nulls out metadata
    /// fields and sets `metadata_source = NULL` so the poller re-fills them.
    pub async fn reset_episode_metadata_source(
        &self,
        episode_id: &str,
        series_id: &str,
    ) -> Result<bool> {
        let rows = sqlx::query(
            "UPDATE episodes SET
                title = NULL,
                description = NULL,
                runtime = NULL,
                image_url = NULL,
                meta_date = NULL,
                metadata_source = NULL
             WHERE episode_id = ? AND series_id = ?",
        )
        .bind(episode_id)
        .bind(series_id)
        .execute(&self.pool)
        .await?;
        Ok(rows.rows_affected() > 0)
    }

    /// Reset all episodes in a season back to provider state. Nulls out
    /// metadata fields and sets `metadata_source = NULL` so polling re-fills.
    pub async fn match_season_to_provider(&self, series_id: &str, season: i32) -> Result<u64> {
        let rows = sqlx::query(
            "UPDATE episodes SET
                title = NULL,
                description = NULL,
                runtime = NULL,
                image_url = NULL,
                meta_date = NULL,
                metadata_source = NULL
             WHERE series_id = ? AND season = ?",
        )
        .bind(series_id)
        .bind(season)
        .execute(&self.pool)
        .await?;
        Ok(rows.rows_affected())
    }

    /// Reset ALL episodes in a series back to provider state.
    pub async fn match_series_to_provider(&self, series_id: &str) -> Result<u64> {
        let rows = sqlx::query(
            "UPDATE episodes SET
                title = NULL,
                description = NULL,
                runtime = NULL,
                image_url = NULL,
                meta_date = NULL,
                metadata_source = NULL
             WHERE series_id = ?",
        )
        .bind(series_id)
        .execute(&self.pool)
        .await?;
        Ok(rows.rows_affected())
    }

    /// Episode rows for one season in one numbering mode, as
    /// `(episode_id, episode, file_path)`; `file_path` is the episode's primary main
    /// file (NULL when unassigned).
    pub async fn get_episode_rows_for_season(
        &self,
        series_id: &str,
        season: i32,
        numbering_mode: i32,
    ) -> Result<Vec<(String, i32, Option<String>)>> {
        let sql = format!(
            "SELECT e.episode_id, e.episode, fp.file_path FROM episodes e \
             LEFT JOIN file_paths fp ON fp.id = {primary} \
             WHERE e.series_id = ? AND e.season = ? AND e.numbering_mode = ?",
            primary = crate::db::EPISODE_PRIMARY_FILE_ID,
        );
        let rows = sqlx::query_as(&sql)
            .bind(series_id)
            .bind(season)
            .bind(numbering_mode)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    /// Delete specific episode rows and their dependent records (episode_files,
    /// file_paths, retry_queue). Used when matching a season to the
    /// provider drops episodes the provider does not have.
    pub async fn delete_episodes(&self, episode_ids: &[String]) -> Result<()> {
        if episode_ids.is_empty() {
            return Ok(());
        }
        with_transaction!(self.pool, |mut tx| {
            // Path rows owned only by these episodes are matched while the
            // associations still exist; the episode rows then cascade their
            // `episode_files` rows.
            Self::delete_episode_owned_paths_tx(&mut tx, episode_ids).await?;

            for chunk in episode_ids.chunks(450) {
                let placeholders = crate::db::sql_in_placeholders(chunk.len());
                for table in ["retry_queue", "episodes"] {
                    let sql = format!("DELETE FROM {table} WHERE episode_id IN ({placeholders})");
                    let mut q = sqlx::query(&sql);
                    for id in chunk {
                        q = q.bind(id);
                    }
                    q.execute(&mut *tx).await?;
                }
            }
            Ok(())
        })
    }
}
