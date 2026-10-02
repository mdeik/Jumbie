// Episode File Assignment & Batch Metadata
// Episode file assignment and bulk metadata insertion — the heaviest episode ops.

use crate::db::DbManager;
use crate::models::activity::{ActivityEvent, ActivityType};
use anyhow::Result;

pub struct AssignFileToEpisodeParams<'a> {
    pub episode_id: &'a str,
    pub series_id: &'a str,
    pub series_title: &'a str,
    pub season: i32,
    pub episode: i32,
    pub file_path: &'a str,
    pub episode_title: &'a str,
    pub all_target_episode_ids: Option<&'a [String]>,
    pub numbering_mode: i32,
    pub only_unassigned: bool,
}

impl DbManager {
    // Core "assign a file to an episode" operation.
    //
    // `all_target_episode_ids` is the set of episodes this file is being assigned to:
    // when clearing old references we exclude them so a multi-episode file doesn't
    // clear itself from its own targets.
    //
    // `only_unassigned`: true = skip if the file is on ANY episode (auto-assignment);
    // false = skip only if it is on the SAME episode (manual assignment; reassign to a
    // different episode is allowed).
    /// Assign a file to an episode on the manual/deliberate path: a same-language-slot
    /// duplicate takes the slot (the caller explicitly chose this file).
    pub async fn assign_file_to_episode(
        &self,
        params: AssignFileToEpisodeParams<'_>,
    ) -> Result<Vec<String>> {
        self.assign_file_to_episode_with(params, crate::db::ownership::SlotPolicy::PreferCandidate)
            .await
    }

    /// Assign a file to an episode on the download-ingestion path: a same-language-slot
    /// duplicate is left alone (never clobber a good existing file).
    pub(crate) async fn ingest_file_to_episode(
        &self,
        params: AssignFileToEpisodeParams<'_>,
    ) -> Result<Vec<String>> {
        self.assign_file_to_episode_with(params, crate::db::ownership::SlotPolicy::KeepExisting)
            .await
    }

    async fn assign_file_to_episode_with(
        &self,
        params: AssignFileToEpisodeParams<'_>,
        slot_policy: crate::db::ownership::SlotPolicy,
    ) -> Result<Vec<String>> {
        use jumbie_shared::validation::{validate_episode_number, validate_season_number};
        validate_season_number(&params.season.to_string())
            .map_err(|e| anyhow::anyhow!("Invalid season {}: {}", params.season, e))?;
        validate_episode_number(params.episode)
            .map_err(|e| anyhow::anyhow!("Invalid episode {}: {}", params.episode, e))?;

        // Auxiliary sidecars never become an episode's playable file: link as aux and
        // leave the canonical file_path (and any existing video) untouched. A
        // deliberate assignment (manual, or download ingestion) materializes the cell
        // first, so an episode that exists only to hold sidecars can be created here;
        // the scanner never does this.
        if let Some(kind) =
            jumbie_shared::media_format::file_kind_for_path(std::path::Path::new(params.file_path))
            && kind.is_auxiliary()
        {
            self.ensure_episode_row(
                params.episode_id,
                params.series_id,
                params.season,
                params.episode,
                params.numbering_mode,
            )
            .await?;
            self.link_file_episode_as(params.file_path, params.episode_id, kind)
                .await?;
            return Ok(Vec::new());
        }

        // Guard: episode_ids encode their mode (`_ABS` prefix = absolute). A mismatch
        // with the caller's numbering_mode is logged so broken future assignments are
        // visible rather than silently invisible to mode-scoped queries.
        let inferred_mode = if params.episode_id.contains("_ABS") {
            1
        } else {
            0
        };
        if params.numbering_mode != inferred_mode {
            tracing::debug!(
                "assign_file_to_episode: numbering_mode {} doesn't match episode_id '{}' (inferred {}). {} Assignment will still proceed but may be invisible to mode-scoped queries.",
                params.numbering_mode,
                params.episode_id,
                inferred_mode,
                if inferred_mode == 0 && params.numbering_mode == 1 {
                    "Episode looks normal-format but caller passed absolute mode."
                } else if inferred_mode == 1 && params.numbering_mode == 0 {
                    "Episode looks absolute-format but caller passed normal mode (the old silent-failure bug)."
                } else {
                    ""
                },
            );
        }

        let strategy = self.collision_strategy().await;
        let tx_event_type = with_transaction!(self.pool, |mut tx| {
            // Assignment conflict (SSoT `db::ownership::is_assignment_conflict`):
            // same-mode episodes that already hold this path OUTSIDE the target set.
            // A holder inside the target set is not a conflict — a multi-episode file
            // is legitimately shared by every episode in its range, so the remaining
            // targets must still be assigned.
            let self_holds = crate::db::ownership::episode_holds_path(
                &mut tx,
                params.episode_id,
                params.file_path,
                params.numbering_mode,
            )
            .await?;
            let conflict = crate::db::ownership::is_assignment_conflict(
                &mut tx,
                params.file_path,
                params.numbering_mode,
                params.all_target_episode_ids,
                params.episode_id,
                None,
            )
            .await?;
            let displaced = conflict.displaced;

            // Skip when the file is already on this episode, or (only_unassigned) when
            // an episode outside the target set occupies it.
            if self_holds || (params.only_unassigned && !displaced.is_empty()) {
                tracing::trace!(
                    "assign_file_to_episode: '{}' already held (self={}, outside_targets={}) — skipping",
                    params.file_path,
                    self_holds,
                    !displaced.is_empty()
                );
                return Ok((false, vec![], vec![]));
            }

            // Displaced episodes lose their file entirely. One SSoT disown keeps their
            // status and part rows consistent with the unassign/delete paths; a
            // displaced multi-episode file is disowned for its complete range.
            if !displaced.is_empty() {
                crate::db::ownership::disown_episodes_tx(&mut tx, &displaced).await?;
            }

            // Assign to the new episode. The ON CONFLICT SET list omits `title` (an
            // existing title is not clobbered) and keeps the first `file_acquired_at`.
            // The file's date is not stored here: it lives in `release_info`
            // (content-keyed), resolved at read time through `file_paths`.
            let title_to_store: Option<&str> = if params.episode_title.is_empty() {
                None
            } else {
                Some(params.episode_title)
            };

            sqlx::query(
                "
                INSERT INTO episodes (
                    episode_id, series_id, season, episode, status, title, numbering_mode
                ) VALUES (?, ?, ?, ?, 'organized', ?, ?)
                ON CONFLICT(episode_id) DO UPDATE SET
                    status = excluded.status,
                    numbering_mode = COALESCE(excluded.numbering_mode, numbering_mode),
                    file_acquired_at = COALESCE(file_acquired_at, CURRENT_TIMESTAMP)
                ",
            )
            .bind(params.episode_id)
            .bind(params.series_id)
            .bind(params.season)
            .bind(params.episode)
            .bind(title_to_store)
            .bind(params.numbering_mode)
            .execute(&mut *tx)
            .await?;

            // Attach the file. A language sibling is reconciled (kept alongside, or
            // promoted to the slot); a same-tag file follows the collision strategy;
            // anything else becomes the episode's main file.
            let ingest = crate::db::DbManager::reconcile_slot_tx(
                &mut tx,
                params.episode_id,
                params.file_path,
                None,
                None,
                slot_policy,
                strategy,
            )
            .await?;
            let mut superseded: Vec<String> = ingest.displaced.into_iter().collect();
            if ingest.outcome.caller_writes_slot() {
                let write = crate::db::DbManager::write_slot_tx(
                    &mut tx,
                    strategy,
                    params.episode_id,
                    params.file_path,
                    None,
                    None,
                )
                .await?;
                superseded.extend(write.displaced);
            }

            let event_type = if displaced.is_empty() {
                "assign"
            } else {
                "reassign"
            };
            sqlx::query("INSERT INTO file_event_log (event_type, source_path, destination_path, status) VALUES (?, ?, ?, 'completed')")
            .bind(event_type)
            .bind(params.file_path)
            .bind(params.episode_id)
            .execute(&mut *tx)
            .await?;

            // Record whether this was a reassign for the caller's post-commit activity
            // logging. Must happen OUTSIDE the tx: record_activity uses self.pool, which
            // would deadlock while this write tx is open (SQLite allows one writer).
            let is_reassign = !displaced.is_empty();
            Ok::<_, anyhow::Error>((is_reassign, displaced, superseded))
        })?;

        let (is_reassign, old_ids, superseded) = tx_event_type;

        // Delete any file the `overwrite` collision strategy replaced, now that the
        // association write has committed.
        for path in &superseded {
            self.delete_superseded(path).await;
        }

        self.record_activity(ActivityEvent {
            event_type: if is_reassign {
                ActivityType::Reassign
            } else {
                ActivityType::Assign
            },
            series_title: params.series_title.to_string(),
            season: Some(params.season.to_string()),
            episode: Some(params.episode),
            episode_end: None,
            title: None,
            details: Some(params.file_path.to_string()),
            status: "Success".to_string(),
        })
        .await?;

        // Explicit assignment resolves any prior "do not auto-adopt" block for this
        // file (a manual assign is a deliberate re-adoption). SSoT: `blocked_files`.
        let _ = self
            .clear_blocked_files_for_path(params.series_id, params.file_path)
            .await;

        Ok(old_ids)
    }

    // Surgical merge of provider metadata into the episodes table (replaces the old
    // delete-all-then-reinsert, which destroyed the file association on organized
    // episodes): in metadata → UPSERT (preserving the association/status); absent +
    // owns a file → clear metadata fields only; absent + fileless → DELETE the row.
    //
    // "owns a file" = has a main `episode_files` association (single, multipart, or
    // shared multi-episode). `metadata_source = instance_id` = "row owned by this
    // provider instance".
    //
    // Fresh inserts are `monitored = 0`: a new metadata episode has no user intent yet,
    // so reapply_monitor_for_series sets the per-mode flag. ON CONFLICT never touches
    // `monitored`, so existing rows keep their state.
    //
    // `instance_id` is the provider INSTANCE id (not the plugin type id): it keys
    // both the episode `metadata_ids` map and the `metadata_source` column.
    pub async fn merge_metadata_episodes(
        &self,
        instance_id: &str,
        series_id: &str,
        episodes: Vec<(String, crate::plugins::metadata::EpisodeMetadata)>,
        numbering_mode: i32,
    ) -> anyhow::Result<()> {
        let new_ids: std::collections::HashSet<String> =
            episodes.iter().map(|(id, _)| id.clone()).collect();

        // Seasons the user deleted must never be (re)created by a sync. The
        // absolute-mode exception lives in `get_suppressed_seasons`.
        let suppressed: std::collections::HashSet<i32> = self
            .get_suppressed_seasons(series_id, numbering_mode)
            .await?
            .into_iter()
            .collect();

        with_transaction!(self.pool, |mut tx| {
            // Phase 1: categorize existing provider rows. `has_file` is the ownership
            // signal — a multipart episode (no single path but owning main
            // associations) must not be mistaken for a fileless row.
            let existing: Vec<(String, i64)> = sqlx::query_as(
                "SELECT e.episode_id, \
                        EXISTS(SELECT 1 FROM episode_files ef \
                               WHERE ef.episode_id = e.episode_id AND ef.kind = 'main') \
                 FROM episodes e \
                 WHERE e.series_id = ? AND e.metadata_source = ?",
            )
            .bind(series_id)
            .bind(instance_id)
            .fetch_all(&mut *tx)
            .await?;

            let mut to_delete = Vec::new();
            let mut to_clear = Vec::new();
            for (ep_id, has_file) in &existing {
                if new_ids.contains(ep_id.as_str()) {
                    continue;
                }
                // A row that still owns a file survives the sync (only its provider
                // metadata is cleared); a fileless stale row is removed. This also
                // protects a file-owning row in a suppressed season — the sync never
                // deletes the user's files.
                if *has_file != 0 {
                    to_clear.push(ep_id.clone());
                } else {
                    to_delete.push(ep_id.clone());
                }
            }

            // Phase 2: DELETE stale rows (no file, gone from provider). The episode
            // row cascades its `episode_files` associations.
            if !to_delete.is_empty() {
                for chunk in to_delete.chunks(450) {
                    let placeholders = crate::db::sql_in_placeholders(chunk.len());
                    let sql = format!(
                        "DELETE FROM episodes WHERE episode_id IN ({})",
                        placeholders
                    );
                    let mut q = sqlx::query(&sql);
                    for id in chunk {
                        q = q.bind(id);
                    }
                    q.execute(&mut *tx).await?;
                }
            }

            // Phase 3: CLEAR metadata on orphaned organized episodes
            if !to_clear.is_empty() {
                for chunk in to_clear.chunks(450) {
                    let placeholders = crate::db::sql_in_placeholders(chunk.len());
                    let sql = format!(
                        "UPDATE episodes SET \
                         title = NULL, description = NULL, metadata_ids = '{{}}', \
                         runtime = NULL, image_url = NULL, meta_date = NULL, \
                         metadata_source = NULL \
                         WHERE episode_id IN ({})",
                        placeholders
                    );
                    let mut q = sqlx::query(&sql);
                    for id in chunk {
                        q = q.bind(id);
                    }
                    q.execute(&mut *tx).await?;
                }
            }

            // Phase 4: UPSERT new metadata (ON CONFLICT preserves file_path)
            for (episode_id, ep) in &episodes {
                // Never (re)create a season the user deleted.
                if suppressed.contains(&ep.season) {
                    continue;
                }
                let mut metadata_ids = std::collections::HashMap::new();
                metadata_ids.insert(instance_id.to_string(), ep.unique_id.clone());
                let meta_json =
                    serde_json::to_string(&metadata_ids).unwrap_or_else(|_| "{}".to_string());

                sqlx::query(
                    "INSERT INTO episodes (
                        episode_id, series_id, season, episode, title, status,
                        metadata_ids, description, runtime, image_url, meta_date,
                        numbering_mode, metadata_source, monitored
                    ) VALUES (?, ?, ?, ?, ?, 'unreleased', ?, ?, ?, ?, ?, ?, ?, 0)
                    ON CONFLICT(episode_id) DO UPDATE SET
                        title = CASE
                            WHEN episodes.metadata_source IN ('custom', 'cleared')
                            THEN episodes.title
                            ELSE COALESCE(NULLIF(excluded.title, ''), episodes.title)
                        END,
                        metadata_ids = COALESCE(excluded.metadata_ids, episodes.metadata_ids),
                        description = CASE
                            WHEN episodes.metadata_source IN ('custom', 'cleared')
                            THEN episodes.description
                            ELSE COALESCE(excluded.description, episodes.description)
                        END,
                        runtime = CASE
                            WHEN episodes.metadata_source IN ('custom', 'cleared')
                            THEN episodes.runtime
                            ELSE COALESCE(excluded.runtime, episodes.runtime)
                        END,
                        image_url = CASE
                            WHEN episodes.metadata_source IN ('custom', 'cleared')
                            THEN episodes.image_url
                            ELSE COALESCE(excluded.image_url, episodes.image_url)
                        END,
                        meta_date = CASE
                            WHEN episodes.metadata_source IN ('custom', 'cleared')
                            THEN episodes.meta_date
                            ELSE COALESCE(excluded.meta_date, episodes.meta_date)
                        END,
                        metadata_source = CASE
                            WHEN episodes.metadata_source IN ('custom', 'cleared')
                            THEN episodes.metadata_source
                            ELSE ?
                        END,
                        episode_cell_type =
                            CASE WHEN episodes.episode_cell_type = 0 THEN 1
                                 ELSE episodes.episode_cell_type
                            END,
                        numbering_mode = COALESCE(excluded.numbering_mode, numbering_mode)",
                )
                .bind(episode_id)
                .bind(series_id)
                .bind(ep.season)
                .bind(ep.episode)
                .bind(&ep.title)
                .bind(&meta_json)
                .bind(ep.description.as_deref())
                .bind(ep.runtime)
                .bind(ep.image_url.as_deref())
                .bind(ep.meta_date)
                .bind(numbering_mode)
                .bind(instance_id)
                .bind(instance_id)
                .execute(&mut *tx)
                .await?;
            }

            Ok(())
        })
    }

    // Bulk-insert episodes from a metadata provider in one transaction (a partial
    // failure rolls back the batch). ON CONFLICT DO UPDATE preserves non-metadata
    // fields (file_path, download_id, …) set by the download pipeline.
    //
    // `instance_id` is the provider INSTANCE id (not the plugin type id): it keys
    // both the episode `metadata_ids` map and the `metadata_source` column.
    pub async fn batch_insert_metadata_episodes(
        &self,
        instance_id: &str,
        series_id: &str,
        episodes: Vec<(String, crate::plugins::metadata::EpisodeMetadata)>,
        numbering_mode: i32,
    ) -> anyhow::Result<()> {
        with_transaction!(self.pool, |mut tx| {
            for (episode_id, ep) in episodes {
                let mut metadata_ids = std::collections::HashMap::new();
                metadata_ids.insert(instance_id.to_string(), ep.unique_id.clone());
                let meta_json =
                    serde_json::to_string(&metadata_ids).unwrap_or_else(|_| "{}".to_string());

                sqlx::query(
                    "
                INSERT INTO episodes (
                    episode_id, series_id, season, episode, title, status,
                    metadata_ids, description, runtime, image_url, meta_date,
                    numbering_mode, metadata_source, monitored
                ) VALUES (?, ?, ?, ?, ?, 'unreleased', ?, ?, ?, ?, ?, ?, ?, 0)
                ON CONFLICT(episode_id) DO UPDATE SET
                    title = CASE
                        WHEN episodes.metadata_source IN ('custom', 'cleared')
                        THEN episodes.title
                        ELSE COALESCE(NULLIF(excluded.title, ''), episodes.title)
                    END,
                    metadata_ids = COALESCE(excluded.metadata_ids, episodes.metadata_ids),
                    description = CASE
                        WHEN episodes.metadata_source IN ('custom', 'cleared')
                        THEN episodes.description
                        ELSE COALESCE(excluded.description, episodes.description)
                    END,
                    runtime = CASE
                        WHEN episodes.metadata_source IN ('custom', 'cleared')
                        THEN episodes.runtime
                        ELSE COALESCE(excluded.runtime, episodes.runtime)
                    END,
                    image_url = CASE
                        WHEN episodes.metadata_source IN ('custom', 'cleared')
                        THEN episodes.image_url
                        ELSE COALESCE(excluded.image_url, episodes.image_url)
                    END,
                    meta_date = CASE
                        WHEN episodes.metadata_source IN ('custom', 'cleared')
                        THEN episodes.meta_date
                        ELSE COALESCE(excluded.meta_date, episodes.meta_date)
                    END,
                    metadata_source = CASE
                        WHEN episodes.metadata_source IN ('custom', 'cleared')
                        THEN episodes.metadata_source
                        ELSE ?
                    END,
                    episode_cell_type =
                        CASE WHEN episodes.episode_cell_type = 0 THEN 1
                             ELSE episodes.episode_cell_type
                        END,
                    numbering_mode = COALESCE(excluded.numbering_mode, numbering_mode)
            ",
                )
                .bind(&episode_id)
                .bind(series_id)
                .bind(ep.season)
                .bind(ep.episode)
                .bind(&ep.title)
                .bind(&meta_json)
                .bind(ep.description.as_deref())
                .bind(ep.runtime)
                .bind(ep.image_url.as_deref())
                .bind(ep.meta_date)
                .bind(numbering_mode)
                .bind(instance_id)
                .bind(instance_id) // for the ON CONFLICT metadata_source CASE
                .execute(&mut *tx)
                .await?;
            }
            Ok(())
        })
    }
}
