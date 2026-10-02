pub mod causes;
pub mod file_ops;
pub mod moves;
pub mod organize;
pub mod path;
pub mod plan;
pub mod plan_cache;
pub mod policy;
pub mod series_ops;
pub mod sibling;

pub use causes::compute_plan_causes;
pub(crate) use file_ops::{CollisionKind, diagnose_collision};
pub use moves::move_files_cycle_safe;
pub use path::{
    format_needs_media_info, resolve_active_episode_format, resolve_active_formats,
    season_folder_name,
};
pub(crate) use plan::filename_has_part_indicator;
pub use plan::{
    AuxiliaryPath, EpisodeSummary, MappingContext, PlannedMove, RenamePlanError,
    auxiliary_paths_for_series, compute_batch_plan, compute_batch_plan_with_aux,
    get_mapping_context,
};
pub use plan_cache::{RenamePlanCache, RenamePlanInputs, filter_plan_by_skip_paths};
pub use policy::{
    ResolveMode, resolve_episode_target_path, should_flatten_season_folders,
    should_rename_episodes, should_use_absolute_numbering,
};
pub use series_ops::{copy_dir_recursively, find_series_id_by_path, move_series_directory};
#[cfg(test)]
mod tests;
