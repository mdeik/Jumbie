//! Part-file registration — the single SSoT for turning a part file on disk into
//! `episode_files` rows plus a parent `episodes` placeholder.
//!
//! Both scanners route through here (`scan_directory` and
//! `import_scan_for_series`), so a multipart episode is represented identically
//! no matter which scan path discovered it:
//!
//!   * the parent episode row is created with `file_path = NULL` — the episode is
//!     discoverable but not directly playable (the player reassembles parts);
//!   * one `episode_files` row per part file;
//!   * background hashing / media-info / automatic-profile work is queued, then
//!     the part's fingerprint is written.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use jumbie_shared::types::MappingRule;

use crate::api::AppState;
use crate::db::episodes::InsertParentEpisodeParams;

/// Register `part_path` as a part of episode `episode_num` of `season_str`.
///
/// Returns the parent episode id. Idempotent: an existing parent row is left
/// alone and the part row is upserted by part number. Errors only when the
/// episode id cannot be derived (e.g. a non-numeric season label).
pub async fn register_part_file(
    state: &Arc<AppState>,
    mapping: &MappingRule,
    season_str: &str,
    episode_num: i32,
    global_absolute: bool,
    part_path: &Path,
    part_number: u32,
) -> Result<String> {
    let base_episode_id = mapping.get_episode_id(season_str, episode_num, global_absolute)?;
    let numbering_mode = mapping.settings.active_mode(global_absolute).is_absolute() as i32;
    let path_str = part_path.to_string_lossy().to_string();

    // Hash-first: record the part's content fingerprint before assignment (bounded
    // concurrency via the scan queue).
    crate::scanner::scan::hash_first(state, part_path).await;

    // Parent placeholder (file_path = NULL), created once.
    state
        .db
        .insert_parent_episode(InsertParentEpisodeParams {
            episode_id: &base_episode_id,
            series_id: &mapping.series_id,
            season: season_str.parse::<i32>().unwrap_or(1),
            episode: episode_num,
            title: "",
            numbering_mode,
        })
        .await?;

    // A language sibling is reconciled (kept alongside, or the plain name promoted
    // into the slot); otherwise the part is registered, applying the organization
    // collision strategy to whatever the part slot already held.
    let file_size = std::fs::metadata(part_path).ok().map(|m| m.len() as i64);
    let writes = state
        .db
        .reconcile_ingested_slot(&base_episode_id, &path_str, Some(part_number), file_size)
        .await?
        .outcome
        .caller_writes_slot();
    if writes {
        state
            .db
            .write_slot_displacing(&base_episode_id, &path_str, Some(part_number), file_size)
            .await?;
    }

    // Background: hashing + media_info + automatic profiles
    // Hash/ffprobe are I/O-heavy and profile checks may have side-effects (DB
    // writes, notifications); both run off the scan loop via the scan queue,
    // which also marks the file as mid-scan so the rename queue skips it.
    let submitter = crate::utils::extract_submitter(
        part_path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );
    let state_ref = state.clone();
    let path_for_queue = part_path.to_path_buf();
    let path_clone = path_for_queue.clone();
    let ep_id_bg = base_episode_id.clone();
    let submitter_bg = submitter.clone();
    state
        .scan_queue
        .submit(path_for_queue, move || {
            let state_ref = state_ref.clone();
            let path_clone = path_clone.clone();
            let ep_id_bg = ep_id_bg.clone();
            let submitter_bg = submitter_bg.clone();
            async move {
                let (hash_val, media_info) = state_ref.db.scan_media_info_only(&path_clone).await;

                if let (Some(info), Some(organizer)) = (media_info.as_ref(), &state_ref.organizer) {
                    let cfg = state_ref.cfg.read().await;
                    if let Some(submitter) = submitter_bg.as_deref() {
                        let filename = path_clone
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        if cfg.general.automatic_profiles.enabled {
                            organizer
                                .check_automatic_profile_rules(
                                    submitter,
                                    filename,
                                    info,
                                    &cfg.general.automatic_profiles.categories,
                                    Some(&hash_val),
                                    true,
                                )
                                .await;
                        }
                        // Persist the scan so recalculation works without the
                        // original file.
                        if let Ok(media_json) = serde_json::to_string(info) {
                            let _ = state_ref
                                .db
                                .record_media_scan(
                                    submitter,
                                    filename,
                                    None,
                                    None,
                                    None,
                                    &media_json,
                                )
                                .await;
                        }
                    }
                }

                let _ = state_ref
                    .db
                    .update_episode_part_fingerprint(&ep_id_bg, part_number, &hash_val)
                    .await;
            }
        })
        .await;

    Ok(base_episode_id)
}
