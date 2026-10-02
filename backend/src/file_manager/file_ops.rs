use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tracing::{info, trace, warn};

use jumbie_shared::config::organization::CollisionRenameSuffix;

use crate::db::DbManager;
use crate::organizer::ContentOrganizer;

/// Outcome of a collision diagnosis between a planned source and destination.
///
/// Both `move_file_to_target` (execution) and `get_rename_queue_detail`
/// (preview) use this same function so their collision logic never drifts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionKind {
    /// No collision — dst is free, or src == dst (same inode).
    NoCollision,
    /// Dst is occupied by a batch source (will itself be moved by this batch).
    TempOccupant,
    /// Dst is occupied by an unassigned file — execution will move it aside.
    /// Preview should NOT flag this as a collision (it resolves silently).
    UnassignedOccupant,
    /// Dst is occupied by an assigned file — execution will suffix/skip/overwrite.
    /// Preview SHOULD flag this as a collision.
    AssignedOccupant,
}

/// Diagnose what kind of collision (if any) exists between a planned move src → dst.
///
/// * `batch_sources` — when `Some(set)`, the set of all source paths in the current
///   batch; an occupant in this set is a `TempOccupant` (it will be moved elsewhere).
/// * `db` — used to query whether the occupant is still assigned to an episode.
pub async fn diagnose_collision(
    src: &Path,
    dst: &Path,
    batch_sources: Option<&HashSet<PathBuf>>,
    db: &DbManager,
) -> CollisionKind {
    if !dst.exists() {
        return CollisionKind::NoCollision;
    }

    // Same file (already in place) — no collision
    let src_canon = crate::validation::normalize_path(src);
    let dst_canon = crate::validation::normalize_path(dst);
    if src_canon == dst_canon {
        return CollisionKind::NoCollision;
    }

    // Batch source occupant — will be moved by this batch
    if batch_sources.map(|set| set.contains(dst)).unwrap_or(false) {
        return CollisionKind::TempOccupant;
    }

    // Occupied by a file on disk — check if it's unassigned
    match db.is_path_unassigned(dst).await {
        Ok(true) => CollisionKind::UnassignedOccupant,
        _ => CollisionKind::AssignedOccupant,
    }
}

/// Remove a directory if it exists and is empty.
///
/// Best-effort (returns `()`): the directory may be re-populated between the
/// `read_dir` and `remove_dir` calls (TOCTOU), or be non-removable for reasons
/// outside the caller's control. Failures are logged and swallowed; the caller can
/// retry next batch cycle.
pub(crate) async fn remove_if_empty_dir(dir: &Path) {
    if !dir.is_dir() {
        return;
    }
    match tokio::fs::read_dir(dir).await {
        Ok(mut entries) => {
            if entries.next_entry().await.unwrap_or(None).is_none() {
                if let Err(e) = tokio::fs::remove_dir(dir).await {
                    tracing::warn!(
                        "Failed to remove empty directory '{}': {}",
                        dir.display(),
                        e
                    );
                } else {
                    tracing::info!("Removed empty directory: '{}'", dir.display());
                }
            }
        }
        Err(e) => tracing::warn!("Could not read directory '{}': {}", dir.display(), e),
    }
}

/// Recursively remove empty directories inside `stop_on` (depth-first, post-order),
/// then remove `stop_on` itself if empty. Does NOT go above `stop_on`.
///
/// For a UUID-subfolder download at `/grab/media/jumbie/{uuid}/`, this removes empty
/// subdirs like `Season 1/` and then `{uuid}/` if empty — but never touches `jumbie/`.
/// Best-effort like `remove_if_empty_dir`.
pub(crate) async fn remove_empty_ancestors(stop_on: &Path) {
    // Recursively clean empty subdirectories (depth-first, post-order).
    remove_empty_subdirs(stop_on).await;
    // Finally, check if stop_on itself is empty and remove it.
    remove_if_empty_dir(stop_on).await;
}

/// Depth-first, post-order traversal: collect all directories, then process in
/// reverse (deepest first) so children are removed before parent (iterative —
/// recursion is awkward with async Rust).
async fn remove_empty_subdirs(root: &Path) {
    if !root.is_dir() {
        return;
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir.is_dir() {
            dirs.push(dir.clone());
            if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if entry.path().is_dir() {
                        stack.push(entry.path());
                    }
                }
            }
        }
    }
    // Process in reverse (deepest first) so children are removed before parent.
    for dir in dirs.into_iter().rev() {
        remove_if_empty_dir(&dir).await;
    }
}

/// The per-download staging folder for `path` — the first path component beneath
/// the most specific configured download root it belongs to.
///
/// Downloads are isolated in a UUID folder directly under a download root
/// (`<root>/<uuid>/<content>/...`), so the folder that becomes empty once a
/// download is consumed is the *first* component beneath the root — NOT the
/// content dir or the file's parent (which `remove_empty_ancestors` would cull
/// only down to). Returns `None` when `path` is not inside any root (e.g. a
/// library-side move), and never returns a root itself.
///
/// The *deepest* matching root wins (not merely the first): when roots overlap or
/// nest — e.g. two clients where one stages into a subfolder of another's root —
/// this keeps the boundary scoped to the download's own folder instead of a shared
/// parent, so unrelated folders in that parent are never targeted.
pub(crate) fn download_staging_dir(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .and_then(|root| {
            path.strip_prefix(root)
                .ok()
                .and_then(|rel| rel.components().next())
                .map(|component| root.join(component))
        })
}

/// Remove the per-download staging folder that `path` lives in, if it is now empty.
///
/// `path` may be a file or the download's content directory. The cleanup boundary
/// is the staging folder (`download_staging_dir`), so this culls the content dir
/// and the UUID folder beneath a configured download root — but never the root
/// itself. A `path` outside every root is a no-op. Best-effort, like
/// `remove_empty_ancestors`.
pub(crate) async fn remove_empty_download_dir(path: &Path, roots: &[PathBuf]) {
    let Some(stop_on) = download_staging_dir(path, roots) else {
        return;
    };
    // Guard: never remove a configured download root, even if a caller hands us a
    // path that resolves to it directly (e.g. a torrent saved straight into the root).
    if roots.iter().any(|root| root == &stop_on) {
        return;
    }
    remove_empty_ancestors(&stop_on).await;
}

// move_file_to_target is the only place that performs filesystem moves and
// cross-device fallback; organize_file and manual_organize_file_explicit both
// delegate here.

impl ContentOrganizer {
    /// Copy a file in chunks with periodic cancellation checks.
    /// If `cancel` is signalled mid-copy, the partial destination file is deleted
    /// and an error is returned. This prevents leaving half-copied files on disk
    /// when the application is shutting down (e.g. during EXDEV cross-device fallback).
    async fn copy_with_cancellation(
        src: &Path,
        dst: &Path,
        cancel: &CancellationToken,
    ) -> Result<()> {
        const CHUNK_SIZE: usize = 8 * 1024 * 1024; // 8 MiB

        let mut src_file = tokio::fs::File::open(src)
            .await
            .context("Failed to open source file for copy")?;
        let mut dst_file = tokio::fs::File::create(dst)
            .await
            .context("Failed to create destination file for copy")?;

        let mut buf = vec![0u8; CHUNK_SIZE];
        loop {
            if cancel.is_cancelled() {
                drop(dst_file);
                let _ = tokio::fs::remove_file(dst).await;
                return Err(anyhow::anyhow!(
                    "Copy cancelled by shutdown signal, deleted partial file '{}'",
                    dst.display()
                ));
            }

            let bytes_read = src_file
                .read(&mut buf)
                .await
                .context("Failed to read from source file during copy")?;
            if bytes_read == 0 {
                break;
            }
            dst_file
                .write_all(&buf[..bytes_read])
                .await
                .context("Failed to write to destination file during copy")?;
        }

        dst_file
            .flush()
            .await
            .context("Failed to flush destination file after copy")?;
        Ok(())
    }

    /// Move `src` to `dst` on disk, handling cross-device fallback (EXDEV),
    /// fingerprint migration, and empty-directory cleanup.
    ///
    /// If `cancel` is signalled during a cross-device copy the partial destination is
    /// deleted and an error returned; a same-filesystem rename is atomic regardless.
    ///
    /// `batch_sources` distinguishes temporary conflicts (an occupant that will itself
    /// be moved by this batch) from permanent ones.
    pub async fn move_file_to_target(
        &self,
        src: &Path,
        dst: &Path,
        batch_sources: Option<&HashSet<PathBuf>>,
    ) -> Result<PathBuf> {
        trace!(
            "move_file_to_target called: src='{}', dst='{}'",
            src.display(),
            dst.display()
        );
        let mut target_path = dst.to_path_buf();

        tracing::trace!(
            "[MOVE] checking if target exists: '{}'",
            target_path.display()
        );

        match diagnose_collision(src, &target_path, batch_sources, &self.db).await {
            CollisionKind::NoCollision | CollisionKind::TempOccupant => {}
            CollisionKind::UnassignedOccupant => {
                let org_config = self.db.get_organization_config().await.unwrap_or_default();
                warn!(
                    "Occupant at '{}' is unassigned — moving it aside with suffix",
                    target_path.display()
                );
                let occupant_new = crate::utils::file_naming::strip_collision_suffix(&target_path)
                    .and_then(|(clean_path, _detected)| {
                        if !clean_path.exists() {
                            Some(clean_path)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| {
                        self.resolve_collision(&target_path, org_config.collision_rename_suffix)
                    });

                tokio::fs::rename(&target_path, &occupant_new).await?;

                let old_str = target_path.to_string_lossy().to_string();
                let new_str = occupant_new.to_string_lossy().to_string();
                if let Err(e) = self.db.move_fingerprint_path(&old_str, &new_str).await {
                    warn!(
                        "Failed to migrate fingerprint from unassigned occupant '{}': {}",
                        target_path.display(),
                        e
                    );
                }
            }
            CollisionKind::AssignedOccupant => {
                let org_config = self.db.get_organization_config().await.unwrap_or_default();
                match jumbie_shared::config::CollisionStrategy::from_config(&org_config) {
                    jumbie_shared::config::CollisionStrategy::Overwrite => {
                        warn!(
                            "Permanent conflict at '{}' (collision_handling={}), overwriting",
                            target_path.display(),
                            org_config.collision_handling
                        );
                    }
                    jumbie_shared::config::CollisionStrategy::Skip => {
                        info!(
                            "Permanent conflict at '{}' (collision_handling={}), skipping move",
                            target_path.display(),
                            org_config.collision_handling
                        );
                        return Ok(target_path);
                    }
                    jumbie_shared::config::CollisionStrategy::Rename => {
                        warn!(
                            "Permanent conflict at '{}' (collision_handling={}), resolving suffix",
                            target_path.display(),
                            org_config.collision_handling
                        );
                        target_path = self
                            .resolve_collision(&target_path, org_config.collision_rename_suffix);
                    }
                }
            }
        }

        if let Some(parent) = target_path.parent() {
            tracing::trace!("[MOVE] create_dir_all: '{}'", parent.display());
            tokio::fs::create_dir_all(parent).await?;
            trace!("Created parent directory: '{}'", parent.display());
        }

        tracing::debug!(
            "[MOVE] rename: '{}' -> '{}'",
            src.display(),
            target_path.display()
        );
        match tokio::fs::rename(src, &target_path).await {
            Ok(_) => {
                let src_str = src.to_string_lossy().to_string();
                let dst_str = target_path.to_string_lossy().to_string();
                if let Err(e) = self.db.move_fingerprint_path(&src_str, &dst_str).await {
                    warn!(
                        "Failed to migrate fingerprint from '{}' to '{}': {}",
                        src_str, dst_str, e
                    );
                }
            }
            Err(e) if crate::platform::is_cross_device_error(&e) => {
                // EXDEV: fall back to a chunked copy (cancellation-aware) + delete.
                warn!(
                    "Cross-device rename (EXDEV) from '{}' to '{}', falling back to copy+delete",
                    src.display(),
                    target_path.display()
                );
                Self::copy_with_cancellation(src, &target_path, &self.shutdown_token)
                    .await
                    .context("Failed to copy file across devices (or cancelled)")?;

                if let Err(e) = crate::platform::remove_file_tolerating_locks(src).await {
                    warn!(
                        "Failed to remove source after cross-device copy '{}': {}. \
                         The file has been copied to '{}' successfully, but the original \
                         could not be deleted. It can be removed manually.",
                        src.display(),
                        e,
                        target_path.display()
                    );
                }

                let src_str = src.to_string_lossy().to_string();
                let dst_str = target_path.to_string_lossy().to_string();
                if let Err(e) = self.db.move_fingerprint_path(&src_str, &dst_str).await {
                    warn!(
                        "Failed to migrate fingerprint from '{}' to '{}': {}",
                        src_str, dst_str, e
                    );
                }
            }
            Err(e) => {
                return Err(e).context("Failed to move file")?;
            }
        }

        if let Some(parent) = src.parent() {
            // stop_on = src.parent() — the file's immediate parent directory.
            // For UUID-isolated downloads, this is the UUID folder itself.
            remove_empty_ancestors(parent).await;
        }
        tracing::debug!(
            "[MOVE] done: '{}' -> '{}'",
            src.display(),
            target_path.display()
        );

        Ok(target_path)
    }

    /// Resolve a naming conflict by appending a uniquifying suffix.
    /// SSoT: delegates to the unified engine `paths::next_free_suffixed_path`.
    pub(crate) fn resolve_collision(
        &self,
        target: &Path,
        suffix: CollisionRenameSuffix,
    ) -> PathBuf {
        crate::paths::next_free_suffixed_path(target, suffix, true, |p| p.exists())
            .unwrap_or_else(|| target.to_path_buf())
    }
}

#[cfg(test)]
mod empty_download_dir_tests {
    use super::*;
    use std::fs;

    /// Create a temp tree from relative file paths (parent dirs included).
    fn setup(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for rel in files {
            let p = dir.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
        }
        dir
    }

    #[test]
    fn staging_dir_is_first_component_below_root() {
        let root = PathBuf::from("/downloads");
        let roots = std::slice::from_ref(&root);

        // Dir torrent: content dir is one level below the UUID staging folder.
        assert_eq!(
            download_staging_dir(Path::new("/downloads/uuid/Torrent.Name/E01.mkv"), roots),
            Some(PathBuf::from("/downloads/uuid"))
        );
        // Single-file torrent: parent is the UUID folder.
        assert_eq!(
            download_staging_dir(Path::new("/downloads/uuid/E01.mkv"), roots),
            Some(PathBuf::from("/downloads/uuid"))
        );
        // Outside every root → none (library-side move).
        assert_eq!(
            download_staging_dir(Path::new("/library/Show/E01.mkv"), roots),
            None
        );
        // The root itself is never returned.
        assert_eq!(download_staging_dir(&root, roots), None);
    }

    #[tokio::test]
    async fn removes_empty_staging_folder_but_keeps_root() {
        let tmp = setup(&["uuid/Torrent.Name/E01.mkv"]);
        let root = tmp.path().to_path_buf();
        let file = root.join("uuid/Torrent.Name/E01.mkv");
        fs::remove_file(&file).unwrap();

        remove_empty_download_dir(&file, std::slice::from_ref(&root)).await;

        assert!(
            !root.join("uuid").exists(),
            "staging folder should be culled"
        );
        assert!(root.exists(), "download root must remain");
    }

    #[tokio::test]
    async fn keeps_staging_folder_with_remaining_files() {
        let tmp = setup(&["uuid/Torrent.Name/E01.mkv", "uuid/Torrent.Name/E02.mkv"]);
        let root = tmp.path().to_path_buf();
        let file = root.join("uuid/Torrent.Name/E01.mkv");
        fs::remove_file(&file).unwrap();

        remove_empty_download_dir(&file, std::slice::from_ref(&root)).await;

        assert!(root.join("uuid/Torrent.Name/E02.mkv").exists());
        assert!(root.join("uuid").exists());
    }

    #[tokio::test]
    async fn noop_outside_roots_and_for_a_root_path() {
        let tmp = setup(&["library/Show/E01.mkv"]);
        let download_root = tmp.path().join("downloads");
        fs::create_dir_all(&download_root).unwrap();
        let roots = std::slice::from_ref(&download_root);

        // A path outside every root must not cull anything.
        let file = tmp.path().join("library/Show/E01.mkv");
        fs::remove_file(&file).unwrap();
        remove_empty_download_dir(&file, roots).await;
        assert!(tmp.path().join("library/Show").exists());

        // A path that is exactly a configured root is never removed.
        remove_empty_download_dir(&download_root, roots).await;
        assert!(download_root.exists(), "root itself must survive");
    }

    #[tokio::test]
    async fn unrelated_empty_dirs_in_the_root_are_untouched() {
        let tmp = setup(&["uuid/Torrent.Name/E01.mkv"]);
        let root = tmp.path().to_path_buf();
        // An unrelated empty folder sitting directly in the shared download root.
        fs::create_dir_all(root.join("Other.Unrelated.Empty")).unwrap();
        let file = root.join("uuid/Torrent.Name/E01.mkv");
        fs::remove_file(&file).unwrap();

        remove_empty_download_dir(&file, std::slice::from_ref(&root)).await;

        assert!(
            !root.join("uuid").exists(),
            "this download's staging folder is culled"
        );
        assert!(
            root.join("Other.Unrelated.Empty").exists(),
            "unrelated empty dirs in the root are preserved"
        );
        assert!(root.exists(), "the download root remains");
    }

    #[tokio::test]
    async fn deepest_matching_root_wins_when_roots_nest() {
        // Nested roots: the more specific root must win so the boundary stays the
        // download's own UUID folder, not the shared parent that is itself a root.
        let tmp = setup(&["outer/uuid/content/E01.mkv"]);
        let outer = tmp.path().to_path_buf();
        let inner = outer.join("outer");
        let file = inner.join("uuid/content/E01.mkv");
        fs::remove_file(&file).unwrap();
        fs::remove_dir(inner.join("uuid/content")).unwrap();

        remove_empty_download_dir(&file, &[outer.clone(), inner.clone()]).await;

        assert!(inner.exists(), "the nested configured root must survive");
        assert!(
            !inner.join("uuid").exists(),
            "the deeper staging folder is still culled"
        );
    }
}
