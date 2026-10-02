use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::organizer::ContentOrganizer;

/// Move a batch of `(src, dst)` files in one cycle-safe pass.
///
/// A rename batch can recycle the same paths (e.g. `A→B, B→C, C→D`). Moving `A→B`
/// while `B` is still a source would clobber it, so first every source that is
/// **also a destination** of another move is renamed to a unique temp path — with
/// the file's `file_paths` row migrated along with it. That frees each destination
/// for the move that targets it. Each file is then moved to its final destination
/// via [`ContentOrganizer::move_file_to_target`], passing the whole batch as
/// `batch_sources` so an in-batch occupant is treated as a temporary conflict (no
/// spurious collision suffix) rather than a permanent one.
///
/// SSoT for cycle-safe batch moves — used by the manual batch-assignment endpoint
/// and the reorganization/rename-queue executor.
///
/// Returns one entry per input move, in order: `Some(final_path)` on success,
/// `None` when the source was missing or the move failed. If the temp-rename phase
/// fails, every file is rolled back to its original path and all entries are `None`.
pub async fn move_files_cycle_safe(
    organizer: &ContentOrganizer,
    moves: &[(PathBuf, PathBuf)],
) -> Vec<Option<PathBuf>> {
    if moves.is_empty() {
        return Vec::new();
    }

    let src_set: HashSet<PathBuf> = moves.iter().map(|(src, _)| src.clone()).collect();
    let dst_set: HashSet<PathBuf> = moves.iter().map(|(_, dst)| dst.clone()).collect();

    // Phase 1: temp-rename sources that are also destinations.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let mut temp_map: HashMap<PathBuf, PathBuf> = HashMap::new();
    for (idx, (src, _)) in moves.iter().enumerate() {
        if !dst_set.contains(src) || !src.exists() {
            continue;
        }
        let tmp = src.with_file_name(format!("jumbie_tmp_{}_{}", ts, idx));
        match tokio::fs::rename(src, &tmp).await {
            Ok(_) => {
                // Keep the fingerprint/identity row with the file so the later move
                // migrates the right row.
                let _ = organizer
                    .db
                    .move_fingerprint_path(&src.to_string_lossy(), &tmp.to_string_lossy())
                    .await;
                temp_map.insert(src.clone(), tmp);
            }
            Err(e) => {
                // A failed temp-rename breaks the plan's move ordering; roll back
                // everything and leave the filesystem in its original state.
                for (orig, t) in &temp_map {
                    let _ = tokio::fs::rename(t, orig).await;
                    let _ = organizer
                        .db
                        .move_fingerprint_path(&t.to_string_lossy(), &orig.to_string_lossy())
                        .await;
                }
                tracing::error!(
                    "move_files_cycle_safe: temp-rename '{}' failed: {}",
                    src.display(),
                    e
                );
                return vec![None; moves.len()];
            }
        }
    }

    // Phase 2: move each file to its final destination.
    let mut results = Vec::with_capacity(moves.len());
    for (src, dst) in moves {
        let effective = temp_map.get(src).cloned().unwrap_or_else(|| src.clone());
        if !effective.exists() {
            tracing::warn!(
                "move_files_cycle_safe: skipping missing file '{}'",
                effective.display()
            );
            results.push(None);
            continue;
        }
        match organizer
            .move_file_to_target(&effective, dst, Some(&src_set))
            .await
        {
            Ok(final_path) => results.push(Some(final_path)),
            Err(e) => {
                tracing::warn!(
                    "move_files_cycle_safe: move '{}' -> '{}' failed: {}",
                    src.display(),
                    dst.display(),
                    e
                );
                results.push(None);
            }
        }
    }
    results
}
