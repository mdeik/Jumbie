use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

/// Convert a `SystemTime` to fractional seconds since UNIX epoch for mtime comparison.
fn mtime_to_f64(mtime: SystemTime) -> f64 {
    mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Collect mtimes of the series root directory and all subdirectories (up to
/// `max_depth`).  Keyed by relative path from the series root:
/// - `"."` → the root dir itself
/// - `"Season 1"` → an immediate subdirectory
/// - `"Season 1/Disc 1"` → a deeper subdirectory
///
/// Uses `WalkDir` visiting only directories (not files) so the cost is
/// O(directories) per series rather than O(files).  The `max_depth` should
/// match the scanner's own `WalkDir::max_depth()` to guarantee we never miss
/// a directory where files could be discovered during a full scan.
pub fn collect_dir_mtimes(path: &Path, max_depth: usize) -> HashMap<String, f64> {
    let mut mtimes = HashMap::new();

    // Root directory
    match std::fs::metadata(path) {
        Ok(meta) => {
            if let Ok(mtime) = meta.modified() {
                mtimes.insert(".".to_string(), mtime_to_f64(mtime));
            } else {
                tracing::trace!(
                    "collect_dir_mtimes: path '{}' does not support mtime",
                    path.display(),
                );
            }
        }
        Err(e) => {
            tracing::trace!(
                "collect_dir_mtimes: failed to stat root '{}': {}",
                path.display(),
                e,
            );
        }
    }

    // Subdirectories: filter_entry stops descent into files, then filter keeps only
    // directory entries in the yielded output.
    for entry in walkdir::WalkDir::new(path)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|e| e.file_type().is_dir())
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
    {
        if entry.path() == path {
            continue; // already recorded as "." above
        }
        let rel = entry
            .path()
            .strip_prefix(path)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| String::from("unknown"));
        match entry.metadata() {
            Ok(meta) => {
                if let Ok(mtime) = meta.modified() {
                    mtimes.insert(rel, mtime_to_f64(mtime));
                } else {
                    tracing::trace!(
                        "collect_dir_mtimes: subdir '{}' does not support mtime",
                        rel,
                    );
                }
            }
            Err(e) => {
                tracing::trace!(
                    "collect_dir_mtimes: failed to stat subdir '{}' ({}): {}",
                    rel,
                    entry.path().display(),
                    e,
                );
            }
        }
    }

    mtimes
}

/// Returns `true` if ANY tracked directory's mtime has changed beyond the
/// 0.1-second fudge factor, or if directories were added or removed since
/// the last scan.
///
/// An empty `last` map (never scanned) always returns `true`.
pub fn has_any_dir_changed(current: &HashMap<String, f64>, last: &HashMap<String, f64>) -> bool {
    if last.is_empty() {
        return true; // never scanned
    }
    for (key, cur) in current {
        match last.get(key) {
            Some(prev) if (cur - prev).abs() > 0.1 => return true,
            None => return true, // new directory
            _ => {}
        }
    }
    for key in last.keys() {
        if !current.contains_key(key) {
            return true;
        }
    }
    false
}

/// Stable, deterministic hash of a series_id string for force-scan bucketing.
/// Uses the djb2 algorithm — not cryptographic, but consistent across process
/// restarts and Rust versions (unlike `std::collections::hash_map::DefaultHasher`).
pub fn stable_hash_series_id(series_id: &str) -> u64 {
    let mut hash: u64 = 5381;
    for b in series_id.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    hash
}
