use crate::config_manager::ConfigManager;
use crate::db::DbManager;
use crate::plugins::downloaders::DownloadManager;
use crate::plugins::notifiers::NotifierManager;

use jumbie_shared::types::EpisodeStatus;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

pub mod modifying_series;
pub mod router;

pub use router::{RouterConfig, create_router};

pub mod progress;
pub use progress::{
    ActiveOperation, BatchMoveProgress, OperationType, ProgressTracker, ProgressUpdater,
    ReorgGuard, TaskHandle,
};

pub struct BanInfo {
    pub fail_count: u32,
    pub ban_count: u32,
    // Compared against `Instant::now()` on every authenticated request; the DB
    // holds the canonical DateTime, this is a fast-path shortcut recomputed from
    // DB timestamps on restart.
    pub banned_until: Option<Instant>,
    // Needed for ban_count_reset_days: the reset decision requires knowing when
    // the most recent ban occurred.
    pub banned_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Last time this entry was touched by an auth attempt. Used by the state
    /// reaper to evict stale, non-banned entries so the map stays bounded.
    pub last_seen: Instant,
}

impl BanInfo {
    /// Whether this entry is an *active* ban at `now`.
    ///
    /// SSoT for the expiry convention: `banned_until == None` is a permanent
    /// (always-active) ban; a tracked-but-not-yet-banned entry carries a past
    /// sentinel instant so it reads as *not* banned. All ban checks must go
    /// through this method so the convention cannot drift.
    pub fn is_banned(&self, now: Instant) -> bool {
        self.banned_until.map(|t| t > now).unwrap_or(true)
    }

    /// Whether this entry is a permanent ban (`banned_until == None`).
    pub fn is_permanent(&self) -> bool {
        self.banned_until.is_none()
    }

    /// A freshly tracked, not-yet-banned entry.
    ///
    /// `banned_until` is `Some(now)` rather than `None` because `None` means a
    /// *permanent* ban (see [`BanInfo::is_banned`]); using it here would make
    /// every tracked IP appear permanently banned on the next request. Only a
    /// real ban or a manual/DB permanent ban sets a future instant (or `None`).
    /// Construct tracked entries only through this method.
    pub fn tracked(now: Instant) -> Self {
        Self {
            fail_count: 0,
            ban_count: 0,
            banned_until: Some(now),
            banned_at: None,
            last_seen: now,
        }
    }
}

pub struct AppState {
    // Arc<ConfigManager>: it merges the 5 startup-path fields from config.toml
    // with 6 DB-backed sections into one in-memory Config, so handlers always read
    // the live view via `state.cfg.read().await` rather than stale Rust defaults;
    // `save()` persists atomically to both homes.
    pub cfg: Arc<ConfigManager>,
    // No extra lock: DbManager already pools connections for concurrent access;
    // wrapping it in another lock would serialize all DB operations.
    pub db: Arc<DbManager>,
    // Optional (some deployments have no download-client credentials); the inner
    // RwLock is needed because queue state changes as items complete or vanish.
    pub downloader: Option<Arc<RwLock<DownloadManager>>>,
    // Same rationale as downloader: notifiers are optional and their state
    // changes when test notifications are sent.
    pub notifications: Option<Arc<RwLock<NotifierManager>>>,
    pub organizer: Option<crate::organizer::ContentOrganizer>,
    // Mutex, not RwLock: ban checks are read-modify-write, and tokio RwLock has
    // no read-to-write upgrade.
    pub ban_list: Arc<Mutex<HashMap<IpAddr, BanInfo>>>,
    // A simple String: resolved at startup from JUMBIE_LOGS_DIR → config.toml →
    // "logs", used to locate log files on disk for the logs API.
    pub logs_dir: String,
    pub log_level: String,
    // Guards against concurrent reorganization operations; AtomicBool gives the
    // needed test-and-set semantics.
    pub is_reorganizing: Arc<std::sync::atomic::AtomicBool>,
    // RwLock: reads (did a rename fail recently?) far outnumber writes.
    pub failed_renames: Arc<RwLock<HashMap<String, u64>>>,
    pub plugin_manager: Arc<RwLock<crate::plugins::PluginManager>>,
    pub scan_queue: Arc<crate::scan_queue::ScanQueue>,
    /// Which series are currently being reorganized (key: series_id). Used by the
    /// rename queue endpoint to mark items `processing: true` for the frontend.
    pub processing_renames: Arc<RwLock<std::collections::HashSet<String>>>,
    /// Priority-aware queue for metadata fetches: 3 tiers (P0=manual,
    /// P1=rename queue, P2=background), single-permit concurrency, and
    /// (series_id, instance_id) dedup for P1/P2.
    pub metadata_queue: Arc<crate::metadata_queue::MetadataQueue>,
    /// Serialized, deduplicated auto-search dispatch shared by all auto-search
    /// paths (wanted-task, displaced, add-series, season button).
    pub search_queue: Arc<crate::search_queue::SearchQueue>,
    /// Whether the rename queue currently has pending items. Set/cleared by
    /// `get_rename_queue` so the lightweight `/api/status` endpoint need not
    /// recompute every series' rename plan on each poll.
    pub rename_queue_has_pending: Arc<RwLock<bool>>,
    /// Wake-up trigger for the auto-apply renames scheduler: set on series
    /// settings changes so it runs immediately instead of waiting out the
    /// 30-second fallback timeout.
    pub rename_queue_trigger: Arc<tokio::sync::Notify>,
    /// Global shutdown signal (SIGTERM / Ctrl+C) for the whole application,
    /// checked by background operations so they can stop accepting work.
    pub shutdown_token: CancellationToken,
    /// Token cancelling a pending debounced monitor-mode sweep so the timer
    /// resets on each preference change; `None` means none is scheduled. A new
    /// token fires after 5 minutes of inactivity.
    pub monitor_sweep_cancel: Arc<Mutex<Option<CancellationToken>>>,
    /// Token debouncing automatic profile recalculation after config saves; a
    /// new save resets it, manual recalculate cancels it and runs immediately.
    pub recalc_cancel: Arc<Mutex<Option<CancellationToken>>>,
    /// Per-IP rate-limiter state. Same dual-storage rationale as ban_list:
    /// in-memory HashMap for O(1) request-time checks, no DB persistence since
    /// rate-limit state is ephemeral (resets on restart, which is acceptable).
    pub rate_limiter: crate::middleware::rate_limit::RateLimiter,
    /// Cached auth material (admin password hash + successful Argon2 results) so
    /// the middleware avoids the DB and Argon2 on every request. Invalidated on
    /// password change.
    pub auth_cache: Arc<crate::middleware::auth_cache::AuthCache>,
    /// In-memory parsed log entries: seeded at startup from existing log files,
    /// then appended to by a tracing layer. The logs API reads this buffer, so
    /// normal operation involves zero disk I/O.
    pub log_buffer: jumbie_shared::types::LogBuffer,
    /// In-progress batch move operations, keyed by task_id (UUID); each entry
    /// tracks total paths and completed count.
    pub progress_tracker: Arc<ProgressTracker>,
    /// Per-series modification lock (key: series_id). A series is "locked" while
    /// any operation is modifying its files or DB paths (batch move, path update,
    /// reorganize, delete); other operations consult this set to skip, conflict,
    /// or wait.
    ///
    /// RwLock because reads vastly outnumber writes — every scan, organize, and
    /// status check queries the set without modifying it.
    pub modifying_series: Arc<RwLock<HashSet<String>>>,
    /// Per-series rename plan cache keyed on a fingerprint of the plan's inputs:
    /// `get_rename_queue` reuses a series' plan until its inputs change, with no
    /// invalidation call required — see `plan_cache.rs`.
    pub rename_plan_cache: Arc<crate::file_manager::RenamePlanCache>,
}

impl AppState {
    /// Snapshot the organization config under a read lock, releasing it before
    /// any await. SSoT accessor so handlers don't repeat the read→clone→drop
    /// dance.
    pub async fn org_config(&self) -> jumbie_shared::config::organization::OrganizationConfig {
        self.cfg.read().await.organization.clone()
    }

    /// SSoT: effective absolute-numbering mode for a mapping — series tristate
    /// override falling back to the global default (`None` = "use global").
    ///
    /// Use this everywhere a numbering-mode decision is made (DB filters,
    /// metadata capability selection, payload flags) so it cannot diverge from
    /// the filename-format resolution (should_use_absolute_numbering).
    pub async fn effective_absolute_numbering(
        &self,
        mapping: &jumbie_shared::types::MappingRule,
    ) -> bool {
        let config = self.cfg.read().await;
        mapping
            .settings
            .effective_absolute_numbering(config.general.absolute_numbering)
    }
}

/// Helper trait to pad numeric strings with leading zeros.
///
/// Templates like `{episode:02}` need zero-padding but values arrive from the
/// DB as strings ("1" or "01"); parsing to i32 then re-formatting is uniform.
/// Non-numeric values (e.g. "Specials") fall back to the raw string.
pub trait PadNumeric {
    fn pad_numeric(&self, width: usize) -> String;
}

impl PadNumeric for String {
    fn pad_numeric(&self, width: usize) -> String {
        if let Ok(n) = self.parse::<i32>() {
            format!("{:0width$}", n, width = width)
        } else {
            self.clone()
        }
    }
}

/// Proactively submit files missing media info to the scan queue so the rename
/// queue never produces plans with empty media-info template variables.
///
/// Paths whose `media_info_scan_failed = 1` (corrupt file, unsupported codec)
/// are **allowed through** into the rename plan with empty media-info variables
/// rather than excluded forever; the rest are submitted and added to
/// `skip_paths` so they're excluded this cycle but ready on the next poll.
///
/// Returns the updated `skip_paths` set.
pub async fn proactively_scan_missing_media_info(
    state: &std::sync::Arc<AppState>,
    episodes_by_title: &std::collections::HashMap<String, Vec<crate::db::EpisodeDetailRow>>,
    mut skip_paths: std::collections::HashSet<std::path::PathBuf>,
) -> std::collections::HashSet<std::path::PathBuf> {
    use std::path::PathBuf;

    // Skip entirely when media-info scanning is disabled or ffprobe is
    // unavailable: otherwise files would be submitted, fail to extract, and be
    // falsely marked permanently failed — blocking retries even after ffprobe
    // is installed.
    {
        let cfg = state.cfg.read().await;
        if !cfg.general.media_info_scan_enabled {
            return skip_paths;
        }
    }
    if crate::utils::media_info::ffprobe_version().is_none() {
        return skip_paths;
    }

    // Phase 1: collect unique file paths that are missing media info
    let mut candidates: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for eps in episodes_by_title.values() {
        for ep in eps {
            if let Some(ref fp) = ep.file_path
                && ep.media_info.is_none()
            {
                let path = PathBuf::from(fp);
                if !skip_paths.contains(&path) {
                    candidates.insert(path);
                }
            }
        }
    }

    if candidates.is_empty() {
        return skip_paths;
    }

    // Batch-check which candidates have permanently failed
    let candidate_strs: Vec<String> = candidates
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let failed_paths = match state
        .db
        .get_media_info_scan_failed_paths(&candidate_strs)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                "proactively_scan_missing_media_info: failed to query failed paths: {}",
                e
            );
            // On error, assume none failed — worst case we retry a scan
            // that fails again and gets marked on the callback side.
            std::collections::HashSet::new()
        }
    };

    // Submit non-failed candidates to the scan queue
    let db = state.db.clone();
    let scan_queue = state.scan_queue.clone();
    for path in &candidates {
        let path_str = path.to_string_lossy().to_string();
        if failed_paths.contains(&path_str) {
            // Permanently failed — don't re-scan, but allow through into the
            // rename plan with empty media-info template variables.
            continue;
        }

        skip_paths.insert(path.clone());
        let db_clone = db.clone();
        let path_clone = path.clone();
        scan_queue
            .submit(path.clone(), move || {
                let db = db_clone.clone();
                let p = path_clone.clone();
                async move {
                    // SSoT: scan_file_fingerprint unifies hashing, extraction,
                    // and failure marking into one call.
                    db.scan_file_fingerprint(&p, EpisodeStatus::Organized.as_str())
                        .await;
                }
            })
            .await;
    }

    skip_paths
}

/// Build the shared `skip_paths` set used by all rename queue entry points.
///
/// SSoT for rename-plan exclusions: files currently being scanned, and files
/// pending organization by `organize_completed` (fingerprint state =
/// 'complete', episode status != 'organized'). Excluding the latter prevents
/// the rename queue from racing `organize_completed` and moving files out of
/// the download directory first.
///
/// Call once per rename queue poll cycle, not per-series.
pub async fn build_rename_queue_skip_paths(
    state: &std::sync::Arc<AppState>,
) -> std::collections::HashSet<std::path::PathBuf> {
    let mut skip = state.scan_queue.active_paths_snapshot().await;
    if let Ok(completed) = state.db.get_completed_files().await {
        for row in &completed {
            skip.insert(std::path::PathBuf::from(&row.file_path));
        }
    }
    skip
}

/// Remove a series from all in-memory per-series state after its DB rows are
/// deleted. SSoT for "this series no longer exists" cleanup — every deletion
/// path calls this so a stale failed-rename hash can't keep the sidebar
/// indicator lit and cached plans for deleted series don't accumulate.
pub async fn prune_series_in_memory_state(state: &Arc<AppState>, series_id: &str) {
    state.failed_renames.write().await.remove(series_id);
    state.processing_renames.write().await.remove(series_id);
    state.rename_plan_cache.invalidate(series_id).await;
}

// Detects episodes whose `title` is NULL/empty AND whose `metadata_source` is
// not 'custom'/'cleared', then triggers an async series metadata fetch so
// naming templates referencing {episode_title}, {description}, etc. produce
// complete filenames. Runs inside the rename queue poll and spawns the fetch in
// the background so the caller is never blocked.
//
// SSoT: `metadata_last_synced_at` on the mapping is the single source of truth
// for "when was metadata last fetched" — all paths update and check it.
// `metadata_source` only protects custom/cleared rows during the INSERT; it is
// NOT a gate here.
pub async fn proactively_fetch_missing_episode_metadata(
    state: &std::sync::Arc<AppState>,
    series_id: &str,
    episodes: &[crate::db::EpisodeDetailRow],
    mapping: &jumbie_shared::mapping::MappingRule,
    // Cooldown in minutes from the caller-held config lock, passed in rather
    // than re-read to avoid acquiring the lock per series.
    cooldown_minutes: u64,
) {
    // Any non-empty provider ID qualifies; provider selection is resolved later
    // by `fetch_metadata_for_series`.
    if !mapping
        .settings
        .metadata_ids
        .values()
        .any(|v| !v.is_empty())
    {
        return;
    }

    // Only episodes that CAN accept metadata count:
    //   • title is NULL or empty string (scanner inserts '')
    //   • metadata_source is NOT 'custom' or 'cleared' (user protected)
    let any_needs_metadata = episodes.iter().any(|ep| {
        let title_empty = ep.title.as_deref().is_none_or(|t| t.is_empty());
        let not_user_protected = ep
            .metadata_source
            .as_deref()
            .map(|s| s != "custom" && s != "cleared")
            .unwrap_or(true); // NULL → not user-protected
        title_empty && not_user_protected
    });
    if !any_needs_metadata {
        return;
    }

    // SSoT: `metadata_last_synced_at` — skip if any provider for this series
    // synced within the cooldown; the timestamp is the shared coordination point
    // across all fetch paths.
    let cooldown = chrono::Duration::minutes(cooldown_minutes as i64);
    let now = chrono::Utc::now();
    let recently_synced = mapping
        .settings
        .metadata_last_synced_at
        .values()
        .filter_map(|ts| crate::datetime::parse_utc(ts).ok())
        .any(|dt| now.signed_duration_since(dt.to_chrono_utc()) < cooldown);
    if recently_synced {
        return;
    }

    let series_id = series_id.to_string();
    let state_for_closure = state.clone();
    state
        .metadata_queue
        .submit(
            series_id.clone(),
            crate::metadata_queue::Priority::Normal,
            move || {
                let state = state_for_closure.clone();
                let sid = series_id.clone();
                async move {
                    match crate::api_routes::series::fetch_metadata_for_series(&state, &sid, None)
                        .await
                    {
                        Ok(_) => tracing::debug!("Proactive metadata fetch succeeded for {}", sid),
                        Err(e) => tracing::debug!(
                            "Proactive metadata fetch for {} did not complete: {:?}",
                            sid,
                            e
                        ),
                    }
                    // Wake the auto-apply scheduler so it re-evaluates
                    // rename plans with the now-available episode titles.
                    state.rename_queue_trigger.notify_one();
                }
            },
        )
        .await;
}

#[derive(serde::Serialize)]
pub struct RefreshResult {
    pub series_found: usize,
    pub downloads_found: usize,
    pub total_episodes: usize,
}

// Calendar Handlers

#[derive(Deserialize)]
pub struct CalendarQuery {
    pub start_date: String,
    pub end_date: String,
}

#[derive(Deserialize)]
pub struct CalendarTokenQuery {
    pub token: Option<String>,
}
