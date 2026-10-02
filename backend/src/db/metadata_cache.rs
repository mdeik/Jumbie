use crate::datetime::UtcDateTime;

use super::DbManager;
use anyhow::Result;

pub struct MetadataSeriesCacheRow {
    pub title: String,
    pub overview: Option<String>,
    pub language: Option<String>,
    pub aliases: Vec<String>,
    pub image_url: Option<String>,
    pub fetched_at: String,
}

pub struct EpisodeMetadataCacheRow {
    pub season_number: i32,
    pub episode_number: i32,
    pub unique_id: String,
    pub title: String,
    pub description: Option<String>,
    pub runtime: Option<i32>,
    pub image_url: Option<String>,
    pub meta_date: Option<String>,
}

pub struct UpsertMetadataCacheParams<'a> {
    pub metadata_id: &'a str,
    pub plugin_id: &'a str,
    pub instance_id: &'a str,
    pub title: &'a str,
    pub overview: Option<&'a str>,
    pub language: Option<&'a str>,
    pub aliases: &'a [String],
    pub image_url: Option<&'a str>,
}

pub struct MergeMetadataCacheParams<'a> {
    pub metadata_id: &'a str,
    pub plugin_id: &'a str,
    pub instance_id: &'a str,
    pub title: &'a str,
    pub overview: Option<&'a str>,
    pub language: Option<&'a str>,
    pub aliases_json: Option<&'a str>,
    pub image_url: Option<&'a str>,
}

pub struct EpisodeMetadataForCache {
    pub season_number: i32,
    pub episode_number: i32,
    pub unique_id: String,
    pub title: String,
    pub description: Option<String>,
    pub runtime: Option<i32>,
    pub image_url: Option<String>,
    pub meta_date: Option<UtcDateTime>,
}

impl DbManager {
    /// Upsert keyed on (metadata_id, plugin_id, instance_id), refreshing the
    /// cached data and timestamp on repeated fetches.
    pub async fn upsert_metadata_series_cache(
        &self,
        params: UpsertMetadataCacheParams<'_>,
    ) -> Result<()> {
        let UpsertMetadataCacheParams {
            metadata_id,
            plugin_id,
            instance_id,
            title,
            overview,
            language,
            aliases,
            image_url,
        } = params;
        let aliases_json = serde_json::to_string(aliases).unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            "INSERT INTO metadata_series_cache (metadata_id, plugin_id, instance_id, title, overview, language, aliases, image_url, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
             ON CONFLICT(metadata_id, plugin_id, instance_id) DO UPDATE SET
                 title      = excluded.title,
                 overview   = excluded.overview,
                 language   = excluded.language,
                 aliases    = excluded.aliases,
                 image_url  = excluded.image_url,
                 fetched_at = datetime('now')",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .bind(title)
        .bind(overview)
        .bind(language)
        .bind(&aliases_json)
        .bind(image_url)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Merge partial series metadata into the cache without overwriting fields that
    /// were not provided (COALESCE, and `NULLIF(?, '')` for aliases). Use this from API
    /// routes: no single endpoint is guaranteed to return every field.
    pub async fn merge_metadata_series_cache(
        &self,
        params: MergeMetadataCacheParams<'_>,
    ) -> Result<()> {
        let MergeMetadataCacheParams {
            metadata_id,
            plugin_id,
            instance_id,
            title,
            overview,
            language,
            aliases_json,
            image_url,
        } = params;
        sqlx::query(
            "INSERT INTO metadata_series_cache (metadata_id, plugin_id, instance_id, title, overview, language, aliases, image_url, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, COALESCE(?, '[]'), ?, datetime('now'))
             ON CONFLICT(metadata_id, plugin_id, instance_id) DO UPDATE SET
                 title      = excluded.title,
                 overview   = COALESCE(excluded.overview, metadata_series_cache.overview),
                 language   = COALESCE(excluded.language, metadata_series_cache.language),
                 aliases    = COALESCE(NULLIF(?, ''), metadata_series_cache.aliases),
                 image_url  = COALESCE(excluded.image_url, metadata_series_cache.image_url),
                 fetched_at = datetime('now')",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .bind(title)
        .bind(overview)
        .bind(language)
        .bind(aliases_json) // INSERT: COALESCE(NULL, '[]') → '[]'; Some(json) → json
        .bind(image_url)    // INSERT: image_url column
        .bind(aliases_json) // UPDATE: NULLIF('', '') → NULL → keep old; Some(json) → write
        .bind(image_url)    // UPDATE: image_url bind (not wrapped in NULLIF)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Retrieve a cached series metadata row, if one exists. The JSON `aliases`
    /// column is deserialised into `Vec<String>` as part of the result.
    pub async fn get_metadata_series_cache(
        &self,
        metadata_id: &str,
        plugin_id: &str,
        instance_id: &str,
    ) -> Result<Option<MetadataSeriesCacheRow>> {
        type CacheRow = (
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            String,
        );
        let row: Option<CacheRow> = sqlx::query_as(
            "SELECT title, overview, language, aliases, image_url, fetched_at
             FROM metadata_series_cache
             WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ?",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some((title, overview, language, aliases_json, image_url, fetched_at)) => {
                let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap_or_default();
                Ok(Some(MetadataSeriesCacheRow {
                    title,
                    overview,
                    language,
                    aliases,
                    image_url,
                    fetched_at,
                }))
            }
            None => Ok(None),
        }
    }

    /// Look up the best cached series image URL by series_id, resolving the mapping's
    /// metadata_ids via json_each and matching on (metadata_id, instance_id) — the
    /// `metadata_ids` map is keyed by INSTANCE id, and the cache row carries the
    /// same instance_id, so keyed on stable identifiers rather than a fragile
    /// `WHERE title = ?`.
    pub async fn get_series_image_by_series_id(&self, series_id: &str) -> Result<Option<String>> {
        let url: Option<String> = sqlx::query_scalar(
            "SELECT c.image_url
             FROM metadata_series_cache c
             WHERE EXISTS (
                 SELECT 1
                 FROM series_mapping m,
                      json_each(m.metadata_ids) AS je
                 WHERE m.series_id = ?
                   AND je.value = c.metadata_id
                   AND je.key = c.instance_id
             )
             AND c.image_url IS NOT NULL
             LIMIT 1",
        )
        .bind(series_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(url)
    }

    /// Remove all cached series metadata rows for the given metadata_id.
    pub async fn delete_metadata_series_cache(&self, metadata_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM metadata_series_cache WHERE metadata_id = ?")
            .bind(metadata_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remove cached series metadata rows for a specific
    /// (metadata_id, plugin_id, instance_id) tuple.
    pub async fn delete_metadata_series_cache_for_provider(
        &self,
        metadata_id: &str,
        plugin_id: &str,
        instance_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM metadata_series_cache WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ?",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Batch-upsert episode cache entries in a single transaction, then prune rows
    /// the provider no longer returns, so the cache mirrors provider state without
    /// delete+reinsert churn. `ordering_mode` isolates entries by numbering mode.
    pub async fn batch_upsert_metadata_episodes_cache(
        &self,
        metadata_id: &str,
        plugin_id: &str,
        instance_id: &str,
        ordering_mode: &str,
        episodes: &[EpisodeMetadataForCache],
    ) -> Result<()> {
        with_transaction!(self.pool, |mut tx| {
            for ep in episodes {
                sqlx::query(
                    "INSERT INTO metadata_episodes_cache (metadata_id, plugin_id, instance_id, ordering_mode, season_number, episode_number, unique_id, title, description, runtime, image_url, meta_date, fetched_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
                     ON CONFLICT(metadata_id, plugin_id, instance_id, ordering_mode, season_number, episode_number) DO UPDATE SET
                         unique_id   = excluded.unique_id,
                         title       = excluded.title,
                         description = excluded.description,
                         runtime     = excluded.runtime,
                         image_url   = excluded.image_url,
                         meta_date    = excluded.meta_date,
                         fetched_at  = datetime('now')",
                )
                .bind(metadata_id)
                .bind(plugin_id)
                .bind(instance_id)
                .bind(ordering_mode)
                .bind(ep.season_number)
                .bind(ep.episode_number)
                .bind(&ep.unique_id)
                .bind(&ep.title)
                .bind(&ep.description)
                .bind(ep.runtime)
                .bind(&ep.image_url)
                .bind(ep.meta_date)
                .execute(&mut *tx)
                .await?;
            }

            // Prune rows cached previously but absent from this fetch. Inlining the
            // (season, episode) pairs is safe: they are parsed i32 values.
            if episodes.is_empty() {
                // Provider returned zero episodes — clear the entire cache scope.
                sqlx::query(
                    "DELETE FROM metadata_episodes_cache \
                     WHERE metadata_id = ? AND plugin_id = ? \
                       AND instance_id = ? AND ordering_mode = ?",
                )
                .bind(metadata_id)
                .bind(plugin_id)
                .bind(instance_id)
                .bind(ordering_mode)
                .execute(&mut *tx)
                .await?;
            } else {
                let pairs: Vec<String> = episodes
                    .iter()
                    .map(|ep| format!("({}, {})", ep.season_number, ep.episode_number))
                    .collect();
                let pair_list = pairs.join(",");
                let delete_sql = format!(
                    "DELETE FROM metadata_episodes_cache \
                     WHERE metadata_id = ? AND plugin_id = ? \
                       AND instance_id = ? AND ordering_mode = ? \
                       AND (season_number, episode_number) NOT IN ({})",
                    pair_list
                );
                sqlx::query(&delete_sql)
                    .bind(metadata_id)
                    .bind(plugin_id)
                    .bind(instance_id)
                    .bind(ordering_mode)
                    .execute(&mut *tx)
                    .await?;
            }

            Ok(())
        })
    }

    /// Retrieve all cached episodes for a given (metadata_id, plugin_id, instance_id, ordering_mode)
    /// ordered by (season_number, episode_number).
    pub async fn get_metadata_episodes_cache(
        &self,
        metadata_id: &str,
        plugin_id: &str,
        instance_id: &str,
        ordering_mode: &str,
    ) -> Result<Vec<EpisodeMetadataCacheRow>> {
        #[derive(Debug, sqlx::FromRow)]
        struct Row {
            season_number: i32,
            episode_number: i32,
            unique_id: String,
            title: String,
            description: Option<String>,
            runtime: Option<i32>,
            image_url: Option<String>,
            meta_date: Option<String>,
        }

        let rows: Vec<Row> = sqlx::query_as(
            "SELECT season_number, episode_number, unique_id, title, description, runtime, image_url, meta_date
             FROM metadata_episodes_cache
             WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ? AND ordering_mode = ?
             ORDER BY season_number, episode_number",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .bind(ordering_mode)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| EpisodeMetadataCacheRow {
                season_number: r.season_number,
                episode_number: r.episode_number,
                unique_id: r.unique_id,
                title: r.title,
                description: r.description,
                runtime: r.runtime,
                image_url: r.image_url,
                meta_date: r.meta_date,
            })
            .collect())
    }

    /// Retrieve cached episodes for a specific season, ordered by episode_number.
    pub async fn get_metadata_episodes_cache_for_season(
        &self,
        metadata_id: &str,
        plugin_id: &str,
        instance_id: &str,
        ordering_mode: &str,
        season_number: i32,
    ) -> Result<Vec<EpisodeMetadataCacheRow>> {
        #[derive(Debug, sqlx::FromRow)]
        struct Row {
            season_number: i32,
            episode_number: i32,
            unique_id: String,
            title: String,
            description: Option<String>,
            runtime: Option<i32>,
            image_url: Option<String>,
            meta_date: Option<String>,
        }

        let rows: Vec<Row> = sqlx::query_as(
            "SELECT season_number, episode_number, unique_id, title, description, runtime, image_url, meta_date
             FROM metadata_episodes_cache
             WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ? AND ordering_mode = ? AND season_number = ?
             ORDER BY episode_number",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .bind(ordering_mode)
        .bind(season_number)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| EpisodeMetadataCacheRow {
                season_number: r.season_number,
                episode_number: r.episode_number,
                unique_id: r.unique_id,
                title: r.title,
                description: r.description,
                runtime: r.runtime,
                image_url: r.image_url,
                meta_date: r.meta_date,
            })
            .collect())
    }

    /// Remove all cached episode rows for the given metadata_id.
    pub async fn delete_metadata_episodes_cache(&self, metadata_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM metadata_episodes_cache WHERE metadata_id = ?")
            .bind(metadata_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remove cached episode rows for a specific
    /// (metadata_id, plugin_id, instance_id) tuple.
    pub async fn delete_metadata_episodes_cache_for_provider(
        &self,
        metadata_id: &str,
        plugin_id: &str,
        instance_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM metadata_episodes_cache WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ?",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove cached episode rows for a specific (metadata_id, plugin_id, instance_id, ordering_mode, season).
    pub async fn delete_metadata_episodes_cache_for_season(
        &self,
        metadata_id: &str,
        plugin_id: &str,
        instance_id: &str,
        ordering_mode: &str,
        season_number: i32,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM metadata_episodes_cache \
             WHERE metadata_id = ? AND plugin_id = ? AND instance_id = ? AND ordering_mode = ? AND season_number = ?",
        )
        .bind(metadata_id)
        .bind(plugin_id)
        .bind(instance_id)
        .bind(ordering_mode)
        .bind(season_number)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
