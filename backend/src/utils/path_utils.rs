use std::path::Path;

// SSoT: path-based season inference lives in the shared crate so the scanner and
// smart_link resolve folder seasons identically. Re-exported here to keep
// `crate::utils::path_utils::infer_season_from_path` call sites intact.
pub use jumbie_shared::mapping::infer_season_from_path;

/// Extension trait for `Path` to extract filenames as owned Strings.
pub trait PathExt {
    /// Extract filename as String, returning empty string if none.
    ///
    /// Returns an empty string rather than `Option`: `Path::file_name()` only
    /// returns `None` for the root directory, so this avoids `unwrap_or_default`
    /// boilerplate at nearly every call site.
    fn file_name_string(&self) -> String;
}

impl PathExt for Path {
    fn file_name_string(&self) -> String {
        self.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}
