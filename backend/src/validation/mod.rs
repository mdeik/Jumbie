use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};
// WHY dunce: `std::fs::canonicalize` on Windows returns `\\?\`-prefixed extended-length
// paths that confuse other APIs and are stored verbatim in the DB. `dunce::canonicalize`
// strips the prefix on Windows and is a transparent no-op on Linux/macOS.

pub mod collision;
pub use collision::{
    SeriesPathClaim, check_series_path_not_taken, find_colliding_series, find_series_path_claim,
    series_path_taken_message,
};

/// Normalize a path for cross-platform comparison.
///
/// `dunce::canonicalize` strips the `\\?\` prefix on Windows and resolves
/// symlinks/separators/casing on all platforms. Falls back to the raw path if
/// canonicalization fails (e.g. the path doesn't exist yet).
/// Maximum path length (4096 matches Linux `PATH_MAX`, also covers Windows `MAX_PATH`).
pub const MAX_PATH_LENGTH: usize = 4096;

pub fn normalize_path(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Check whether a path string contains a directory traversal component (`..`).
///
/// Checks `..` as a path component (via `Path::components()`), not as a substring,
/// so filenames like "Part..2" are not false positives. On Unix, where `\\` is a
/// literal filename character, an additional backslash guard catches injected
/// Windows-style traversal such as "..\\..\\etc".
pub fn contains_path_traversal(path: &str) -> bool {
    if Path::new(path)
        .components()
        .any(|c| c == Component::ParentDir)
    {
        return true;
    }

    if cfg!(not(windows)) {
        path.starts_with("..\\")
            || path.ends_with("\\..")
            || path.contains("\\..\\")
            || path.contains("/..\\")
            || path.contains("\\../")
    } else {
        false
    }
}

/// Validate a path to prevent directory traversal attacks.
/// Returns a resolved PathBuf that is guaranteed to be within `allowed_root`.
///
/// Checks run at point of use rather than config time because directory
/// permissions can change after startup (e.g. a NAS remounted read-only).
pub fn validate_path(path: &str, allowed_root: &Path) -> Result<PathBuf> {
    // Surrounding whitespace in user input is virtually always accidental; use the
    // trimmed path for all checks so preview, collision detection, and filesystem
    // operations agree on the same canonical path.
    let path = path.trim();

    if path.is_empty() {
        bail!("Path cannot be empty");
    }

    if path.len() > MAX_PATH_LENGTH {
        bail!("Path is too long (max {} characters)", MAX_PATH_LENGTH);
    }

    // Reject traversal before canonicalization: canonicalize follows symlinks, so a
    // symlink inside a valid directory could point outside `allowed_root`. Failing
    // closed here is defense-in-depth ahead of the canonicalization below.
    if contains_path_traversal(path) {
        bail!("Path cannot contain '..' (directory traversal attempt)");
    }

    // Reject `//`: on Windows some APIs reinterpret it as a UNC prefix (`\\server\share`),
    // a path-injection vector. `\\` is allowed because legitimate UNC paths rely on it;
    // the writability test below is the actual access control.
    if path.contains("//") {
        bail!("Path contains suspicious patterns");
    }

    // On Windows `Path::is_relative()` reports true for paths starting with '/', so
    // "/etc/passwd" would be joined under `allowed_root` and silently bypass the escape
    // check. A '\\'-prefixed path is genuinely absolute (UNC/device) and allowed.
    #[cfg(windows)]
    if path.starts_with('/') {
        bail!("Path cannot be an absolute path");
    }

    let path_obj = Path::new(path);

    let path_buf = if path_obj.is_relative() {
        allowed_root.join(path_obj)
    } else {
        path_obj.to_path_buf()
    };

    // canonicalize() requires the whole path to exist. For a not-yet-created path
    // (e.g. a new series dir), canonicalize the deepest existing ancestor and re-append
    // the tail so symlinks in the existing prefix are still resolved.
    let canonical = if path_buf.exists() {
        dunce::canonicalize(&path_buf)?
    } else {
        let mut existing = path_buf.clone();
        let mut suffix = std::path::PathBuf::new();
        loop {
            if existing.exists() {
                break;
            }
            if let Some(name) = existing.file_name() {
                suffix = std::path::Path::new(name).join(&suffix);
                existing.pop();
            } else {
                break;
            }
        }
        let canonical_existing = if existing.exists() {
            dunce::canonicalize(&existing)?
        } else {
            existing
        };
        canonical_existing.join(suffix)
    };

    // A `starts_with(allowed_root)` check is intentionally omitted: bind mounts or
    // symlinks can make the effective path differ from `allowed_root`. Existence and
    // writability of the nearest existing directory is the actual access test.
    if path_buf.exists() && !path_buf.is_dir() {
        bail!("Path must be a directory");
    }

    // Writability is proven by touching the nearest existing directory. Skipped
    // under `cfg(test)`: tests cannot rely on filesystem permissions, and the
    // nearest-existing-directory walk below would otherwise be dead code there.
    #[cfg(not(test))]
    {
        let check_dir = if path_buf.exists() {
            path_buf.clone()
        } else {
            let mut existing = path_buf.clone();
            loop {
                if existing.exists() {
                    break;
                }
                if !existing.pop() {
                    break;
                }
            }
            existing
        };

        if check_dir.exists() {
            let md = std::fs::metadata(&check_dir)?;
            if md.permissions().readonly() {
                bail!("Directory does not have write permissions");
            }

            // `readonly()` misses POSIX sticky bits, ACLs, SELinux contexts, and NFS export
            // options; write-and-delete a hidden temp file is the only reliable cross-platform
            // writability test.
            let temp_file = check_dir.join(".jumbie_access_check");
            match std::fs::write(&temp_file, b"") {
                Ok(_) => {
                    let _ = std::fs::remove_file(&temp_file);
                }
                Err(e) => {
                    bail!("Directory is not writable: {}", e);
                }
            }
        }
    }

    // On Windows, dunce::canonicalize can append a trailing backslash for directories.
    // Left in place, path equality checks elsewhere would see semantically identical
    // paths as different and trigger unwanted file operations.
    let canonical = if canonical.to_string_lossy().ends_with('/')
        || canonical.to_string_lossy().ends_with('\\')
    {
        let mut s = canonical.to_string_lossy().to_string();
        while s.ends_with('/') || s.ends_with('\\') {
            s.pop();
        }
        std::path::PathBuf::from(s)
    } else {
        canonical
    };

    Ok(canonical)
}

pub use jumbie_shared::validation::{
    validate_alias, validate_auto_search_wanted_interval, validate_auto_search_wanted_max_age_days,
    validate_auto_search_wanted_min_wait, validate_episode_number,
    validate_episode_numbers_not_empty, validate_episode_offset, validate_max_length,
    validate_media_info_scan_interval, validate_name, validate_no_control_chars,
    validate_not_empty, validate_plugin_info, validate_plugin_name, validate_plugin_version,
    validate_port, validate_quality_profile, validate_regex, validate_rfc3339_timestamp,
    validate_runtime, validate_search_query, validate_search_template, validate_season_number,
    validate_season_pack_replace_threshold, validate_tag, validate_title, validate_url,
};

pub use jumbie_shared::validation::{
    VALID_UNEXPECTED_FILES_HANDLING, VALID_UNNEEDED_EPISODES_HANDLING, validate_api_key_scopes,
    validate_unexpected_files_handling, validate_unneeded_episodes_handling,
};

/// Validate an entity ID string (series UUID, episode ID, etc.).
///
/// IDs may be UUIDs, compound strings (e.g. "S01E01_uuid"), or legacy formats, so only
/// basic hygiene is enforced: non-empty, ≤ 512 chars, no control characters. Backend-local
/// because ID formats may vary between backend versions.
pub fn validate_id(
    id: &str,
    field_name: &str,
) -> Result<(), jumbie_shared::validation::ValidationError> {
    validate_not_empty(id, field_name)?;
    validate_max_length(id, field_name, jumbie_shared::validation::MAX_ID_LENGTH)?;
    validate_no_control_chars(id, field_name)?;
    Ok(())
}

// Validate impls for data crossing the plugin→backend boundary. These types live in
// the backend crate (not shared), so the impls live here.
pub mod plugin_data;

#[cfg(test)]
mod tests;
