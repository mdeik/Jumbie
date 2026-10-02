use crate::db::DbManager;
use crate::models::activity::ActivityEvent;
use anyhow::{Context, Result};

use notify::{
    Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

// File lifecycle: Pending → Complete → Organized. Partial is an early
// fingerprint of an in-progress write, so a later Complete can skip re-analysis
// when the hash matches. Missing is terminal (a previously Organized file
// vanished from disk) and is set by the organizer, not by the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    Pending,
    Partial,
    Complete,
    Organized,
    Missing,
}

impl std::fmt::Display for FileState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileState::Pending => write!(f, "pending"),
            FileState::Partial => write!(f, "partial"),
            FileState::Complete => write!(f, "complete"),
            FileState::Organized => write!(f, "organized"),
            FileState::Missing => write!(f, "missing"),
        }
    }
}

// Owns the filesystem watcher (held so notify doesn't stop watching on drop).
// Shutdown is signalled via the global application CancellationToken passed to
// `start_watching` — the SSoT for all shutdown signals.
pub struct FileStateManager {
    db: Arc<DbManager>,
    // Held to keep the watcher alive — dropping it stops all filesystem notifications.
    _watcher: Option<RecommendedWatcher>,
}

impl FileStateManager {
    pub fn new(db: Arc<DbManager>) -> Self {
        Self { db, _watcher: None }
    }

    // Watches directories recursively. An mpsc channel bridges notify's
    // synchronous callback thread into the async runtime (blocking_send is safe
    // there), and a 2s poll interval makes notify fall back to polling where
    // inotify/FSEvents may miss short-lived events. Only Create and Modify are
    // acted on; Remove is logged because Missing is set by the organizer.
    ///
    /// The token is checked inside the event loop so file-change processing can't
    /// delay the global shutdown signal.
    pub fn start_watching(
        &mut self,
        paths: Vec<PathBuf>,
        shutdown_token: Option<CancellationToken>,
    ) -> Result<()> {
        let (tx, mut rx) = mpsc::channel(100);
        let db = self.db.clone();

        // Fallback poll interval — some downloaders write via temporary renames that inotify misses.
        let watcher_config = NotifyConfig::default().with_poll_interval(Duration::from_secs(2));

        let mut watcher = notify::RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    // blocking_send is safe: notify's callback runs on its own thread, not async.
                    let _ = tx.blocking_send(event);
                }
            },
            watcher_config,
        )?;

        for path in &paths {
            if path.exists() {
                watcher.watch(path, RecursiveMode::Recursive)?;
                info!("Started watching: '{}'", path.display());
            } else {
                warn!("Watch path does not exist: '{}'", path.display());
            }
        }

        // Wake on either the global shutdown token (SSoT) or a filesystem event.
        tokio::spawn(async move {
            loop {
                // Check before awaiting rx.recv() so an already-signalled shutdown doesn't block.
                if let Some(ref token) = shutdown_token
                    && token.is_cancelled()
                {
                    info!("File watcher event loop stopping (global shutdown)");
                    break;
                }

                tokio::select! {
                    maybe_event = rx.recv() => {
                        match maybe_event {
                            Some(event) => {
                                // Checked between events: if signalled, stop without draining the channel.
                                if let Some(ref token) = shutdown_token
                                    && token.is_cancelled() {
                                        info!("File watcher event loop stopping (global shutdown, mid-event)");
                                        break;
                                    }
                                match event.kind {
                                    EventKind::Create(_) | EventKind::Modify(_) => {
                                        for path in event.paths {
                                            if path.is_file()
                                                && let Err(e) = Self::handle_file_change(&db, &path).await {
                                                    error!("Error handling file change for '{}': {}", path.display(), e);
                                                }
                                        }
                                    },
                                    EventKind::Remove(_) => {
                                        for path in event.paths {
                                             // Removal alone can't mark Missing — it could be a
                                             // temporary delete+recreate (atomic save). Only the
                                             // organizer finalizes Missing after a move fails.
                                             debug!("File removed: '{}'", path.display());
                                        }
                                    },
                                    _ => {}
                                }
                            }
                            None => break, // Channel closed
                        }
                    }
                }
            }
        });

        self._watcher = Some(watcher);
        Ok(())
    }

    // Determine the FileState for a changed file and fingerprint it.
    //
    // Partial-file patterns (.part, .!qB, .tmp) map to Partial rather than being
    // ignored, so the pre-rename fingerprint can be matched post-rename by
    // inode/dev without re-hashing. Complete requires a stable size across a 2s
    // window (a heuristic for "steady state" that defers instead of fingerprinting
    // mid-write; a very slow download can still fool it).
    async fn handle_file_change(db: &Arc<DbManager>, path: &Path) -> Result<()> {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if name.ends_with(".part") || name.ends_with(".!qB") || name.ends_with(".tmp") {
            Self::fingerprint_file(db, path, FileState::Partial).await?;
        } else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if jumbie_shared::media_format::is_video_ext(ext) {
                let initial = match tokio::fs::metadata(path).await {
                    Ok(m) if m.len() > 0 => m.len(),
                    _ => {
                        debug!(
                            "File '{}' is empty or inaccessible; deferring",
                            path.display()
                        );
                        return Ok(());
                    }
                };

                // Wait 2s: unchanged size ⇒ stable enough to fingerprint as Complete (best-effort).
                tokio::time::sleep(Duration::from_secs(2)).await;

                match tokio::fs::metadata(path).await {
                    Ok(m) if m.len() == initial => {
                        Self::fingerprint_file(db, path, FileState::Complete).await?;
                    }
                    _ => {
                        trace!(
                            "File '{}' size changed or disappeared; deferring",
                            path.display()
                        );
                    }
                }
            }
        }
        Ok(())
    }

    // Record a file's metadata + identity into the DB.
    //
    // (ino, dev) is the kernel's stable file identity and survives renames within a
    // filesystem, so a pre-rename Partial entry can be correlated with a post-rename
    // Complete entry. Complete/Organized media files route through
    // `update_file_fingerprint` (SSoT, handles content-hash dedup); all other cases
    // store an identity-fallback hash (inode-size-mtime).
    pub async fn fingerprint_file(
        db: &Arc<DbManager>,
        path: &Path,
        state: FileState,
    ) -> Result<()> {
        let metadata = tokio::fs::metadata(path)
            .await
            .context("Failed to get metadata")?;
        let size = metadata.len();
        let (inode, dev, mtime) = crate::platform::file_identity(&metadata);

        let path_str = path.to_str().unwrap_or("");
        let media_info = if state == FileState::Complete || state == FileState::Organized {
            let is_media = path
                .extension()
                .and_then(|e| e.to_str())
                .map(jumbie_shared::media_format::is_video_ext)
                .unwrap_or(false);

            if is_media {
                // SSoT: update_file_fingerprint dedups by content hash (skips ffprobe on copies).
                let (_hash_val, _hash, mi) =
                    db.update_file_fingerprint(path, &state.to_string()).await;
                mi
            } else {
                // Non-media files: identity-fallback hash only, no ffprobe.
                let quick_hash = format!("{}-{}-{}", inode, size, mtime);
                db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                    path: path_str,
                    inode,
                    dev,
                    size,
                    mtime,
                    quick_hash: &quick_hash,
                    state: &state.to_string(),
                    media_info: None,
                })
                .await?;
                None
            }
        } else {
            // Non-terminal states (Pending, Partial, Missing): identity-fallback only.
            let quick_hash = format!("{}-{}-{}", inode, size, mtime);
            db.save_fingerprint(crate::db::files::SaveFingerprintParams {
                path: path_str,
                inode,
                dev,
                size,
                mtime,
                quick_hash: &quick_hash,
                state: &state.to_string(),
                media_info: None,
            })
            .await?;
            None
        };

        if media_info.is_some() {
            let _ = sqlx::query(
                "INSERT INTO file_event_log (event_type, source_path, status) \
                 VALUES ('analyze', ?, 'completed')",
            )
            .bind(path.to_string_lossy().to_string())
            .execute(db.get_pool())
            .await;

            let _ = db
                .record_activity(ActivityEvent {
                    event_type: jumbie_shared::types::ActivityType::Analyze,
                    series_title: String::new(),
                    season: None,
                    episode: None,
                    episode_end: None,
                    title: None,
                    details: Some(path.to_string_lossy().to_string()),
                    status: "Success".to_string(),
                })
                .await;
        }

        trace!(
            "Fingerprinted '{}' as {}",
            path.display(),
            state.to_string()
        );
        Ok(())
    }
    // Crash recovery: reconcile in-flight move operations from the journal.
    //
    //   1. Dest exists, src gone    → move succeeded → mark completed.
    //   2. Both exist               → partial move (copy ok, delete failed) → warn.
    //   3. Dest missing, src exists → rolled back / never ran → mark completed.
    //   4. Both missing             → temp cleaned up externally → mark completed.
    //
    // Cases 3 and 4 forfeit the move rather than retrying: the failure cause is
    // unknown (disk space, permissions) and retrying could lose data. The watcher
    // rediscovers any remaining files on next startup.
    pub async fn recover_from_crash(&self) -> Result<()> {
        info!("Checking for incomplete file operations...");
        let pending = self.db.get_pending_journal_entries().await?;

        for (id, event_type, src, dest) in pending {
            if event_type == "move"
                && let Some(destination_path) = dest
            {
                let src_p = PathBuf::from(&src);
                let dest_p = PathBuf::from(&destination_path);

                if tokio::fs::try_exists(&dest_p).await.unwrap_or(false) {
                    if !tokio::fs::try_exists(&src_p).await.unwrap_or(false) {
                        info!(
                            "Recovering: Move '{}' -> '{}' appears complete.",
                            src, destination_path
                        );
                        self.db.complete_journal_entry(id).await?;
                    } else {
                        warn!(
                            "Recovering: Both source and dest exist for move '{}' -> '{}'. Manual intervention required.",
                            src, destination_path
                        );
                    }
                } else if tokio::fs::try_exists(&src_p).await.unwrap_or(false) {
                    info!(
                        "Recovering: Move '{}' -> '{}' did not complete. Source remains.",
                        src, destination_path
                    );
                    // Completed even though the move didn't happen — safer than retrying
                    // blindly; the watcher re-processes the file on next startup.
                    self.db.complete_journal_entry(id).await?;
                } else {
                    warn!(
                        "Recovering: Both source and dest missing for move '{}' -> '{}'.",
                        src, destination_path
                    );
                    // Neither exists: src was likely a temp/partial already cleaned up;
                    // nothing to recover, just close the journal entry.
                    self.db.complete_journal_entry(id).await?;
                }
            }
        }
        Ok(())
    }

    // Write-ahead journal entry for a move, written before the filesystem move so
    // crash recovery can identify the in-flight move.
    pub async fn log_move_start(&self, src: &Path, dest: &Path) -> Result<i64> {
        self.db
            .log_journal_start(
                "move",
                src.to_str().unwrap_or(""),
                Some(dest.to_str().unwrap_or("")),
            )
            .await
    }

    // Commit a journal entry after the move succeeds; if never called (crash),
    // recover_from_crash reconciles it.
    pub async fn log_move_end(&self, id: i64) -> Result<()> {
        self.db.complete_journal_entry(id).await
    }
}
