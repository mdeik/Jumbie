pub mod dir_mtimes;
pub mod import;
pub mod parts;
pub mod preview;
pub mod scan;

#[cfg(test)]
pub mod tests;

// Re-export public API
pub use dir_mtimes::{collect_dir_mtimes, has_any_dir_changed, stable_hash_series_id};
pub use import::{import_scan_for_series, scan_series_directory};
pub use parts::register_part_file;
pub use preview::quick_preview_directory;
pub use scan::scan_directory;

// SSoT: effective-season resolution (season alias → filename → folder → default)
// lives in the shared crate (`jumbie_shared::mapping::season_resolve`) so the
// frontend "Manage Series Files" auto-assign flow can never drift from what a
// scan produces. Re-exported here to keep `crate::scanner::*` call sites intact.
pub(crate) use jumbie_shared::mapping::{
    ResolvedSeason, resolve_season_for_series, resolve_season_for_series_with_fallback,
    resolve_season_raw,
};
