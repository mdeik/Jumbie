//! SSoT for episode ↔ file ownership.
//!
//! Ownership is the `episode_files` association: each row links an episode to a
//! tracked `file_paths` row, tagged `main` (the episode's playable file — single,
//! multipart part, or a shared multi-episode file) or `auxiliary` (an
//! episode-scoped sidecar). An episode's **primary** main file is its non-part
//! association if present, else its lowest part (see
//! [`crate::db::EPISODE_FILE_JOIN`]).
//!
//! Every operation that removes an episode's file — reassignment, unassign,
//! delete, clear-missing, series-path clears — MUST go through this module so the
//! disowned episode state and its associations never drift between callers.

use super::DbManager;
use anyhow::Result;

/// Apply the canonical "episode no longer owns any file" state to `episode_ids`
/// inside an open transaction: mark the episode `missing`, clear
/// `file_acquired_at`, and drop all of its associations (main and auxiliary).
/// Episode-level dates (`meta_date`, `est_date`) are untouched — the file's date
/// lives in `release_info`, not on the episode.
///
/// This is the single definition of the disowned state. Callers that only know a
/// file path should use [`DbManager::disown_path`] instead.
pub(crate) async fn disown_episodes_tx(
    conn: &mut sqlx::SqliteConnection,
    episode_ids: &[String],
) -> Result<()> {
    for chunk in episode_ids.chunks(500) {
        let ph = crate::db::sql_in_placeholders(chunk.len());

        let sql = format!(
            "UPDATE episodes SET status = 'missing', file_acquired_at = NULL \
             WHERE episode_id IN ({ph})"
        );
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        q.execute(&mut *conn).await?;

        let sql = format!("DELETE FROM episode_files WHERE episode_id IN ({ph})");
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        q.execute(&mut *conn).await?;
    }
    Ok(())
}

/// The result of [`is_assignment_conflict`].
#[derive(Debug, Default, Clone)]
pub(crate) struct AssignmentConflict {
    /// Same-mode episodes, outside the target set, that hold `file_path` as a main
    /// file and must be disowned for the assignment to proceed. A displaced
    /// multi-episode file is unassigned for its **complete** range — this set.
    pub displaced: Vec<String>,
    /// The target episode already has a DIFFERENT file in the candidate multipart
    /// slot (`part_number`). The caller decides the resolution; a deliberate manual
    /// assignment replaces the slot (via `upsert_episode_part`).
    pub part_slot_taken: bool,
}

/// An existing video association (main or linked) that occupies a language slot.
struct SlotOccupant {
    file_path: String,
    kind: String,
    part_number: Option<i32>,
}

/// What ingesting a video file into an episode's slot did, so the caller knows
/// whether to perform its usual slot write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IngestOutcome {
    /// Reconciled with a language sibling: attached alongside, promoted to the slot,
    /// or a same-slot association replaced. The caller must not write.
    Handled,
    /// The file duplicates a language slot the episode already holds. The caller must
    /// not write (one file per language tag).
    Duplicate,
    /// No language relationship; the caller must perform its usual slot write.
    Unrelated,
}

impl IngestOutcome {
    /// Whether the caller still has to write the slot itself.
    pub(crate) fn caller_writes_slot(self) -> bool {
        matches!(self, IngestOutcome::Unrelated)
    }
}

/// The result of [`DbManager::reconcile_slot_tx`]: the language-slot decision plus
/// the file the collision strategy replaced, if any.
pub(crate) struct SlotIngest {
    pub outcome: IngestOutcome,
    /// A file superseded under the `overwrite` strategy, to be deleted by the caller
    /// AFTER the association write commits. `None` under `rename`/`skip`.
    pub displaced: Option<String>,
}

/// The result of a slot write under the organization collision strategy.
pub(crate) struct SlotWrite {
    /// Whether the slot was written (`false` when the strategy skipped it).
    pub wrote: bool,
    /// The superseded file the `overwrite` strategy replaced, to be deleted by the
    /// caller after committing. `None` under `rename`/`skip` or with no collision.
    pub displaced: Option<String>,
}

/// How an incoming file is reconciled with a file already in its language slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotPolicy {
    /// Keep an existing same-slot file that is still on disk: an incoming duplicate is
    /// not attached (scanner / download ingestion).
    KeepExisting,
    /// The caller deliberately chose this file: it takes the slot, replacing the
    /// existing same-slot association (manual assignment).
    PreferCandidate,
}

/// Assignment conflict predicate (SSoT).
///
/// Reports everything that makes assigning `file_path` to `episode_id` a conflict:
///
///  * `displaced` — same-mode episodes **outside** the caller's target set that
///    already hold `file_path` as a main file. The check is kind-agnostic: a path
///    held as a single file, a part, or a shared multi-episode file all count, so a
///    cross-kind assignment (single↔multipart↔multi-episode) is caught. Holders
///    inside the target set are NOT conflicts: a multi-episode file is legitimately
///    shared by every episode in its range.
///  * `part_slot_taken` — for a multipart assignment (`part_number` is `Some`), the
///    target episode already has a *different* file in that part slot.
///
/// Whether the episode already holds the path itself is answered separately by
/// [`episode_holds_path`], so a re-assign is an idempotent no-op.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn is_assignment_conflict(
    conn: &mut sqlx::SqliteConnection,
    file_path: &str,
    numbering_mode: i32,
    target_episode_ids: Option<&[String]>,
    episode_id: &str,
    part_number: Option<i32>,
) -> Result<AssignmentConflict> {
    let mut sql = "SELECT e.episode_id FROM episodes e \
                   JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main' \
                   JOIN file_paths fp ON fp.id = ef.file_path_id \
                   WHERE fp.file_path = ? AND e.numbering_mode = ? AND e.episode_id != ?"
        .to_string();
    let excluded: &[String] = target_episode_ids.unwrap_or(&[]);
    if !excluded.is_empty() {
        sql.push_str(&format!(
            " AND e.episode_id NOT IN ({})",
            crate::db::sql_in_placeholders(excluded.len())
        ));
    }
    let mut q = sqlx::query_scalar::<_, String>(&sql)
        .bind(file_path)
        .bind(numbering_mode)
        .bind(episode_id);
    for id in excluded {
        q = q.bind(id);
    }
    let displaced = q.fetch_all(&mut *conn).await?;

    let part_slot_taken = match part_number {
        Some(n) => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number = ? \
               AND fp.file_path <> ?)",
            )
            .bind(episode_id)
            .bind(n)
            .bind(file_path)
            .fetch_one(&mut *conn)
            .await?
        }
        None => false,
    };

    Ok(AssignmentConflict {
        displaced,
        part_slot_taken,
    })
}

/// Whether `episode_id` currently holds `file_path` as a main file in the mode.
pub(crate) async fn episode_holds_path(
    conn: &mut sqlx::SqliteConnection,
    episode_id: &str,
    file_path: &str,
    numbering_mode: i32,
) -> Result<bool> {
    let found: Option<String> = sqlx::query_scalar(
        "SELECT e.episode_id FROM episodes e \
         JOIN episode_files ef ON ef.episode_id = e.episode_id AND ef.kind = 'main' \
         JOIN file_paths fp ON fp.id = ef.file_path_id \
         WHERE e.episode_id = ? AND fp.file_path = ? AND e.numbering_mode = ?",
    )
    .bind(episode_id)
    .bind(file_path)
    .bind(numbering_mode)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(found.is_some())
}

/// A single multipart registration, applied atomically by
/// [`DbManager::register_multipart_assignment`].
pub(crate) struct MultipartAssignment<'a> {
    pub episode_id: &'a str,
    pub series_id: &'a str,
    pub season: i32,
    pub episode: i32,
    pub numbering_mode: i32,
    /// The incoming part file and the slot it occupies.
    pub file_path: &'a str,
    pub part_number: u32,
    pub size: Option<i64>,
    /// Sibling episodes of the same logical assignment; holders here are not
    /// conflicts (they legitimately share the file).
    pub target_episode_ids: Option<&'a [String]>,
}

impl DbManager {
    /// Register a multipart assignment in ONE transaction:
    ///
    ///  1. ensure the parent episode row exists;
    ///  2. detect collisions ([`is_assignment_conflict`]) and disown every episode
    ///     that already holds the incoming file as a main file (cross-kind /
    ///     multipart displacement);
    ///  3. associate the incoming part (applying the organization collision strategy
    ///     to whatever the part slot already held, and normalizing any implicit
    ///     primary — see [`Self::write_slot_tx`]).
    ///
    /// Returns the displaced episode ids (for monitor re-evaluation after commit).
    pub(crate) async fn register_multipart_assignment(
        &self,
        a: MultipartAssignment<'_>,
    ) -> Result<Vec<String>> {
        let strategy = self.collision_strategy().await;
        let (displaced_episodes, superseded) = with_transaction!(self.pool, |mut tx| {
            Self::ensure_episode_row_tx(
                &mut tx,
                a.episode_id,
                a.series_id,
                a.season,
                a.episode,
                a.numbering_mode,
            )
            .await?;

            let conflict = is_assignment_conflict(
                &mut tx,
                a.file_path,
                a.numbering_mode,
                a.target_episode_ids,
                a.episode_id,
                Some(a.part_number as i32),
            )
            .await?;
            if !conflict.displaced.is_empty() {
                disown_episodes_tx(&mut tx, &conflict.displaced).await?;
            }
            if conflict.part_slot_taken {
                tracing::debug!(
                    episode_id = a.episode_id,
                    part_number = a.part_number,
                    "replacing an occupied multipart slot"
                );
            }

            // A language sibling is reconciled (kept alongside, or the plain name
            // promoted into the part slot); a duplicate language slot follows the
            // collision strategy; otherwise the part is registered, applying the
            // collision strategy to the part slot and normalizing any implicit
            // primary.
            let SlotIngest { outcome, displaced } = Self::reconcile_slot_tx(
                &mut tx,
                a.episode_id,
                a.file_path,
                Some(a.part_number),
                a.size,
                SlotPolicy::PreferCandidate,
                strategy,
            )
            .await?;
            let mut superseded: Vec<String> = displaced.into_iter().collect();
            if outcome.caller_writes_slot() {
                let write = Self::write_slot_tx(
                    &mut tx,
                    strategy,
                    a.episode_id,
                    a.file_path,
                    Some(a.part_number),
                    a.size,
                )
                .await?;
                superseded.extend(write.displaced);
            }

            Ok::<_, anyhow::Error>((conflict.displaced, superseded))
        })?;

        for path in &superseded {
            self.delete_superseded(path).await;
        }
        Ok(displaced_episodes)
    }
}

impl DbManager {
    /// Ensure a `file_paths` row exists for `file_path` on an open connection,
    /// creating a placeholder (identity-fallback fingerprint, `pending` state) when
    /// absent. Returns its id. The SSoT entry point for "this path is tracked".
    pub(crate) async fn ensure_path_row_tx(
        conn: &mut sqlx::SqliteConnection,
        file_path: &str,
    ) -> Result<i64> {
        let key = super::fingerprints::content_key("", file_path);
        sqlx::query(
            "INSERT INTO file_paths (file_path, fingerprint, state) VALUES (?, ?, 'pending') \
             ON CONFLICT(file_path) DO NOTHING",
        )
        .bind(file_path)
        .bind(key.as_ref())
        .execute(&mut *conn)
        .await?;
        let id: i64 = sqlx::query_scalar("SELECT id FROM file_paths WHERE file_path = ?")
            .bind(file_path)
            .fetch_one(&mut *conn)
            .await?;
        Ok(id)
    }

    /// Associate `episode_id` with `file_path` as a **main** file on an open
    /// connection. A non-part association (`part_number` NULL) replaces any existing
    /// non-part main file; a part association replaces any file already in that slot.
    pub(crate) async fn associate_main_file_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        file_path: &str,
        part_number: Option<i32>,
    ) -> Result<()> {
        let file_path_id = Self::ensure_path_row_tx(conn, file_path).await?;

        match part_number {
            None => {
                sqlx::query(
                    "DELETE FROM episode_files WHERE episode_id = ? AND kind = 'main' \
                     AND part_number IS NULL",
                )
                .bind(episode_id)
                .execute(&mut *conn)
                .await?;
            }
            Some(n) => {
                sqlx::query(
                    "DELETE FROM episode_files WHERE episode_id = ? AND kind = 'main' \
                     AND part_number = ? AND file_path_id <> ?",
                )
                .bind(episode_id)
                .bind(n)
                .bind(file_path_id)
                .execute(&mut *conn)
                .await?;
            }
        }

        sqlx::query(
            "INSERT INTO episode_files (episode_id, file_path_id, kind, part_number) \
             VALUES (?, ?, 'main', ?) \
             ON CONFLICT(episode_id, file_path_id) DO UPDATE SET \
                 kind = 'main', part_number = excluded.part_number",
        )
        .bind(episode_id)
        .bind(file_path_id)
        .bind(part_number)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// Associate `episode_id` with `file_path` as a **linked** video attachment on an
    /// open connection (a non-primary file kept alongside the episode's main file —
    /// e.g. a language/version variant, or a pending download). A path that is
    /// already the episode's main file stays `main`.
    pub(crate) async fn associate_linked_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        file_path: &str,
    ) -> Result<()> {
        let file_path_id = Self::ensure_path_row_tx(conn, file_path).await?;
        sqlx::query(
            "INSERT INTO episode_files (episode_id, file_path_id, kind, part_number) \
             VALUES (?, ?, 'linked', NULL) \
             ON CONFLICT(episode_id, file_path_id) DO UPDATE SET \
                 kind = CASE WHEN episode_files.kind = 'main' THEN 'main' ELSE 'linked' END",
        )
        .bind(episode_id)
        .bind(file_path_id)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// If the episode already holds a **main** file that is a language/version
    /// variant of `candidate_path`, return that occupant's path. Used to attach the
    /// variant alongside the occupant (as `linked`) instead of colliding with it.
    ///
    /// The comparison is uniform across artifact kinds: the variant base encodes the
    /// whole **slot identity**, so a variant can only match a file in the *same* slot.
    /// The slot is the episode for a single file, the `(episode, part)` for a
    /// multipart file, and the episode **range** for a multi-episode file. Two files
    /// therefore coexist only when they share that slot and differ by language tag;
    /// a different episode/range or a different kind (single vs multipart vs
    /// multi-episode) is a different artifact and is replaced, never kept alongside.
    pub(crate) async fn variant_occupant(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        candidate_path: &str,
    ) -> Result<Option<String>> {
        let paths: Vec<String> = sqlx::query_scalar(
            "SELECT fp.file_path FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind = 'main' AND fp.file_path <> ?",
        )
        .bind(episode_id)
        .bind(candidate_path)
        .fetch_all(&mut *conn)
        .await?;
        Ok(paths
            .into_iter()
            .find(|path| jumbie_shared::parsing::is_variant_of(path, candidate_path)))
    }

    /// Whether the episode already holds a main file that is a language variant of
    /// `candidate_path` (see [`DbManager::variant_occupant`]).
    pub(crate) async fn has_variant_occupant(
        &self,
        episode_id: &str,
        candidate_path: &str,
    ) -> Result<bool> {
        Ok(self
            .variant_occupant_path(episode_id, candidate_path)
            .await?
            .is_some())
    }

    /// [`Self::variant_occupant`] on the pool.
    pub async fn variant_occupant_path(
        &self,
        episode_id: &str,
        candidate_path: &str,
    ) -> Result<Option<String>> {
        Self::variant_occupant(&mut *self.pool.acquire().await?, episode_id, candidate_path).await
    }

    /// [`Self::associate_linked_tx`] on the pool.
    pub async fn associate_linked_file(&self, episode_id: &str, file_path: &str) -> Result<()> {
        Self::associate_linked_tx(&mut *self.pool.acquire().await?, episode_id, file_path).await
    }

    /// The organization `collision_handling` strategy, so a slot replacement applies
    /// the same strategy a colliding move does (`move_file_to_target`).
    pub(crate) async fn collision_strategy(&self) -> jumbie_shared::config::CollisionStrategy {
        jumbie_shared::config::CollisionStrategy::from_config(
            &self.get_organization_config().await.unwrap_or_default(),
        )
    }

    /// Delete a file superseded under the `overwrite` collision strategy (and its
    /// content fingerprint). Best-effort: a failure is logged, never fatal.
    pub(crate) async fn delete_superseded(&self, path: &str) {
        match crate::platform::remove_file_tolerating_locks(std::path::Path::new(path)).await {
            Ok(()) => {
                let _ = self.delete_fingerprint(path).await;
            }
            Err(e) => tracing::warn!(
                "Failed to delete file replaced by collision_handling=overwrite '{}': {}",
                path,
                e
            ),
        }
    }

    /// Ingest a video file into the episode's slot in a language-variant aware way
    /// (see [`Self::reconcile_slot_tx`]).
    pub(crate) async fn reconcile_ingested_slot(
        &self,
        episode_id: &str,
        file_path: &str,
        part_number: Option<u32>,
        size: Option<i64>,
    ) -> Result<SlotIngest> {
        let strategy = self.collision_strategy().await;
        let ingest = with_transaction!(self.pool, |mut tx| {
            Self::reconcile_slot_tx(
                &mut tx,
                episode_id,
                file_path,
                part_number,
                size,
                SlotPolicy::KeepExisting,
                strategy,
            )
            .await
        })?;
        if let Some(displaced) = &ingest.displaced {
            self.delete_superseded(displaced).await;
        }
        Ok(ingest)
    }

    /// Resolve an incoming video file against the episode's existing video
    /// associations on an open connection.
    ///
    /// * A file whose **language tag** the episode already holds (the same artifact
    ///   base *and* tag) is a **duplicate**. Ingestion ([`SlotPolicy::KeepExisting`])
    ///   and the `skip` collision strategy leave the existing file in the slot; a
    ///   manual replacement ([`SlotPolicy::PreferCandidate`]) takes the slot. A stale
    ///   same-slot association whose file is gone is re-pointed either way, and the
    ///   `overwrite` strategy reports the replaced file via
    ///   [`SlotIngest::displaced`]. The plain (untagged) slot keeps its usual
    ///   replace/preserve semantics.
    /// * Otherwise a language sibling of the same artifact is reconciled: the tagged
    ///   file is kept alongside the **plain** (non-tagged) name, which owns the slot,
    ///   regardless of arrival order. The `skip` strategy declines to promote.
    /// * Anything else is [`IngestOutcome::Unrelated`]: the caller writes the slot
    ///   itself via the collision-aware [`Self::write_slot_tx`].
    pub(crate) async fn reconcile_slot_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        file_path: &str,
        part_number: Option<u32>,
        size: Option<i64>,
        policy: SlotPolicy,
        strategy: jumbie_shared::config::CollisionStrategy,
    ) -> Result<SlotIngest> {
        use jumbie_shared::config::CollisionStrategy;

        // One file per language tag: a tagged file whose tag the episode already holds
        // is a duplicate, not a coexisting variant.
        if jumbie_shared::parsing::has_language_tag(file_path)
            && let Some(occ) =
                Self::same_slot_occupant_tx(&mut *conn, episode_id, file_path).await?
        {
            let live = std::path::Path::new(&occ.file_path).exists();
            if live && (policy == SlotPolicy::KeepExisting || strategy == CollisionStrategy::Skip) {
                // Ingestion never clobbers a good file, and `skip` yields to the
                // occupant: the candidate is not attached.
                return Ok(SlotIngest {
                    outcome: IngestOutcome::Duplicate,
                    displaced: None,
                });
            }
            // The candidate takes the slot: write it in the slot's shape (linked, or
            // the part number it occupied) and drop the association it replaces.
            match occ.kind.as_str() {
                "linked" => {
                    Self::associate_linked_tx(&mut *conn, episode_id, file_path).await?;
                }
                _ => {
                    let slot = occ.part_number.map(|n| n as u32).or(part_number);
                    Self::write_slot_raw_tx(&mut *conn, episode_id, file_path, slot, size).await?;
                }
            }
            Self::unassociate_file_tx(&mut *conn, episode_id, &occ.file_path).await?;
            return Ok(SlotIngest {
                outcome: IngestOutcome::Handled,
                // `overwrite` deletes the file it superseded; `rename` keeps both.
                displaced: (live && strategy == CollisionStrategy::Overwrite)
                    .then(|| occ.file_path.clone()),
            });
        }

        let Some(occupant) = Self::variant_occupant(&mut *conn, episode_id, file_path).await?
        else {
            return Ok(SlotIngest {
                outcome: IngestOutcome::Unrelated,
                displaced: None,
            });
        };
        if !std::path::Path::new(&occupant).exists() {
            // A stale sibling must not keep the artifact: the candidate takes the slot.
            return Ok(SlotIngest {
                outcome: IngestOutcome::Unrelated,
                displaced: None,
            });
        }
        if jumbie_shared::parsing::has_language_tag(file_path) {
            Self::associate_linked_tx(&mut *conn, episode_id, file_path).await?;
        } else {
            // The plain name takes the slot; the tagged sibling is kept as a variant.
            // `skip` declines the promotion, keeping the tagged file as the main file.
            if strategy == CollisionStrategy::Skip {
                return Ok(SlotIngest {
                    outcome: IngestOutcome::Duplicate,
                    displaced: None,
                });
            }
            Self::write_slot_raw_tx(&mut *conn, episode_id, file_path, part_number, size).await?;
            Self::associate_linked_tx(&mut *conn, episode_id, &occupant).await?;
        }
        Ok(SlotIngest {
            outcome: IngestOutcome::Handled,
            displaced: None,
        })
    }

    /// Whether the episode already holds a language tag matching `candidate_path`'s
    /// (`has_same_slot_occupant` on the pool): one file per language tag.
    pub(crate) async fn has_same_slot_occupant(
        &self,
        episode_id: &str,
        candidate_path: &str,
    ) -> Result<bool> {
        Ok(Self::same_slot_occupant_tx(
            &mut *self.pool.acquire().await?,
            episode_id,
            candidate_path,
        )
        .await?
        .is_some())
    }

    /// Whether the episode holds `file_path` in any association (main, linked, or
    /// auxiliary). Confirms an assignment actually took effect — e.g. it was not
    /// declined by the organization collision strategy.
    pub(crate) async fn episode_holds_any_path(
        &self,
        episode_id: &str,
        file_path: &str,
    ) -> Result<bool> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND fp.file_path = ?",
        )
        .bind(episode_id)
        .bind(file_path)
        .fetch_optional(&self.pool)
        .await?;
        Ok(found.is_some())
    }

    /// The episode's existing video association (main or linked) that occupies
    /// `candidate_path`'s language slot at a different path, if any.
    async fn same_slot_occupant_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        candidate_path: &str,
    ) -> Result<Option<SlotOccupant>> {
        let rows: Vec<(String, String, Option<i32>)> = sqlx::query_as(
            "SELECT fp.file_path, ef.kind, ef.part_number FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind IN ('main','linked') AND fp.file_path <> ?",
        )
        .bind(episode_id)
        .bind(candidate_path)
        .fetch_all(&mut *conn)
        .await?;
        Ok(rows.into_iter().find_map(|(file_path, kind, part_number)| {
            jumbie_shared::parsing::same_language_slot(&file_path, candidate_path).then_some(
                SlotOccupant {
                    file_path,
                    kind,
                    part_number,
                },
            )
        }))
    }

    /// Drop the episode's association for `file_path` (any kind), leaving the file on
    /// disk. Used when a stale same-slot association is re-pointed to a new path.
    async fn unassociate_file_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        file_path: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM episode_files WHERE episode_id = ? AND file_path_id = \
             (SELECT id FROM file_paths WHERE file_path = ?)",
        )
        .bind(episode_id)
        .bind(file_path)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// Write `file_path` into the episode's slot without collision handling: the given
    /// part slot when `part_number` is set, else the non-part main file.
    async fn write_slot_raw_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        file_path: &str,
        part_number: Option<u32>,
        size: Option<i64>,
    ) -> Result<()> {
        match part_number {
            Some(n) => Self::upsert_episode_part_tx(conn, episode_id, n, file_path, size).await,
            None => Self::associate_main_file_tx(conn, episode_id, file_path, None).await,
        }
    }

    /// Write `file_path` into the episode's slot, applying the organization collision
    /// strategy to the file currently in that slot — the same strategy
    /// `move_file_to_target` applies to a colliding move:
    ///
    /// * `rename` — keep both: the displaced file stays on disk;
    /// * `overwrite` — write, and report the displaced file in
    ///   [`SlotWrite::displaced`] for the caller to delete after committing;
    /// * `skip` — with a different live file in the slot, do not write.
    pub(crate) async fn write_slot_tx(
        conn: &mut sqlx::SqliteConnection,
        strategy: jumbie_shared::config::CollisionStrategy,
        episode_id: &str,
        file_path: &str,
        part_number: Option<u32>,
        size: Option<i64>,
    ) -> Result<SlotWrite> {
        use jumbie_shared::config::CollisionStrategy;

        let occupant = Self::slot_occupant_tx(&mut *conn, episode_id, part_number)
            .await?
            .filter(|p| p != file_path && std::path::Path::new(p).exists());
        if matches!((strategy, &occupant), (CollisionStrategy::Skip, Some(_))) {
            return Ok(SlotWrite {
                wrote: false,
                displaced: None,
            });
        }

        let displaced = match (strategy, &occupant) {
            (CollisionStrategy::Overwrite, Some(displaced)) => Some(displaced.clone()),
            _ => None,
        };
        Self::write_slot_raw_tx(conn, episode_id, file_path, part_number, size).await?;
        if let Some(n) = part_number {
            Self::normalize_primary_after_part_write_tx(&mut *conn, episode_id, n).await?;
        }
        Ok(SlotWrite {
            wrote: true,
            displaced,
        })
    }

    /// Collision-aware slot write on the pool: [`Self::write_slot_tx`] plus deleting a
    /// superseded file (`overwrite`) after the write commits. Returns whether the slot
    /// was written (`false` when the strategy skipped it).
    pub(crate) async fn write_slot_displacing(
        &self,
        episode_id: &str,
        file_path: &str,
        part_number: Option<u32>,
        size: Option<i64>,
    ) -> Result<bool> {
        let strategy = self.collision_strategy().await;
        let write = with_transaction!(self.pool, |mut tx| {
            Self::write_slot_tx(&mut tx, strategy, episode_id, file_path, part_number, size).await
        })?;
        if let Some(displaced) = &write.displaced {
            self.delete_superseded(displaced).await;
        }
        Ok(write.wrote)
    }

    /// The path of the episode's file currently occupying `part_number`'s slot, if
    /// any — the file a same-slot write displaces.
    ///
    /// * non-part slot (`None`): the episode's non-part main;
    /// * part slot (`Some(n)`): the explicit part association, or — because an
    ///   episode's **first** part is stored as the non-part primary until a second
    ///   part arrives — the primary whose filename implies part `n`.
    async fn slot_occupant_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        part_number: Option<u32>,
    ) -> Result<Option<String>> {
        let primary = Self::primary_main_path_tx(&mut *conn, episode_id).await?;
        let Some(n) = part_number else {
            return Ok(primary);
        };

        let explicit: Option<String> = sqlx::query_scalar(
            "SELECT fp.file_path FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number = ?",
        )
        .bind(episode_id)
        .bind(n as i32)
        .fetch_optional(&mut *conn)
        .await?;
        if explicit.is_some() {
            return Ok(explicit);
        }

        Ok(primary.filter(|path| Self::implied_part(path) == Some(n)))
    }

    /// Reconcile the episode's non-part primary with the part just written to `part`.
    ///
    /// The episode's **first** part is stored as its non-part primary until a second
    /// part arrives. A primary naming the written part is now redundant — the written
    /// file owns that slot — so its association is dropped; a primary naming a
    /// *different* part is promoted to that explicit part slot. This keeps manual,
    /// scanner, and download ingestion representing multipart episodes identically.
    async fn normalize_primary_after_part_write_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
        part: u32,
    ) -> Result<()> {
        let Some(primary) = Self::primary_main_path_tx(&mut *conn, episode_id).await? else {
            return Ok(());
        };
        match Self::implied_part(&primary) {
            Some(n) if n == part => Self::unassociate_main_tx(&mut *conn, episode_id).await,
            Some(n) => {
                let size = std::fs::metadata(&primary).ok().map(|m| m.len() as i64);
                Self::upsert_episode_part_tx(&mut *conn, episode_id, n, &primary, size).await
            }
            None => Ok(()),
        }
    }

    /// The episode's non-part main path (its implicit primary), if any.
    async fn primary_main_path_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT fp.file_path FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number IS NULL",
        )
        .bind(episode_id)
        .fetch_optional(&mut *conn)
        .await?)
    }

    /// The part number `file_path`'s filename implies, if any.
    fn implied_part(file_path: &str) -> Option<u32> {
        use crate::utils::path_utils::PathExt;
        crate::utils::detect_part_number(&std::path::Path::new(file_path).file_name_string())
    }

    /// Drop the episode's non-part main association (its implicit primary), leaving
    /// the file on disk.
    async fn unassociate_main_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM episode_files WHERE episode_id = ? AND kind = 'main' \
             AND part_number IS NULL",
        )
        .bind(episode_id)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// [`Self::ensure_path_row_tx`] on the pool.
    pub(crate) async fn ensure_path_row(&self, file_path: &str) -> Result<i64> {
        Self::ensure_path_row_tx(&mut *self.pool.acquire().await?, file_path).await
    }

    /// [`Self::associate_main_file_tx`] on the pool.
    pub async fn associate_main_file(
        &self,
        episode_id: &str,
        file_path: &str,
        part_number: Option<i32>,
    ) -> Result<()> {
        Self::associate_main_file_tx(
            &mut *self.pool.acquire().await?,
            episode_id,
            file_path,
            part_number,
        )
        .await
    }

    /// Associate `episode_id` with `file_path` as an episode-scoped **auxiliary**
    /// file. Enforces "at most one nfo per episode" (a new nfo replaces the old);
    /// subtitles are unlimited. The file kind is derived from the path extension.
    pub async fn associate_auxiliary_file(&self, episode_id: &str, file_path: &str) -> Result<()> {
        let file_path_id = self.ensure_path_row(file_path).await?;

        if is_nfo_path(file_path) {
            sqlx::query(
                "DELETE FROM episode_files WHERE episode_id = ? AND kind = 'auxiliary' \
                 AND file_path_id IN (SELECT id FROM file_paths WHERE lower(file_path) LIKE '%.nfo')",
            )
            .bind(episode_id)
            .execute(&self.pool)
            .await?;
        }

        sqlx::query(
            "INSERT INTO episode_files (episode_id, file_path_id, kind, part_number) \
             VALUES (?, ?, 'auxiliary', NULL) \
             ON CONFLICT(episode_id, file_path_id) DO UPDATE SET kind = 'auxiliary'",
        )
        .bind(episode_id)
        .bind(file_path_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Whether a path is an NFO sidecar (mirrors `file_kind_for_path` for the one
/// uniqueness rule the association table enforces).
fn is_nfo_path(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".nfo")
}

impl DbManager {
    /// Delete `file_paths` rows referenced only by `episode_ids` (via `episode_files`),
    /// leaving paths still referenced by another episode intact. Run before deleting
    /// the episodes so the association rows are still present to inspect.
    pub(crate) async fn delete_episode_owned_paths_tx(
        conn: &mut sqlx::SqliteConnection,
        episode_ids: &[String],
    ) -> Result<()> {
        for chunk in episode_ids.chunks(450) {
            let ph = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "DELETE FROM file_paths WHERE id IN (\
                     SELECT ef.file_path_id FROM episode_files ef WHERE ef.episode_id IN ({ph})) \
                 AND NOT EXISTS (\
                     SELECT 1 FROM episode_files ef2 \
                     WHERE ef2.file_path_id = file_paths.id AND ef2.episode_id NOT IN ({ph}))"
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            for id in chunk {
                q = q.bind(id);
            }
            q.execute(&mut *conn).await?;
        }
        Ok(())
    }

    /// Disown `episode_ids`: apply the canonical disowned state and drop their
    /// associations. The single entry point for "this episode lost its file".
    pub async fn clear_episode_ownership(&self, episode_ids: &[String]) -> Result<()> {
        if episode_ids.is_empty() {
            return Ok(());
        }
        with_transaction!(self.pool, |mut tx| {
            disown_episodes_tx(&mut tx, episode_ids).await?;
            Ok::<_, anyhow::Error>(())
        })
    }

    /// Remove every DB reference to `file_path`: drop the path's associations and
    /// fully disown any episode left with no main file. Returns the episode ids that
    /// were associated with the path (the caller needs them for monitor
    /// re-evaluation). Reassignment (which must preserve the other numbering mode's
    /// representation) uses a mode-scoped disown inside `assign_file_to_episode`
    /// rather than this method.
    pub async fn disown_path(&self, file_path: &str) -> Result<Vec<String>> {
        with_transaction!(self.pool, |mut tx| {
            let holders: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT ef.episode_id FROM episode_files ef \
                 JOIN file_paths fp ON fp.id = ef.file_path_id \
                 WHERE fp.file_path = ?",
            )
            .bind(file_path)
            .fetch_all(&mut *tx)
            .await?;

            sqlx::query(
                "DELETE FROM episode_files WHERE file_path_id IN \
                 (SELECT id FROM file_paths WHERE file_path = ?)",
            )
            .bind(file_path)
            .execute(&mut *tx)
            .await?;

            // Episodes left with no main file (they had only this path, or only
            // parts of which this path is the last) are fully disowned.
            if !holders.is_empty() {
                let ph = crate::db::sql_in_placeholders(holders.len());
                let sql = format!(
                    "SELECT e.episode_id FROM episodes e WHERE e.episode_id IN ({ph}) \
                     AND NOT EXISTS (SELECT 1 FROM episode_files ef \
                                     WHERE ef.episode_id = e.episode_id AND ef.kind = 'main')"
                );
                let mut q = sqlx::query_scalar::<_, String>(&sql);
                for id in &holders {
                    q = q.bind(id);
                }
                let emptied = q.fetch_all(&mut *tx).await?;
                disown_episodes_tx(&mut tx, &emptied).await?;
            }

            Ok::<_, anyhow::Error>(holders)
        })
    }

    /// Every file attached to the episodes that the given `main_paths` leave
    /// **without a main file** — the requested paths themselves plus their linked
    /// variants and auxiliary sidecars. Deleting an episode's last video must take
    /// its sidecars with it, or subtitles/nfo are orphaned on disk; an episode that
    /// keeps another main file (e.g. a sibling multipart part) contributes nothing,
    /// so deleting one part never tears down the rest.
    pub async fn attached_files_of_emptied_episodes(
        &self,
        main_paths: &[String],
    ) -> Result<Vec<String>> {
        if main_paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for chunk in main_paths.chunks(500) {
            let ph = crate::db::sql_in_placeholders(chunk.len());
            let sql = format!(
                "SELECT DISTINCT attached_path.file_path \
                 FROM episode_files owner_ef \
                 JOIN file_paths owner_path ON owner_path.id = owner_ef.file_path_id \
                 JOIN episode_files attached_ef ON attached_ef.episode_id = owner_ef.episode_id \
                 JOIN file_paths attached_path ON attached_path.id = attached_ef.file_path_id \
                 WHERE owner_ef.kind = 'main' AND owner_path.file_path IN ({ph}) \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM episode_files keep_ef \
                       JOIN file_paths keep_path ON keep_path.id = keep_ef.file_path_id \
                       WHERE keep_ef.episode_id = owner_ef.episode_id \
                         AND keep_ef.kind = 'main' AND keep_path.file_path NOT IN ({ph}) \
                   )"
            );
            let mut q = sqlx::query_scalar::<_, String>(&sql);
            for path in chunk {
                q = q.bind(path);
            }
            for path in chunk {
                q = q.bind(path);
            }
            out.extend(q.fetch_all(&self.pool).await?);
        }
        Ok(out)
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

    /// Run the predicates on a pooled connection, as production callers do inside a
    /// transaction.
    async fn conflict(
        db: &DbManager,
        file_path: &str,
        numbering_mode: i32,
        targets: Option<&[String]>,
        episode_id: &str,
        part_number: Option<i32>,
    ) -> AssignmentConflict {
        let mut conn = db.get_pool().acquire().await.unwrap();
        is_assignment_conflict(
            &mut conn,
            file_path,
            numbering_mode,
            targets,
            episode_id,
            part_number,
        )
        .await
        .unwrap()
    }

    /// The displacement check is kind-agnostic: a path held as a single file or as a
    /// multipart part is an equally real conflict for another episode.
    #[tokio::test]
    async fn conflict_is_kind_agnostic_across_shapes() {
        let (db, _tmp) = test_db().await;
        db.ensure_episode_row("holder", "s", 1, 1, 0).await.unwrap();

        db.associate_main_file("holder", "/p.mkv", None)
            .await
            .unwrap();
        let c = conflict(&db, "/p.mkv", 0, None, "target", None).await;
        assert_eq!(c.displaced, vec!["holder".to_string()]);
        assert!(!c.part_slot_taken);

        db.clear_episode_ownership(&["holder".to_string()])
            .await
            .unwrap();
        db.upsert_episode_part("holder", 1, "/p.mkv", None)
            .await
            .unwrap();
        let c = conflict(&db, "/p.mkv", 0, None, "target", None).await;
        assert_eq!(c.displaced, vec!["holder".to_string()]);
    }

    /// Holders inside the target set, and holders in another numbering mode, are not
    /// conflicts.
    #[tokio::test]
    async fn target_and_other_mode_holders_are_not_conflicts() {
        let (db, _tmp) = test_db().await;
        db.ensure_episode_row("t1", "s", 1, 1, 0).await.unwrap();
        db.ensure_episode_row("t2", "s", 1, 2, 0).await.unwrap();
        db.associate_main_file("t1", "/shared.mkv", None)
            .await
            .unwrap();

        let targets = vec!["t1".to_string(), "t2".to_string()];
        let c = conflict(&db, "/shared.mkv", 0, Some(targets.as_slice()), "t2", None).await;
        assert!(c.displaced.is_empty(), "a target holder is not a conflict");

        db.ensure_episode_row("abs", "s", 1, 1, 1).await.unwrap();
        db.associate_main_file("abs", "/other.mkv", None)
            .await
            .unwrap();
        let c = conflict(&db, "/other.mkv", 0, None, "t2", None).await;
        assert!(c.displaced.is_empty(), "the check is mode-scoped");
    }

    /// A multipart slot is a conflict only when a *different* file occupies it;
    /// a free slot or a re-assign of the same file is clear.
    #[tokio::test]
    async fn part_slot_collision_is_reported_only_for_a_different_file() {
        let (db, _tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        db.upsert_episode_part("ep", 1, "/a.mkv", None)
            .await
            .unwrap();

        let taken = conflict(&db, "/b.mkv", 0, None, "ep", Some(1)).await;
        assert!(taken.part_slot_taken, "slot 1 holds a different file");
        assert!(taken.displaced.is_empty());

        let free = conflict(&db, "/b.mkv", 0, None, "ep", Some(2)).await;
        assert!(!free.part_slot_taken && free.displaced.is_empty());

        let same = conflict(&db, "/a.mkv", 0, None, "ep", Some(1)).await;
        assert!(
            !same.part_slot_taken,
            "re-assigning the same file into its own slot is idempotent"
        );
    }

    async fn linked_paths(db: &DbManager, episode_id: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT fp.file_path FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind = 'linked' ORDER BY fp.file_path",
        )
        .bind(episode_id)
        .fetch_all(db.get_pool())
        .await
        .unwrap()
    }

    /// The path of the episode's main file in explicit `part` slot, if any.
    async fn part_path(db: &DbManager, episode_id: &str, part: i32) -> Option<String> {
        sqlx::query_scalar(
            "SELECT fp.file_path FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = ? AND ef.kind = 'main' AND ef.part_number = ?",
        )
        .bind(episode_id)
        .bind(part)
        .fetch_optional(db.get_pool())
        .await
        .unwrap()
    }

    /// The language-slot decision from `reconcile_ingested_slot`.
    async fn ingested_outcome(
        db: &DbManager,
        episode_id: &str,
        path: &str,
        part: Option<u32>,
        size: Option<i64>,
    ) -> IngestOutcome {
        db.reconcile_ingested_slot(episode_id, path, part, size)
            .await
            .unwrap()
            .outcome
    }

    /// Create an on-disk file under `tmp/sub/name` and return its path (a live
    /// same-slot file is what makes a duplicate a duplicate).
    fn write_file(tmp: &std::path::Path, sub: &str, name: &str) -> String {
        let p = tmp.join(sub).join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
        p.to_string_lossy().to_string()
    }

    /// Distinct language tags of one artifact coexist as `linked`; a second file with
    /// the same tag is a duplicate and is not attached.
    #[tokio::test]
    async fn reconcile_attaches_new_language_tags_and_blocks_duplicates() {
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let plain = write_file(tmp.path(), "lib", "Show.S01E01.mkv");
        let en = write_file(tmp.path(), "lib", "Show.S01E01.en.mkv");
        let eng = write_file(tmp.path(), "lib", "Show.S01E01.eng.mkv");
        let en_dup = write_file(tmp.path(), "dl", "Show.S01E01.en.mkv");

        // A lone file has no sibling: the caller owns the slot.
        assert_eq!(
            ingested_outcome(&db, "ep", &plain, None, None).await,
            IngestOutcome::Unrelated
        );
        db.associate_main_file("ep", &plain, None).await.unwrap();

        // New tags attach alongside.
        for tag in [&en, &eng] {
            assert_eq!(
                ingested_outcome(&db, "ep", tag, None, None).await,
                IngestOutcome::Handled
            );
        }
        assert_eq!(linked_paths(&db, "ep").await, vec![en.clone(), eng.clone()]);

        // The same tag again is a duplicate: not attached, main untouched.
        assert_eq!(
            ingested_outcome(&db, "ep", &en_dup, None, None).await,
            IngestOutcome::Duplicate
        );
        assert_eq!(linked_paths(&db, "ep").await, vec![en.clone(), eng.clone()]);
        assert_eq!(
            db.get_episode_file_path("ep").await.unwrap().as_deref(),
            Some(plain.as_str())
        );
    }

    /// A lone language-tagged file has no sibling, so the caller makes it the main
    /// file — the episode is a normal assigned episode.
    #[tokio::test]
    async fn reconcile_leaves_a_lone_tagged_file_to_the_caller() {
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let en = write_file(tmp.path(), "lib", "Show.S01E01.en.mkv");
        assert_eq!(
            ingested_outcome(&db, "ep", &en, None, None).await,
            IngestOutcome::Unrelated
        );
        db.associate_main_file("ep", &en, None).await.unwrap();
        assert_eq!(
            db.get_episode_file_path("ep").await.unwrap().as_deref(),
            Some(en.as_str())
        );
        assert!(linked_paths(&db, "ep").await.is_empty());
    }

    /// A part's language slot is scoped to that part: `pt1.en` and `pt2.en` coexist.
    #[tokio::test]
    async fn language_slots_are_scoped_to_the_part() {
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let pt1 = write_file(tmp.path(), "lib", "Show.S01E01.pt1.mkv");
        let pt1_en = write_file(tmp.path(), "lib", "Show.S01E01.pt1.en.mkv");
        let pt2 = write_file(tmp.path(), "lib", "Show.S01E01.pt2.mkv");
        let pt2_en = write_file(tmp.path(), "lib", "Show.S01E01.pt2.en.mkv");
        db.upsert_episode_part("ep", 1, &pt1, None).await.unwrap();
        db.upsert_episode_part("ep", 2, &pt2, None).await.unwrap();

        for (slot, path) in [(1, &pt1_en), (2, &pt2_en)] {
            assert_eq!(
                ingested_outcome(&db, "ep", path, Some(slot), None).await,
                IngestOutcome::Handled
            );
        }
        assert_eq!(
            linked_paths(&db, "ep").await,
            vec![pt1_en.clone(), pt2_en.clone()]
        );
    }

    /// The "same slot" rule is uniform across artifact kinds: a language variant
    /// coexists only within its own slot — the episode (single), `(episode, part)`
    /// (multipart), or the episode range (multi-episode). Any other slot, including a
    /// different kind, is a different artifact and replaces rather than coexisting.
    #[tokio::test]
    async fn language_variants_require_the_same_slot_identity() {
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let single = write_file(tmp.path(), "lib", "Show.S01E01.mkv");
        db.associate_main_file("ep", &single, None).await.unwrap();

        // Single slot: a different language tag coexists.
        let single_en = write_file(tmp.path(), "dl", "Show.S01E01.en.mkv");
        assert_eq!(
            ingested_outcome(&db, "ep", &single_en, None, None).await,
            IngestOutcome::Handled
        );

        // A different episode is a different slot → different artifact (replacement).
        let different_episode = write_file(tmp.path(), "dl", "Show.S01E02.en.mkv");
        assert_eq!(
            ingested_outcome(&db, "ep", &different_episode, None, None).await,
            IngestOutcome::Unrelated
        );

        // A multi-episode file is a different kind → different artifact (replacement).
        let multi = write_file(tmp.path(), "dl", "Show.S01E01E02.mkv");
        assert_eq!(
            ingested_outcome(&db, "ep", &multi, None, None).await,
            IngestOutcome::Unrelated
        );

        // Multipart slot: a variant of a *different* part is a different slot.
        let pt2 = write_file(tmp.path(), "lib", "Show.S01E01.pt2.mkv");
        db.upsert_episode_part("ep", 2, &pt2, None).await.unwrap();
        let pt1_en = write_file(tmp.path(), "dl", "Show.S01E01.pt1.en.mkv");
        assert_eq!(
            ingested_outcome(&db, "ep", &pt1_en, Some(1), None).await,
            IngestOutcome::Unrelated
        );
    }

    /// Resolution is not a language axis: a different resolution of the same tag is not
    /// a sibling, so it is never attached as a variant of the other resolution.
    #[tokio::test]
    async fn resolution_variants_do_not_coexist_in_one_language_slot() {
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let hd = write_file(tmp.path(), "lib", "Show.S01E01.720p.mkv");
        let hd_en = write_file(tmp.path(), "lib", "Show.S01E01.720p.en.mkv");
        let fhd_en = write_file(tmp.path(), "lib", "Show.S01E01.1080p.en.mkv");
        db.associate_main_file("ep", &hd, None).await.unwrap();
        assert_eq!(
            ingested_outcome(&db, "ep", &hd_en, None, None).await,
            IngestOutcome::Handled
        );
        assert_eq!(
            ingested_outcome(&db, "ep", &fhd_en, None, None).await,
            IngestOutcome::Unrelated
        );
        assert_eq!(linked_paths(&db, "ep").await, vec![hd_en.clone()]);
    }

    /// `link_file_episode_as` never attaches a second file into one language slot.
    #[tokio::test]
    async fn linking_never_creates_a_second_file_in_one_language_slot() {
        use jumbie_shared::media_format::FileKind;
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let plain = write_file(tmp.path(), "lib", "Show.S01E01.mkv");
        let en = write_file(tmp.path(), "lib", "Show.S01E01.en.mkv");
        let en_dup = write_file(tmp.path(), "dl", "Show.S01E01.en.mkv");
        db.associate_main_file("ep", &plain, None).await.unwrap();
        db.link_file_episode_as(&en, "ep", FileKind::Video)
            .await
            .unwrap();
        db.link_file_episode_as(&en_dup, "ep", FileKind::Video)
            .await
            .unwrap();
        assert_eq!(linked_paths(&db, "ep").await, vec![en]);
    }

    /// A part's language tag is unique: a second `pt1.en` is a duplicate and does not
    /// disturb the part it accompanies.
    #[tokio::test]
    async fn reconcile_blocks_a_duplicate_part_language_tag() {
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let pt1 = write_file(tmp.path(), "lib", "Show.S01E01.pt1.mkv");
        let pt1_en = write_file(tmp.path(), "lib", "Show.S01E01.pt1.en.mkv");
        let pt1_en_dup = write_file(tmp.path(), "dl", "Show.S01E01.pt1.en.mkv");
        db.upsert_episode_part("ep", 1, &pt1, None).await.unwrap();

        assert_eq!(
            ingested_outcome(&db, "ep", &pt1_en, Some(1), None).await,
            IngestOutcome::Handled
        );
        assert_eq!(
            ingested_outcome(&db, "ep", &pt1_en_dup, Some(1), None).await,
            IngestOutcome::Duplicate
        );
        assert_eq!(linked_paths(&db, "ep").await, vec![pt1_en.clone()]);
        let part1: Option<String> = sqlx::query_scalar(
            "SELECT fp.file_path FROM episode_files ef \
             JOIN file_paths fp ON fp.id = ef.file_path_id \
             WHERE ef.episode_id = 'ep' AND ef.kind = 'main' AND ef.part_number = 1",
        )
        .fetch_optional(db.get_pool())
        .await
        .unwrap();
        assert_eq!(part1.as_deref(), Some(pt1.as_str()));
    }

    /// A lone language-tagged part has no sibling: the caller registers it.
    #[tokio::test]
    async fn reconcile_leaves_a_lone_tagged_part_to_the_caller() {
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let pt1_en = write_file(tmp.path(), "lib", "Show.S01E01.pt1.en.mkv");
        assert_eq!(
            ingested_outcome(&db, "ep", &pt1_en, Some(1), None).await,
            IngestOutcome::Unrelated
        );
    }

    /// A deliberate manual assignment (`PreferCandidate`) replaces the file in the
    /// same language slot instead of leaving it (ingestion's `KeepExisting`).
    #[tokio::test]
    async fn reconcile_prefer_candidate_replaces_the_same_language_slot() {
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let plain = write_file(tmp.path(), "lib", "Show.S01E01.mkv");
        let en = write_file(tmp.path(), "lib", "Show.S01E01.en.mkv");
        let en_dup = write_file(tmp.path(), "dl", "Show.S01E01.en.mkv");
        db.associate_main_file("ep", &plain, None).await.unwrap();
        db.associate_linked_file("ep", &en).await.unwrap();

        let mut conn = db.get_pool().acquire().await.unwrap();
        let ingest = DbManager::reconcile_slot_tx(
            &mut conn,
            "ep",
            &en_dup,
            None,
            None,
            SlotPolicy::PreferCandidate,
            jumbie_shared::config::CollisionStrategy::Rename,
        )
        .await
        .unwrap();
        assert_eq!(ingest.outcome, IngestOutcome::Handled);
        assert_eq!(linked_paths(&db, "ep").await, vec![en_dup.clone()]);
        assert_eq!(
            db.get_episode_file_path("ep").await.unwrap().as_deref(),
            Some(plain.as_str())
        );
    }

    /// A slot replacement (a different release taking the episode's file slot) obeys
    /// the organization collision strategy: `rename` keeps the superseded file on
    /// disk, `overwrite` reports it for deletion, `skip` declines the replacement.
    #[tokio::test]
    async fn slot_replace_obeys_collision_strategy() {
        use jumbie_shared::config::CollisionStrategy;
        for (strategy, wrote, superseded) in [
            (CollisionStrategy::Rename, true, false),
            (CollisionStrategy::Overwrite, true, true),
            (CollisionStrategy::Skip, false, false),
        ] {
            let (db, tmp) = test_db().await;
            db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
            let old = write_file(tmp.path(), "lib", "Show.S01E01.mkv");
            let new = write_file(tmp.path(), "lib", "Show.S01E01.720p.mkv");
            db.associate_main_file("ep", &old, None).await.unwrap();

            let mut conn = db.get_pool().acquire().await.unwrap();
            let write = DbManager::write_slot_tx(&mut conn, strategy, "ep", &new, None, None)
                .await
                .unwrap();
            drop(conn);

            assert_eq!(write.wrote, wrote, "{strategy:?}: wrote");
            assert_eq!(
                write.displaced.is_some(),
                superseded,
                "{strategy:?}: superseded"
            );
            let main = db.get_episode_file_path("ep").await.unwrap();
            assert_eq!(
                main.as_deref() == Some(new.as_str()),
                wrote,
                "{strategy:?}: main"
            );
            if let Some(displaced) = &write.displaced {
                db.delete_superseded(displaced).await;
                assert!(
                    !std::path::Path::new(&old).exists(),
                    "{strategy:?}: overwrite deletes the superseded file"
                );
            }
        }
    }

    /// Writing a second part promotes the episode's implicit primary (its first part,
    /// stored part-less until a second part arrives) into its explicit part slot, so
    /// manual, scanner, and download ingestion represent multipart episodes alike.
    #[tokio::test]
    async fn part_write_promotes_the_implicit_primary() {
        let (db, tmp) = test_db().await;
        db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
        let pt1 = write_file(tmp.path(), "lib", "Show.S01E01-pt1.mkv");
        let pt2 = write_file(tmp.path(), "lib", "Show.S01E01-pt2.mkv");
        db.associate_main_file("ep", &pt1, None).await.unwrap();

        let mut conn = db.get_pool().acquire().await.unwrap();
        DbManager::write_slot_tx(
            &mut conn,
            jumbie_shared::config::CollisionStrategy::Rename,
            "ep",
            &pt2,
            Some(2),
            None,
        )
        .await
        .unwrap();
        drop(conn);

        assert_eq!(part_path(&db, "ep", 1).await.as_deref(), Some(pt1.as_str()));
        assert_eq!(part_path(&db, "ep", 2).await.as_deref(), Some(pt2.as_str()));
    }

    /// Writing the part the implicit primary implies replaces it: the strategy governs
    /// the superseded file and the redundant primary association is dropped.
    #[tokio::test]
    async fn part_write_over_the_implicit_primary_obeys_collision_strategy() {
        use jumbie_shared::config::CollisionStrategy;
        for (strategy, wrote, superseded) in [
            (CollisionStrategy::Rename, true, false),
            (CollisionStrategy::Overwrite, true, true),
            (CollisionStrategy::Skip, false, false),
        ] {
            let (db, tmp) = test_db().await;
            db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
            let pt1 = write_file(tmp.path(), "lib", "Show.S01E01-pt1.mkv");
            let pt1_new = write_file(tmp.path(), "dl", "Show.S01E01-pt1.1080p.mkv");
            db.associate_main_file("ep", &pt1, None).await.unwrap();

            let mut conn = db.get_pool().acquire().await.unwrap();
            let write =
                DbManager::write_slot_tx(&mut conn, strategy, "ep", &pt1_new, Some(1), None)
                    .await
                    .unwrap();
            drop(conn);

            assert_eq!(write.wrote, wrote, "{strategy:?}: wrote");
            assert_eq!(
                write.displaced.as_deref() == Some(pt1.as_str()),
                superseded,
                "{strategy:?}: superseded"
            );
            let part1 = part_path(&db, "ep", 1).await;
            let main = db.get_episode_file_path("ep").await.unwrap();
            if wrote {
                assert_eq!(
                    part1.as_deref(),
                    Some(pt1_new.as_str()),
                    "{strategy:?}: part 1 takes the new file"
                );
                assert_eq!(
                    main.as_deref(),
                    Some(pt1_new.as_str()),
                    "{strategy:?}: the redundant implicit primary is gone"
                );
            } else {
                assert!(
                    part1.is_none(),
                    "{strategy:?}: declined, part 1 not written"
                );
                assert_eq!(
                    main.as_deref(),
                    Some(pt1.as_str()),
                    "{strategy:?}: the primary is untouched"
                );
            }
        }
    }

    /// The same strategy governs a language-variant replacement (a second file with a
    /// tag the episode already holds).
    #[tokio::test]
    async fn variant_replace_obeys_collision_strategy() {
        use jumbie_shared::config::CollisionStrategy;
        for (strategy, replaced, superseded) in [
            (CollisionStrategy::Rename, true, false),
            (CollisionStrategy::Overwrite, true, true),
            (CollisionStrategy::Skip, false, false),
        ] {
            let (db, tmp) = test_db().await;
            db.ensure_episode_row("ep", "s", 1, 1, 0).await.unwrap();
            let plain = write_file(tmp.path(), "lib", "Show.S01E01.mkv");
            let en = write_file(tmp.path(), "lib", "Show.S01E01.en.mkv");
            let en_dup = write_file(tmp.path(), "dl", "Show.S01E01.en.mkv");
            db.associate_main_file("ep", &plain, None).await.unwrap();
            db.associate_linked_file("ep", &en).await.unwrap();

            let mut conn = db.get_pool().acquire().await.unwrap();
            let ingest = DbManager::reconcile_slot_tx(
                &mut conn,
                "ep",
                &en_dup,
                None,
                None,
                SlotPolicy::PreferCandidate,
                strategy,
            )
            .await
            .unwrap();
            drop(conn);

            let expected = if replaced {
                vec![en_dup.clone()]
            } else {
                vec![en.clone()]
            };
            assert_eq!(linked_paths(&db, "ep").await, expected, "{strategy:?}");
            assert_eq!(
                ingest.displaced.is_some(),
                superseded,
                "{strategy:?}: superseded"
            );
        }
    }
}
