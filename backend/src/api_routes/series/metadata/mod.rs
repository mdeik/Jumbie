//! Metadata endpoint handlers — split into focused sub-modules.
//!
//! All public items are re-exported here so callers keep using
//! `crate::api_routes::series::metadata::*`.

pub(crate) mod batch;
pub(crate) mod custom;
pub(crate) mod fetch;
pub(crate) mod sync;

#[cfg(test)]
mod tests;

pub use batch::batch_fetch_metadata;
pub use custom::{
    clear_episode_metadata, match_season_to_provider, match_series_to_provider,
    restore_episode_metadata, save_custom_metadata,
};
pub use fetch::{
    fetch_metadata, fetch_metadata_for_series, fetch_series_aliases, fetch_series_info,
    sync_series_metadata,
};
pub use sync::sync_metadata;
