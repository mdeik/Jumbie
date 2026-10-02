// Retroactive score recalculation. When a release profile changes (term weights,
// keywords, submitter scores, etc.), existing episode scores become stale. This
// module recalculates `release_info.score` for episodes that have a `release_title`
// AND stored scoring inputs (size, seeders, meta_date, episode_count).
//
// Scores live on `release_info` (keyed by content fingerprint via file_paths),
// not on `episodes`. The rescore is identical to the original `calculate()` call
// because the exact inputs are persisted at download time — without them, scoring
// would lose size/peers/age contributions.
//
// Called from `PUT /api/config/release_profiles`, `PUT /api/series/{id}`, and
// `POST /api/series/batch-edit`.

use crate::db::DbManager;
use anyhow::Result;
use jumbie_shared::scoring::ReleaseProfile;
use std::collections::HashMap;

impl DbManager {
    /// Recalculate `release_info.score` for all episodes in the given series that
    /// have a `release_title` and stored scoring inputs. The global release profile
    /// is fetched by name and merged with any series-level override; series with no
    /// profile configured are skipped.
    pub async fn rescore_episodes_for_profiles(&self, series_ids: &[String]) -> Result<()> {
        if series_ids.is_empty() {
            return Ok(());
        }

        let mappings = self.get_series_mappings_batch(series_ids).await?;

        let episodes = self.get_episodes_for_rescore(series_ids).await?;

        if episodes.is_empty() {
            return Ok(());
        }

        let mut by_series: HashMap<&str, Vec<&crate::db::EpisodeRescoreRow>> = HashMap::new();
        for ep in &episodes {
            by_series.entry(ep.series_id.as_str()).or_default().push(ep);
        }

        for sid in series_ids {
            let Some(series_eps) = by_series.get(sid.as_str()) else {
                continue;
            };

            let Some(mapping) = mappings.get(sid) else {
                continue;
            };

            let profile_id = match mapping.release_profile.as_deref() {
                Some(name) if !name.is_empty() => name,
                _ => continue, // No release profile configured → skip
            };

            let global_profile = self
                .get_release_profile(profile_id)
                .await?
                .unwrap_or_default();

            let merged = mapping.get_merged_scoring(&global_profile);
            let mut merged = merged.clone();
            merged.compile();

            self.rescore_episode_batch(&merged, series_eps).await?;
        }

        Ok(())
    }

    /// Rescore a single batch of episodes using the given compiled scoring profile.
    /// Updates `release_info.score` (via file_paths → fingerprint)
    /// in chunks within transactions.
    async fn rescore_episode_batch(
        &self,
        merged: &ReleaseProfile,
        episodes: &[&crate::db::EpisodeRescoreRow],
    ) -> Result<()> {
        if episodes.is_empty() {
            return Ok(());
        }

        for chunk in episodes.chunks(100) {
            // BEGIN IMMEDIATE for the same reason as `with_transaction!`: a
            // deferred write transaction can hit SQLITE_BUSY_SNAPSHOT (517).
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;

            for ep in chunk {
                let release_title = match ep.release_title.as_deref() {
                    Some(t) if !t.is_empty() => t,
                    _ => continue,
                };

                let size = ep.scoring_size_bytes.unwrap_or(0) as u64;
                let seeders = ep.scoring_seeders.unwrap_or(0) as u32;
                let meta_date = ep
                    .scoring_upload_date
                    .as_deref()
                    .and_then(|d| crate::datetime::parse_utc(d).ok())
                    .map(|u| u.to_chrono_utc());
                let episode_count = ep.scoring_episode_count.map(|c| c as u32);

                let (new_score, _) = merged.calculate_with_submitter(
                    release_title,
                    size,
                    seeders,
                    meta_date,
                    episode_count,
                    ep.submitter.as_deref(),
                );

                // Match insert_episode's normalization: store 0 as NULL
                let score_to_store: Option<i32> = if new_score == 0 {
                    None
                } else {
                    Some(new_score)
                };

                sqlx::query(&format!(
                    "UPDATE release_info SET score = ? WHERE quick_hash IN (
                         SELECT fp.fingerprint FROM file_paths fp WHERE {pred}
                     )",
                    pred = crate::db::EPISODE_FILES_PREDICATE,
                ))
                .bind(score_to_store)
                .bind(&ep.episode_id)
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;
        }

        Ok(())
    }
}
