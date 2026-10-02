// Series CRUD: insert/update/delete on series_mappings and series-level data.

use crate::db::DbManager;
use anyhow::Result;
use std::sync::atomic::Ordering;

impl DbManager {
    pub async fn upsert_series_mapping(
        &self,
        id: &str,
        rule: &jumbie_shared::types::MappingRule,
    ) -> Result<()> {
        // Transaction keeps series_mappings and season_overrides in sync — a crash
        // between the two would leave stale overrides contradicting the JSON blob (SSoT).
        with_transaction!(self.pool, |mut tx| {
            // Materialized column: 1 = absolute override, else 0. The "use global"
            // tristate (None) is not representable here by design — readers use the JSON.
            let abs_num = match rule.settings.absolute_numbering {
                Some(true) => 1,
                _ => 0,
            };
            let json_data = serde_json::to_string(rule)?;
            sqlx::query(
                "INSERT INTO series_mappings (id, data, absolute_numbering, target_title) VALUES (?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET data = excluded.data, absolute_numbering = excluded.absolute_numbering, target_title = excluded.target_title",
            )
            .bind(id)
            .bind(&json_data)
            .bind(abs_num)
            .bind(&rule.target_title)
            .execute(&mut *tx)
            .await?;

            // season_overrides
            sqlx::query("DELETE FROM season_overrides WHERE series_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;

            for (season_key, so) in &rule.settings.season {
                // Guard: a non-numeric key must NOT silently materialize as
                // season 1 — this table is joined against `episodes.season` for
                // stats, so a wrong key corrupts counts.
                let Some(season_num) = jumbie_shared::mapping::parse_season_num(season_key) else {
                    tracing::warn!(
                        "Skipping season override with non-numeric key {:?} for series {} ({})",
                        season_key,
                        rule.target_title,
                        id
                    );
                    continue;
                };
                sqlx::query(
                    "INSERT INTO season_overrides \
                     (series_id, season, numbering_mode, episode_start, episode_end, episode_offset, cell_count) \
                     VALUES (?, ?, 0, ?, ?, ?, ?)",
                )
                .bind(id)
                .bind(season_num)
                .bind(so.episode_start)
                .bind(so.episode_end)
                .bind(so.episode_offset.unwrap_or(0))
                .bind(so.cell_count)
                .execute(&mut *tx)
                .await?;
            }

            for (season_key, so) in &rule.settings.season_absolute {
                // Guard: see the normal-mode loop above.
                let Some(season_num) = jumbie_shared::mapping::parse_season_num(season_key) else {
                    tracing::warn!(
                        "Skipping absolute season override with non-numeric key {:?} for series {} ({})",
                        season_key,
                        rule.target_title,
                        id
                    );
                    continue;
                };
                sqlx::query(
                    "INSERT INTO season_overrides \
                     (series_id, season, numbering_mode, episode_start, episode_end, episode_offset, cell_count) \
                     VALUES (?, ?, 1, ?, ?, ?, ?)",
                )
                .bind(id)
                .bind(season_num)
                .bind(so.episode_start)
                .bind(so.episode_end)
                .bind(so.episode_offset.unwrap_or(0))
                .bind(so.cell_count)
                .execute(&mut *tx)
                .await?;
            }

            // Invalidate the cache — any reader that loaded before this
            // transaction committed will see a stale generation on next access.
            self.mappings_cache_gen.fetch_add(1, Ordering::Release);
            Ok(())
        })
    }

    pub async fn delete_series_mapping(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM series_mappings WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        self.mappings_cache_gen.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub async fn set_series_total_size(&self, series_id: &str, total_size: i64) -> Result<()> {
        sqlx::query("UPDATE series_mappings SET total_size = ? WHERE id = ?")
            .bind(total_size)
            .bind(series_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // Series/Season Deletion
    // Deletion must run in a transaction: dependent rows (file_paths,
    // retry_queue) must be deleted BEFORE the episodes row, otherwise the subqueries
    // find no matches and the foreign data is orphaned.
    pub async fn delete_series_data(&self, series_id: &str) -> Result<()> {
        // Clean per-provider metadata cache before deleting the main data (done
        // outside the transaction so a cache-cleanup failure doesn't block deletion).
        if let Ok(all) = self.get_all_series_mappings().await
            && let Some(m) = all.get(series_id)
        {
            for metadata_id in m.settings.metadata_ids.values() {
                let _ = self.delete_metadata_series_cache(metadata_id).await;
                let _ = self.delete_metadata_episodes_cache(metadata_id).await;
            }
        }

        with_transaction!(self.pool, |mut tx| {
            let episode_ids: Vec<String> =
                sqlx::query_scalar("SELECT episode_id FROM episodes WHERE series_id = ?")
                    .bind(series_id)
                    .fetch_all(&mut *tx)
                    .await?;
            Self::delete_episode_owned_paths_tx(&mut tx, &episode_ids).await?;

            Self::tx_execute_bound(&mut tx, "DELETE FROM retry_queue WHERE episode_id IN (SELECT episode_id FROM episodes WHERE series_id = ?)", series_id.to_string()).await?;

            Self::tx_execute_bound(
                &mut tx,
                "DELETE FROM episodes WHERE series_id = ?",
                series_id.to_string(),
            )
            .await?;

            // Reset materialized total_size — all episode files are gone.
            sqlx::query("UPDATE series_mappings SET total_size = 0 WHERE id = ?")
                .bind(series_id)
                .execute(&mut *tx)
                .await?;

            Ok(())
        })
    }

    pub async fn delete_season_data(
        &self,
        series_id: &str,
        season: i32,
        numbering_mode: i32,
    ) -> Result<()> {
        // Intentionally do NOT delete the episode cache here: metadata_episodes_cache
        // is an immutable provider snapshot meant for rehydration via
        // restore_season_metadata, so it must survive season-level deletion. It is
        // cleaned only on full series removal or orphan cleanup.

        with_transaction!(self.pool, |mut tx| {
            let episode_ids: Vec<String> = sqlx::query_scalar(
                "SELECT episode_id FROM episodes \
                 WHERE series_id = ? AND season = ? AND numbering_mode = ?",
            )
            .bind(series_id)
            .bind(season)
            .bind(numbering_mode)
            .fetch_all(&mut *tx)
            .await?;
            Self::delete_episode_owned_paths_tx(&mut tx, &episode_ids).await?;

            sqlx::query(
                "DELETE FROM retry_queue \
                 WHERE episode_id IN (
                     SELECT episode_id FROM episodes \
                     WHERE series_id = ? AND season = ? AND numbering_mode = ?
                 )",
            )
            .bind(series_id)
            .bind(season)
            .bind(numbering_mode)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "DELETE FROM episodes \
                 WHERE series_id = ? AND season = ? AND numbering_mode = ?",
            )
            .bind(series_id)
            .bind(season)
            .bind(numbering_mode)
            .execute(&mut *tx)
            .await?;

            Ok(())
        })
    }

    // Season suppression (durable "removed season"): a deleted season's `episodes`
    // rows are gone, so `suppressed_seasons` is the tombstone the metadata sync
    // consults to avoid re-creating it. Clearing it is a deliberate re-adopt.

    /// Mark a season as suppressed for a numbering mode. Idempotent.
    pub async fn suppress_season(
        &self,
        series_id: &str,
        season: i32,
        numbering_mode: i32,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO suppressed_seasons (series_id, season, numbering_mode) \
             VALUES (?, ?, ?) \
             ON CONFLICT(series_id, season, numbering_mode) DO NOTHING",
        )
        .bind(series_id)
        .bind(season)
        .bind(numbering_mode)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Clear suppression for a single season in one numbering mode.
    pub async fn unsuppress_season(
        &self,
        series_id: &str,
        season: i32,
        numbering_mode: i32,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM suppressed_seasons \
             WHERE series_id = ? AND season = ? AND numbering_mode = ?",
        )
        .bind(series_id)
        .bind(season)
        .bind(numbering_mode)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Clear ALL season suppressions for a series (every numbering mode).
    pub async fn unsuppress_all_seasons(&self, series_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM suppressed_seasons WHERE series_id = ?")
            .bind(series_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Suppressed season numbers for a series in a single numbering mode.
    ///
    /// Absolute numbering (`numbering_mode == 1`) has one canonical season, so
    /// deleting it is a reset rather than a durable removal: nothing is reported
    /// suppressed. This also heals series that acquired a tombstone before that
    /// rule existed.
    pub async fn get_suppressed_seasons(
        &self,
        series_id: &str,
        numbering_mode: i32,
    ) -> Result<Vec<i32>> {
        if numbering_mode == 1 {
            return Ok(Vec::new());
        }
        let rows: Vec<i32> = sqlx::query_scalar(
            "SELECT season FROM suppressed_seasons \
             WHERE series_id = ? AND numbering_mode = ?",
        )
        .bind(series_id)
        .bind(numbering_mode)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}
