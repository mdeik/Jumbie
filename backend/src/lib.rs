// Console-output SSoT: application logging goes through `tracing` (always to the
// log file + in-memory buffer, and to stdout only via the release-gated
// `logging::console_layer`). Direct print macros are denied; the sanctioned
// exceptions route through `logging::{pre_init_warning,report}`, which are the
// only raw stdout/stderr writers. Exempt `cfg(test)` so unit tests can emit
// diagnostics. A raw stdout/stderr writer is not a print macro and is instead
// guarded by the grep in .github/workflows/rust.yml.
#![cfg_attr(
    not(test),
    deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)
)]

// Platform-specific allocator selection (jemalloc on Linux/macOS, system
// allocator on Windows). `pub` so the binary crate and integration tests share
// the same global allocator instance.
pub mod alloc;

// Size-rolled log files (2 MiB files, 8 MiB disk budget), shared by the setup
// writer and the logs API.
pub mod logging;

// `api` contains endpoint logic; `api_routes` wires URL paths to handlers.
pub mod api;
pub mod api_routes;

// Cross-cutting infrastructure exposed for companion binaries (CLI scanners,
// admin tools): auth helpers, the database pool, and the unified error type.
pub mod auth_utils;
pub mod db;
pub mod error;

// `download_orchestrator` manages sequencing, retries, and cancellation.
// Downloader implementations live under `plugins::downloaders`.
pub mod download_orchestrator;

// Domain modules exposed for external integration tests. Plugin traits &
// implementations live under the `plugins::` hierarchy.
pub mod file_manager;
pub mod middleware;
pub mod models;
pub mod organizer;
pub mod paths;
pub mod scanner;
pub mod source_processor;
pub mod task;

// `utils` and `validation` live at crate root so every domain module can
// consume them without risking circular dependencies.
pub mod config_manager;
pub mod datetime;
pub mod embedded_frontend;
pub mod platform;
pub mod state;
pub mod tray;
pub mod utils;
pub mod validation;

// `search` provides SSoT functions for payload construction and alias routing
// used by both `auto_search_season` and `auto_search_missing`.
pub mod search;

// `release_checks` centralizes the acceptance gates auto-search applies, so
// manual search can report the same gates without duplicating the rules.
pub mod release_checks;

// Plugins are self-contained extensions; `patterns` holds matching/extraction
// rules shared across domain modules.
pub mod metadata_queue;
pub mod patterns;
pub mod plugins;
pub mod release_estimator;
pub mod search_queue;
pub use release_estimator::{
    calculate_estimations, run_release_date_estimation, trigger_estimation_for_mapping,
    trigger_estimation_for_series,
};
pub mod scan_queue;

/// Whether `ffmpeg`/`ffprobe` are on PATH at compile time (set by `build.rs`
/// via `cfg=ffmpeg_installed`). Integration tests call this instead of
/// duplicating the detection logic.
pub fn ffmpeg_installed() -> bool {
    cfg!(ffmpeg_installed)
}

// Test scaffolding (excluded from release builds): `tests::fixtures` is
// re-exported under a flat alias for integration tests in sibling crates.
#[cfg(test)]
pub use tests::fixtures as test_fixtures;
#[cfg(test)]
pub mod tests;
