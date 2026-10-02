//! Per-series rename-plan cache keyed by a fingerprint of the plan's inputs.
//!
//! `compute_batch_plan` runs for every non-hidden series on every poll of
//! `get_rename_queue`; most of the time the inputs are unchanged, so the computed
//! plan is cached per series and reused while its fingerprint stays stable.
//!
//! The fingerprint is derived from the plan's inputs — the database rows, the config,
//! and each candidate file's on-disk existence (see [`plan_inputs_fingerprint`]) — so
//! an entry's validity is a property of the data itself. Mutation sites do not signal
//! the cache: there is no invalidation call to forget, and no generation counter to
//! drift out of sync.
//!
//! `skip_paths` is excluded on purpose: it changes on every poll and is applied
//! after retrieval, so it must not invalidate the stored (unfiltered) plan.
//!
//! Symlinks are not resolved, so re-pointing an existing file at an equivalent
//! canonical path is not observed.
//!
//! Thread safety: `entries` uses `tokio::sync::RwLock` because callers may hold a
//! lookup across `.await`.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use xxhash_rust::xxh3::Xxh3;

use crate::file_manager::plan::{PlannedMove, RenamePlanError, compute_batch_plan_with_aux};
use jumbie_shared::config::Config;
use jumbie_shared::types::MappingRule;

struct CachedEntry {
    /// Fingerprint of the plan inputs at compute time.
    fingerprint: u64,
    /// The computed plan (all episodes/parts, unfiltered by skip_paths).
    /// Stored behind `Arc` so cache hits share the plan instead of deep-cloning
    /// the per-series move list on every poll.
    plan: Arc<Vec<PlannedMove>>,
}

pub struct RenamePlanCache {
    /// series_id → cached plan.
    entries: RwLock<HashMap<String, CachedEntry>>,
}

/// Inputs for [`RenamePlanCache::get_or_compute_with_aux`]. Grouped so the
/// signature stays readable as the plan inputs grow.
pub struct RenamePlanInputs<'a> {
    pub series_id: &'a str,
    pub config: &'a Config,
    pub mapping: &'a MappingRule,
    pub db_episodes: &'a [crate::db::EpisodeDetailRow],
    pub episode_parts: &'a [(String, crate::db::EpisodePartRow)],
    pub aux_files: &'a [crate::file_manager::AuxiliaryPath],
    pub skip_paths: &'a HashSet<PathBuf>,
}

impl RenamePlanCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: RwLock::new(HashMap::new()),
        })
    }

    /// The cached plan for `series_id` when its fingerprint still matches the
    /// current inputs.
    async fn get_matching(
        &self,
        series_id: &str,
        fingerprint: u64,
    ) -> Option<Arc<Vec<PlannedMove>>> {
        let entries = self.entries.read().await;
        let entry = entries.get(series_id)?;
        (entry.fingerprint == fingerprint).then(|| entry.plan.clone())
    }

    /// Whether any plan is cached for `series_id`, fresh or not.
    #[cfg(test)]
    pub async fn contains(&self, series_id: &str) -> bool {
        self.entries.read().await.contains_key(series_id)
    }

    /// Get the cached plan if fresh, otherwise compute, cache, and return it,
    /// filtering by `skip_paths` inside this method.
    pub async fn get_or_compute(
        &self,
        series_id: &str,
        config: &Config,
        mapping: &MappingRule,
        db_episodes: &[crate::db::EpisodeDetailRow],
        episode_parts: &[(String, crate::db::EpisodePartRow)],
        skip_paths: &HashSet<PathBuf>,
    ) -> Result<Arc<Vec<PlannedMove>>, RenamePlanError> {
        self.get_or_compute_with_aux(RenamePlanInputs {
            series_id,
            config,
            mapping,
            db_episodes,
            episode_parts,
            aux_files: &[],
            skip_paths,
        })
        .await
    }

    /// Like [`Self::get_or_compute`] but also plans auxiliary sidecar moves.
    pub async fn get_or_compute_with_aux(
        &self,
        inputs: RenamePlanInputs<'_>,
    ) -> Result<Arc<Vec<PlannedMove>>, RenamePlanError> {
        let fingerprint = plan_inputs_fingerprint(&inputs);
        if let Some(cached) = self.get_matching(inputs.series_id, fingerprint).await {
            return Ok(filter_plan_by_skip_paths(cached, inputs.skip_paths));
        }

        // `target_absolute` is derived from (config, mapping) — the same inputs the
        // fingerprint covers — so callers never pass it.
        let target_absolute =
            crate::file_manager::should_use_absolute_numbering(inputs.config, inputs.mapping);
        let plan = Arc::new(compute_batch_plan_with_aux(
            inputs.config,
            inputs.mapping,
            inputs.db_episodes,
            inputs.episode_parts,
            inputs.aux_files,
            target_absolute,
            &HashSet::new(),
        )?);

        let filtered = filter_plan_by_skip_paths(plan.clone(), inputs.skip_paths);
        self.entries.write().await.insert(
            inputs.series_id.to_string(),
            CachedEntry { fingerprint, plan },
        );
        Ok(filtered)
    }

    /// Drop the cached plan for a series (used when the series is pruned).
    pub async fn invalidate(&self, series_id: &str) {
        self.entries.write().await.remove(series_id);
    }

    /// Drop every cached plan.
    pub async fn invalidate_all(&self) {
        self.entries.write().await.clear();
    }
}

/// Fingerprint of every input that can change `compute_batch_plan_with_aux`'s output.
///
/// `skip_paths` is excluded on purpose — see the module docs.
///
/// The destructure of [`RenamePlanInputs`] is exhaustive: adding a field to that
/// struct fails to compile here until the new input is folded into the fingerprint.
/// That is what keeps the cache from going stale when the plan's inputs grow.
fn plan_inputs_fingerprint(inputs: &RenamePlanInputs<'_>) -> u64 {
    let RenamePlanInputs {
        series_id,
        config,
        mapping,
        db_episodes,
        episode_parts,
        aux_files,
        skip_paths: _,
    } = inputs;

    let mut hasher = Xxh3::default();
    series_id.hash(&mut hasher);
    crate::file_manager::should_use_absolute_numbering(config, mapping).hash(&mut hasher);

    // The whole config, not just the sections the plan reads today: it is small and
    // O(1) per series, and hashing it wholesale means a config field newly read by
    // `compute_batch_plan` can never be left out of the fingerprint.
    hash_serialized(&mut hasher, *config);

    // Every mapping field, less the scan bookkeeping that changes on each scan but
    // affects no plan output. Clearing it also drops it from the serialized form.
    let mut plan_mapping = (*mapping).clone();
    plan_mapping.settings.last_known_dir_mtimes.clear();
    hash_serialized(&mut hasher, &plan_mapping);

    // Derived `Hash` covers every field of these rows, so a column added to a row
    // is included automatically rather than needing a second update here. Each list
    // is sorted first: the caller's slices arrive in DB/`HashMap` iteration order,
    // which varies between polls, and an unstable fingerprint would recompute the
    // plan every time. On-disk existence joins the fingerprint because
    // `compute_batch_plan` drops sources that are missing from disk, so a file
    // appearing or vanishing changes the plan even when its DB row does not.
    let mut episodes: Vec<&crate::db::EpisodeDetailRow> = db_episodes.iter().collect();
    episodes.sort_by(|a, b| {
        (a.season, a.episode, &a.episode_id).cmp(&(b.season, b.episode, &b.episode_id))
    });
    for episode in episodes {
        episode.hash(&mut hasher);
        if let Some(file_path) = &episode.file_path {
            Path::new(file_path).exists().hash(&mut hasher);
        }
    }

    let mut parts: Vec<&(String, crate::db::EpisodePartRow)> = episode_parts.iter().collect();
    parts.sort_by(|a, b| {
        (&a.0, a.1.part_number, &a.1.file_path).cmp(&(&b.0, b.1.part_number, &b.1.file_path))
    });
    for (episode_id, part) in parts {
        episode_id.hash(&mut hasher);
        part.hash(&mut hasher);
        Path::new(&part.file_path).exists().hash(&mut hasher);
    }

    let mut aux: Vec<&crate::file_manager::AuxiliaryPath> = aux_files.iter().collect();
    aux.sort_by(|a, b| (&a.path, &a.episode_id).cmp(&(&b.path, &b.episode_id)));
    for aux_file in aux {
        aux_file.hash(&mut hasher);
        Path::new(&aux_file.path).exists().hash(&mut hasher);
    }

    hasher.finish()
}

/// Fold a serde value into the hasher. These config/mapping types always serialize;
/// the error arm only avoids panicking inside a poll, and writes a marker rather than
/// empty bytes so two different inputs cannot collide on a serialization failure.
fn hash_serialized<T: serde::Serialize>(hasher: &mut impl Hasher, value: &T) {
    match serde_json::to_vec(value) {
        Ok(bytes) => hasher.write(&bytes),
        Err(_) => hasher.write(b"<unserializable>"),
    }
}

// Skip-path filtering helper

/// Filter a plan so that any `PlannedMove` whose `src` is in `skip_paths` is removed.
/// Returns a shared `Arc`: when nothing needs filtering the input is returned as-is
/// (zero copy), so cache hits cost O(1) instead of deep-cloning every move.
pub fn filter_plan_by_skip_paths(
    plan: Arc<Vec<PlannedMove>>,
    skip_paths: &std::collections::HashSet<PathBuf>,
) -> Arc<Vec<PlannedMove>> {
    // Fast path: no skips requested, or none of the sources are skipped —
    // hand back the shared plan without copying a single move.
    if skip_paths.is_empty() || !plan.iter().any(|m| skip_paths.contains(&m.src)) {
        return plan;
    }
    // Slow path: rebuild only the surviving moves.
    let filtered: Vec<PlannedMove> = plan
        .iter()
        .filter(|m| !skip_paths.contains(&m.src))
        .cloned()
        .collect();
    Arc::new(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{EpisodeDetailRow, EpisodePartRow};
    use jumbie_shared::media_format::FileKind;

    fn episode(id: &str, episode: i32, file_path: Option<&str>) -> EpisodeDetailRow {
        EpisodeDetailRow {
            episode_id: id.to_string(),
            season: Some(1),
            episode,
            status: None,
            file_path: file_path.map(str::to_string),
            release_title: None,
            size: 0,
            title: None,
            quality_profile_id: None,
            submitter: None,
            media_info: None,
            quick_hash: None,
            created_at: None,
            file_acquired_at: None,
            monitored: false,
            meta_date: None,
            upload_date: None,
            est_date: None,
            metadata_ids: None,
            description: None,
            runtime: None,
            image_url: None,
            download_id: None,
            score: None,
            numbering_mode: 0,
            metadata_source: None,
            series_id: "s1".to_string(),
            download_link: None,
            monitor_override: false,
            original_path: None,
        }
    }

    fn part(episode_id: &str, part_number: i32) -> (String, EpisodePartRow) {
        (
            episode_id.to_string(),
            EpisodePartRow {
                part_number,
                file_path: format!("/lib/{episode_id}-pt{part_number}.mkv"),
                size: None,
                fingerprint: None,
                media_info: None,
                original_path: None,
            },
        )
    }

    fn aux(path: &str) -> crate::file_manager::AuxiliaryPath {
        crate::file_manager::AuxiliaryPath {
            path: path.to_string(),
            kind: FileKind::Subtitle,
            episode_id: "s1_1_1".to_string(),
        }
    }

    fn fingerprint(
        config: &Config,
        mapping: &MappingRule,
        episodes: &[EpisodeDetailRow],
        parts: &[(String, EpisodePartRow)],
        aux_files: &[crate::file_manager::AuxiliaryPath],
    ) -> u64 {
        plan_inputs_fingerprint(&RenamePlanInputs {
            series_id: "s1",
            config,
            mapping,
            db_episodes: episodes,
            episode_parts: parts,
            aux_files,
            skip_paths: &HashSet::new(),
        })
    }

    #[test]
    fn fingerprint_is_stable_for_identical_inputs() {
        let config = crate::test_fixtures::default_config();
        let mapping = MappingRule::default();
        let episodes = vec![episode("s1_1_1", 1, Some("/lib/E01.mkv"))];
        let parts = vec![part("s1_1_1", 1)];
        let aux_files = vec![aux("/lib/E01.en.srt")];

        assert_eq!(
            fingerprint(&config, &mapping, &episodes, &parts, &aux_files),
            fingerprint(&config, &mapping, &episodes, &parts, &aux_files),
        );
    }

    #[test]
    fn fingerprint_is_order_insensitive() {
        let config = crate::test_fixtures::default_config();
        let mapping = MappingRule::default();
        let episodes = vec![
            episode("s1_1_1", 1, Some("/lib/E01.mkv")),
            episode("s1_1_2", 2, Some("/lib/E02.mkv")),
        ];
        let parts = vec![part("s1_1_1", 1), part("s1_1_1", 2)];
        let aux_files = vec![aux("/lib/E01.en.srt"), aux("/lib/E01.nfo")];

        let base = fingerprint(&config, &mapping, &episodes, &parts, &aux_files);

        let mut episodes_rev = episodes.clone();
        episodes_rev.reverse();
        let mut parts_rev = parts.clone();
        parts_rev.reverse();
        let mut aux_rev = aux_files.clone();
        aux_rev.reverse();

        assert_eq!(
            base,
            fingerprint(&config, &mapping, &episodes_rev, &parts_rev, &aux_rev),
        );
    }

    #[test]
    fn fingerprint_changes_on_episode_and_part_and_aux_edits() {
        let config = crate::test_fixtures::default_config();
        let mapping = MappingRule::default();
        let episodes = vec![episode("s1_1_1", 1, Some("/lib/E01.mkv"))];
        let parts = vec![part("s1_1_1", 1)];
        let aux_files = vec![aux("/lib/E01.en.srt")];
        let base = fingerprint(&config, &mapping, &episodes, &parts, &aux_files);

        // A new episode row.
        let mut episodes2 = episodes.clone();
        episodes2.push(episode("s1_1_2", 2, Some("/lib/E02.mkv")));
        assert_ne!(
            base,
            fingerprint(&config, &mapping, &episodes2, &parts, &aux_files)
        );

        // A changed episode field.
        let mut episodes3 = episodes.clone();
        episodes3[0].title = Some("Pilot".to_string());
        assert_ne!(
            base,
            fingerprint(&config, &mapping, &episodes3, &parts, &aux_files)
        );

        // A changed part number, and a second part.
        let parts2 = vec![part("s1_1_1", 2)];
        assert_ne!(
            base,
            fingerprint(&config, &mapping, &episodes, &parts2, &aux_files)
        );
        let parts3 = vec![part("s1_1_1", 1), part("s1_1_1", 2)];
        assert_ne!(
            base,
            fingerprint(&config, &mapping, &episodes, &parts3, &aux_files)
        );

        // A new sidecar.
        let mut aux2 = aux_files.clone();
        aux2.push(aux("/lib/E01.nfo"));
        assert_ne!(
            base,
            fingerprint(&config, &mapping, &episodes, &parts, &aux2)
        );
    }

    #[test]
    fn fingerprint_changes_on_config_and_mapping_edits() {
        let mapping = MappingRule::default();
        let episodes = vec![episode("s1_1_1", 1, Some("/lib/E01.mkv"))];
        let parts: Vec<(String, EpisodePartRow)> = Vec::new();
        let aux_files: Vec<crate::file_manager::AuxiliaryPath> = Vec::new();

        let config = crate::test_fixtures::default_config();
        let base = fingerprint(&config, &mapping, &episodes, &parts, &aux_files);

        // organization section
        let mut config_org = config.clone();
        config_org.organization.rename_episodes = !config_org.organization.rename_episodes;
        assert_ne!(
            base,
            fingerprint(&config_org, &mapping, &episodes, &parts, &aux_files)
        );

        // general section
        let mut config_gen = config.clone();
        config_gen.general.flatten_season_folders = !config_gen.general.flatten_season_folders;
        assert_ne!(
            base,
            fingerprint(&config_gen, &mapping, &episodes, &parts, &aux_files)
        );

        // mapping
        let mut mapping2 = mapping.clone();
        mapping2.settings.episode_file_format = Some("${series} E${episode:02}.mkv".to_string());
        assert_ne!(
            base,
            fingerprint(&config, &mapping2, &episodes, &parts, &aux_files)
        );
    }

    #[test]
    fn fingerprint_ignores_scan_mtime_bookkeeping() {
        // `last_known_dir_mtimes` is written on every scan cycle but never affects
        // the plan, so it must not invalidate the cache.
        let config = crate::test_fixtures::default_config();
        let episodes = vec![episode("s1_1_1", 1, Some("/lib/E01.mkv"))];
        let parts: Vec<(String, EpisodePartRow)> = Vec::new();
        let aux_files: Vec<crate::file_manager::AuxiliaryPath> = Vec::new();

        let mapping = MappingRule::default();
        let base = fingerprint(&config, &mapping, &episodes, &parts, &aux_files);

        let mut mapping2 = mapping.clone();
        mapping2
            .settings
            .last_known_dir_mtimes
            .insert(".".to_string(), 1_789_000_000.5);
        assert_eq!(
            base,
            fingerprint(&config, &mapping2, &episodes, &parts, &aux_files)
        );
    }

    #[test]
    fn fingerprint_ignores_skip_paths() {
        let config = crate::test_fixtures::default_config();
        let mapping = MappingRule::default();
        let episodes = vec![episode("s1_1_1", 1, Some("/lib/E01.mkv"))];
        let parts: Vec<(String, EpisodePartRow)> = Vec::new();
        let aux_files: Vec<crate::file_manager::AuxiliaryPath> = Vec::new();

        let mut skip = HashSet::new();
        skip.insert(PathBuf::from("/lib/E01.mkv"));
        let with_skip = plan_inputs_fingerprint(&RenamePlanInputs {
            series_id: "s1",
            config: &config,
            mapping: &mapping,
            db_episodes: &episodes,
            episode_parts: &parts,
            aux_files: &aux_files,
            skip_paths: &skip,
        });
        assert_eq!(
            with_skip,
            fingerprint(&config, &mapping, &episodes, &parts, &aux_files)
        );
    }

    #[test]
    fn fingerprint_tracks_source_existence() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("E01.mkv");
        let config = crate::test_fixtures::default_config();
        let mapping = MappingRule::default();
        let parts: Vec<(String, EpisodePartRow)> = Vec::new();
        let aux_files: Vec<crate::file_manager::AuxiliaryPath> = Vec::new();
        let episodes = vec![episode("s1_1_1", 1, Some(src.to_str().unwrap()))];

        let absent = fingerprint(&config, &mapping, &episodes, &parts, &aux_files);
        std::fs::write(&src, b"x").unwrap();
        let present = fingerprint(&config, &mapping, &episodes, &parts, &aux_files);

        assert_ne!(
            absent, present,
            "a source appearing on disk must change the fingerprint"
        );
    }

    #[tokio::test]
    async fn cache_reflects_new_inputs_without_invalidation() {
        // Regression: assigning a file to an already-cached series must show up on
        // the next compute, with no invalidation call in between.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("fake.mkv");
        std::fs::write(&src, b"x").unwrap();

        let cache = RenamePlanCache::new();
        let config = crate::test_fixtures::default_config();
        let mapping = MappingRule {
            target_title: "test".into(),
            series_id: "s1".into(),
            ..Default::default()
        };
        let parts: Vec<(String, EpisodePartRow)> = Vec::new();

        // Prime while the episode has no file.
        let unassigned = vec![episode("s1_1_1", 1, None)];
        let before = cache
            .get_or_compute(
                "s1",
                &config,
                &mapping,
                &unassigned,
                &parts,
                &HashSet::new(),
            )
            .await
            .unwrap();
        assert!(before.is_empty());

        // The file is assigned — only the inputs change.
        let assigned = vec![episode("s1_1_1", 1, Some(src.to_str().unwrap()))];
        let after = cache
            .get_or_compute("s1", &config, &mapping, &assigned, &parts, &HashSet::new())
            .await
            .unwrap();
        assert_eq!(
            after.len(),
            1,
            "assigned file must appear without invalidation"
        );
        assert_eq!(after[0].src, src);
    }
}
