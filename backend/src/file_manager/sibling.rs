use std::path::Path;

use anyhow::Result;

use crate::file_manager::path::PathBuildVars;
use crate::organizer::ContentOrganizer;
use jumbie_shared::types::EpisodeStatus;

pub(crate) struct LinkSiblingParams<'a> {
    pub src: &'a Path,
    pub episode_id: &'a str,
    pub mapping: &'a jumbie_shared::types::MappingRule,
    pub context: &'a crate::file_manager::MappingContext,
}

impl ContentOrganizer {
    // Sibling Episode Linking (hard-link sharing for season packs)

    pub(crate) async fn link_sibling_episodes(&self, params: LinkSiblingParams<'_>) -> Result<()> {
        let src_str = params.src.to_string_lossy().to_string();
        let siblings = match self
            .db
            .get_episodes_by_source_path(&src_str, params.episode_id)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "Failed to query sibling episodes for '{}': {}",
                    params.src.display(),
                    e
                );
                return Ok(());
            }
        };

        if siblings.is_empty() {
            return Ok(());
        }

        tracing::debug!(
            "Linking {} sibling episode(s) sharing source: '{}'",
            siblings.len(),
            params.src.display()
        );

        let (_, _, siblings_media_info) = self
            .db
            .update_file_fingerprint(params.src, EpisodeStatus::Organized.as_str())
            .await;

        let config = self.db_config().await;

        for (sib_episode_id, sib_season, sib_episode, sib_title, ..) in &siblings {
            let season_num = sib_season
                .as_ref()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(1);

            let sib_media_info = siblings_media_info.as_ref();

            let episode_var = sib_episode.to_string();
            let pad_options = crate::utils::TemplatePadOptions {
                max_season: params.context.max_season as u32,
                max_episode: params.context.max_episode_for(season_num) as u32,
                max_length: 0,
            };
            let mut target_path = match Self::build_target_path(&PathBuildVars {
                config: &config,
                mapping: params.mapping,
                season_num,
                episode_num: *sib_episode,
                episode_var: &episode_var,
                source_path: params.src,
                episode_title: sib_title.as_deref(),
                episode_quality: None,
                episode_submitter: None,
                release_date: None,
                created_at: None,
                pad_options: &pad_options,
                part_number: None,
                media_info: sib_media_info,
            }) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        "Skipping sibling {}: failed to build target path: {}",
                        sib_episode_id,
                        e
                    );
                    continue;
                }
            };

            target_path = crate::file_manager::resolve_episode_target_path(
                params.src,
                &target_path,
                &config,
                params.mapping,
                crate::file_manager::ResolveMode::Apply,
            );

            if let Some(parent) = target_path.parent()
                && let Err(e) = tokio::fs::create_dir_all(parent).await
            {
                tracing::warn!(
                    "Failed to create parent dir for sibling {}: {}",
                    sib_episode_id,
                    e
                );
                continue;
            }

            match tokio::fs::hard_link(params.src, &target_path).await {
                Ok(_) => {
                    if let Err(e) = self
                        .db
                        .update_file_path(sib_episode_id, &target_path.to_string_lossy())
                        .await
                    {
                        tracing::warn!(
                            "Sibling {} linked but failed to update DB: {}",
                            sib_episode_id,
                            e
                        );
                    }
                    if let Err(e) = crate::state::FileStateManager::fingerprint_file(
                        &self.db,
                        &target_path,
                        crate::state::FileState::Organized,
                    )
                    .await
                    {
                        tracing::warn!(
                            "Sibling {} linked but failed to fingerprint {}: {}",
                            sib_episode_id,
                            target_path.display(),
                            e
                        );
                    }
                    tracing::debug!(
                        "Linked sibling episode {} -> '{}'",
                        sib_episode_id,
                        target_path.display()
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    match tokio::fs::metadata(&target_path).await {
                        Ok(meta)
                            if meta.len()
                                == std::fs::metadata(params.src).map(|m| m.len()).unwrap_or(0) =>
                        {
                            tracing::debug!(
                                "Sibling {} target already exists with matching size; updating DB",
                                sib_episode_id
                            );
                            if let Err(e) = self
                                .db
                                .update_file_path(sib_episode_id, &target_path.to_string_lossy())
                                .await
                            {
                                tracing::warn!(
                                    "Sibling {} target exists but failed to update DB: {}",
                                    sib_episode_id,
                                    e
                                );
                            }
                            if let Err(e) = crate::state::FileStateManager::fingerprint_file(
                                &self.db,
                                &target_path,
                                crate::state::FileState::Organized,
                            )
                            .await
                            {
                                tracing::warn!(
                                    "Sibling {} exists but failed to fingerprint {}: {}",
                                    sib_episode_id,
                                    target_path.display(),
                                    e
                                );
                            }
                        }
                        _ => {
                            tracing::warn!(
                                "Skipping sibling {}: target exists at '{}' with different size",
                                sib_episode_id,
                                target_path.display()
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to hard link sibling {} -> '{}': {}",
                        sib_episode_id,
                        target_path.display(),
                        e
                    );
                }
            }
        }

        Ok(())
    }
}
