use crate::db::DbManager;
use crate::db::autoresolve::RejectionTarget;
use crate::plugins::downloaders::DownloadManager;
use crate::plugins::notifiers::NotifierManager;
use anyhow::Result;
use jumbie_shared::config::Config;
use jumbie_shared::types::DownloadQueueItem;

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::plugins::PluginManager;
use crate::state::FileStateManager;
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;
use tracing::trace;

/// Progress delta below which a sample counts as "no progress". Absorbs f32
/// rounding noise without hiding real advancement.
const PROGRESS_EPSILON: f32 = 0.0001;

/// A `Downloading` item with no progress for this long is treated as stalled and
/// shows the "No Progress" warning badge.
pub(crate) const NO_PROGRESS_MINUTES: i64 = 30;

/// Minutes without progress for a `no_progress_since` anchor, or `None` when the
/// item has no active window or has not reached `NO_PROGRESS_MINUTES` yet.
///
/// SSoT for the threshold: the orchestrator (detection) and the queue API (the
/// warning badge) derive the same value from here.
pub(crate) fn no_progress_minutes(
    since: Option<chrono::NaiveDateTime>,
    now: chrono::NaiveDateTime,
) -> Option<i64> {
    let elapsed = (now - since?).num_minutes();
    (elapsed >= NO_PROGRESS_MINUTES).then_some(elapsed)
}
/// Alternative releases tried for an episode before it is marked Failed.
const MAX_AUTORESOLVE_ATTEMPTS: i64 = 2;
/// Attempt budget window, measured from the most recent attempt. Attempts older
/// than this no longer count toward the limit.
pub(crate) const AUTORESOLVE_BUDGET_HOURS: i64 = 12;
/// Once the limit is hit the count resets after this cooldown instead of the
/// full budget window (a failed episode is retried sooner).
pub(crate) const AUTORESOLVE_HIT_COOLDOWN_HOURS: i64 = 2;
/// A release that stalled is excluded from auto-search for this long.
const REJECTION_HOURS: i64 = 48;

#[derive(Clone)]
pub struct ContentOrganizer {
    // Fields are Arc-wrapped (and RwLock for mutable ones) because ContentOrganizer
    // is Clone and shared across API handlers, background tasks, and WebSockets.
    pub(crate) db: Arc<DbManager>,
    pub(crate) downloader: Arc<RwLock<DownloadManager>>,
    pub(crate) _state_manager: Arc<tokio::sync::Mutex<FileStateManager>>,
    pub notifications: Arc<RwLock<NotifierManager>>,
    pub(crate) plugin_manager: Arc<RwLock<PluginManager>>,
    pub(crate) shutdown_token: CancellationToken,
    /// Shared lock set — injected from AppState so API route lock checks
    /// (try_lock_series) and background organize checks see the same
    /// in-memory state. The ContentOrganizer does NOT own this lock;
    /// it is created by the application root and shared.
    pub(crate) modifying_series: Arc<RwLock<HashSet<String>>>,
}

impl ContentOrganizer {
    pub async fn new(
        db_path: &str,
        db: Arc<DbManager>,
        plugin_manager: Arc<RwLock<PluginManager>>,
        shutdown_token: CancellationToken,
        modifying_series: Arc<RwLock<HashSet<String>>>,
    ) -> Result<Self> {
        let plugins_cfg = db.get_plugins_config().await.unwrap_or_default();

        let download_manager = DownloadManager::new(plugin_manager.clone());

        let mut state_manager = FileStateManager::new(db.clone());
        state_manager.recover_from_crash().await?;

        // With no downloader plugin configured (empty organizer_paths) there is
        // nothing for the file watcher to monitor.
        let watch_paths = download_manager.get_organizer_paths().await;
        state_manager.start_watching(watch_paths, Some(shutdown_token.clone()))?;

        let notifications = NotifierManager::new(plugin_manager.clone());

        let organizer = Self {
            db: db.clone(),
            downloader: Arc::new(RwLock::new(download_manager)),
            _state_manager: Arc::new(tokio::sync::Mutex::new(state_manager)),
            notifications: Arc::new(RwLock::new(notifications)),
            plugin_manager: plugin_manager.clone(),
            shutdown_token,
            modifying_series,
        };

        // load_internal_plugins needs the global config, which needs `self`, so it
        // must run after construction rather than inside it.
        let mut plugin_cfg = organizer.db_config().await;
        plugin_cfg.database = db_path.to_string();
        let global_cfg = Arc::new(plugin_cfg);
        organizer
            .plugin_manager
            .write()
            .await
            .load_internal_plugins(&plugins_cfg, global_cfg.clone())
            .await;

        // Apply saved DB config to external plugins (discovered from disk and
        // started with empty config). Internal instances already hold this config,
        // so `apply_config` only reconciles externals.
        {
            let db_cfg = db.get_plugins_config().await.unwrap_or_default();
            let mut pm = organizer.plugin_manager.write().await;
            pm.apply_config(&db_cfg, global_cfg.clone()).await;
        }

        Ok(organizer)
    }

    pub fn downloader(&self) -> Arc<RwLock<DownloadManager>> {
        self.downloader.clone()
    }

    pub fn notifications(&self) -> Arc<RwLock<NotifierManager>> {
        self.notifications.clone()
    }

    pub fn plugin_manager(&self) -> Arc<RwLock<PluginManager>> {
        self.plugin_manager.clone()
    }

    /// All configured download roots (staging paths plus raw download paths),
    /// used to bound empty-staging-folder cleanup. See
    /// `DownloadManager::get_download_roots`.
    pub(crate) async fn download_roots(&self) -> Vec<std::path::PathBuf> {
        self.downloader.read().await.get_download_roots().await
    }

    /// Trigger a targeted search for specific missing episodes — used when a
    /// multi-episode pack is cancelled or a monitored episode wasn't found by the
    /// regular source sweep.
    ///
    /// Queries every FeedProvider, merges results, scores them against the
    /// series' merged scoring profile, and downloads the best candidates.
    ///
    /// Callers SHOULD space out invocations — to respect external indexer rate
    /// limits and to avoid self-DoS'ing the application's own plugin IPC.
    pub async fn auto_search_missing(
        &self,
        series_id: &str,
        season: &str,
        missing_episodes: &[i32],
    ) -> Result<()> {
        if missing_episodes.is_empty() {
            tracing::trace!("auto_search_missing: missing_episodes is empty, returning early");
            return Ok(());
        }

        tracing::debug!(
            "auto_search_missing: series={} season={} episodes={:?}",
            series_id,
            season,
            missing_episodes
        );

        let mapping = match self.db.get_series_mapping(series_id).await? {
            Some(m) => m,
            None => {
                tracing::warn!("auto_search_missing: series {} not found", series_id);
                return Ok(());
            }
        };

        let profile_id = mapping.release_profile.as_deref().unwrap_or("").to_string();
        let profile_scoring = self
            .db
            .get_release_profile(&profile_id)
            .await
            .unwrap_or(None)
            .unwrap_or_default();
        let min_score = profile_scoring.min_score;
        let merged_scoring = mapping.get_merged_scoring(&profile_scoring);

        // SSoT: effective numbering-mode default (series tristate → global).
        let (global_absolute, global_format, global_format_absolute) = {
            let c = self.db_config().await;
            (
                c.general.absolute_numbering,
                c.organization.search_format.clone(),
                c.organization.search_format_absolute.clone(),
            )
        };

        // SSoT: absolute numbering is canonically ABSOLUTE_SEASON_NUM; in normal
        // mode the season label must be numeric — never guessed.
        let absolute = mapping.settings.active_mode(global_absolute).is_absolute();
        let season_num = jumbie_shared::mapping::resolve_season_num(season, absolute)?;

        let meta = crate::search::resolve_season_search_meta(
            &mapping,
            season,
            season_num,
            global_absolute,
            &global_format,
            &global_format_absolute,
        );
        let search_season_num = meta.search_season_num;
        let episode_offset = meta.episode_offset;

        let db_seasons = self
            .db
            .get_series_seasons(&mapping.series_id)
            .await
            .unwrap_or_default();
        let suppressed: std::collections::HashSet<i32> = self
            .db
            .get_suppressed_seasons(&mapping.series_id, global_absolute as i32)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        let normalized_seasons = crate::api_routes::series::build_season_list(
            db_seasons,
            mapping
                .settings
                .season_for_active_mode(global_absolute)
                .values(),
            &suppressed,
        );

        // Ensure DB rows exist for configured episode cells.
        if let Err(e) = self
            .db
            .ensure_episode_cells(&mapping, &normalized_seasons, global_absolute)
            .await
        {
            tracing::warn!(
                "Failed to ensure episode cells for series '{}': {}",
                mapping.target_title,
                e
            );
        }

        // Resolve UUID-scoped aliases into plugin-specific and generic maps.
        let (plugin_aliases, generic_aliases) = {
            let pm = self.plugin_manager.read().await;
            crate::search::resolve_search_aliases(&pm, &mapping, season, global_absolute).await
        };

        let pm = self.plugin_manager.read().await;
        let sources = pm.get_plugins_by_all_capabilities(&[
            jumbie_shared::plugin::Capability::FeedProvider,
            jumbie_shared::plugin::Capability::AutomaticSearch,
        ]);
        let mut all_entries: Vec<(String, crate::models::media::MediaEntry)> = Vec::new();
        // Source plugins queried — included in the summary log.
        let mut queried_sources: Vec<String> = Vec::new();

        for source in sources {
            let source_id = source.instance_id().to_string();
            let source_name = pm.get_instance_name(&source_id);
            queried_sources.push(source_name.clone());
            let final_aliases = crate::search::resolve_final_aliases(
                &source_id,
                &plugin_aliases,
                &generic_aliases,
                &mapping.target_title,
            );

            // Episodes in SOURCE numbering (local + season offset).
            let source_episodes =
                crate::search::source_episode_numbers(missing_episodes, episode_offset);
            let log = crate::search::SearchLog {
                kind: crate::search::SearchKind::AutoMissing,
                source: source_name,
                series_id: Some(mapping.series_id.clone()),
                series_title: Some(mapping.target_title.clone()),
                season: Some(search_season_num),
                source_episodes,
                aliases: final_aliases.clone(),
            };

            let payload = crate::search::build_search_payload(
                &mapping.target_title,
                search_season_num,
                missing_episodes,
                final_aliases,
                &meta.search_format,
                episode_offset,
            );

            tracing::trace!(
                "auto_search_missing: querying source {}",
                source.instance_id()
            );

            match crate::plugins::bridge::sources::auto_search(source.as_ref(), payload, &log).await
            {
                Ok(entries) => {
                    tracing::debug!(
                        "auto_search_missing: source {} ({}) returned {} result(s)",
                        log.source,
                        source_id,
                        entries.len()
                    );
                    for entry in entries {
                        all_entries.push((source_id.clone(), entry));
                    }
                }
                Err(e) => tracing::warn!(
                    "auto_search_missing: source {} ({}) error: {}",
                    log.source,
                    source.instance_id(),
                    e
                ),
            }
        }

        // Score and filter — read from DB, not from stale self.config
        let auto_profiles_enabled = self
            .db
            .get_general_config()
            .await
            .map(|g| g.automatic_profiles.enabled)
            .unwrap_or(false);

        // Total entries returned across all sources, before any filtering.
        let found = all_entries.len();

        let mut scored: Vec<(i32, String, crate::models::media::MediaEntry)> = Vec::new();
        for (source_id, entry) in all_entries {
            let (title_ref, size, seeders, published) = entry.scoring_fields();
            let (mut score, _) = merged_scoring.calculate_with_submitter(
                title_ref,
                size,
                seeders,
                published,
                None,
                entry.submitter.as_deref(),
            );

            if auto_profiles_enabled {
                let submitter = entry
                    .submitter
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string());
                if let Ok(Some(profile)) = self.db.get_automatic_profile(&submitter).await {
                    score += profile.score;
                }
            }

            // SSoT: the same minimum-score gate manual search reports.
            if crate::release_checks::score_failure(score, min_score).is_none() {
                scored.push((score, source_id, entry));
            }
        }

        tracing::info!(
            "auto_search_missing: query series='{}' season={} episodes={:?} aliases={:?} sources={:?} — {} of {} result(s) passed min_score {}",
            mapping.target_title,
            season,
            missing_episodes,
            generic_aliases,
            queried_sources,
            scored.len(),
            found,
            min_score,
        );

        // Build candidates via the normal pipeline. Mappings and the compiled
        // matcher are loaded once for the whole batch rather than per entry.
        let all_mappings = self.db.get_all_series_mappings().await?;
        let matcher = crate::source_processor::SeriesMatcher::build(&all_mappings);
        let mut candidates = Vec::new();
        for (_, source_id, entry) in &scored {
            if let Ok(mut cands) = self
                .process_entry_with_mappings(entry, source_id, &all_mappings, &matcher, None)
                .await
            {
                let release = meta.match_release(&entry.title);
                // The reject gate is applied centrally in `select_winners`; here we
                // only keep candidates that cover a requested episode.
                cands.retain(|c| {
                    c.needed_episodes.iter().any(|ep| {
                        missing_episodes.contains(ep)
                            && release.accepts(jumbie_shared::mapping::local_to_source_episode(
                                *ep,
                                episode_offset,
                            ))
                    })
                });
                candidates.extend(cands);
            }
        }

        if candidates.is_empty() {
            tracing::info!(
                "auto_search_missing: no candidates found for series='{}' season={} episodes={:?}",
                mapping.target_title,
                season,
                missing_episodes
            );
            return Ok(());
        }

        tracing::info!(
            "auto_search_missing: {} candidate(s) from {} result(s) for episodes {:?}",
            candidates.len(),
            found,
            missing_episodes
        );

        let winners = self.select_winners(candidates).await?;
        for winner in winners {
            tracing::trace!("auto_search_missing: downloading winner {}", winner.title);
            self.download_winner(&winner).await?;
        }

        Ok(())
    }

    /// Build a Config with DB-managed sections (organization, general) read
    /// directly from the database.  All file-operation callers should use this
    /// instead of self.config, which is TOML-only and never synced.
    pub(crate) async fn db_config(&self) -> Config {
        let mut config = Config::default();
        config.organization = self.db.get_organization_config().await.unwrap_or_default();
        if let Ok(g) = self.db.get_general_config().await {
            config.general = g;
        }
        config
    }

    /// SSoT: the global absolute-numbering default
    /// (`config.general.absolute_numbering`), for code paths that need the
    /// effective numbering mode but receive no caller-provided value.
    pub(crate) async fn global_absolute_default(&self) -> bool {
        self.db
            .get_general_config()
            .await
            .map(|g| g.absolute_numbering)
            .unwrap_or(false)
    }

    /// Process the download queue:
    ///   1. Dispatch queued items: read download_id from episodes table, send to download client.
    ///   2. Poll downloading + organizing items: check content path, finalize when available.
    pub async fn process_download_queue(&self) -> Result<()> {
        const MAX_DISPATCH_RETRIES: i32 = 5;

        // Stage 1: dispatch queued items.
        // `download_queue.download_id` is set at queue time by enrich_and_enqueue
        // (SSoT); items without one cannot be dispatched.
        let mut dispatched = std::collections::HashSet::new();
        let queued = self.db.get_queued_items().await?;
        for item in &queued {
            // Items re-queued by Stage 2 have a future next_retry_at; skip until then.
            if let Some(retry_at) = item.next_retry_at
                && retry_at > chrono::Utc::now().naive_utc()
            {
                trace!(
                    "process_download_queue: skipping dispatch for '{}' — retry at {}",
                    item.media_name, retry_at
                );
                continue;
            }

            // Read the torrent hash from the item itself (SSoT: set at queue time
            // by enrich_and_enqueue) rather than re-deriving via episode_id, which
            // may still be NULL for series-level downloads until smart-link resolves.
            let dispatch_hash = item.download_id.clone().filter(|h| !h.is_empty());

            // Skip items already dispatched with this hash, so cycles don't re-dispatch.
            let already_dispatched = match (&item.downloader_id, &dispatch_hash) {
                (Some(current), Some(hash)) => current == hash,
                _ => false,
            };
            if !already_dispatched {
                let Some(hash) = dispatch_hash.as_deref().filter(|h| !h.is_empty()) else {
                    tracing::warn!(
                        "process_download_queue: item {} '{}' has no download_id — \
                         source plugin did not provide one, cannot dispatch",
                        item.id,
                        item.media_name,
                    );
                    continue;
                };

                let client_id = {
                    let download_mgr = self.downloader.read().await;

                    download_mgr
                        .add_download(
                            &item.media_link,
                            &item.category,
                            None, // tag — removed
                            Some(&item.media_name),
                        )
                        .await
                };

                match client_id {
                    Ok(cid) if !cid.is_empty() => {
                        // Confirm the content path before entering Downloading; if
                        // unavailable the item stays Queued and the retry mechanism
                        // re-checks next cycle (add_download is idempotent).
                        let path_ok = {
                            let download_mgr = self.downloader.read().await;
                            download_mgr.get_download_content_path(hash).await.is_ok()
                        };

                        if path_ok {
                            self.db.reset_queue_item_retry(item.id).await?;
                            self.db.reset_progress_tracking(item.id).await?;
                            self.db
                                .update_queue_item_status(
                                    item.id,
                                    "Downloading",
                                    Some(hash),
                                    Some(&cid),
                                    None,
                                )
                                .await?;
                            dispatched.insert(item.id);
                        } else {
                            // Client accepted the download but the path hasn't
                            // resolved yet; keep Queued and re-check next cycle.
                            let next_count = item.retry_count + 1;
                            if next_count >= MAX_DISPATCH_RETRIES {
                                tracing::warn!(
                                    "process_download_queue: max dispatch retries ({}) \
                                     reached for '{}' after content path check failed",
                                    MAX_DISPATCH_RETRIES,
                                    item.media_name,
                                );
                                self.db
                                    .update_queue_item_status(
                                        item.id,
                                        "Failed",
                                        None,
                                        None,
                                        Some(&format!(
                                            "Content path not available after {} retries",
                                            MAX_DISPATCH_RETRIES,
                                        )),
                                    )
                                    .await?;
                            } else {
                                let now = chrono::Utc::now().naive_utc();
                                let minutes = retry_backoff_minutes(next_count);
                                self.db
                                    .update_queue_item_retry(
                                        item.id,
                                        "Queued",
                                        next_count,
                                        now + chrono::Duration::minutes(minutes),
                                    )
                                    .await?;
                                tracing::debug!(
                                    "process_download_queue: dispatched '{}' to client \
                                     but content path not yet available — retry {}/{} in {} min",
                                    item.media_name,
                                    next_count,
                                    MAX_DISPATCH_RETRIES,
                                    minutes,
                                );
                            }
                        }
                    }
                    Ok(_) => {
                        // Empty id means no-client mode: the item can never complete
                        // without a client, so leave it Queued and let it dispatch
                        // automatically once one is configured.
                        tracing::debug!(
                            "process_download_queue: no downloader configured — \
                             '{}' staying Queued",
                            item.media_name,
                        );
                    }
                    Err(e) => {
                        // add_download failed (client unreachable, auth, etc.) — back off.
                        let next_count = item.retry_count + 1;

                        if next_count >= MAX_DISPATCH_RETRIES {
                            tracing::warn!(
                                "process_download_queue: max dispatch retries ({}) \
                                 reached for '{}' — marking Failed: {}",
                                MAX_DISPATCH_RETRIES,
                                item.media_name,
                                e,
                            );
                            self.db
                                .update_queue_item_status(
                                    item.id,
                                    "Failed",
                                    None,
                                    None,
                                    Some(&format!(
                                        "Download client unreachable after {} retries: {}",
                                        MAX_DISPATCH_RETRIES, e,
                                    )),
                                )
                                .await?;
                        } else {
                            let now = chrono::Utc::now().naive_utc();
                            let minutes = retry_backoff_minutes(next_count);
                            self.db
                                .update_queue_item_retry(
                                    item.id,
                                    "Queued",
                                    next_count,
                                    now + chrono::Duration::minutes(minutes),
                                )
                                .await?;
                            tracing::info!(
                                "process_download_queue: failed to send '{}' to \
                                 download client: {} — retry {}/{} in {} min",
                                item.media_name,
                                e,
                                next_count,
                                MAX_DISPATCH_RETRIES,
                                minutes,
                            );
                        }
                    }
                }
            }
        }

        // Stage 2: poll downloading + organizing items. Organizing items are
        // included because they too are waiting for the content path to appear.
        let mut items = self.db.get_downloading_items().await?;
        items.extend(self.db.get_organizing_items().await?);

        const MAX_PATH_RETRIES: i32 = 5;

        for item in &items {
            // Items just dispatched can't have content available this cycle.
            if dispatched.contains(&item.id) {
                continue;
            }

            let content_path = if let Some(ref hash) = item.downloader_id {
                let download_mgr = self.downloader.read().await;
                download_mgr.get_download_content_path(hash).await.ok() // Result<String> → Option<String>; Err → None = torrent missing/errored
            } else {
                continue;
            };

            if let Some(ref path) = content_path {
                if path.is_empty() {
                    continue;
                }

                if tokio::fs::metadata(path).await.is_ok() {
                    let _ = self.db.reset_queue_item_retry(item.id).await;
                    if let Err(e) = self.finalize_download(item, path).await {
                        tracing::warn!(
                            "process_download_queue: finalize_download failed for {}: {}",
                            item.media_name,
                            e
                        );
                    }
                } else if item.status == "Downloading" {
                    // Sample progress once: it drives both the completion check and
                    // no-progress detection.
                    let progress = {
                        let download_mgr = self.downloader.read().await;
                        download_mgr
                            .get_download_progress(item.downloader_id.as_deref().unwrap_or(""))
                            .await
                            .ok()
                            .flatten()
                    };

                    if progress.is_some_and(|p| p >= 1.0) {
                        // Complete but not on disk yet (e.g. rename delay) —
                        // transition to Organizing so the retry mechanism handles it.
                        tracing::debug!(
                            "process_download_queue: content path '{}' for '{}' not on disk, \
                             transitioning to Organizing (retry 1/5)",
                            path,
                            item.media_name
                        );
                        let now = chrono::Utc::now().naive_utc();
                        let next_retry = now + chrono::Duration::minutes(1);
                        self.db
                            .update_queue_item_status(
                                item.id,
                                "Organizing",
                                item.downloader_id.as_deref(),
                                None,
                                None,
                            )
                            .await?;
                        self.db
                            .update_queue_item_retry(item.id, "Organizing", 1, next_retry)
                            .await?;
                    } else {
                        trace!(
                            "process_download_queue: content path '{}' for '{}' not on disk \
                             and download not yet complete — staying Downloading",
                            path, item.media_name
                        );
                        self.track_no_progress(item, progress).await?;
                    }
                } else {
                    let next_count = item.retry_count + 1;
                    if next_count >= MAX_PATH_RETRIES {
                        tracing::warn!(
                            "process_download_queue: max retries ({}) reached for '{}' — marking Failed",
                            MAX_PATH_RETRIES,
                            item.media_name
                        );
                        self.db
                            .update_queue_item_status(
                                item.id,
                                "Failed",
                                None,
                                None,
                                Some(&format!(
                                    "Content path not found after {} retries: {}",
                                    MAX_PATH_RETRIES, path
                                )),
                            )
                            .await?;
                    } else {
                        let now = chrono::Utc::now().naive_utc();
                        let minutes = retry_backoff_minutes(next_count);
                        self.db
                            .update_queue_item_retry(
                                item.id,
                                "Organizing",
                                next_count,
                                now + chrono::Duration::minutes(minutes),
                            )
                            .await?;
                    }
                }
            } else if item.status == "Downloading" || item.status == "Organizing" {
                // No content path: the hash wasn't found in any downloader plugin
                // (removed from client, network error, …). Re-queue with retry,
                // then mark Failed. Organizing is included as defense-in-depth
                // against a crash leaving a stale Organizing item.
                let next_count = item.retry_count + 1;
                if next_count >= MAX_PATH_RETRIES {
                    tracing::warn!(
                        "process_download_queue: max retries ({}) reached for '{}' \
                         after download was removed from client — marking Failed",
                        MAX_PATH_RETRIES,
                        item.media_name
                    );
                    self.db
                        .update_queue_item_status(
                            item.id,
                            "Failed",
                            None,
                            None,
                            Some(&format!(
                                "Download not found in client after {} retries",
                                MAX_PATH_RETRIES
                            )),
                        )
                        .await?;
                } else {
                    tracing::debug!(
                        "process_download_queue: download '{}' (hash={:?}) removed \
                         from client — re-queueing (retry {}/{})",
                        item.media_name,
                        item.downloader_id,
                        next_count,
                        MAX_PATH_RETRIES
                    );
                    let now = chrono::Utc::now().naive_utc();
                    let minutes = retry_backoff_minutes(next_count);
                    self.db
                        .update_queue_item_status(item.id, "Queued", None, None, None)
                        .await?;
                    self.db.reset_progress_tracking(item.id).await?;
                    self.db
                        .update_queue_item_retry(
                            item.id,
                            "Queued",
                            next_count,
                            now + chrono::Duration::minutes(minutes),
                        )
                        .await?;
                }
            }
        }

        Ok(())
    }

    /// Track a `Downloading` item's progress and react to a stall.
    ///
    /// Progress that advances (or the first sample) resets the window. A window
    /// that exceeds the threshold marks the item as a warning; automatic items
    /// additionally have the stalled release removed and re-searched.
    async fn track_no_progress(
        &self,
        item: &DownloadQueueItem,
        progress: Option<f32>,
    ) -> Result<()> {
        // Without a progress sample the client can't be monitored for stalls.
        let Some(progress) = progress else {
            return Ok(());
        };
        let now = chrono::Utc::now().naive_utc();

        let has_window = item.last_progress.is_some() && item.no_progress_since.is_some();
        let advanced = !has_window
            || item
                .last_progress
                .is_some_and(|last| (progress - last).abs() > PROGRESS_EPSILON);

        if advanced {
            self.db
                .record_download_progress(item.id, progress, now)
                .await?;
            return Ok(());
        }

        let Some(elapsed_minutes) = no_progress_minutes(item.no_progress_since, now) else {
            return Ok(());
        };

        // Only ask about a hard failure once the download is already stalled: a
        // client-reported failure always presents as frozen progress, so this keeps
        // the probe off the hot path for healthy downloads. It takes precedence over
        // the stall handling below — a failed release is never auto-resolved.
        let failure = {
            let download_mgr = self.downloader.read().await;
            download_mgr
                .get_download_failure(item.downloader_id.as_deref().unwrap_or(""))
                .await
        };
        if let Some(reason) = failure {
            tracing::warn!(
                "process_download_queue: '{}' (id={}) reported failed by the download \
                 client: {}",
                item.media_name,
                item.id,
                reason
            );
            self.db
                .update_queue_item_status(item.id, "Failed", None, None, Some(&reason))
                .await?;
            return Ok(());
        }

        // Manually chosen releases only get the warning badge — never auto-removed.
        // Keyed on `is_manual`, not `is_user_requested`: a user-initiated auto-search
        // is still eligible for autoresolve.
        if item.is_manual {
            tracing::debug!(
                "process_download_queue: '{}' (id={}) has made no progress for {} min",
                item.media_name,
                item.id,
                elapsed_minutes
            );
            return Ok(());
        }

        // A series-level download whose episodes are not resolved cannot be scoped
        // to a re-search; leave it as a warning.
        let Some(targets) = no_progress_targets(
            &item.series_id,
            item.season.as_deref(),
            item.episode,
            item.episode_end,
            item.episode_intentions.as_deref(),
        ) else {
            tracing::debug!(
                "process_download_queue: '{}' (id={}) stalled but has no resolvable \
                 episode to re-search",
                item.media_name,
                item.id
            );
            return Ok(());
        };

        let rejection_target = RejectionTarget {
            series_id: Some(targets.series_id.as_str()),
            season: targets.season.parse::<i32>().ok(),
            episode: targets.episodes.first().copied(),
        };

        // Attempt budget, independent of the rejection list and cleared when a
        // download for the episode succeeds.
        let attempted = self
            .db
            .get_autoresolve_attempts(
                rejection_target,
                AUTORESOLVE_BUDGET_HOURS,
                AUTORESOLVE_HIT_COOLDOWN_HOURS,
            )
            .await
            .unwrap_or(0);
        if attempted >= MAX_AUTORESOLVE_ATTEMPTS {
            tracing::warn!(
                "process_download_queue: '{}' (id={}) exhausted {} alternative \
                 attempt(s) — marking Failed",
                item.media_name,
                item.id,
                MAX_AUTORESOLVE_ATTEMPTS
            );
            self.db
                .update_queue_item_status(
                    item.id,
                    "Failed",
                    None,
                    None,
                    Some(&format!(
                        "No progress after {} alternative attempt(s)",
                        MAX_AUTORESOLVE_ATTEMPTS
                    )),
                )
                .await?;
            // Hold the budget until the exhaustion cooldown also elapses.
            let _ = self.db.mark_autoresolve_exhausted(rejection_target).await;

            // Exclude this release too, so the retry after the cooldown picks a
            // different one instead of the release that just failed.
            let reason = format!("No progress after {} min", elapsed_minutes);
            let expires_at =
                chrono::Utc::now().naive_utc() + chrono::Duration::hours(REJECTION_HOURS);
            let _ = self
                .db
                .reject_download(
                    &item.media_link,
                    item.download_id.as_deref(),
                    &reason,
                    rejection_target,
                    expires_at,
                )
                .await;
            return Ok(());
        }

        self.autoresolve_no_progress(item, &targets, rejection_target, elapsed_minutes)
            .await
    }

    /// Remove a stalled automatic download (with its partial data) and re-search
    /// an alternative release for its episode(s).
    async fn autoresolve_no_progress(
        &self,
        item: &DownloadQueueItem,
        targets: &NoProgressTargets,
        rejection_target: RejectionTarget<'_>,
        elapsed_minutes: i64,
    ) -> Result<()> {
        let Some(hash) = item.downloader_id.as_deref().filter(|h| !h.is_empty()) else {
            return Ok(());
        };

        tracing::warn!(
            "process_download_queue: removing '{}' (id={}) after {} min of no progress; \
             searching for an alternative release",
            item.media_name,
            item.id,
            elapsed_minutes
        );

        // Drop the stalled transfer and its partial data.
        {
            let download_mgr = self.downloader.read().await;
            if let Err(e) = download_mgr
                .delete_download(hash, true, item.client_id.as_deref())
                .await
            {
                tracing::warn!(
                    "autoresolve: failed to remove stalled download '{}' from client: {}",
                    item.media_name,
                    e
                );
            }
        }

        // Exclude this release until the rejection window elapses, so the
        // re-search picks something other than the release that just stalled.
        let reason = format!("No progress after {} min", elapsed_minutes);
        let expires_at = chrono::Utc::now().naive_utc() + chrono::Duration::hours(REJECTION_HOURS);
        if let Err(e) = self
            .db
            .reject_download(
                &item.media_link,
                item.download_id.as_deref(),
                &reason,
                rejection_target,
                expires_at,
            )
            .await
        {
            tracing::warn!("autoresolve: failed to record rejection: {}", e);
        }

        if let Err(e) = self
            .db
            .increment_autoresolve_attempts(
                rejection_target,
                AUTORESOLVE_BUDGET_HOURS,
                AUTORESOLVE_HIT_COOLDOWN_HOURS,
            )
            .await
        {
            tracing::warn!("autoresolve: failed to record attempt: {}", e);
        }

        let _ = self.db.remove_from_download_queue(item.id).await;

        // Source queries must not block the queue pass.
        let organizer = self.clone();
        let series_id = targets.series_id.clone();
        let season = targets.season.clone();
        let episodes = targets.episodes.clone();
        tokio::spawn(async move {
            if let Err(e) = organizer
                .auto_search_missing(&series_id, &season, &episodes)
                .await
            {
                tracing::warn!(
                    "autoresolve: re-search failed for series={} season={} episodes={:?}: {}",
                    series_id,
                    season,
                    episodes,
                    e
                );
            }
        });

        Ok(())
    }
}

/// The re-search target for a no-progress item.
pub(crate) struct NoProgressTargets {
    pub(crate) series_id: String,
    pub(crate) season: String,
    pub(crate) episodes: Vec<i32>,
}

/// Episodes to re-search for a no-progress item.
///
/// Prefers the immutable intentions recorded at queue time (a pack expands to its
/// covered range, so re-searching those episodes can find another pack). Series-level
/// downloads whose episodes are not yet resolved return `None`.
pub(crate) fn no_progress_targets(
    series_id: &str,
    season: Option<&str>,
    episode: Option<i32>,
    episode_end: Option<i32>,
    episode_intentions: Option<&str>,
) -> Option<NoProgressTargets> {
    if series_id.is_empty() {
        return None;
    }
    let season = season.filter(|s| !s.is_empty())?.to_string();

    if let Some(json) = episode_intentions
        && let Ok(intentions) =
            serde_json::from_str::<Vec<jumbie_shared::types::EpisodeIntention>>(json)
    {
        let mut episodes: Vec<i32> = intentions.iter().map(|i| i.episode_num).collect();
        episodes.sort_unstable();
        episodes.dedup();
        if !episodes.is_empty() {
            return Some(NoProgressTargets {
                series_id: series_id.to_string(),
                season,
                episodes,
            });
        }
    }

    let start = episode?;
    let end = episode_end.unwrap_or(start);
    Some(NoProgressTargets {
        series_id: series_id.to_string(),
        season,
        episodes: (start..=end).collect(),
    })
}

/// Exponential backoff for download queue retries: 1, 2, 4, 8, 16... minutes.
/// Caps at 2^10 = 1024 minutes (~17 hours) to prevent unbounded growth.
fn retry_backoff_minutes(retry_count: i32) -> i64 {
    1i64 << retry_count.min(10) as u32
}

impl ContentOrganizer {
    /// Run the download lifecycle pipeline.
    /// Stages: download queue → orphan adoption → organize completed → retry queue.
    pub async fn process_downloads(&self) -> Result<()> {
        tracing::trace!("process_downloads: starting lifecycle run");

        if let Err(e) = self.process_download_queue().await {
            tracing::warn!("process_downloads: process_download_queue failed: {}", e);
        }

        // Adopt orphans before organizing so newly adopted files are picked up.
        if let Err(e) = self.adopt_orphans().await {
            tracing::warn!("process_downloads: adopt_orphans failed: {}", e);
        }

        if let Err(e) = self.organize_completed().await {
            tracing::warn!("process_downloads: organize_completed failed: {}", e);
        }

        if let Err(e) = self.process_retry_queue().await {
            tracing::warn!("process_downloads: process_retry_queue failed: {}", e);
        }

        Ok(())
    }
}

#[cfg(test)]
mod no_progress_tests {
    use super::no_progress_targets;
    use jumbie_shared::types::EpisodeIntention;

    fn targets(
        series_id: &str,
        season: Option<&str>,
        episode: Option<i32>,
        episode_end: Option<i32>,
        episode_intentions: Option<&str>,
    ) -> Option<(String, String, Vec<i32>)> {
        no_progress_targets(series_id, season, episode, episode_end, episode_intentions)
            .map(|t| (t.series_id, t.season, t.episodes))
    }

    fn intentions(episodes: &[i32]) -> String {
        let list: Vec<EpisodeIntention> = episodes
            .iter()
            .map(|&ep| EpisodeIntention {
                episode_num: ep,
                source_episode_num: ep,
                episode_id: format!("ep{ep}"),
                score: 0,
                keep: true,
            })
            .collect();
        serde_json::to_string(&list).unwrap()
    }

    #[test]
    fn uses_queue_intentions_when_present() {
        let json = intentions(&[4, 2, 3]);
        assert_eq!(
            targets("series", Some("01"), Some(2), Some(4), Some(&json)),
            Some(("series".to_string(), "01".to_string(), vec![2, 3, 4]))
        );
    }

    #[test]
    fn falls_back_to_episode_range() {
        assert_eq!(
            targets("series", Some("02"), Some(5), None, None),
            Some(("series".to_string(), "02".to_string(), vec![5]))
        );
    }

    #[test]
    fn expands_multi_episode_range() {
        assert_eq!(
            targets("series", Some("01"), Some(3), Some(5), None),
            Some(("series".to_string(), "01".to_string(), vec![3, 4, 5]))
        );
    }

    #[test]
    fn none_without_series_or_season() {
        assert_eq!(targets("", Some("01"), Some(1), None, None), None);
        assert_eq!(targets("series", None, Some(1), None, None), None);
        assert_eq!(targets("series", Some(""), Some(1), None, None), None);
    }

    #[test]
    fn none_when_episode_unresolved() {
        assert_eq!(targets("series", Some("01"), None, None, None), None);
    }

    #[test]
    fn falls_back_when_intentions_unparseable() {
        assert_eq!(
            targets("series", Some("01"), Some(7), None, Some("not-json")),
            Some(("series".to_string(), "01".to_string(), vec![7]))
        );
    }
}
