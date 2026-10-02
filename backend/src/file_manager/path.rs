use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use jumbie_shared::config::Config;
use jumbie_shared::types::MappingRule;

use crate::organizer::ContentOrganizer;
use crate::paths::sanitize_with_policy;

// Shared context for path building — SSoT for all path-related params. Every
// function needing template/path variables takes this struct, so adding a variable
// here adds it everywhere.

pub struct PathBuildVars<'a> {
    pub config: &'a Config,
    pub mapping: &'a MappingRule,
    pub season_num: i32,
    pub episode_num: i32,
    pub episode_var: &'a str,
    pub source_path: &'a Path,
    pub episode_title: Option<&'a str>,
    pub episode_quality: Option<&'a str>,
    pub episode_submitter: Option<&'a str>,
    pub release_date: Option<chrono::NaiveDateTime>,
    pub created_at: Option<chrono::NaiveDateTime>,
    pub pad_options: &'a crate::utils::TemplatePadOptions,
    pub part_number: Option<u32>,
    pub media_info: Option<&'a jumbie_shared::types::MediaInfo>,
}

// Template / path helpers — shared by batch planning and single-file moves.

/// SSoT: resolve the active naming formats for a series — mapping-level overrides
/// first, falling back to the global config, honoring the active numbering mode.
/// Returns `(season_folder_format, episode_file_format)`. Used by single-file
/// renames, batch planning, and the filename renderer so the "which formats apply"
/// decision lives in one place.
pub fn resolve_active_formats<'a>(
    config: &'a Config,
    mapping: &'a MappingRule,
) -> (&'a str, &'a str) {
    if crate::file_manager::should_use_absolute_numbering(config, mapping) {
        (
            mapping
                .settings
                .season_folder_format_absolute
                .as_deref()
                .unwrap_or(&config.organization.season_folder_format_absolute),
            mapping
                .settings
                .episode_file_format_absolute
                .as_deref()
                .unwrap_or(&config.organization.episode_file_format_absolute),
        )
    } else {
        (
            mapping
                .settings
                .season_folder_format
                .as_deref()
                .unwrap_or(&config.organization.season_folder_format),
            mapping
                .settings
                .episode_file_format
                .as_deref()
                .unwrap_or(&config.organization.episode_file_format),
        )
    }
}

/// Convenience wrapper over [`resolve_active_formats`] returning only the
/// episode filename format — used by the media-info gates in plan.rs/organize.rs.
pub fn resolve_active_episode_format<'a>(config: &'a Config, mapping: &'a MappingRule) -> &'a str {
    resolve_active_formats(config, mapping).1
}

/// SSoT: render the season-folder name for `season` using the series' active
/// season-folder format. Used by relocations outside the organizer (e.g.
/// restoring `_unmatched/` files on a numbering-mode switch) so the folder
/// matches what [`ContentOrganizer::build_target_path_with_episode_var`] creates.
pub fn season_folder_name(
    config: &Config,
    mapping: &MappingRule,
    season: i32,
    pad_options: &crate::utils::TemplatePadOptions,
) -> String {
    let (season_fmt, _) = resolve_active_formats(config, mapping);
    let mut vars = HashMap::new();
    vars.insert("season".to_string(), season.to_string());
    vars.insert("series".to_string(), mapping.target_title.clone());
    crate::utils::apply_template(season_fmt, &vars, Some(pad_options))
}

/// SSoT: does a filename template reference any media-info variable
/// (e.g. `{resolution}`, `{codec}`)? Used to decide whether media info must be
/// parsed/fetched at all — see [`jumbie_shared::types::MEDIA_INFO_FIELDS`].
pub fn format_needs_media_info(fmt: &str) -> bool {
    jumbie_shared::types::MEDIA_INFO_FIELDS.iter().any(|field| {
        let brace = format!("{{{}}}", field);
        let dollar_brace = format!("${{{}}}", field);
        fmt.contains(&brace) || fmt.contains(&dollar_brace)
    })
}

/// SSoT: build the template-variable map used for path rendering. Callers share
/// this so a new variable is added everywhere at once.
///
/// Illegal characters are NOT scrubbed here — `sanitize_with_policy` handles them at
/// the single output point in [`build_target_path_with_episode_var`].
fn build_template_vars(pbv: &PathBuildVars<'_>) -> HashMap<String, String> {
    let clean_title = crate::utils::file_naming::truncate_name_to_bytes(
        &pbv.mapping.target_title,
        crate::utils::file_naming::MAX_NAME_BYTES,
    );
    let ep_title = pbv.episode_title.unwrap_or_default().to_string();

    let mut vars = HashMap::new();

    // Primary variable: `{series}` — the canonical key used in all internal templates.
    vars.insert("series".to_string(), clean_title.clone());
    vars.insert("title".to_string(), ep_title);
    vars.insert("season".to_string(), pbv.season_num.to_string());
    vars.insert("episode".to_string(), pbv.episode_var.to_string());

    let filename_str = pbv.source_path.to_string_lossy();
    let ext = crate::utils::get_extended_extension(&filename_str);
    vars.insert(
        "ext".to_string(),
        if ext.is_empty() {
            jumbie_shared::media_format::DEFAULT_VIDEO_EXT.to_string()
        } else {
            ext.clone()
        },
    );

    let quality_val = match pbv.episode_quality {
        Some(q) if !q.is_empty() && q != "Unknown" && q != "None" => q.to_string(),
        _ => String::new(),
    };
    vars.insert("quality".to_string(), quality_val);

    let submitter = pbv.episode_submitter.unwrap_or_default().to_string();
    vars.insert("group".to_string(), submitter);

    // Media Info Variables
    if let Some(mi) = pbv.media_info {
        vars.extend(mi.to_template_vars());
    } else {
        for key in jumbie_shared::types::MEDIA_INFO_FIELDS {
            vars.entry(key.to_string()).or_insert_with(String::new);
        }
    }

    // Date variables. `%{release:…}` / `%{download:…}` render in the SERVER's local
    // timezone (chrono `Local`), not UTC and not the browser's. `%{download:…}` comes
    // from `created_at` (a real instant); `%{release:…}` comes from the release date,
    // which for date-only metadata is midnight UTC — a non-UTC server can shift it to
    // an adjacent day. Operators should set `TZ` to match their library's locale.
    let release_date_str = pbv
        .release_date
        .map(|d| crate::datetime::UtcDateTime::from_naive_utc(d).format_local_date())
        .unwrap_or_default();
    vars.insert("__release_date".to_string(), release_date_str);

    let created_date_str = pbv
        .created_at
        .map(|d| crate::datetime::UtcDateTime::from_naive_utc(d).format_local_date())
        .unwrap_or_default();
    vars.insert("__created_at".to_string(), created_date_str);

    vars
}

/// Resolve template variables (e.g. `${series}`, `${season}`) in the user-supplied
/// custom path. Only simple `${var}` substitutions — no conditionals, date formatting,
/// or zero-padding. For full episode-filename rendering use
/// [`build_target_path_with_episode_var`].
pub(crate) fn resolve_path_vars(path_str: &str, vars: &HashMap<String, String>) -> String {
    let mut result = path_str.to_string();
    for (key, value) in vars {
        // Replace both ${key} and {key} syntax for maximum compatibility
        result = result.replace(&format!("${{{}}}", key), value);
        result = result.replace(&format!("{{{}}}", key), value);
    }
    result
}

impl ContentOrganizer {
    /// Core path builder. Accepts a raw `episode_var` (e.g. `"5"`, `"1-3"`)
    /// so callers can pass range strings for multi-episode files. Padding is
    /// applied by the template's own format spec (`:0N` or `:auto` on
    /// `${season}` / `${episode}`).
    ///
    /// `part_number`: when `Some(n)`, injects a part suffix (configured via
    /// `OrganizationConfig::part_number_format`) before the extension, only if the
    /// original filename does not already contain a part indicator.
    ///
    /// When `mapping.settings.path` is set, `${var}` references in it are resolved
    /// before it is used as the base directory.
    pub fn build_target_path_with_episode_var(vars: &PathBuildVars<'_>) -> Result<PathBuf> {
        let mut tpl_vars = build_template_vars(vars);

        // Sanitize the ${series} value at this single output point so both the series
        // directory and any template embedding ${series} honour the org policy. The
        // final filename is re-sanitized below (idempotent).
        if let Some(series) = tpl_vars.get_mut("series") {
            *series = crate::paths::sanitize_title(series, &vars.config.organization);
        }

        // SSoT: resolve_active_formats (path.rs) decides which season/episode
        // formats apply (mapping override → global config, honoring the active
        // numbering mode) — the same source used by the media gates in
        // plan.rs/organize.rs, so the gate and the renderer can never drift.
        let (season_fmt, episode_fmt) =
            crate::file_manager::resolve_active_formats(vars.config, vars.mapping);

        let season_folder = crate::utils::file_naming::truncate_name_to_bytes(
            &crate::utils::apply_template(season_fmt, &tpl_vars, Some(vars.pad_options)),
            crate::utils::file_naming::MAX_NAME_BYTES,
        );
        let mut new_filename =
            crate::utils::apply_template(episode_fmt, &tpl_vars, Some(vars.pad_options));

        // Inject the part suffix when configured and not already present.
        let ext = tpl_vars
            .get("ext")
            .cloned()
            .unwrap_or_else(|| jumbie_shared::media_format::DEFAULT_VIDEO_EXT.to_string());
        if let Some(n) = vars.part_number {
            if !crate::file_manager::filename_has_part_indicator(&new_filename) {
                let stem = if let Some(s) = new_filename.strip_suffix(&format!(".{}", ext)) {
                    s.to_string()
                } else {
                    new_filename.clone()
                };
                let part_suffix = vars.config.organization.part_number_format.render(n);
                new_filename = format!("{}{}.{}", stem, part_suffix, ext);
            } else if !new_filename.ends_with(&format!(".{}", ext)) {
                new_filename = format!("{}.{}", new_filename, ext);
            }
        } else if !new_filename.ends_with(&format!(".{}", ext)) {
            new_filename = format!("{}.{}", new_filename, ext);
        }

        new_filename = crate::utils::file_naming::truncate_name_to_bytes(
            &new_filename,
            crate::utils::file_naming::MAX_NAME_BYTES,
        );

        // Sanitize the template-rendered filename. Template variables (series and
        // episode titles) can contain ILLEGAL_CHARS; without this a title like
        // "Show: The Best" would put a literal `:` on disk.
        new_filename = sanitize_with_policy(
            &new_filename,
            &vars.config.organization.illegal_char_policy,
            vars.config.organization.allow_platform_specific_chars,
        );

        // Custom path: resolve known template variables in `mapping.settings.path`.
        let base_dir = if let Some(custom_path) = &vars.mapping.settings.path {
            let resolved = resolve_path_vars(custom_path, &tpl_vars);
            let mut dir = PathBuf::from(resolved);

            // Truncate the final path component to filesystem limits
            if let Some(file_name) = dir.file_name().and_then(|n| n.to_str()) {
                let truncated = crate::utils::file_naming::truncate_name_to_bytes(
                    file_name,
                    crate::utils::file_naming::MAX_NAME_BYTES,
                );
                if truncated.len() != file_name.len() {
                    dir.set_file_name(&truncated);
                }
            }
            dir
        // SSoT: config.organization.primary_root
        } else {
            vars.config
                .organization
                .primary_root()
                .join(tpl_vars.get("series").cloned().unwrap_or_default())
        };

        let target_dir =
            if crate::file_manager::should_flatten_season_folders(vars.config, vars.mapping) {
                base_dir
            } else {
                base_dir.join(season_folder)
            };

        Ok(target_dir.join(&new_filename))
    }

    /// Convenience wrapper for the common single-episode case.
    pub fn build_target_path(vars: &PathBuildVars<'_>) -> Result<PathBuf> {
        Self::build_target_path_with_episode_var(vars)
    }
}
