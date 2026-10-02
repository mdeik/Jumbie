// Series organization & maintenance: metadata management, episode state updates,
// provider/instance-scoped cache deletion, and orphaned metadata cleanup.

use crate::db::DbManager;
use anyhow::Result;

impl DbManager {
    // Orphaned Metadata Cleanup
    // When a series is deleted its mapping is removed but metadata_season_cache and
    // metadata_fetch_log rows remain, accumulating over time. This collects the
    // metadata IDs still referenced by series_mappings and deletes the rest. The 100-ID
    // chunk avoids SQLite's variable limit for users with 2000+ series.
    pub async fn cleanup_orphaned_metadata(&self) -> Result<usize> {
        let all_mappings = self.get_all_series_mappings().await?;
        let mut active_metadata_ids = std::collections::HashSet::new();
        for rule in all_mappings.values() {
            for metadata_id in rule.settings.metadata_ids.values() {
                active_metadata_ids.insert(metadata_id.clone());
            }
        }

        // One UNION query over all four cache tables finds orphaned metadata_ids.
        let orphaned_ids: std::collections::HashSet<String> = sqlx::query_scalar(
            "SELECT DISTINCT metadata_id FROM (\
             SELECT metadata_id FROM metadata_season_cache \
             UNION \
             SELECT metadata_id FROM metadata_fetch_log \
             UNION \
             SELECT metadata_id FROM metadata_series_cache \
             UNION \
             SELECT metadata_id FROM metadata_episodes_cache\
            )",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .filter(|id| !active_metadata_ids.contains(id))
        .collect();

        if orphaned_ids.is_empty() {
            return Ok(0);
        }

        let orphaned_vec: Vec<String> = orphaned_ids.into_iter().collect();
        let mut deleted_count = 0;

        with_transaction!(self.pool, |mut tx| {
            for chunk in orphaned_vec.chunks(100) {
                let placeholders = crate::db::sql_in_placeholders(chunk.len());

                // metadata_season_cache: grace period of 7 days based on updated_at
                let q1_str = format!(
                    "DELETE FROM metadata_season_cache WHERE metadata_id IN ({}) AND updated_at < datetime('now', '-7 days')",
                    placeholders
                );
                let mut q1 = sqlx::query(&q1_str);
                for id in chunk {
                    q1 = q1.bind(id);
                }
                deleted_count += q1.execute(&mut *tx).await?.rows_affected();

                // metadata_fetch_log: no grace period — lightweight, immediate cleanup
                let q3_str = format!(
                    "DELETE FROM metadata_fetch_log WHERE metadata_id IN ({})",
                    placeholders
                );
                let mut q3 = sqlx::query(&q3_str);
                for id in chunk {
                    q3 = q3.bind(id);
                }
                deleted_count += q3.execute(&mut *tx).await?.rows_affected();

                // metadata_series_cache: grace period of 7 days based on fetched_at
                let q4_str = format!(
                    "DELETE FROM metadata_series_cache WHERE metadata_id IN ({}) AND fetched_at < datetime('now', '-7 days')",
                    placeholders
                );
                let mut q4 = sqlx::query(&q4_str);
                for id in chunk {
                    q4 = q4.bind(id);
                }
                deleted_count += q4.execute(&mut *tx).await?.rows_affected();

                // metadata_episodes_cache: grace period of 7 days based on fetched_at
                let q5_str = format!(
                    "DELETE FROM metadata_episodes_cache WHERE metadata_id IN ({}) AND fetched_at < datetime('now', '-7 days')",
                    placeholders
                );
                let mut q5 = sqlx::query(&q5_str);
                for id in chunk {
                    q5 = q5.bind(id);
                }
                deleted_count += q5.execute(&mut *tx).await?.rows_affected();
            }
            Ok::<(), sqlx::Error>(())
        })?;

        Ok(deleted_count as usize)
    }

    // Metadata season cache stores per-season episode counts from providers.
    // `ordering_mode` distinguishes normal (aired) from absolute (production) order,
    // which can differ. The UPSERT key is
    // (metadata_id, plugin_id, instance_id, ordering_mode, season_number).
    pub async fn upsert_metadata_season_cache(
        &self,
        plugin_series_id: &str,
        plugin_id: &str,
        instance_id: &str,
        ordering_mode: &str,
        counts: &[crate::plugins::metadata::SeasonMetadata],
    ) -> anyhow::Result<()> {
        with_transaction!(self.pool, |mut tx| {
            for count in counts {
                sqlx::query(
                "INSERT INTO metadata_season_cache
                                     (metadata_id, plugin_id, instance_id, ordering_mode, season_number, episode_count, updated_at)
                                 VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
                                 ON CONFLICT(metadata_id, plugin_id, instance_id, ordering_mode, season_number) DO UPDATE SET
                                     episode_count = excluded.episode_count,
                                     updated_at    = excluded.updated_at",
            )
            .bind(plugin_series_id)
            .bind(plugin_id)
            .bind(instance_id)
            .bind(ordering_mode)
            .bind(count.season.to_string())
            .bind(count.episode_count)
            .execute(&mut *tx)
            .await?;
            }
            Ok(())
        })
    }

    // Metadata fetch log: see `has_fetch_been_attempted` in queries.rs.
    pub async fn mark_fetch_attempted(
        &self,
        plugin_series_id: &str,
        plugin_id: &str,
        instance_id: &str,
        ordering_mode: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO metadata_fetch_log (metadata_id, plugin_id, instance_id, ordering_mode)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(metadata_id, plugin_id, instance_id, ordering_mode)
             DO UPDATE SET attempted_at = CURRENT_TIMESTAMP",
        )
        .bind(plugin_series_id)
        .bind(plugin_id)
        .bind(instance_id)
        .bind(ordering_mode)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_est_date(
        &self,
        episode_id: &str,
        date: Option<crate::datetime::UtcDateTime>,
    ) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE episodes SET est_date = ? WHERE episode_id = ?",
            date,
            episode_id
        )
    }

    // Batch Monitor Update
    // Batch monitor update: takes a slice to avoid N+1 queries. Chunked at 500 to
    // stay under SQLite's 999-variable limit with headroom for other bindings.
    pub async fn update_episode_monitor_status(
        &self,
        episode_ids: &[String],
        monitored: bool,
    ) -> Result<()> {
        if episode_ids.is_empty() {
            return Ok(());
        }

        const CHUNK_SIZE: usize = 500;
        for chunk in episode_ids.chunks(CHUNK_SIZE) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let query = format!(
                "UPDATE episodes SET monitored = ? WHERE episode_id IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&query).bind(monitored);
            for id in chunk {
                q = q.bind(id);
            }
            q.execute(&self.pool).await?;
        }
        Ok(())
    }

    pub async fn update_all_episode_monitor_status(
        &self,
        series_id: &str,
        monitored: bool,
    ) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE episodes SET monitored = ? WHERE series_id = ?",
            monitored,
            series_id
        )
    }

    // Monitor override methods
    // Monitor override: `monitor_override` tracks whether the user explicitly
    // toggled an episode's `monitored` flag (vs it being derived from the series
    // mode). Set on API toggle, cleared on series monitor-mode change, and
    // self-healed when the mode's evaluation matches the user's choice.

    /// Set `monitored` + `monitor_override=1` for a batch of episodes.
    /// Used by `toggle_episode_monitor` and `batch_monitor_episodes`.
    pub async fn set_episode_monitor_override(
        &self,
        episode_ids: &[String],
        monitored: bool,
    ) -> Result<()> {
        if episode_ids.is_empty() {
            return Ok(());
        }

        const CHUNK_SIZE: usize = 500;
        for chunk in episode_ids.chunks(CHUNK_SIZE) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let query = format!(
                "UPDATE episodes SET monitored = ?, monitor_override = 1 WHERE episode_id IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&query).bind(monitored);
            for id in chunk {
                q = q.bind(id);
            }
            q.execute(&self.pool).await?;
        }
        Ok(())
    }

    /// Clear `monitor_override` for all episodes in a series.
    /// Called when the user changes the series' monitor mode (fresh_apply=true).
    pub async fn clear_monitor_overrides_for_series(&self, series_id: &str) -> Result<()> {
        exec_with_bindings!(
            self,
            "UPDATE episodes SET monitor_override = 0 WHERE series_id = ?",
            series_id
        )
    }

    /// Batch clear `monitor_override` for episodes across multiple series.
    /// Called by `batch_apply_monitor_mode` with fresh_apply=true.
    pub async fn clear_monitor_overrides_batch(&self, series_ids: &[String]) -> Result<()> {
        if series_ids.is_empty() {
            return Ok(());
        }
        const CHUNK_SIZE: usize = 500;
        for chunk in series_ids.chunks(CHUNK_SIZE) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let query = format!(
                "UPDATE episodes SET monitor_override = 0 WHERE series_id IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&query);
            for id in chunk {
                q = q.bind(id);
            }
            q.execute(&self.pool).await?;
        }
        Ok(())
    }

    /// Return the set of episode IDs with `monitor_override=1` for the given series.
    /// Called by `reapply_monitor_for_series` with fresh_apply=false to know which
    /// episodes to skip during re-evaluation.
    pub async fn get_overridden_episode_ids(
        &self,
        series_ids: &[String],
    ) -> Result<std::collections::HashSet<String>> {
        if series_ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let mut overridden = std::collections::HashSet::new();
        const CHUNK_SIZE: usize = 500;
        for chunk in series_ids.chunks(CHUNK_SIZE) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let query = format!(
                "SELECT episode_id FROM episodes WHERE series_id IN ({}) AND monitor_override = 1",
                placeholders
            );
            let mut q = sqlx::query(&query);
            for id in chunk {
                q = q.bind(id);
            }
            let rows = q.fetch_all(&self.pool).await?;
            for row in rows {
                let ep_id: String = sqlx::Row::get(&row, 0);
                overridden.insert(ep_id);
            }
        }
        Ok(overridden)
    }

    /// Clear `monitor_override` for specific episode IDs (self-heal).
    /// Called when a sweep or organize finds that mode_wants == current_monitored
    /// for an overridden episode — the override is stale.
    pub async fn clear_monitor_overrides_for_episodes(&self, episode_ids: &[String]) -> Result<()> {
        if episode_ids.is_empty() {
            return Ok(());
        }
        const CHUNK_SIZE: usize = 500;
        for chunk in episode_ids.chunks(CHUNK_SIZE) {
            let placeholders = crate::db::sql_in_placeholders(chunk.len());
            let query = format!(
                "UPDATE episodes SET monitor_override = 0 WHERE episode_id IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&query);
            for id in chunk {
                q = q.bind(id);
            }
            q.execute(&self.pool).await?;
        }
        Ok(())
    }

    // Provider-scoped deletion methods, used by the clear-cache endpoint to delete
    // cached metadata for a specific (metadata_id, plugin_id, instance_id) entry.

    /// Delete metadata_season_cache rows for a specific (metadata_id, plugin_id, instance_id).
    /// Deletes all ordering_modes under that provider instance.
    pub async fn delete_metadata_season_cache_for_provider(
        &self,
        metadata_id: &str,
        plugin_id: &str,
        instance_id: &str,
    ) -> anyhow::Result<u64> {
        let affected = sqlx::query(
            "DELETE FROM metadata_season_cache WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ?",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(affected)
    }

    /// Delete metadata_fetch_log rows for a specific (metadata_id, plugin_id, instance_id).
    /// Deletes all ordering_modes under that provider instance.
    pub async fn delete_metadata_fetch_log_for_provider(
        &self,
        metadata_id: &str,
        plugin_id: &str,
        instance_id: &str,
    ) -> anyhow::Result<u64> {
        let affected = sqlx::query(
            "DELETE FROM metadata_fetch_log WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ?",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(affected)
    }

    // Instance-scoped deletion methods, used when a plugin instance is deleted —
    // they remove all cached data for that instance across all metadata_ids.

    /// Delete all metadata_episodes_cache rows for a specific instance.
    pub async fn delete_metadata_episodes_cache_for_instance(
        &self,
        instance_id: &str,
    ) -> anyhow::Result<u64> {
        let affected = sqlx::query("DELETE FROM metadata_episodes_cache WHERE instance_id = ?")
            .bind(instance_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected)
    }

    /// Delete all metadata_season_cache rows for a specific instance.
    pub async fn delete_metadata_season_cache_for_instance(
        &self,
        instance_id: &str,
    ) -> anyhow::Result<u64> {
        let affected = sqlx::query("DELETE FROM metadata_season_cache WHERE instance_id = ?")
            .bind(instance_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected)
    }

    /// Delete all metadata_fetch_log rows for a specific instance.
    pub async fn delete_metadata_fetch_log_for_instance(
        &self,
        instance_id: &str,
    ) -> anyhow::Result<u64> {
        let affected = sqlx::query("DELETE FROM metadata_fetch_log WHERE instance_id = ?")
            .bind(instance_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected)
    }

    /// Delete all metadata_series_cache rows for a specific instance.
    pub async fn delete_metadata_series_cache_for_instance(
        &self,
        instance_id: &str,
    ) -> anyhow::Result<u64> {
        let affected = sqlx::query("DELETE FROM metadata_series_cache WHERE instance_id = ?")
            .bind(instance_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected)
    }

    /// Null out ALL metadata fields on episodes that came from a specific instance.
    /// Sets metadata_source = NULL so polling can re-fill, and clears title,
    /// description, runtime, image_url, and meta_date so stale data isn't shown.
    pub async fn clear_episode_metadata_for_instance(
        &self,
        instance_id: &str,
    ) -> anyhow::Result<u64> {
        let affected = sqlx::query(
            "UPDATE episodes SET
                title = NULL,
                description = NULL,
                runtime = NULL,
                image_url = NULL,
                meta_date = NULL,
                metadata_source = NULL
             WHERE metadata_source = ?",
        )
        .bind(instance_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(affected)
    }
}
