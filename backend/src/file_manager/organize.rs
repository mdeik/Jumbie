use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use anyhow::{Context, Result};
use tracing::{debug, warn};

use crate::db::DbManager;
use crate::file_manager::ResolveMode;
use crate::file_manager::path::PathBuildVars;
use crate::organizer::ContentOrganizer;
use jumbie_shared::types::EpisodeStatus;

pub struct ManualOrganizeParams<'a> {
    pub src: &'a Path,
    pub series_id: &'a str,
    pub season_val: &'a str,
    pub episode_val: i32,
    pub target_absolute: bool,
    pub batch_sources: Option<&'a HashSet<PathBuf>>,
}

/// Resolve the naming context (max episode number) for a series.
async fn resolve_mapping_context(
    db: &DbManager,
    series_id: &str,
    global_absolute: bool,
) -> anyhow::Result<crate::file_manager::MappingContext> {
    let mapping = db
        .get_series_mapping(series_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Series mapping not found"))?;
    // SSoT: effective numbering mode (series tristate → global default).
    let absolute = mapping.settings.active_mode(global_absolute).is_absolute();
    let db_episodes = db
        .get_series_episodes_details(&mapping.series_id, absolute)
        .await
        .unwrap_or_default();
    Ok(crate::file_manager::get_mapping_context(
        &db_episodes,
        absolute,
    ))
}

impl ContentOrganizer {
    // Manual Organize (explicit episode details provided by the caller)

    pub async fn manual_organize_file_explicit(
        &self,
        params: ManualOrganizeParams<'_>,
    ) -> Result<PathBuf> {
        let context = resolve_mapping_context(
            &self.db,
            params.series_id,
            self.db_config().await.general.absolute_numbering,
        )
        .await?;
        let mapping = self
            .db
            .get_series_mapping(params.series_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Series mapping not found"))?;
        let season_num = params.season_val.parse::<i32>().unwrap_or(1);

        let mut temp_mapping = mapping.clone();
        temp_mapping.settings.absolute_numbering = Some(params.target_absolute);

        let ep_title = self
            .db
            .get_episode_title_by_number(params.series_id, season_num, params.episode_val)
            .await
            .unwrap_or(None);

        let target_path = {
            let config = self.db_config().await;

            let pad_options = crate::utils::TemplatePadOptions {
                max_season: context.max_season as u32,
                max_episode: context.max_episode_for(season_num) as u32,
                max_length: 0,
            };

            let (_, _, manual_media_info) = self
                .db
                .update_file_fingerprint(params.src, EpisodeStatus::Organized.as_str())
                .await;

            let episode_var = params.episode_val.to_string();
            let tp = Self::build_target_path(&PathBuildVars {
                config: &config,
                mapping: &temp_mapping,
                season_num,
                episode_num: params.episode_val,
                episode_var: &episode_var,
                source_path: params.src,
                episode_title: ep_title.as_deref(),
                episode_quality: None,
                episode_submitter: None,
                release_date: None,
                created_at: None,
                pad_options: &pad_options,
                part_number: None,
                media_info: manual_media_info.as_ref(),
            })?;

            crate::file_manager::resolve_episode_target_path(
                params.src,
                &tp,
                &config,
                &mapping,
                ResolveMode::Apply,
            )
        };

        self.move_file_to_target(params.src, &target_path, params.batch_sources)
            .await
    }

    // After a file is assigned to an episode (assign_series_file or
    // batch_assign_series_files), move it to the directory computed by
    // build_target_path — organization always runs, even when renaming is disabled
    // (in which case the original filename is preserved). Same chain organize_file
    // and the rename queue use.

    pub async fn organize_single_assigned_file(
        &self,
        src: &Path,
        series_id: &str,
        season_val: &str,
        episode_val: i32,
    ) -> Result<Option<PathBuf>> {
        // Auxiliary sidecars (subtitle/nfo) are never the episode's playable file:
        // organizing one through the video template would move it to the video's
        // path and wrongly associate the sidecar as the episode's main file,
        // unlinking the real video. Callers link sidecars as `auxiliary` instead.
        if jumbie_shared::media_format::is_auxiliary_path(src) {
            return Ok(None);
        }

        let target_path = self
            .resolve_assigned_target_path(src, series_id, season_val, episode_val)
            .await?;

        let new_path = self.move_file_to_target(src, &target_path, None).await?;

        let mapping = self
            .db
            .get_series_mapping(series_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Series mapping not found"))?;
        let global_absolute = self.global_absolute_default().await;
        let episode_id = mapping.get_episode_id(season_val, episode_val, global_absolute)?;
        let new_path_str = new_path.to_string_lossy().to_string();
        let _ = self.db.update_file_path(&episode_id, &new_path_str).await;

        Ok(Some(new_path))
    }

    /// Resolve the final destination path for a file assigned to
    /// `(series, season, episode)` **without moving it**.
    ///
    /// Shared by `organize_single_assigned_file` and batch callers that must
    /// pre-resolve every destination up front so they can move files cycle-safely
    /// (a shift like E17→E18, E18→E19, … recycles the same paths).
    pub async fn resolve_assigned_target_path(
        &self,
        src: &Path,
        series_id: &str,
        season_val: &str,
        episode_val: i32,
    ) -> Result<PathBuf> {
        let context = resolve_mapping_context(
            &self.db,
            series_id,
            self.db_config().await.general.absolute_numbering,
        )
        .await?;
        let mapping = self
            .db
            .get_series_mapping(series_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Series mapping not found"))?;
        let season_num = season_val.parse::<i32>().unwrap_or(1);

        let ep_title = self
            .db
            .get_episode_title_by_number(series_id, season_num, episode_val)
            .await
            .unwrap_or(None);

        let config = self.db_config().await;

        let pad_options = crate::utils::TemplatePadOptions {
            max_season: context.max_season as u32,
            max_episode: context.max_episode_for(season_num) as u32,
            max_length: 0,
        };

        let episode_var = episode_val.to_string();
        let tp = Self::build_target_path(&PathBuildVars {
            config: &config,
            mapping: &mapping,
            season_num,
            episode_num: episode_val,
            episode_var: &episode_var,
            source_path: src,
            episode_title: ep_title.as_deref(),
            episode_quality: None,
            episode_submitter: None,
            release_date: None,
            created_at: None,
            pad_options: &pad_options,
            part_number: None,
            media_info: None,
        })?;

        Ok(crate::file_manager::resolve_episode_target_path(
            src,
            &tp,
            &config,
            &mapping,
            ResolveMode::Apply,
        ))
    }

    /// After a background fingerprint completes with real media_info, recompute and
    /// rename the file in-place when the episode template uses media-info variables
    /// ({width}, {codec}, …) and renaming is enabled. Cheap — the expensive
    /// fingerprint has already run.
    pub async fn maybe_rename_with_media_info(
        &self,
        file_path: &Path,
        series_id: &str,
        season_val: &str,
        episode_val: i32,
        media_info: &jumbie_shared::types::MediaInfo,
    ) -> Result<()> {
        let config = self.db_config().await;
        let mapping = self
            .db
            .get_series_mapping(series_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Series mapping not found"))?;

        // Early exit: renaming disabled → no filename to fix
        if !crate::file_manager::should_rename_episodes(&config, &mapping)
            || !config.organization.auto_apply_renames
        {
            return Ok(());
        }

        // SSoT: resolve_active_episode_format + format_needs_media_info are shared with
        // compute_batch_plan (plan.rs) so "which format applies and does it need media
        // info" is decided in one place.
        let episode_fmt = crate::file_manager::resolve_active_episode_format(&config, &mapping);
        let needs_media = crate::file_manager::format_needs_media_info(episode_fmt);

        if !needs_media {
            return Ok(());
        }

        // Compute the correct filename with the real media_info.
        let season_num = season_val.parse::<i32>().unwrap_or(1);
        let context = resolve_mapping_context(
            &self.db,
            series_id,
            self.db_config().await.general.absolute_numbering,
        )
        .await?;

        let ep_title = self
            .db
            .get_episode_title_by_number(series_id, season_num, episode_val)
            .await
            .unwrap_or(None);

        let pad_options = crate::utils::TemplatePadOptions {
            max_season: context.max_season as u32,
            max_episode: context.max_episode_for(season_num) as u32,
            max_length: 0,
        };

        let episode_var = episode_val.to_string();

        let target_path = Self::build_target_path(&PathBuildVars {
            config: &config,
            mapping: &mapping,
            season_num,
            episode_num: episode_val,
            episode_var: &episode_var,
            source_path: file_path,
            episode_title: ep_title.as_deref(),
            episode_quality: None,
            episode_submitter: None,
            release_date: None,
            created_at: None,
            pad_options: &pad_options,
            part_number: None,
            media_info: Some(media_info),
        })?;

        let final_target = crate::file_manager::resolve_episode_target_path(
            file_path,
            &target_path,
            &config,
            &mapping,
            ResolveMode::Apply,
        );

        // Rename in-place (same directory, different filename)
        // Rename in-place (same directory, different filename).
        if final_target != file_path {
            // Parent directory already exists (file was organized earlier).
            tokio::fs::rename(file_path, &final_target).await?;

            let season_str = season_val.to_string();
            let episode_id = mapping.get_episode_id(
                &season_str,
                episode_val,
                config.general.absolute_numbering,
            )?;
            let new_path_str = final_target.to_string_lossy().to_string();
            let _ = self.db.update_file_path(&episode_id, &new_path_str).await;

            // Migrate the fingerprint path so the hash stays linked.
            let old_path_str = file_path.to_string_lossy().to_string();
            let _ = self
                .db
                .move_fingerprint_path(&old_path_str, &new_path_str)
                .await;

            tracing::debug!(
                "Renamed with media info: '{}' -> '{}'",
                file_path.display(),
                final_target.display()
            );
        }

        Ok(())
    }

    // Organize a downloaded file to its final destination.
    ///
    /// SSoT: the `episode_id` is authoritative. It is used to look up the episode's
    /// DB record and series mapping — the filename is NOT parsed to determine the
    /// destination. Filename parsing is only a fallback when the lookup fails, and
    /// that path is logged.
    pub(crate) fn organize_file<'a>(
        &'a self,
        src: &'a Path,
        episode_id: &'a str,
        _download_id: &'a str,
        media_info: Option<&'a jumbie_shared::types::MediaInfo>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PathBuf>>> + Send + 'a>> {
        Box::pin(async move {
            let episode_row = self.db.get_episode_by_id(episode_id).await?;
            let row = match episode_row {
                Some(r) => r,
                None => {
                    warn!(
                        "episode_id '{}' not found in DB, cannot organize",
                        episode_id
                    );
                    return Ok(None);
                }
            };

            let series_id = &row.series_id;
            // SSoT: the row's own mode is authoritative here — get_episode_by_id
            // selects episodes.numbering_mode. A NULL season on a normal-mode row is
            // legacy data with no season recorded, defaulted to 1.
            let season =
                jumbie_shared::mapping::resolve_season_opt(row.season, row.numbering_mode == 1)
                    .unwrap_or(1);
            let episode = row.episode;

            let (mapping, season_str, episode_num, ep_title) = {
                let found_mapping = match self.db.get_series_mapping(series_id).await? {
                    Some(m) => m,
                    None => {
                        warn!(
                            "No mapping found for series_id '{}' (episode_id={})",
                            series_id, episode_id
                        );
                        return Ok(None);
                    }
                };

                (found_mapping, season, episode, row.title.clone())
            };

            let season_num = season_str;

            // Build the target path.
            let config = self.db_config().await;
            // SSoT: effective numbering mode (series tristate → global default).
            let absolute = mapping
                .settings
                .active_mode(config.general.absolute_numbering)
                .is_absolute();
            let db_episodes = self
                .db
                .get_series_episodes_details(&mapping.series_id, absolute)
                .await
                .unwrap_or_default();
            let context = crate::file_manager::get_mapping_context(&db_episodes, absolute);

            let pad_options = crate::utils::TemplatePadOptions {
                max_season: context.max_season as u32,
                max_episode: context.max_episode_for(season_num) as u32,
                max_length: 0,
            };

            let episode_var = episode_num.to_string();
            let target_path = Self::build_target_path(&PathBuildVars {
                config: &config,
                mapping: &mapping,
                season_num,
                episode_num,
                episode_var: &episode_var,
                source_path: src,
                episode_title: ep_title.as_deref(),
                episode_quality: None,
                episode_submitter: None,
                release_date: None,
                created_at: None,
                pad_options: &pad_options,
                part_number: None,
                media_info,
            })?;

            // Organization vs. Renaming (two independent concerns)
            // Organization vs. renaming: the file is ALWAYS moved to the directory
            // build_target_path computes; the template filename is applied only when
            // renaming is enabled (both should_rename_episodes and auto_apply_renames).
            let final_target = crate::file_manager::resolve_episode_target_path(
                src,
                &target_path,
                &config,
                &mapping,
                ResolveMode::Apply,
            );

            if let Some(parent) = final_target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            // Step 3: Hardlink siblings (same inode, zero-copy)
            // Hardlink siblings FIRST — before the source is moved — so each sibling
            // gets its own hard-link to the same inode regardless of what happens to `src`.
            self.link_sibling_episodes(super::sibling::LinkSiblingParams {
                src,
                episode_id,
                mapping: &mapping,
                context: &context,
            })
            .await
            .unwrap_or_else(|e| tracing::warn!("Sibling episode linking warning: {}", e));

            // Step 4: Place file at target
            let actual_target = self
                .move_file_to_target(src, &final_target, None)
                .await
                .context("Failed to move file to target")?;

            debug!(
                "Organized: '{}' -> '{}'",
                src.display(),
                actual_target.display()
            );

            Ok(Some(actual_target))
        })
    }
}
