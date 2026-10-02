//! Series API route handlers — split into focused sub-modules.
//!
//! All public items are re-exported here so callers keep using
//! `crate::api_routes::series::*`; sub-modules stay `pub(crate)` behind this
//! stable interface.

mod clear_cache;
mod crud;
mod episodes;
pub(crate) mod files;
pub(crate) mod helpers;
mod import;
mod list;
mod metadata;
pub(crate) mod monitor;
mod reorganize;
pub mod restore;
mod update;

// Re-exports

pub use list::{build_season_list, get_series, get_series_details, get_series_details_batch};

pub use update::{UpdateSeriesResponse, handle_mode_switch, update_series};

pub use episodes::{
    UpdateEstimatedReleasePayload, batch_monitor_episodes, monitor_episodes, scan_episode_media,
    toggle_episode_monitor, update_est_date,
};

pub use reorganize::{
    calculate_rename_plan_hash, handle_reorganization_success, reorganize_all_impl,
    reorganize_all_series, reorganize_all_series_async, reorganize_all_status, reorganize_series,
};

pub use metadata::{
    batch_fetch_metadata, clear_episode_metadata, fetch_metadata, fetch_metadata_for_series,
    fetch_series_aliases, fetch_series_info, match_season_to_provider, match_series_to_provider,
    restore_episode_metadata, save_custom_metadata, sync_metadata,
};

pub use crud::{
    batch_edit_series, batch_remove_series, batch_upsert_seasons, create_series,
    delete_episode_data, delete_season, delete_season_episode_data, remove_series,
    reset_configuration, reset_season_configuration,
};

pub use import::{bulk_create_series, preview_import};

pub use clear_cache::clear_metadata_cache;
pub use monitor::{apply_monitor_mode, batch_apply_monitor_mode, sweep_monitor_status};
