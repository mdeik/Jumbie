use crate::utils::ParseContext;
use std::path::PathBuf;

/// Preview a directory for series import — walks video files and returns
/// a summary of what would be imported.
///
/// In bulk mode, the path is treated as a root directory whose immediate
/// children are individual series folders. In single mode, the path itself
/// is evaluated as a single series directory.
pub async fn quick_preview_directory(
    path: &PathBuf,
    is_bulk: bool,
    existing_paths: &[String],
) -> Result<Vec<jumbie_shared::types::PreviewSeriesItem>, anyhow::Error> {
    let mut results = Vec::new();

    // Bulk mode: the caller already knows it's a root full of series folders
    // (e.g. a torrent watch-dir), so we enumerate immediate children only.
    // Single mode: the user selected one folder, so we evaluate that folder
    // directly.  The caller determines which path applies — not the
    // scanner — so we keep the interface simple instead of guessing.
    if is_bulk {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type()
                    && ft.is_dir()
                {
                    let series_path = entry.path();
                    if let Some(item) =
                        evaluate_single_directory_preview(&series_path, existing_paths)
                    {
                        results.push(item);
                    }
                }
            }
        }
    } else if let Some(item) = evaluate_single_directory_preview(path, existing_paths) {
        results.push(item);
    }

    Ok(results)
}

/// Evaluate a single directory and return a preview item describing its contents.
pub(crate) fn evaluate_single_directory_preview(
    path: &PathBuf,
    existing_paths: &[String],
) -> Option<jumbie_shared::types::PreviewSeriesItem> {
    let folder_name = path.file_name()?.to_string_lossy().to_string();
    let abs_path = path.to_string_lossy().to_string();

    // Normalize both the candidate and each existing path so that
    // symlink-equivalent directories are recognised as duplicates even
    // when their surface-level paths differ (e.g. /data/series/Show vs
    // /mnt/storage/Show where /data/series → /mnt/storage).
    let abs_path_canonical = crate::validation::normalize_path(path)
        .to_string_lossy()
        .to_string();
    let mut already_exists = false;
    for existing in existing_paths {
        let existing_canonical = crate::validation::normalize_path(std::path::Path::new(existing))
            .to_string_lossy()
            .to_string();
        if abs_path_canonical == existing_canonical {
            already_exists = true;
            break;
        }
    }

    let mut distinct_seasons = std::collections::HashSet::new();
    let mut episode_count = 0;
    let mut files_in_root = 0usize;
    let mut files_in_subdir = 0usize;

    for entry in walkdir::WalkDir::new(path)
        .max_depth(5) // safety: cap traversal depth to prevent runaway filesystem crawl
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let entry_path = entry.path();

        if let Some(ext) = entry_path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if jumbie_shared::media_format::is_video_ext(ext_str.as_str())
                && let Some(filename_os) = entry_path.file_name()
                && let Some(info) = crate::utils::parse_filename(
                    &filename_os.to_string_lossy(),
                    ParseContext::FileScan,
                )
            {
                // Only count a season that the filename actually declared — a
                // season-less file has no season to attribute, so inventing one
                // would inflate `season_count` below.
                if let Some(season) = info.seasons.first().copied() {
                    distinct_seasons.insert(season);
                }
                episode_count += 1;
                // Track whether the file lives directly in the series root
                // or inside a subdirectory (e.g. a "Season 01" folder).
                if entry_path.parent() == Some(path) {
                    files_in_root += 1;
                } else {
                    files_in_subdir += 1;
                }
            }
        }
    }

    let all_files_in_root = files_in_root > 0 && files_in_subdir == 0;

    Some(jumbie_shared::types::PreviewSeriesItem {
        path: abs_path,
        original_folder_name: folder_name.clone(),
        final_title: folder_name,
        season_count: distinct_seasons.len(),
        episode_count,
        selected: !already_exists,
        already_exists,
        all_files_in_root,
    })
}
