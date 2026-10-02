use std::path::{Path, PathBuf};

use jumbie_shared::config::Config;
use jumbie_shared::types::MappingRule;

use crate::paths::sanitize_with_policy;

/// Whether to use the template-based filename when organizing/renaming.
///
/// Priority:
///   1. `mapping.settings.rename_episodes` (per-series override) if `Some`
///   2. `config.organization.rename_episodes` (global default)
///   3. `true` (fallback if both are unset)
pub fn should_rename_episodes(config: &Config, mapping: &MappingRule) -> bool {
    mapping
        .settings
        .rename_episodes
        .unwrap_or(config.organization.rename_episodes)
}

/// Controls whether [`resolve_episode_target_path`] applies the template name.
///
/// * `Preview` — always use the template name (rename queue display).
/// * `Apply`   — use the template name only when `auto_apply_renames` is on;
///   otherwise keep the original filename.
pub enum ResolveMode {
    Preview,
    Apply,
}

/// Resolve the final filename for an episode file.
///
/// SSoT for the "rename or keep original" decision — every code path that moves a
/// file or computes a target path must call this. Mode semantics: see [`ResolveMode`].
pub fn resolve_episode_target_path(
    src: &Path,
    template_path: &Path,
    config: &Config,
    mapping: &MappingRule,
    mode: ResolveMode,
) -> PathBuf {
    let use_template = should_rename_episodes(config, mapping)
        && match mode {
            ResolveMode::Preview => true,
            ResolveMode::Apply => config.organization.auto_apply_renames,
        };

    if use_template {
        template_path.to_path_buf()
    } else if let Some(orig_name) = src.file_name() {
        let name_str = orig_name.to_string_lossy();
        let safe_name = sanitize_with_policy(
            &name_str,
            &config.organization.illegal_char_policy,
            config.organization.allow_platform_specific_chars,
        );
        template_path.with_file_name(&safe_name)
    } else {
        template_path.to_path_buf()
    }
}

pub fn should_flatten_season_folders(config: &Config, mapping: &MappingRule) -> bool {
    mapping
        .settings
        .flatten_season_folders
        .unwrap_or(config.general.flatten_season_folders)
}

/// SSoT: resolve whether a series renders in absolute numbering — the series tristate
/// override falls back to the global default, delegating to
/// [`SeriesSettings::effective_absolute_numbering`].
pub fn should_use_absolute_numbering(config: &Config, mapping: &MappingRule) -> bool {
    mapping
        .settings
        .effective_absolute_numbering(config.general.absolute_numbering)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(rename: bool, auto_apply: bool) -> Config {
        serde_json::from_value(serde_json::json!({
            "organization": {
                "rename_episodes": rename,
                "auto_apply_renames": auto_apply,
            }
        }))
        .expect("config_with")
    }

    fn default_mapping() -> MappingRule {
        MappingRule::default()
    }

    // resolve_episode_target_path(Apply): rename enabled + auto-apply on → template

    #[test]
    fn test_apply_uses_template_when_rename_and_auto_apply() {
        let cfg = config_with(true, true);
        let mapping = default_mapping();
        let src = Path::new("/downloads/My.Show.S01E01.mkv");
        let target = Path::new("/media/My Show/Season 01/My Show - S01E01 - Title.mkv");

        let result = resolve_episode_target_path(src, target, &cfg, &mapping, ResolveMode::Apply);
        assert_eq!(result, target);
    }

    #[test]
    fn test_apply_preserves_original_when_rename_disabled() {
        let cfg = config_with(false, true);
        let mapping = default_mapping();
        let src = Path::new("/downloads/My.Show.S01E01.mkv");
        let target = Path::new("/media/Show/S01/Title.mkv");

        let result = resolve_episode_target_path(src, target, &cfg, &mapping, ResolveMode::Apply);
        let expected = target.with_file_name("My.Show.S01E01.mkv");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_apply_preserves_original_when_auto_apply_off() {
        let cfg = config_with(true, false);
        let mapping = default_mapping();
        let src = Path::new("/downloads/My.Show.S01E01.mkv");
        let target = Path::new("/media/Show/S01/My Show - S01E01.mkv");

        let result = resolve_episode_target_path(src, target, &cfg, &mapping, ResolveMode::Apply);
        let expected = target.with_file_name("My.Show.S01E01.mkv");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_apply_sanitizes_illegal_chars_in_original() {
        let cfg = config_with(false, false);
        let mapping = default_mapping();
        let src = Path::new("/downloads/Show?Title:Best<ep>.mkv");
        let target = Path::new("/media/Show/S01/ignored.mkv");

        let result = resolve_episode_target_path(src, target, &cfg, &mapping, ResolveMode::Apply);
        let filename = result.file_name().unwrap().to_str().unwrap();
        assert_eq!(filename, "Show_Title_Best_ep_.mkv");
    }

    #[test]
    fn test_apply_remove_policy_does_not_crash() {
        let mut cfg = config_with(false, false);
        cfg.organization.illegal_char_policy =
            jumbie_shared::config::organization::InvalidCharPolicy::Remove;
        let mapping = default_mapping();
        let src = Path::new("/downloads/?:<>");
        let target = Path::new("/media/Show/S01/ignored.mkv");

        let result = resolve_episode_target_path(src, target, &cfg, &mapping, ResolveMode::Apply);
        let filename = result.file_name().unwrap().to_str().unwrap();
        assert!(!filename.is_empty());
        assert!(filename.contains("_invalid_"), "got: {}", filename);
    }

    // resolve_episode_target_path(Preview): always template when rename enabled

    #[test]
    fn test_preview_uses_template_when_rename_enabled_even_without_auto_apply() {
        let cfg = config_with(true, false);
        let mapping = default_mapping();
        let src = Path::new("/downloads/raw.name.mkv");
        let target = Path::new("/media/Show/S01/Show - S01E01.mkv");

        let result = resolve_episode_target_path(src, target, &cfg, &mapping, ResolveMode::Preview);
        assert_eq!(result, target);
    }

    #[test]
    fn test_preview_reverts_to_original_when_rename_disabled() {
        let cfg = config_with(false, false);
        let mapping = default_mapping();
        let src = Path::new("/downloads/raw.name.mkv");
        let target = Path::new("/media/Show/S01/Show - S01E01.mkv");

        let result = resolve_episode_target_path(src, target, &cfg, &mapping, ResolveMode::Preview);
        let expected = target.with_file_name("raw.name.mkv");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_preview_sanitizes_illegal_chars() {
        let cfg = config_with(false, false);
        let mapping = default_mapping();
        let src = Path::new("/downloads/bad:file?name.mkv");
        let target = Path::new("/media/Show/S01/Template.mkv");

        let result = resolve_episode_target_path(src, target, &cfg, &mapping, ResolveMode::Preview);
        let filename = result.file_name().unwrap().to_str().unwrap();
        assert_eq!(filename, "bad_file_name.mkv");
    }
}
