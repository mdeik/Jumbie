// Automatic profiles accumulate a per-submitter score from media characteristics:
// each matching rule adds its modifier, and the total (positive or negative) feeds
// search ranking. Scores persist across restarts via the automatic_profile table.
use super::{DbManager, MediaScanRow};
use anyhow::Result;

/// Parameters for [`DbManager::record_submitter_score`].
pub struct SubmitterScoreInput<'a> {
    pub submitter: &'a str,
    pub description: &'a str,
    pub value: i32,
    pub category: &'a str,
    pub bound: i32,
    pub source_identity: Option<&'a str>,
    pub extension: Option<&'a str>,
}

impl DbManager {
    pub async fn get_automatic_profiles(
        &self,
    ) -> Result<Vec<jumbie_shared::types::AutomaticProfile>> {
        let rows = sqlx::query_as::<_, jumbie_shared::types::AutomaticProfile>(
            "SELECT submitter, score, \
             (SELECT COUNT(*) FROM automatic_profile_media_scans WHERE submitter = automatic_profile.submitter) \
             AS media_scan_count \
             FROM automatic_profile ORDER BY score ASC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_automatic_profile(
        &self,
        submitter: &str,
    ) -> Result<Option<jumbie_shared::types::AutomaticProfile>> {
        let profile = sqlx::query_as::<_, jumbie_shared::types::AutomaticProfile>(
            "SELECT submitter, score, \
             (SELECT COUNT(*) FROM automatic_profile_media_scans WHERE submitter = automatic_profile.submitter) \
             AS media_scan_count \
             FROM automatic_profile WHERE submitter = ?",
        )
        .bind(submitter)
        .fetch_optional(&self.pool)
        .await?;
        Ok(profile)
    }

    pub async fn delete_automatic_profile(&self, submitter: &str) -> Result<()> {
        // Records cascade delete from the foreign key
        sqlx::query("DELETE FROM automatic_profile WHERE submitter = ?")
            .bind(submitter)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn record_submitter_score(&self, params: SubmitterScoreInput<'_>) -> Result<()> {
        // Per-category bounding stops one batch of files from dominating a
        // submitter's score: a positive bound caps the category total, a negative
        // bound floors it, and zero means no limit.
        with_transaction!(self.pool, |mut tx| {
            let existing_record: Option<(String, i32)> =
                if let Some(identity) = params.source_identity {
                    sqlx::query_as(
                        "SELECT id, score FROM automatic_profile_records \
                             WHERE submitter = ? AND category = ? AND source_identity = ?",
                    )
                    .bind(params.submitter)
                    .bind(params.category)
                    .bind(identity)
                    .fetch_optional(&mut *tx)
                    .await?
                } else {
                    None
                };

            let current_category_total: (i32,) = if let Some((ref id, _)) = existing_record {
                sqlx::query_as(
                    "SELECT IFNULL(SUM(score), 0) FROM automatic_profile_records \
                     WHERE submitter = ? AND category = ? AND id != ?",
                )
                .bind(params.submitter)
                .bind(params.category)
                .bind(id)
                .fetch_one(&mut *tx)
                .await?
            } else {
                sqlx::query_as(
                    "SELECT IFNULL(SUM(score), 0) FROM automatic_profile_records \
                     WHERE submitter = ? AND category = ?",
                )
                .bind(params.submitter)
                .bind(params.category)
                .fetch_one(&mut *tx)
                .await?
            };

            let other_files_total = current_category_total.0;
            let mut applied_value = params.value;

            // Clamp by bound (if set):
            //   bound > 0 → upper limit (total must not exceed bound)
            //   bound < 0 → lower limit (total must not go below bound)
            //   bound = 0 → no limit
            let total_after = other_files_total + params.value;
            if params.bound > 0 && total_after > params.bound {
                applied_value = (params.bound - other_files_total).max(0);
            } else if params.bound < 0 && total_after < params.bound {
                applied_value = (params.bound - other_files_total).min(0);
            }

            if let Some((id, old_value)) = existing_record {
                if old_value != applied_value {
                    sqlx::query(
                        "UPDATE automatic_profile_records SET score = ?, description = ? WHERE id = ?",
                    )
                    .bind(applied_value)
                    .bind(params.description)
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;

                    let diff = applied_value - old_value;
                    sqlx::query(
                        "INSERT INTO automatic_profile (submitter, score) VALUES (?, ?)
                         ON CONFLICT(submitter) DO UPDATE SET score = score + excluded.score",
                    )
                    .bind(params.submitter)
                    .bind(diff)
                    .execute(&mut *tx)
                    .await?;
                }
            } else {
                sqlx::query(
                    "INSERT INTO automatic_profile (submitter, score) VALUES (?, 0)
                     ON CONFLICT(submitter) DO NOTHING",
                )
                .bind(params.submitter)
                .execute(&mut *tx)
                .await?;

                let record_id = jumbie_shared::config::generate_uuid();
                sqlx::query(
                    "INSERT INTO automatic_profile_records \
                     (id, submitter, category, score, description, source_identity, extension) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(record_id)
                .bind(params.submitter)
                .bind(params.category)
                .bind(applied_value)
                .bind(params.description)
                .bind(params.source_identity)
                .bind(params.extension)
                .execute(&mut *tx)
                .await?;

                if applied_value != 0 {
                    sqlx::query(
                        "INSERT INTO automatic_profile (submitter, score) VALUES (?, ?)
                         ON CONFLICT(submitter) DO UPDATE SET score = score + excluded.score",
                    )
                    .bind(params.submitter)
                    .bind(applied_value)
                    .execute(&mut *tx)
                    .await?;
                }
            }

            Ok(())
        })
    }

    pub async fn clear_submitter_score(
        &self,
        submitter: &str,
        category: &str,
        source_identity: &str,
    ) -> Result<()> {
        // Cleared (not just reversed) when a later file from the same submitter
        // passes the rule check, so the score updates as better releases arrive.
        with_transaction!(self.pool, |mut tx| {
            let existing: Option<(String, i32)> = sqlx::query_as(
                "SELECT id, score FROM automatic_profile_records \
                         WHERE submitter = ? AND category = ? AND source_identity = ?",
            )
            .bind(submitter)
            .bind(category)
            .bind(source_identity)
            .fetch_optional(&mut *tx)
            .await?;

            if let Some((id, old_value)) = existing {
                sqlx::query("DELETE FROM automatic_profile_records WHERE id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;

                sqlx::query("UPDATE automatic_profile SET score = score - ? WHERE submitter = ?")
                    .bind(old_value)
                    .bind(submitter)
                    .execute(&mut *tx)
                    .await?;
            }

            Ok(())
        })
    }

    /// Clear a record by its primary key (id), updating the submitter's
    /// aggregate score accordingly. Used during rescoring when we
    /// already know the record ID but not its source_identity.
    pub async fn clear_record_by_id(&self, record_id: &str) -> Result<()> {
        with_transaction!(self.pool, |mut tx| {
            let existing: Option<(String, i32)> = sqlx::query_as(
                "SELECT submitter, score FROM automatic_profile_records WHERE id = ?",
            )
            .bind(record_id)
            .fetch_optional(&mut *tx)
            .await?;

            if let Some((submitter, value)) = existing {
                sqlx::query("DELETE FROM automatic_profile_records WHERE id = ?")
                    .bind(record_id)
                    .execute(&mut *tx)
                    .await?;

                sqlx::query("UPDATE automatic_profile SET score = score - ? WHERE submitter = ?")
                    .bind(value)
                    .bind(&submitter)
                    .execute(&mut *tx)
                    .await?;
            }

            Ok(())
        })
    }

    pub async fn get_automatic_profile_records(
        &self,
        submitter: &str,
    ) -> Result<Vec<jumbie_shared::types::AutomaticProfileRecord>> {
        let rows = sqlx::query_as::<_, jumbie_shared::types::AutomaticProfileRecord>(
            "SELECT id, submitter, category, score, description, date_added, extension \
             FROM automatic_profile_records WHERE submitter = ? ORDER BY date_added DESC",
        )
        .bind(submitter)
        .fetch_all(&self.pool)
        .await?;
        // `date_added` is a naive-UTC SQLite timestamp; normalize it to RFC 3339
        // so the API never exposes a zone-less timestamp.
        Ok(rows
            .into_iter()
            .map(|mut record| {
                record.date_added = crate::datetime::naive_utc_str_to_rfc3339(&record.date_added);
                record
            })
            .collect())
    }

    /// Load all records for the given UnexpectedFiles category names, across
    /// all submitters. Used by the rescoring logic to reconcile against the
    /// persisted unknown file extensions.
    /// Returns (id, submitter, category, extension) tuples.
    pub async fn get_unexpected_file_records(
        &self,
        category_names: &[String],
    ) -> Result<Vec<(String, String, String, Option<String>)>> {
        let rows = sqlx::query_as::<_, (String, String, String, Option<String>)>(
            "SELECT id, submitter, category, extension \
             FROM automatic_profile_records \
             WHERE category IN (SELECT value FROM json_each(?)) \
             ORDER BY submitter ASC, date_added DESC",
        )
        .bind(serde_json::to_string(category_names).unwrap_or("[]".to_string()))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Persist unknown file extension counts for a submitter, accumulating
    /// across walks via UPSERT so the total count per extension reflects all
    /// files ever seen for this submitter.
    pub async fn record_unknown_file_counts(
        &self,
        submitter: &str,
        counts: &[(String, i32)],
    ) -> Result<()> {
        // Ensure the profile row exists first (required by FK).
        sqlx::query(
            "INSERT INTO automatic_profile (submitter, score) VALUES (?, 0) \
             ON CONFLICT(submitter) DO NOTHING",
        )
        .bind(submitter)
        .execute(&self.pool)
        .await?;

        for (ext, count) in counts {
            sqlx::query(
                "INSERT INTO automatic_profile_unknown_files (submitter, extension, count) \
                 VALUES (?, ?, ?) \
                 ON CONFLICT(submitter, extension) \
                 DO UPDATE SET count = count + excluded.count",
            )
            .bind(submitter)
            .bind(ext)
            .bind(count)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Load all persisted unknown file extensions across all submitters.
    /// Returns (submitter, extension, count) triples.
    pub async fn get_unknown_file_extensions(&self) -> Result<Vec<(String, String, i32)>> {
        let rows = sqlx::query_as::<_, (String, String, i32)>(
            "SELECT submitter, extension, count \
             FROM automatic_profile_unknown_files \
             ORDER BY submitter ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Bulk-delete all records for the given category names and adjust each
    /// submitter's total score accordingly.  Runs in a single transaction.
    pub async fn clear_records_by_categories(&self, category_names: &[String]) -> Result<()> {
        with_transaction!(self.pool, |mut tx| {
            let totals: Vec<(String, i32)> = sqlx::query_as(
                "SELECT submitter, SUM(score) \
                 FROM automatic_profile_records \
                 WHERE category IN (SELECT value FROM json_each(?)) \
                 GROUP BY submitter",
            )
            .bind(serde_json::to_string(category_names).unwrap_or("[]".to_string()))
            .fetch_all(&mut *tx)
            .await?;

            for (submitter, total_value) in &totals {
                sqlx::query(
                    "UPDATE automatic_profile \
                     SET score = score - ? \
                     WHERE submitter = ?",
                )
                .bind(total_value)
                .bind(submitter)
                .execute(&mut *tx)
                .await?;
            }

            sqlx::query(
                "DELETE FROM automatic_profile_records \
                 WHERE category IN (SELECT value FROM json_each(?))",
            )
            .bind(serde_json::to_string(category_names).unwrap_or("[]".to_string()))
            .execute(&mut *tx)
            .await?;

            Ok(())
        })
    }

    pub async fn cleanup_orphaned_automatic_records(
        &self,
        current_categories: &std::collections::HashMap<
            String,
            jumbie_shared::config::AutomaticProfileCategory,
        >,
    ) -> Result<()> {
        // When the user removes a category from the config, records for it become
        // orphaned: this removes both the rows and their score contribution.
        let valid_categories: Vec<String> = current_categories.keys().cloned().collect();

        with_transaction!(self.pool, |mut tx| {
            let orphans: Vec<(String, String, i32)> = sqlx::query_as(
                "SELECT id, submitter, score FROM automatic_profile_records \
                 WHERE category NOT IN (SELECT value FROM json_each(?))",
            )
            .bind(serde_json::to_string(&valid_categories).unwrap_or("[]".to_string()))
            .fetch_all(&mut *tx)
            .await?;

            for (id, submitter, value) in orphans {
                sqlx::query("UPDATE automatic_profile SET score = score - ? WHERE submitter = ?")
                    .bind(value)
                    .bind(submitter)
                    .execute(&mut *tx)
                    .await?;

                sqlx::query("DELETE FROM automatic_profile_records WHERE id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }

            Ok(())
        })
    }

    // Media scans are the SSoT for scoring recalculation: they persist per-file
    // media characteristics so reapply can re-evaluate rules after files are deleted.

    /// Insert a new media scan record for a file with a known submitter.
    /// Ensures the parent `automatic_profile` row exists first so the FK
    /// constraint on `automatic_profile_media_scans` is satisfied.
    pub async fn record_media_scan(
        &self,
        submitter: &str,
        filename: &str,
        series_id: Option<&str>,
        season: Option<i32>,
        episode: Option<i32>,
        media_info_json: &str,
    ) -> Result<()> {
        let id = jumbie_shared::config::generate_uuid();
        with_transaction!(self.pool, |mut tx| {
            // Parent row must exist first (FK).
            sqlx::query(
                "INSERT INTO automatic_profile (submitter, score) VALUES (?, 0)
                 ON CONFLICT(submitter) DO NOTHING",
            )
            .bind(submitter)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "INSERT INTO automatic_profile_media_scans
                 (id, submitter, filename, series_id, season, episode, media_info)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(submitter)
            .bind(filename)
            .bind(series_id)
            .bind(season)
            .bind(episode)
            .bind(media_info_json)
            .execute(&mut *tx)
            .await?;

            Ok(())
        })
    }

    /// Return all media scans, used by reapply_automatic_profiles_to_library.
    /// This is the SSoT for scoring — reads from this table rather than
    /// from episodes + file_paths so that scoring is independent
    /// of file/ episode existence.
    pub async fn get_all_media_scans(&self) -> Result<Vec<MediaScanRow>> {
        let rows = sqlx::query_as::<_, MediaScanRow>(
            "SELECT id, submitter, filename, series_id, season, episode, media_info
             FROM automatic_profile_media_scans
             ORDER BY date_added ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Delete all media scans — used during a full rescore to rebuild
    /// from current files.
    pub async fn clear_all_media_scans(&self) -> Result<()> {
        sqlx::query("DELETE FROM automatic_profile_media_scans")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
