//! Shared helpers for series-level file operations.
//! Used by both `update_series` (single-series move) and `batch_move_series` (batch move).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::api::AppState;
use crate::error::AppError;
use crate::file_manager::file_ops::remove_if_empty_dir;
use jumbie_shared::types::PathOperation;

// Recursive directory copy (with optional shutdown awareness)

/// Recursively copy a directory (EXDEV fallback when `rename` fails across
/// filesystems). When `cancel` is provided, checks between file copies and, on
/// cancellation or any failure, cleans up partial files at the destination.
/// SSoT for recursive directory copy.
pub async fn copy_dir_recursively(
    src: &Path,
    dst: &Path,
    cancel: Option<&CancellationToken>,
) -> Result<(), String> {
    tokio::fs::create_dir_all(dst)
        .await
        .map_err(|e| format!("Failed to create destination dir: {}", e))?;

    // Track created files/dirs so we can clean up on failure
    let mut created: Vec<PathBuf> = Vec::new();

    let result = copy_dir_recursively_inner(src, dst, &mut created, cancel).await;

    if result.is_err() {
        // Clean up partial copies — remove in reverse order (files before dirs)
        for path in created.iter().rev() {
            if path.is_dir() {
                let _ = tokio::fs::remove_dir(path).await;
            } else {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
        // Remove the top-level destination dir
        let _ = tokio::fs::remove_dir(dst).await;
    }

    result
}

/// Inner recursive copy — accumulates created paths for cleanup on failure.
/// Checks `cancel` between file copies when provided.
async fn copy_dir_recursively_inner(
    src: &Path,
    dst: &Path,
    created: &mut Vec<PathBuf>,
    cancel: Option<&CancellationToken>,
) -> Result<(), String> {
    // Check cancellation before reading each directory.
    if let Some(c) = cancel
        && c.is_cancelled()
    {
        return Err("Copy cancelled by shutdown signal".to_string());
    }

    let mut entries = tokio::fs::read_dir(src)
        .await
        .map_err(|e| format!("Failed to read source dir: {}", e))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read directory entry: {}", e))?
    {
        // Check cancellation between entries.
        if let Some(c) = cancel
            && c.is_cancelled()
        {
            return Err("Copy cancelled by shutdown signal".to_string());
        }

        let file_type = entry
            .file_type()
            .await
            .map_err(|e| format!("Failed to get file type: {}", e))?;
        let src_path = entry.path();
        let rel_path = src_path
            .strip_prefix(src)
            .map_err(|_| "Failed to compute relative path".to_string())?;
        let dst_path = dst.join(rel_path);

        if file_type.is_dir() {
            tokio::fs::create_dir_all(&dst_path)
                .await
                .map_err(|e| format!("Failed to create subdir: {}", e))?;
            created.push(dst_path.clone());
            Box::pin(copy_dir_recursively_inner(
                &src_path, &dst_path, created, cancel,
            ))
            .await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path)
                .await
                .map_err(|e| format!("Failed to copy file: {}", e))?;
            created.push(dst_path);
        }
    }

    Ok(())
}

// move_series_directory is the single place that performs filesystem operations on
// an entire series directory (Move/Copy/Delete/DoNothing), including EXDEV fallback
// and empty-parent cleanup. It does NOT update DB paths — the caller does.
// process_batch_move_path and update_series both delegate here.

/// Move a series directory from `source` to `destination` according to `operation`.
///
/// Returns an error if the filesystem operation fails. On success, the caller is
/// responsible for updating the database to reflect the new path.
pub async fn move_series_directory(
    state: &Arc<AppState>,
    source: &Path,
    destination: &Path,
    operation: PathOperation,
    shutdown_token: &CancellationToken,
) -> Result<PathBuf, AppError> {
    // Cancel pending scan-queue entries for the source path (they would be wasted I/O).
    state.scan_queue.cancel_paths_with_prefix(source).await;

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            AppError::Internal(anyhow::anyhow!(
                "Failed to create parent directory '{}': {}",
                parent.display(),
                e
            ))
        })?;
    }

    // If source doesn't exist, create destination dir (for Delete/DoNothing
    // where source may have already been removed, or Move where the directory
    // is a new creation).
    if !source.exists() {
        tokio::fs::create_dir_all(destination).await.map_err(|e| {
            AppError::Internal(anyhow::anyhow!(
                "Failed to create destination directory '{}': {}",
                destination.display(),
                e
            ))
        })?;
        return Ok(destination.to_path_buf());
    }

    // Execute the requested operation.
    match operation {
        PathOperation::Move => {
            match tokio::fs::rename(source, destination).await {
                Ok(_) => {
                    tracing::debug!("Moved '{}' → '{}'", source.display(), destination.display());
                }
                Err(e) if crate::platform::is_cross_device_error(&e) => {
                    // EXDEV (cross-device link): copy + delete with cancellation
                    tracing::warn!(
                        "EXDEV move '{}' → '{}', falling back to copy+delete",
                        source.display(),
                        destination.display()
                    );
                    copy_dir_recursively(source, destination, Some(shutdown_token))
                        .await
                        .map_err(|e| {
                            AppError::Internal(anyhow::anyhow!(
                                "Cross-device copy failed for '{}' → '{}': {}",
                                source.display(),
                                destination.display(),
                                e
                            ))
                        })?;

                    if let Err(e) = crate::platform::remove_dir_all_tolerating_locks(source).await {
                        tracing::warn!(
                            "Failed to remove source after EXDEV copy '{}': {}. \
                             The files have been copied to '{}' successfully, but the \
                             original could not be deleted. It can be removed manually.",
                            source.display(),
                            e,
                            destination.display()
                        );
                    }
                }
                Err(e) => {
                    return Err(AppError::Internal(anyhow::anyhow!(
                        "Failed to move '{}' → '{}': {}",
                        source.display(),
                        destination.display(),
                        e
                    )));
                }
            }

            if let Some(parent) = source.parent() {
                remove_if_empty_dir(parent).await;
            }
        }

        PathOperation::Copy => {
            if source.exists() {
                copy_dir_recursively(source, destination, Some(shutdown_token))
                    .await
                    .map_err(|e| {
                        AppError::Internal(anyhow::anyhow!(
                            "Failed to copy '{}' → '{}': {}",
                            source.display(),
                            destination.display(),
                            e
                        ))
                    })?;
                tracing::debug!(
                    "Copied '{}' → '{}'",
                    source.display(),
                    destination.display()
                );
            }
        }

        PathOperation::Delete => {
            if source.exists() {
                tokio::fs::remove_dir_all(source).await.map_err(|e| {
                    AppError::Internal(anyhow::anyhow!(
                        "Failed to delete '{}': {}",
                        source.display(),
                        e
                    ))
                })?;
                tracing::debug!("Deleted '{}'", source.display());
            }

            // Recreate destination as empty directory so the series path exists
            tokio::fs::create_dir_all(destination).await.map_err(|e| {
                AppError::Internal(anyhow::anyhow!(
                    "Failed to create destination '{}' after delete: {}",
                    destination.display(),
                    e
                ))
            })?;

            // Clean up empty parent directory
            if let Some(parent) = source.parent() {
                remove_if_empty_dir(parent).await;
            }
        }

        PathOperation::DoNothing => {
            tokio::fs::create_dir_all(destination).await.map_err(|e| {
                AppError::Internal(anyhow::anyhow!(
                    "Failed to create destination '{}': {}",
                    destination.display(),
                    e
                ))
            })?;
        }
    }

    Ok(destination.to_path_buf())
}

/// Precomputed `path → series_id` index for O(1) path lookups.
///
/// Batch operations build this ONCE from `(all_mappings, all_roots, org)` instead of
/// re-scanning every mapping per path (which the collision closures multiplied by
/// suffix candidates). Raw candidates (explicit path, `${series}` template,
/// synthesized `root/title`) and their normalized forms are both keyed
/// (first-claim-wins); the org policy is applied as at creation, so a folder named
/// with the SANITIZED title matches its mapping.
pub struct SeriesPathIndex {
    index: std::collections::HashMap<std::path::PathBuf, String>,
}

impl SeriesPathIndex {
    /// Build the index for a snapshot of mappings/roots under one org config.
    pub fn build(
        all_mappings: &std::collections::HashMap<String, jumbie_shared::types::MappingRule>,
        all_roots: &[std::path::PathBuf],
        org: &jumbie_shared::config::organization::OrganizationConfig,
    ) -> Self {
        let mut index = std::collections::HashMap::new();
        for (id, mapping) in all_mappings {
            let possible_paths: Vec<std::path::PathBuf> = if let Some(p) = &mapping.settings.path {
                if p.is_empty() {
                    all_roots
                        .iter()
                        .map(|r| r.join(crate::paths::sanitize_title(&mapping.target_title, org)))
                        .collect()
                } else {
                    vec![crate::paths::resolve_template(
                        p,
                        &mapping.target_title,
                        org,
                    )]
                }
            } else {
                all_roots
                    .iter()
                    .map(|r| r.join(crate::paths::sanitize_title(&mapping.target_title, org)))
                    .collect()
            };

            for mapped_path in &possible_paths {
                // First-claim-wins: identical raw or normalized candidates map to
                // the first series that claims them.
                index
                    .entry(mapped_path.clone())
                    .or_insert_with(|| id.clone());
                index
                    .entry(crate::validation::normalize_path(mapped_path))
                    .or_insert_with(|| id.clone());
            }
        }
        Self { index }
    }

    /// Look up the series_id claiming `path_str`, or `None`. The raw form is checked
    /// first, then the normalized query path.
    pub fn get(&self, path_str: &str) -> Option<&str> {
        let target_path = std::path::Path::new(path_str);
        if let Some(id) = self.index.get(target_path) {
            return Some(id);
        }
        let canonical = crate::validation::normalize_path(target_path);
        if let Some(id) = self.index.get(&canonical) {
            return Some(id);
        }
        None
    }
}

/// Find a series_id for a given absolute path by matching against all known series
/// mappings and destination roots.
///
/// Thin wrapper over [`SeriesPathIndex`] that builds the index inline, for call
/// sites doing a single lookup; batch operations should build the index once.
///
/// `org` supplies the illegal-char policy so synthesized/template candidates are
/// sanitized exactly like creation and `get_organized_series` — otherwise a folder
/// named with a sanitized title would never match its mapping.
pub fn find_series_id_by_path(
    path_str: &str,
    all_mappings: &std::collections::HashMap<String, jumbie_shared::types::MappingRule>,
    all_roots: &[std::path::PathBuf],
    org: &jumbie_shared::config::organization::OrganizationConfig,
) -> Option<String> {
    SeriesPathIndex::build(all_mappings, all_roots, org)
        .get(path_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::config::organization::{InvalidCharPolicy, OrganizationConfig};
    use jumbie_shared::types::{MappingRule, SeriesSettings};
    use std::collections::HashMap;

    fn make_mapping(id: &str, title: &str, path: Option<&str>) -> (String, MappingRule) {
        (
            id.to_string(),
            MappingRule {
                target_title: title.to_string(),
                series_id: id.to_string(),
                settings: SeriesSettings {
                    path: path.map(|s| s.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
    }

    fn org_with(policy: InvalidCharPolicy) -> OrganizationConfig {
        OrganizationConfig {
            illegal_char_policy: policy,
            ..Default::default()
        }
    }

    #[test]
    fn index_matches_explicit_concrete_path() {
        let path = std::env::temp_dir().join("jumbie_index_explicit");
        std::fs::create_dir_all(&path).unwrap();

        let mappings = HashMap::from([make_mapping("s1", "Show", Some(path.to_str().unwrap()))]);
        let index = SeriesPathIndex::build(&mappings, &[], &OrganizationConfig::default());
        assert_eq!(index.get(path.to_str().unwrap()), Some("s1"));
        assert_eq!(index.get("/nonexistent/nowhere"), None);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn index_resolves_template_with_sanitized_title() {
        let mappings = HashMap::from([make_mapping(
            "s1",
            "Show: Part",
            Some("/media/tv/${series}"),
        )]);
        let org = org_with(InvalidCharPolicy::Underscore);
        let index = SeriesPathIndex::build(&mappings, &[], &org);

        // The SANITIZED folder is what the organizer actually creates.
        assert_eq!(index.get("/media/tv/Show_ Part"), Some("s1"));
        // The raw title would never match.
        assert_eq!(index.get("/media/tv/Show: Part"), None);
    }

    #[test]
    fn index_synthesizes_from_roots_when_path_unset() {
        let roots = vec![
            std::path::PathBuf::from("/media/tv"),
            std::path::PathBuf::from("/data"),
        ];
        let mappings = HashMap::from([make_mapping("s1", "The Time Machine", None)]);
        let index = SeriesPathIndex::build(&mappings, &roots, &OrganizationConfig::default());
        assert_eq!(index.get("/media/tv/The Time Machine"), Some("s1"));
        assert_eq!(index.get("/data/The Time Machine"), Some("s1"));
    }

    #[test]
    fn index_first_claim_wins_on_identical_candidates() {
        // Two series claiming the same synthesized path — the first inserted
        // wins (HashMap iteration order, same as the old scan).
        let roots = vec![std::path::PathBuf::from("/media/tv")];
        let mappings = HashMap::from([
            make_mapping("s1", "Duplicate Title", None),
            make_mapping("s2", "Duplicate Title", None),
        ]);
        let index = SeriesPathIndex::build(&mappings, &roots, &OrganizationConfig::default());
        let found = index.get("/media/tv/Duplicate Title");
        assert!(
            found == Some("s1") || found == Some("s2"),
            "one of the duplicate series must claim the path, got {:?}",
            found
        );
    }

    #[test]
    fn index_matches_symlinked_mapping_dir() {
        // The old fallback (find_colliding_series) canonicalized BOTH sides:
        // a mapping whose stored path is a symlink to the query's real dir
        // must match. The index pre-canonicalizes candidates, so this works
        // with no per-query fallback.
        let real_dir = std::env::temp_dir().join("jumbie_index_real");
        let link_dir = std::env::temp_dir().join("jumbie_index_link");
        std::fs::create_dir_all(&real_dir).unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();
            let mappings =
                HashMap::from([make_mapping("s1", "Show", Some(link_dir.to_str().unwrap()))]);
            let index = SeriesPathIndex::build(&mappings, &[], &OrganizationConfig::default());
            // Query by the REAL path — canonicalized candidate matches.
            assert_eq!(index.get(real_dir.to_str().unwrap()), Some("s1"));
            // Query by the symlink itself — raw candidate matches.
            assert_eq!(index.get(link_dir.to_str().unwrap()), Some("s1"));
            let _ = std::fs::remove_dir_all(&link_dir);
        }

        #[cfg(not(unix))]
        {
            let _ = &link_dir;
        }

        let _ = std::fs::remove_dir_all(&real_dir);
    }

    #[test]
    fn wrapper_and_index_agree() {
        let roots = vec![std::path::PathBuf::from("/media/tv")];
        let mappings = HashMap::from([
            make_mapping("s1", "Show: Part", Some("/media/tv/${series}")),
            make_mapping("s2", "Other Show", None),
        ]);
        let org = org_with(InvalidCharPolicy::Underscore);
        let index = SeriesPathIndex::build(&mappings, &roots, &org);

        for probe in [
            "/media/tv/Show_ Part",
            "/media/tv/Other Show",
            "/media/tv/Missing",
        ] {
            assert_eq!(
                index.get(probe).map(str::to_string),
                find_series_id_by_path(probe, &mappings, &roots, &org),
                "index and wrapper must agree for '{}'",
                probe
            );
        }
    }
}
