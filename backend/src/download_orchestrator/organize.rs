// organize_completed is the final stage of the download lifecycle: it moves files
// whose fingerprint state is "complete" to their destination via organize_file(),
// updates the DB, and cleans up the queue. The `is_in_destination_root` guard skips
// files already in a destination root (e.g. the client saves straight into the library).
//
// Cancellation: a signal between files skips the rest (picked up next cycle); a move
// already in progress completes before the next check.
use anyhow::{Context, Result};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use tracing::{debug, info, trace, warn};

use crate::db::episodes::InsertEpisodeParams;
use crate::file_manager::file_ops::remove_empty_download_dir;
use crate::file_manager::path::PathBuildVars;
use crate::organizer::ContentOrganizer;
use crate::plugins::notifiers::{NotifierContext, NotifierEvent};
use crate::state::{FileState, FileStateManager};
use jumbie_shared::types::EpisodeStatus;
use jumbie_shared::types::MultiTarget;

impl ContentOrganizer {
    pub(crate) fn organize_completed<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // Read organization and general config from DB directly (SSoT).
            let config = self.db_config().await;
            let general_cfg = self.db.get_general_config().await.unwrap_or_default();

            // Configured download roots, resolved once per cycle so the per-download
            // UUID staging folder is culled after each file leaves it.
            let download_roots = self.download_roots().await;

            let files = self.db.get_completed_files().await?;

            // Post-organize re-estimation: the organize pass writes the authoritative
            // source feed date into `release_info`, so estimates must be re-anchored now
            // rather than waiting for the periodic sweep (up to 6 hours).
            let affected_series: std::collections::HashSet<String> =
                files.iter().map(|row| row.series_id.clone()).collect();

            for row in files {
                // A signal skips the remaining files; they are picked up next cycle.
                if crate::task::is_shutdown_requested(&self.shutdown_token, "organize_completed") {
                    break;
                }
                let current_path_str = row.file_path;
                trace!("Processing file: '{}'", current_path_str);
                let episode_id = row.episode_id;
                let download_id = row.download_id;
                let path = PathBuf::from(&current_path_str);

                // Scanner-claimed episode guard: the episode is already 'organized' or
                // 'downloaded' but its file_path differs from this fingerprint's path, so a
                // different file claimed the slot. Either upgrade (replace it when upgrades
                // are enabled and the download has a meaningful score) or clean up the
                // orphaned download, its fingerprint, and its queue entry.
                if (row.episode_status == EpisodeStatus::Organized.as_str()
                    || row.episode_status == EpisodeStatus::Downloaded.as_str())
                    && row.episode_file_path.as_deref() != Some(&current_path_str)
                {
                    // Fetch the queue item once — needed by both paths.
                    let queue_item = self
                        .db
                        .get_queue_item_by_episode_id(&episode_id)
                        .await
                        .ok()
                        .flatten();

                    // SSoT: release_info.score holds the download's score (set by
                    // finalize_download). The scanner creates release_info rows without a
                    // score, so a non-zero score means the download was deliberately
                    // selected and takes priority when upgrades are enabled.
                    let should_upgrade = if let Some(episode_target_path_str) =
                        &row.episode_file_path
                    {
                        let (is_upgrade, reason) = if let Ok(Some(mapping)) =
                            self.db.get_series_mapping(&row.series_id).await
                        {
                            // A manually chosen release always upgrades regardless of score,
                            // quality restrictions, or monitor status — those are search-level
                            // gates already applied before the item reached the queue.
                            let is_manual_override =
                                queue_item.as_ref().map(|qi| qi.is_manual).unwrap_or(false);

                            if is_manual_override {
                                (true, "manually chosen release")
                            } else {
                                // SSoT: shared `should_upgrade` (same logic as `select_winners`).
                                let (
                                    current_score,
                                    current_submitter,
                                    current_version,
                                    current_release_title,
                                ) = self
                                    .db
                                    .get_episode_release_info(&episode_id)
                                    .await
                                    .unwrap_or(None)
                                    .unwrap_or((0, None, 1, None));

                                let r_profile_id = mapping.release_profile.as_deref().unwrap_or("");
                                let min_score = self
                                    .db
                                    .get_effective_min_score(r_profile_id)
                                    .await
                                    .unwrap_or(0);

                                let intention_score: i32 = queue_item
                                    .as_ref()
                                    .and_then(|qi| {
                                        qi.episode_intentions.as_deref().and_then(|json| {
                                            let intents: Vec<
                                                jumbie_shared::types::EpisodeIntention,
                                            > = serde_json::from_str(json).ok()?;
                                            intents
                                                .iter()
                                                .find(|i| i.episode_id == episode_id)
                                                .map(|i| i.score)
                                        })
                                    })
                                    .unwrap_or(row.episode_score.unwrap_or(0));

                                let candidate_submitter =
                                    queue_item.as_ref().and_then(|qi| qi.submitter.as_deref());
                                let candidate_version =
                                    queue_item.as_ref().map(|qi| qi.version).unwrap_or(1);
                                let candidate_release_title = queue_item
                                    .as_ref()
                                    .map(|qi| qi.media_name.as_str())
                                    .unwrap_or("");

                                let should_upgrade = jumbie_shared::quality::should_upgrade(
                                    jumbie_shared::quality::ReleaseVersion {
                                        score: current_score,
                                        submitter: current_submitter.as_deref(),
                                        version: current_version,
                                        release_title: current_release_title.as_deref(),
                                    },
                                    jumbie_shared::quality::ReleaseVersion {
                                        score: intention_score,
                                        submitter: candidate_submitter,
                                        version: candidate_version,
                                        release_title: Some(candidate_release_title),
                                    },
                                    min_score,
                                );

                                if should_upgrade {
                                    (true, "")
                                } else {
                                    (false, "new release does not outrank existing episode")
                                }
                            }
                        } else {
                            (false, "series mapping not found")
                        };
                        (is_upgrade, episode_target_path_str.clone(), reason)
                    } else {
                        (false, String::new(), "no target file path")
                    };

                    if should_upgrade.0 {
                        let episode_target_path_str = should_upgrade.1;
                        let episode_target_path = PathBuf::from(&episode_target_path_str);

                        info!(
                            "Episode {} already organized at '{}' — upgrading with download at '{}' (score: {})",
                            episode_id,
                            episode_target_path_str,
                            path.display(),
                            row.episode_score.unwrap_or(0)
                        );

                        // Notify the client FIRST, before any state changes, so a shutdown
                        // between steps can't leave a stray torrent. delete_files = false
                        // because the file is moved (rename below) or cleaned up later.
                        if let (Some(dl_id), Some(qi)) = (
                            queue_item.as_ref().and_then(|i| i.downloader_id.clone()),
                            &queue_item,
                        ) && let Ok(downloader) = self.downloader.try_read()
                        {
                            let _ = downloader
                                .delete_download(&dl_id, false, qi.client_id.as_deref())
                                .await;
                        }

                        let _ = self.db.delete_fingerprint(&current_path_str).await;

                        if let Err(e) = tokio::fs::remove_file(&episode_target_path).await {
                            warn!(
                                "Failed to remove existing file '{}' during upgrade: {}",
                                episode_target_path.display(),
                                e
                            );
                        }

                        let mut hash = String::new();
                        if let Err(e) = tokio::fs::rename(&path, &episode_target_path).await {
                            tracing::error!(
                                "Failed to move download file to '{}' during upgrade: {} — deleting orphan",
                                episode_target_path.display(),
                                e
                            );
                            // The orphaned download stayed at the original path — delete it.
                            if let Err(e2) = tokio::fs::remove_file(&path).await {
                                warn!(
                                    "Failed to delete orphaned download '{}': {}",
                                    path.display(),
                                    e2
                                );
                            }
                        } else {
                            let (_hash_val, h, _media_info) = self
                                .db
                                .update_file_fingerprint(
                                    &episode_target_path,
                                    EpisodeStatus::Organized.as_str(),
                                )
                                .await;

                            // Hash is stored in release_info via the fingerprint; status
                            // stays 'organized'.
                            hash = h;

                            // After an upgrade only this episode's conditions changed
                            // (has_file, score); the episode-scoped refresh respects user
                            // soft-overrides on OTHER episodes.
                            self.refresh_monitor_status_for_episode(&row.series_id, &episode_id)
                                .await;
                        }

                        // Cull the per-download staging folder once the source file has
                        // been consumed (renamed away or orphan-deleted above).
                        remove_empty_download_dir(&path, &download_roots).await;

                        // 8. Distribute multi-target copies (before queue cleanup)
                        if let Some(qi) = &queue_item
                            && let Err(e) = self
                                .distribute_multi_target_copies(&episode_target_path, qi, &hash)
                                .await
                        {
                            warn!(
                                "Failed to distribute multi-target copies for '{}': {}",
                                episode_target_path.display(),
                                e
                            );
                        }

                        // Remove the queue item (status-filtered lookup avoids removing a
                        // newer upgrade item).
                        if let Err(e) = self.db.remove_completed_queue_item(&episode_id).await {
                            warn!(
                                "Failed to remove completed queue item for {}: {}",
                                episode_id, e
                            );
                        }
                    } else {
                        let reason = should_upgrade.2;

                        // Redundant pack/series-level file: a manual download treats it as
                        // unmatched (unexpected-files preference), an automatic pack as an
                        // unneeded episode. Scoped to pack/series queue items so a lost
                        // single-episode upgrade is still cleaned up.
                        let is_pack_ctx = queue_item
                            .as_ref()
                            .map(|qi| qi.is_season_pack || qi.episode.is_none())
                            .unwrap_or(false);
                        // Only a live existing file makes the incoming one redundant; a stale
                        // path means the episode still needs filling.
                        let existing_present = row
                            .episode_file_path
                            .as_deref()
                            .is_some_and(|p| !p.is_empty() && std::path::Path::new(p).exists());
                        let force_keep_review = queue_item
                            .as_ref()
                            .is_some_and(|qi| qi.is_manual && qi.episode.is_none());
                        let handling = super::redundant_file_handling(force_keep_review);
                        let keep_for_review = handling.keep_setting(&general_cfg) == "keep";
                        if is_pack_ctx && existing_present && keep_for_review {
                            info!(
                                "Episode {} already has a file — leaving redundant pack file for review: '{}' ({})",
                                episode_id,
                                path.display(),
                                reason
                            );
                            // Drop the torrent but keep its files on disk.
                            if let (Some(dl_id), Some(qi)) = (
                                queue_item.as_ref().and_then(|i| i.downloader_id.clone()),
                                &queue_item,
                            ) && let Ok(downloader) = self.downloader.try_read()
                            {
                                let _ = downloader
                                    .delete_download(&dl_id, false, qi.client_id.as_deref())
                                    .await;
                            }
                            // Record durably for review so the file surfaces in Manage
                            // Series Files (also blocks orphan adoption).
                            super::mark_file_for_review(
                                &self.db,
                                &current_path_str,
                                Some(row.series_id.as_str()),
                                handling.review_reason(),
                            )
                            .await;
                            if let Err(e) = self.db.remove_completed_queue_item(&episode_id).await {
                                warn!(
                                    "Failed to remove completed queue item for {}: {}",
                                    episode_id, e
                                );
                            }
                            continue;
                        }

                        info!(
                            "Episode {} already organized at {:?} — removing orphaned download at '{}' ({})",
                            episode_id,
                            row.episode_file_path,
                            path.display(),
                            reason
                        );

                        // Notify the client FIRST, before state changes, so a shutdown
                        // can't leave a stray torrent; delete_files = true (file is orphaned).
                        if let (Some(dl_id), Some(qi)) = (
                            queue_item.as_ref().and_then(|i| i.downloader_id.clone()),
                            &queue_item,
                        ) && let Ok(downloader) = self.downloader.try_read()
                        {
                            let _ = downloader
                                .delete_download(&dl_id, true, qi.client_id.as_deref())
                                .await;
                        }

                        let _ = self.db.delete_fingerprint(&current_path_str).await;

                        // The scanner's file takes precedence, so the orphaned download is
                        // never used — delete it.
                        if let Err(e) = tokio::fs::remove_file(&path).await {
                            warn!(
                                "Failed to delete orphaned download file '{}': {}",
                                path.display(),
                                e
                            );
                        }

                        // Cull the per-download staging folder now the download is gone.
                        remove_empty_download_dir(&path, &download_roots).await;

                        let notifications = self.notifications.read().await;
                        notifications
                            .notify(
                                NotifierEvent::Error,
                                &NotifierContext::new().with_error(
                                    "Download Cancelled",
                                    format!(
                                        "Download for {} was cancelled: {}",
                                        episode_id, reason
                                    ),
                                ),
                            )
                            .await;
                        drop(notifications);

                        // Remove the queue item (status-filtered lookup avoids removing a
                        // newer upgrade item).
                        if let Err(e) = self.db.remove_completed_queue_item(&episode_id).await {
                            warn!(
                                "Failed to remove completed queue item for {}: {}",
                                episode_id, e
                            );
                        }
                    }
                    continue;
                }

                if !path.exists() {
                    // Reported "complete" by the client but gone from disk (user or cleanup
                    // script). Delete the fingerprint so every poll cycle doesn't retry.
                    debug!(
                        "File marked complete/known but missing, removing fingerprint: '{}'",
                        path.display()
                    );
                    let _ = self.db.delete_fingerprint(&current_path_str).await;

                    // The file (and possibly its whole download) was removed externally;
                    // cull the now-empty staging folder if this was its last file.
                    remove_empty_download_dir(&path, &download_roots).await;

                    // Clean up the queue entry too, so "Completed" items don't linger in the
                    // download queue UI (status-filtered lookup avoids removing a newer item).
                    if let Err(e) = self.db.remove_completed_queue_item(&episode_id).await {
                        warn!(
                            "Failed to remove completed queue item for {}: {}",
                            episode_id, e
                        );
                    }
                    continue;
                }

                // SSoT: component-aware (Path::starts_with), so "/tv/My Show" won't
                // falsely match "/tv/My Showcase/S01" the way a string prefix would.
                let is_in_destination_root = config.organization.contains_path(&path);

                // If the file is already inside a destination root, skip organize — the
                // downloader placed it straight into the library. Clean up the queue entry
                // so "Completed" items don't linger in the UI.
                if is_in_destination_root {
                    trace!(
                        "Skipping organize for '{}': already in destination root",
                        path.display()
                    );
                    if let Err(e) = self.db.remove_completed_queue_item(&episode_id).await {
                        warn!(
                            "Failed to remove completed queue item for {}: {}",
                            episode_id, e
                        );
                    }
                    continue;
                }

                // Another pipeline (REORG/rename queue) already moved the file to a valid
                // episode file_path different from this fingerprint's source path. Don't
                // move it again — just clean up the stale fingerprint and queue entry.
                // Separate from the scanner-claimed guard above, which requires 'organized'
                // status; this catches a valid file_path regardless of status.
                if let Some(ref ep_path) = row.episode_file_path
                    && !ep_path.is_empty()
                    && ep_path != &current_path_str
                    && std::path::Path::new(ep_path).exists()
                {
                    info!(
                        "Episode {} already has file at '{}' (source '{}' is missing/redundant) — cleaning up stale fingerprint",
                        episode_id,
                        ep_path,
                        path.display()
                    );
                    let _ = self.db.delete_fingerprint(&current_path_str).await;
                    if let Err(e) = self.db.remove_completed_queue_item(&episode_id).await {
                        warn!(
                            "Failed to remove queue item for already-organized episode {}: {}",
                            episode_id, e
                        );
                    }
                    continue;
                }

                // A series being modified defers organize to the next cycle.
                if self
                    .modifying_series
                    .try_read()
                    .map(|guard| guard.contains(&row.series_id))
                    .unwrap_or(true)
                {
                    tracing::debug!(
                        "Series {} is currently being modified by another operation, \
                         deferring organize to next cycle",
                        row.series_id
                    );
                    continue;
                }

                // Mark as Organizing for UI visibility (purple badge) while the file is
                // fingerprinted and moved; on success the item is deleted, on error Failed.
                if let Ok(Some(qi)) = self.db.get_queue_item_by_episode_id(&episode_id).await
                    && qi.status == "Completed"
                {
                    let _ =
                        sqlx::query("UPDATE download_queue SET status = 'Organizing' WHERE id = ?")
                            .bind(qi.id)
                            .execute(self.db.get_pool())
                            .await;
                }

                // Extract media info at the download path so template vars like ${codec}
                // and ${resolution} are populated when the filename is rendered, and the
                // post-move scan can trigger a rename. update_file_fingerprint also scans
                // files not previously fingerprinted as Complete (retries, direct downloads).
                let (_hash_val, hash, media_info) = self
                    .db
                    .update_file_fingerprint(&path, EpisodeStatus::Organized.as_str())
                    .await;

                match self
                    .organize_file(&path, &episode_id, &download_id, media_info.as_ref())
                    .await
                {
                    Ok(Some(final_path)) => {
                        debug!(
                            "Organized '{}' -> '{}'",
                            path.display(),
                            final_path.display()
                        );

                        // Only video files update file_path and clean up the queue; non-video
                        // files (subs, nfo) are still moved by organize_file but must not mark
                        // the episode organized — the video file is the completion indicator.
                        let ext =
                            crate::utils::get_extended_extension(final_path.to_str().unwrap_or(""));
                        let main_ext = ext.split('.').next_back().unwrap_or(&ext).to_lowercase();
                        if jumbie_shared::media_format::is_video_ext(&main_ext) {
                            self.db
                                .update_file_path(&episode_id, final_path.to_str().unwrap_or(""))
                                .await?;

                            // SSoT: the queue item (snapshot at queue time) is the authoritative
                            // attribution source — submitter/quality profile are read from it,
                            // not re-derived from the fingerprint path.
                            if let Ok(Some(meta)) = self.db.get_organize_meta(&episode_id).await {
                                let upload_date = meta.source_pub_date.as_deref().and_then(|spd| {
                                    // SSoT inbound parser — date-only becomes midnight UTC.
                                    crate::datetime::parse_utc(spd).ok().map(|u| u.naive_utc())
                                });

                                // Write the release snapshot to `release_info` (keyed by
                                // content hash). score/version overwrite; attribution,
                                // download_id, the source feed date and scoring inputs are
                                // COALESCEd so a partial snapshot can't erase them. The
                                // file's date lives here — not on the episode row.
                                let _ = self
                                    .db
                                    .upsert_organize_release_meta(&hash, &meta, upload_date)
                                    .await;

                                // Backfill the episode's quality profile from the
                                // queue-time snapshot (COALESCE preserves any
                                // user-set override).
                                if let Some(qpid) =
                                    meta.quality_profile_id.as_deref().filter(|q| !q.is_empty())
                                {
                                    let _ = sqlx::query(
                                        "UPDATE episodes SET \
                                         quality_profile_id = COALESCE(?, quality_profile_id) \
                                         WHERE episode_id = ?",
                                    )
                                    .bind(qpid)
                                    .bind(&episode_id)
                                    .execute(self.db.get_pool())
                                    .await;
                                }
                            }

                            // Only this episode's conditions changed (has_file); the
                            // episode-scoped refresh respects user soft-overrides on OTHER
                            // episodes, and monitor_override on this one (with self-heal if the
                            // mode caught up).
                            self.refresh_monitor_status_for_episode(&row.series_id, &episode_id)
                                .await;
                        }

                        // The file was hard-linked (content identical), so the pre-scanned hash
                        // and media_info stay valid; only the path and identity fields change
                        // (inode after the move). Avoids a second ffprobe on the same file.
                        if let Ok(metadata) = tokio::fs::metadata(&final_path).await {
                            let (inode, dev, mtime) = crate::platform::file_identity(&metadata);
                            let size = metadata.len();
                            let _ = self
                                .db
                                .save_fingerprint(crate::db::files::SaveFingerprintParams {
                                    path: final_path.to_str().unwrap_or(""),
                                    inode,
                                    dev,
                                    size,
                                    mtime,
                                    quick_hash: &hash,
                                    state: EpisodeStatus::Organized.as_str(),
                                    media_info: media_info.as_ref(),
                                })
                                .await;
                        }

                        // Check for multi-targets wanting a copy before queue cleanup.
                        if let Ok(Some(queue_item)) =
                            self.db.get_queue_item_by_episode_id(&episode_id).await
                            && queue_item.multi_targets.is_some()
                            && let Err(e) = self
                                .distribute_multi_target_copies(&final_path, &queue_item, &hash)
                                .await
                        {
                            warn!(
                                "Failed to distribute multi-target copies for '{}': {}",
                                final_path.display(),
                                e
                            );
                        }

                        // Tell the client to clean up this torrent's state (for qBittorrent
                        // this drops the entry without deleting files, already moved).
                        if let Ok(Some(dl_item)) =
                            self.db.get_queue_item_by_episode_id(&episode_id).await
                            && let Some(ref dl_id) = dl_item.downloader_id
                            && let Some(cid) = dl_item.client_id.as_deref()
                        {
                            let downloader = self.downloader.read().await;
                            if let Err(e) = downloader.complete_download(dl_id, cid).await {
                                warn!(
                                    "Failed to notify download client of completion for {}: {}",
                                    episode_id, e
                                );
                            }
                        }

                        if let Err(e) = self.db.remove_completed_queue_item(&episode_id).await {
                            warn!(
                                "Failed to remove completed queue item for {}: {}",
                                episode_id, e
                            );
                        }

                        // The download has been consumed; cull its now-empty staging
                        // folder (the content dir was removed by move_file_to_target).
                        remove_empty_download_dir(&path, &download_roots).await;
                    }
                    Ok(None) => {
                        // Metadata unparseable or no series mapping. Parsing is deterministic,
                        // so retries won't help — mark Failed so the user can fix and retry.
                        warn!(
                            "organize_file returned None for '{}' — marking as Failed (parsing/mapping failure)",
                            path.display()
                        );
                        if let Ok(Some(mut queue_item)) =
                            self.db.get_queue_item_by_episode_id(&episode_id).await
                        {
                            queue_item.status = "Failed".to_string();
                            queue_item.score = -99999;
                            queue_item.error_message =
                                Some("Could not parse filename or find series mapping".to_string());
                            let _ = self.db.update_download_queue_item(&queue_item).await;
                        }
                    }
                    Err(e) => {
                        // Source gone: another pipeline (REORG/rename) moved it between our
                        // DB query and this call, so retrying would also fail. Clean up the
                        // stale fingerprint and queue item instead.
                        if !path.exists() {
                            info!(
                                "Source file missing during organize (already organized?), removing fingerprint: '{}'",
                                path.display()
                            );
                            let _ = self.db.delete_fingerprint(&current_path_str).await;
                            if let Err(e) = self.db.remove_completed_queue_item(&episode_id).await {
                                warn!(
                                    "Failed to remove queue item for missing file {}: {}",
                                    episode_id, e
                                );
                            }
                            continue;
                        }

                        // Transient error (permission denied, disk full, cross-device link).
                        // Mark Failed with a very low score (-99999) so the winner algorithm
                        // doesn't re-select it while the retry is pending. Both video and
                        // non-video items are marked, else they'd linger as "Completed".
                        tracing::error!("Failed to organize '{}': {}", path.display(), e);
                        self.db
                            .schedule_retry(
                                "organize",
                                current_path_str.as_str(),
                                None,
                                Some(&episode_id),
                                &e.to_string(),
                                0,
                            )
                            .await?;
                        info!("Scheduled retry for '{}'", path.display());

                        if let Ok(Some(mut queue_item)) =
                            self.db.get_queue_item_by_episode_id(&episode_id).await
                        {
                            queue_item.status = "Failed".to_string();
                            queue_item.score = -99999;
                            queue_item.error_message = Some(e.to_string());
                            let _ = self.db.update_download_queue_item(&queue_item).await;
                        }

                        let notifications = self.notifications.read().await;
                        notifications
                            .notify(
                                NotifierEvent::Error,
                                &NotifierContext::new().with_error(
                                    "Organize Failure",
                                    format!(
                                        "Failed to organize '{}': {}. Retrying.",
                                        path.display(),
                                        e
                                    ),
                                ),
                            )
                            .await;
                    }
                }
            }

            // Items with status 'Completed'/'Failed' whose episode is already 'organized'
            // should have been removed above; only a skipped cleanup (e.g. a DB error) would
            // leave them, and get_completed_files() never returns 'organized' episodes.
            if let Err(e) = self.db.remove_stale_terminal_queue_items().await {
                warn!("Failed to remove stale terminal queue items: {}", e);
            }

            // The organize pass wrote authoritative upload_dates; re-anchor estimates now
            // instead of waiting for the periodic sweep (up to 6 hours). Idempotent.
            for series_id in &affected_series {
                if crate::task::is_shutdown_requested(&self.shutdown_token, "organize_completed") {
                    break;
                }
                if let Err(e) =
                    crate::release_estimator::trigger_estimation_for_series(&self.db, series_id)
                        .await
                {
                    warn!(
                        "Failed to trigger release date estimation for {}: {}",
                        series_id, e
                    );
                }
            }

            Ok(())
        })
    }

    /// After the primary file has been organized to its final location, create
    /// independent copies for any multi-targets on the queue entry.
    ///
    /// SSoT: The `multi_targets` JSON field on the queue entry is the single
    /// source of truth for which additional (series, season, episode) targets
    /// want a copy of this file.  Each target gets:
    ///   1. An independent file copy at its series' destination path
    ///   2. An episode row in the episodes table pointing to the copy
    ///   3. A fingerprint entry for the copy (so file state tracking works)
    ///
    /// Targets whose episodes already have a file are skipped (idempotent).
    /// Failed copies are logged but don't block other targets.
    pub(crate) async fn distribute_multi_target_copies(
        &self,
        organized_source: &std::path::Path,
        queue_item: &jumbie_shared::types::DownloadQueueItem,
        quick_hash: &str,
    ) -> Result<()> {
        let multi_json = match queue_item.multi_targets.as_deref() {
            Some(json) if !json.is_empty() => json,
            _ => return Ok(()),
        };

        let targets: Vec<MultiTarget> =
            serde_json::from_str(multi_json).context("Failed to parse multi_targets JSON")?;

        if targets.is_empty() {
            return Ok(());
        }

        let primary_series_id = &queue_item.series_id;
        let primary_season: Option<i32> = queue_item.season.as_deref().and_then(|s| s.parse().ok());
        let primary_episode = queue_item.episode;

        // Read organization and general config from DB directly (SSoT).
        let config = self.db_config().await;

        for target in &targets {
            // Skip the primary target — it's already organized
            if target.series_id == *primary_series_id
                && Some(target.season) == primary_season
                && Some(target.episode) == primary_episode
            {
                continue;
            }

            // SSoT: fetch the mapping once per target (episode id + destination path).
            let season_str = format!("{:02}", target.season);
            let mapping = match self.db.get_series_mapping(&target.series_id).await {
                Ok(Some(m)) => m,
                Ok(None) => {
                    warn!(
                        "Multi-target series '{}' not found, skipping",
                        target.series_id
                    );
                    continue;
                }
                Err(e) => {
                    warn!(
                        "Error looking up multi-target series '{}': {}",
                        target.series_id, e
                    );
                    continue;
                }
            };
            let ep_id = match mapping.get_episode_id(
                &season_str,
                target.episode,
                config.general.absolute_numbering,
            ) {
                Ok(id) => id,
                Err(e) => {
                    debug!("skipping non-numeric season: {e}");
                    continue;
                }
            };

            // The target episode row may already exist from metadata sync. Use it
            // for two things: skip targets that already have a file (idempotent),
            // and inherit its metadata (air) date. This is the ONLY date the copy
            // episode may take from elsewhere — never the download time.
            let existing = self.db.get_episode_by_id(&ep_id).await.ok().flatten();
            if existing
                .as_ref()
                .and_then(|e| e.file_path.as_deref())
                .filter(|p| !p.is_empty())
                .is_some()
            {
                trace!("Multi-target {} already has a file, skipping copy", ep_id);
                continue;
            }
            // Episode metadata date if metadata sync already knows it; NULL otherwise.
            // `insert_episode` COALESCEs on conflict, so NULL never clobbers a value.
            let target_meta_date = existing.and_then(|e| e.meta_date);

            // SSoT: use the existing build_target_path for path resolution.
            let pad_opts = crate::utils::TemplatePadOptions {
                max_season: 0,
                max_episode: 0,
                max_length: 0,
            };
            let episode_var = target.episode.to_string();
            let dest = match ContentOrganizer::build_target_path(&PathBuildVars {
                config: &config,
                mapping: &mapping,
                season_num: target.season,
                episode_num: target.episode,
                episode_var: &episode_var,
                source_path: organized_source,
                episode_title: None,
                episode_quality: None,
                episode_submitter: None,
                release_date: None,
                created_at: None,
                pad_options: &pad_opts,
                part_number: None,
                media_info: None,
            }) {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        "Failed to build target path for multi-target {}: {}",
                        ep_id, e
                    );
                    continue;
                }
            };

            debug!(
                "Distributing multi-target copy: '{}' -> '{}'",
                organized_source.display(),
                dest.display()
            );

            if let Some(parent) = dest.parent()
                && let Err(e) = tokio::fs::create_dir_all(parent).await
            {
                warn!(
                    "Failed to create directory '{}' for multi-target {}: {}",
                    parent.display(),
                    ep_id,
                    e
                );
                continue;
            }

            // Independent copy, not a hardlink.
            if let Err(e) = tokio::fs::copy(organized_source, &dest).await {
                warn!(
                    "Failed to copy file to '{}' for multi-target {}: {}",
                    dest.display(),
                    ep_id,
                    e
                );
                continue;
            }

            let episode_id = match mapping.get_episode_id(
                &season_str,
                target.episode,
                config.general.absolute_numbering,
            ) {
                Ok(id) => id,
                Err(e) => {
                    debug!("skipping non-numeric season: {e}");
                    continue;
                }
            };

            let _ = self
                .db
                .insert_episode(InsertEpisodeParams {
                    episode_id: &episode_id,
                    series_id: &target.series_id,
                    season: target.season,
                    episode: target.episode,
                    file_path: Some(dest.to_str().unwrap_or("")),
                    title: None,
                    // SSoT: the queue-time snapshot, not the current mapping —
                    // the mapping may have changed since the download was queued.
                    quality_profile_id: queue_item.quality_profile_id.as_deref(),
                    status: EpisodeStatus::Organized.as_str(),
                    meta_date: target_meta_date,
                    est_date: None,
                    metadata_ids: &std::collections::HashMap::new(),
                    description: None,
                    runtime: None,
                    image_url: None,
                    metadata_source: None,
                    numbering_mode: None,
                })
                .await;

            if let Ok(metadata) = tokio::fs::metadata(&dest).await {
                let (inode, dev, mtime) = crate::platform::file_identity(&metadata);
                let size = metadata.len();
                let _ = self
                    .db
                    .save_fingerprint(crate::db::files::SaveFingerprintParams {
                        path: dest.to_str().unwrap_or(""),
                        inode,
                        dev,
                        size,
                        mtime,
                        quick_hash,
                        state: EpisodeStatus::Organized.as_str(),
                        media_info: None,
                    })
                    .await;

                // Write release metadata to the normalized release_info table.
                if !queue_item.media_link.is_empty() || !queue_item.media_name.is_empty() {
                    // Submitter: prefer the episode's stored value (set from the resolved
                    // queue-time submitter); extract_submitter is the legacy fallback.
                    let sub_multi: Option<String> = self
                        .db
                        .get_episode_submitter(&episode_id)
                        .await
                        .ok()
                        .flatten()
                        .or_else(|| {
                            jumbie_shared::parsing::extract_submitter(&queue_item.media_name)
                        });
                    let _ = self
                        .db
                        .set_release_info(
                            quick_hash,
                            Some(&queue_item.media_name),
                            Some(&queue_item.media_link),
                            sub_multi.as_deref(),
                        )
                        .await;
                }
            }

            info!(
                "Multi-target copy complete: '{}' -> {}",
                dest.display(),
                episode_id
            );
        }

        Ok(())
    }

    // Retry queue: the safety net for transient organize failures, with exponential
    // backoff (1min, 2min, 4min, …) and permanent failure only when max_retries is
    // exhausted. Only "organize" is retryable — download dispatch failures are re-queued
    // in process_download_queue.
    //
    // Cancellation: a signal between items skips the rest (picked up next cycle).
    pub(crate) fn process_retry_queue<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let pending = self.db.get_pending_retries().await?;
            if pending.is_empty() {
                return Ok(());
            }

            info!("Processing {} retry queue items", pending.len());

            // Configured download roots, resolved once so a successful retry culls the
            // per-download staging folder it left behind.
            let download_roots = self.download_roots().await;

            for item in pending {
                if crate::task::is_shutdown_requested(&self.shutdown_token, "retry queue") {
                    break;
                }
                debug!("Processing retry item: '{}'", item.source_path);
                debug!(
                    "Retrying {} (Attempt {}/{}): {}",
                    item.operation,
                    item.retry_count + 1,
                    item.max_retries,
                    item.source_path
                );

                let mut success = false;
                let mut error_msg = None;

                if item.operation == "organize" {
                    let path = PathBuf::from(&item.source_path);
                    let hash = if let Some(ep_id) = &item.episode_id {
                        self.db
                            .get_episode_hash(ep_id)
                            .await
                            .unwrap_or_default()
                            .unwrap_or_default()
                    } else {
                        String::new()
                    };

                    if let Some(ep_id) = &item.episode_id {
                        // Scan media info before retry so template vars are populated.
                        let (_, _, retry_media_info) = self
                            .db
                            .update_file_fingerprint(&path, EpisodeStatus::Organized.as_str())
                            .await;

                        match self
                            .organize_file(&path, ep_id, &hash, retry_media_info.as_ref())
                            .await
                        {
                            Ok(Some(final_path)) => {
                                success = true;
                                self.db
                                    .update_file_path(ep_id, final_path.to_str().unwrap_or(""))
                                    .await?;
                                FileStateManager::fingerprint_file(
                                    &self.db,
                                    &final_path,
                                    FileState::Organized,
                                )
                                .await
                                .ok();

                                // Status-filtered lookup avoids removing a newer upgrade item.
                                let _ = self.db.remove_completed_queue_item(ep_id).await;

                                // The retried download has been consumed; cull its staging folder.
                                remove_empty_download_dir(&path, &download_roots).await;
                            }
                            Ok(None) => {
                                error_msg = Some(
                                    "Organization returned None (parsing/mapping)".to_string(),
                                );
                            }
                            Err(e) => {
                                error_msg = Some(e.to_string());
                            }
                        }
                    } else {
                        error_msg = Some("Missing episode_id for organize retry".to_string());
                    }
                } else {
                    error_msg = Some(format!("Unknown operation: {}", item.operation));
                }

                if success {
                    self.db
                        .update_retry_status(item.id, "completed", None)
                        .await?;
                    debug!("Retry successful for {}", item.source_path);
                } else {
                    let next_count = item.retry_count + 1;
                    if next_count >= item.max_retries {
                        self.db
                            .update_retry_status(item.id, "failed", error_msg.as_deref())
                            .await?;
                        warn!("Retry permanently failed: '{}'", item.source_path);

                        if let Some(ep_id) = &item.episode_id
                            && let Ok(Some(mut queue_item)) =
                                self.db.get_queue_item_by_episode_id(ep_id).await
                        {
                            queue_item.status = "Failed".to_string();
                            queue_item.score = -99999;
                            queue_item.error_message = error_msg.clone();
                            let _ = self.db.update_download_queue_item(&queue_item).await;
                        }

                        let notifications = self.notifications.read().await;
                        notifications
                            .notify(
                                NotifierEvent::Error,
                                &NotifierContext::new().with_error(
                                    "Retry Failed",
                                    format!(
                                        "Permanently failed after {} retries: {}",
                                        item.max_retries, item.source_path
                                    ),
                                ),
                            )
                            .await;
                        drop(notifications);
                    } else {
                        self.db
                            .update_retry_status(item.id, "retrying", error_msg.as_deref())
                            .await?;
                        self.db
                            .schedule_retry(
                                &item.operation,
                                &item.source_path,
                                item.destination_path.as_deref(),
                                item.episode_id.as_deref(),
                                error_msg.as_deref().unwrap_or("Unknown error"),
                                next_count,
                            )
                            .await?;
                    }
                }
            }
            Ok(())
        })
    }
}
