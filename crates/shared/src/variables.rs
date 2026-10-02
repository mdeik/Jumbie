use serde::{Deserialize, Serialize};

// Variables are grouped by `TemplateContext` so each UI panel lists only the
// variables available there and validation rejects ones that won't be populated
// at render time. `get_allowed_vars` strips the `${...}` braces once at definition
// time so validation can compare bare names (e.g. `"series"`).

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemplateContext {
    SeasonFolder,
    EpisodeFile,
    SeriesPath,
    SearchFormat,
    Notifier,
    NotifierDownload,
    NotifierError,
    NotifierRenameQueue,
}

impl TemplateContext {
    pub const fn schema_value(&self) -> &'static str {
        match self {
            Self::SeasonFolder => "SeasonFolder",
            Self::EpisodeFile => "EpisodeFile",
            Self::SeriesPath => "SeriesPath",
            Self::SearchFormat => "SearchFormat",
            Self::Notifier => "Notifier",
            Self::NotifierDownload => "NotifierDownload",
            Self::NotifierError => "NotifierError",
            Self::NotifierRenameQueue => "NotifierRenameQueue",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableDefinition {
    pub key: &'static str,
    pub description: &'static str,
    pub example: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableSection {
    pub title: &'static str,
    pub variables: Vec<VariableDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormattingInstruction {
    pub syntax: &'static str,
    pub description: &'static str,
}

// Helper constructors for the variable sections below.

fn vardef(key: &'static str, description: &'static str) -> VariableDefinition {
    VariableDefinition {
        key,
        description,
        example: None,
    }
}

fn vardef_ex(
    key: &'static str,
    description: &'static str,
    example: &'static str,
) -> VariableDefinition {
    VariableDefinition {
        key,
        description,
        example: Some(example),
    }
}

fn finst(syntax: &'static str, description: &'static str) -> FormattingInstruction {
    FormattingInstruction {
        syntax,
        description,
    }
}

/// Padding/width formatting instructions. Search-key templates accept digits
/// only, so `include_auto` is set for the full (filename/season) list but not
/// for search (see [`crate::validation::validate_search_template`]).
fn padding_instructions(include_auto: bool) -> Vec<FormattingInstruction> {
    let mut instructions = vec![finst(
        "${variable:0N}",
        "Zero-pad to N digits (e.g. :02 → 05)",
    )];
    if include_auto {
        instructions.push(finst(
            "${variable:auto}",
            "Zero-pad to the width the highest value needs",
        ));
        instructions.push(finst(
            "${variable:autoN}",
            "Like :auto, but never narrower than N digits",
        ));
    }
    instructions
}

/// Non-padding modifiers (crop, default, replace, conditional).
fn other_formatting_instructions() -> Vec<FormattingInstruction> {
    vec![
        finst("${variable:<N}", "Crop to N characters"),
        finst(
            "${variable:<N:suffix}",
            "Crop to N, append suffix (e.g. '…')",
        ),
        finst("${variable:-default}", "Use default if variable is absent"),
        finst("${variable/old/new}", "Replace first occurrence"),
        finst("${variable//old/new}", "Replace all occurrences"),
        finst(
            "?{ - ${title}}",
            "Conditional: include only if all vars present",
        ),
    ]
}

pub fn get_formatting_instructions() -> Vec<FormattingInstruction> {
    let mut instructions = padding_instructions(true);
    instructions.extend(other_formatting_instructions());
    instructions
}

/// Formatting instructions shown for a context. Search templates only permit
/// padding (see [`crate::validation::validate_search_template`]), so their
/// tooltip must not advertise crop/default/replace/conditional syntax.
pub fn get_formatting_instructions_for_context(
    context: &TemplateContext,
) -> Vec<FormattingInstruction> {
    match context {
        TemplateContext::SearchFormat => padding_instructions(false),
        _ => get_formatting_instructions(),
    }
}

/// Sections of variables available in a context, so the frontend can render
/// grouped UI dropdowns (e.g. "Episode Variables") rather than one flat list.
fn notifier_download_section() -> VariableSection {
    VariableSection {
        title: "Download Variables",
        variables: vec![
            vardef("${series}", "Series Title"),
            vardef("${title}", "Episode Title"),
            vardef("${season}", "Season Number"),
            vardef("${episode}", "Episode Number"),
        ],
    }
}

fn notifier_error_section() -> VariableSection {
    VariableSection {
        title: "Error Variables",
        variables: vec![
            vardef(
                "${error_context}",
                "Error context label (e.g. 'Organize Failure', 'Download Cancelled')",
            ),
            vardef("${error_message}", "Detailed error message"),
        ],
    }
}

fn notifier_rename_queue_section() -> VariableSection {
    VariableSection {
        title: "Rename Queue Variables",
        variables: vec![
            vardef("${series}", "Series Title"),
            vardef("${affected_count}", "Number of files pending rename"),
        ],
    }
}

pub fn get_sections_for_context(context: &TemplateContext) -> Vec<VariableSection> {
    let mut sections = Vec::new();

    match context {
        TemplateContext::SeasonFolder => {
            sections.push(VariableSection {
                title: "Basic Variables",
                variables: vec![
                    vardef("${series}", "Series Title"),
                    vardef("${season}", "Season Number"),
                ],
            });
        }

        TemplateContext::EpisodeFile => {
            sections.push(VariableSection {
                title: "Episode Variables",
                variables: vec![
                    vardef("${series}", "Series Title"),
                    vardef("${season}", "Season Number"),
                    vardef("${episode}", "Episode Number"),
                    vardef("${title}", "Episode Title"),
                    vardef("${ext}", "File Extension"),
                    vardef("${quality}", "Quality (e.g. 1080p)"),
                    vardef("${group}", "Release Group"),
                ],
            });
            sections.push(VariableSection {
                title: "Media Info Variables",
                variables: vec![
                    vardef_ex("${codec}", "Video codec (e.g. h264, hevc, x265)", "h264"),
                    vardef_ex(
                        "${resolution}",
                        "Video resolution (e.g. 1920x1080)",
                        "1920x1080",
                    ),
                    vardef_ex("${width}", "Video width in pixels", "1920"),
                    vardef_ex("${height}", "Video height in pixels", "1080"),
                    vardef_ex("${bitrate}", "Video bitrate (e.g. 2500 kb/s)", "2500 kb/s"),
                    vardef_ex("${duration}", "Media duration (e.g. 24m 30s)", "24m 30s"),
                    vardef_ex("${audio_codec}", "Audio codec (e.g. aac, ac3, dts)", "ac3"),
                    vardef_ex(
                        "${audio_channels}",
                        "Audio channel count (e.g. 6 for 5.1)",
                        "6",
                    ),
                    vardef_ex(
                        "${audio_languages}",
                        "Audio languages (comma-separated)",
                        "eng,jpn",
                    ),
                    vardef_ex(
                        "${subtitle_languages}",
                        "Subtitle languages (comma-separated)",
                        "eng,spa",
                    ),
                    vardef_ex("${video_track_count}", "Number of video tracks", "1"),
                    vardef_ex("${audio_track_count}", "Number of audio tracks", "2"),
                    vardef_ex("${subtitle_track_count}", "Number of subtitle tracks", "1"),
                    vardef_ex(
                        "${has_chapters}",
                        "Whether the file contains chapter markers (yes/no)",
                        "yes",
                    ),
                ],
            });
            sections.push(VariableSection {
                title: "Date Variables",
                variables: vec![
                    vardef_ex("%{release:FORMAT}", "Release (air) date formatted with chrono strftime, in the server's local timezone. Use any format string: %Y (year), %m (month), %d (day), etc. Examples: %{release:%Y-%m-%d}, %{release:%Y}, %{release:%b %d, %Y}", "%{release:%Y-%m-%d} → 2024-03-15"),
                    vardef_ex("%{download:FORMAT}", "Download date/time formatted with chrono strftime, in the server's local timezone. Same syntax as release. Examples: %{download:%Y-%m-%d}, %{download:%Y%m%d_%H%M%S}", "%{download:%Y-%m-%d %H:%M:%S} → 2024-06-20 14:30:00"),
                ],
            });
        }
        TemplateContext::SeriesPath => {
            sections.push(VariableSection {
                title: "Path Variables",
                variables: vec![vardef("${series}", "Title of the series")],
            });
        }
        TemplateContext::SearchFormat => {
            sections.push(VariableSection {
                title: "Search Variables",
                variables: vec![
                    vardef("${season}", "Season Number (source numbering)"),
                    vardef("${episode}", "Episode Number (source numbering)"),
                ],
            });
        }
        TemplateContext::Notifier => {
            // Fallback: include all notifier sections.
            sections.push(notifier_download_section());
            sections.push(notifier_error_section());
            sections.push(notifier_rename_queue_section());
        }
        TemplateContext::NotifierDownload => {
            sections.push(notifier_download_section());
        }
        TemplateContext::NotifierError => {
            sections.push(notifier_error_section());
        }
        TemplateContext::NotifierRenameQueue => {
            sections.push(notifier_rename_queue_section());
        }
    }

    sections
}

/// Return bare variable names (without `${...}`) allowed in the given context.
///
/// Callers like `validate_template` split on `:` and compare the base name
/// directly, without parsing template syntax.
pub fn get_allowed_vars(context: &TemplateContext) -> Vec<&'static str> {
    get_sections_for_context(context)
        .into_iter()
        .flat_map(|section| {
            section.variables.into_iter().map(|v| {
                let key = v.key;
                &key[2..key.len() - 1]
            })
        })
        .collect()
}

/// Check whether a bare name (e.g., `"series"`, `"episode"`) is a valid variable
/// in *any* context. This is used for global validation where the context isn't
/// yet known (e.g., validating a template before assigning it to a specific use).
pub fn is_valid_variable(key: &str) -> bool {
    // All known variables across any context
    let all_contexts = [
        TemplateContext::SeasonFolder,
        TemplateContext::EpisodeFile,
        TemplateContext::SeriesPath,
        TemplateContext::SearchFormat,
        TemplateContext::Notifier,
        TemplateContext::NotifierDownload,
        TemplateContext::NotifierError,
        TemplateContext::NotifierRenameQueue,
    ];

    all_contexts
        .iter()
        .any(|ctx| get_allowed_vars(ctx).contains(&key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_episode_file_has_date_variables() {
        let sections = get_sections_for_context(&TemplateContext::EpisodeFile);
        let date_section = sections
            .iter()
            .find(|s| s.title == "Date Variables")
            .expect("Should have a Date Variables section");
        let keys: HashSet<&str> = date_section.variables.iter().map(|v| v.key).collect();
        assert!(
            keys.contains("%{release:FORMAT}"),
            "Date section should have %{{release:FORMAT}}"
        );
        assert!(
            keys.contains("%{download:FORMAT}"),
            "Date section should have %{{download:FORMAT}}"
        );
    }

    #[test]
    fn test_date_variables_not_in_other_contexts() {
        let date_keys = ["%{release:FORMAT}", "%{download:FORMAT}"];
        for ctx in &[
            TemplateContext::SeasonFolder,
            TemplateContext::SeriesPath,
            TemplateContext::Notifier,
            TemplateContext::NotifierDownload,
            TemplateContext::NotifierError,
            TemplateContext::NotifierRenameQueue,
        ] {
            let sections = get_sections_for_context(ctx);
            // Collect the raw keys across all sections
            let all_keys: HashSet<&str> = sections
                .iter()
                .flat_map(|s| s.variables.iter())
                .map(|v| v.key)
                .collect();
            for key in &date_keys {
                assert!(
                    !all_keys.contains(key),
                    "{:?} should NOT contain {}",
                    ctx,
                    key
                );
            }
        }
    }

    #[test]
    fn test_date_variables_have_examples() {
        let sections = get_sections_for_context(&TemplateContext::EpisodeFile);

        // Collect all variables from the Date Variables section
        let date_section = sections
            .iter()
            .find(|s| s.title == "Date Variables")
            .expect("Should have a Date Variables section");

        for var in &date_section.variables {
            assert!(
                var.example.is_some(),
                "Date variable {} should have an example",
                var.key
            );
        }
    }

    #[test]
    fn test_episode_file_has_both_sections() {
        let sections = get_sections_for_context(&TemplateContext::EpisodeFile);
        let titles: HashSet<&str> = sections.iter().map(|s| s.title).collect();
        assert!(titles.contains("Episode Variables"));
        assert!(titles.contains("Date Variables"));
    }

    #[test]
    fn test_season_folder_does_not_have_date_section() {
        let sections = get_sections_for_context(&TemplateContext::SeasonFolder);
        let titles: HashSet<&str> = sections.iter().map(|s| s.title).collect();
        assert!(!titles.contains("Date Variables"));
    }

    #[test]
    fn test_episode_file_has_media_info_section() {
        let sections = get_sections_for_context(&TemplateContext::EpisodeFile);
        let media_section = sections
            .iter()
            .find(|s| s.title == "Media Info Variables")
            .expect("Should have a Media Info Variables section");
        let keys: HashSet<&str> = media_section.variables.iter().map(|v| v.key).collect();
        assert!(
            keys.contains("${codec}"),
            "Media info section should have ${{codec}}"
        );
        assert!(
            keys.contains("${resolution}"),
            "Media info section should have ${{resolution}}"
        );
        assert!(
            keys.contains("${audio_codec}"),
            "Media info section should have ${{audio_codec}}"
        );
        assert!(
            keys.contains("${has_chapters}"),
            "Media info section should have ${{has_chapters}}"
        );
    }

    #[test]
    fn test_media_info_variables_have_examples() {
        let sections = get_sections_for_context(&TemplateContext::EpisodeFile);

        let media_section = sections
            .iter()
            .find(|s| s.title == "Media Info Variables")
            .expect("Should have a Media Info Variables section");

        for var in &media_section.variables {
            assert!(
                var.example.is_some(),
                "Media info variable {} should have an example",
                var.key
            );
        }
    }

    #[test]
    fn test_media_info_variables_not_in_season_folder() {
        let sections = get_sections_for_context(&TemplateContext::SeasonFolder);
        let all_keys: HashSet<&str> = sections
            .iter()
            .flat_map(|s| s.variables.iter())
            .map(|v| v.key)
            .collect();

        assert!(!all_keys.contains("${codec}"));
        assert!(!all_keys.contains("${resolution}"));
        assert!(!all_keys.contains("${audio_codec}"));
    }

    fn notifier_contexts_without_media_info() -> Vec<TemplateContext> {
        vec![
            TemplateContext::Notifier,
            TemplateContext::NotifierDownload,
            TemplateContext::NotifierError,
            TemplateContext::NotifierRenameQueue,
        ]
    }

    #[test]
    fn test_media_info_variables_not_in_notifier() {
        let sections = get_sections_for_context(&TemplateContext::Notifier);
        let all_keys: HashSet<&str> = sections
            .iter()
            .flat_map(|s| s.variables.iter())
            .map(|v| v.key)
            .collect();

        assert!(!all_keys.contains("${codec}"));
        assert!(!all_keys.contains("${resolution}"));
    }

    #[test]
    fn test_media_info_variables_not_in_notifier_sub_contexts() {
        for ctx in notifier_contexts_without_media_info() {
            let sections = get_sections_for_context(&ctx);
            let all_keys: HashSet<&str> = sections
                .iter()
                .flat_map(|s| s.variables.iter())
                .map(|v| v.key)
                .collect();
            assert!(!all_keys.contains("${codec}"));
            assert!(!all_keys.contains("${resolution}"));
        }
    }

    #[test]
    fn test_media_info_variables_not_in_series_path() {
        let sections = get_sections_for_context(&TemplateContext::SeriesPath);
        let all_keys: HashSet<&str> = sections
            .iter()
            .flat_map(|s| s.variables.iter())
            .map(|v| v.key)
            .collect();

        assert!(!all_keys.contains("${codec}"));
        assert!(!all_keys.contains("${resolution}"));
    }

    #[test]
    fn test_episode_file_has_three_sections() {
        let sections = get_sections_for_context(&TemplateContext::EpisodeFile);
        let titles: HashSet<&str> = sections.iter().map(|s| s.title).collect();
        assert!(titles.contains("Episode Variables"));
        assert!(titles.contains("Media Info Variables"));
        assert!(titles.contains("Date Variables"));
    }

    #[test]
    fn test_formatting_instructions_updated_syntax() {
        let instructions = get_formatting_instructions();
        // Conditional syntax updated from {?...} to ?{...}
        assert!(instructions.iter().any(|i| i.syntax == "${variable:0N}"));
        assert!(instructions.iter().any(|i| i.syntax == "?{ - ${title}}"));
        // The series-aware padding forms are advertised for filename templates...
        assert!(instructions.iter().any(|i| i.syntax == "${variable:auto}"));
        assert!(instructions.iter().any(|i| i.syntax == "${variable:autoN}"));
    }

    #[test]
    fn test_search_context_exposes_season_episode_and_padding_only() {
        let vars = get_allowed_vars(&TemplateContext::SearchFormat);
        assert_eq!(vars, vec!["season", "episode"]);

        let instructions = get_formatting_instructions_for_context(&TemplateContext::SearchFormat);
        assert!(instructions.iter().any(|i| i.syntax == "${variable:0N}"));
        // ...but not for search keys, which accept digits-only padding.
        assert!(instructions.iter().all(|i| !i.syntax.contains("auto")));
        assert!(
            instructions.iter().all(|i| !i.syntax.contains('<')
                && !i.syntax.contains("?{")
                && !i.syntax.contains(":-")),
            "search tooltip must not advertise non-padding modifiers: {instructions:?}"
        );
        // Other contexts keep the full instruction list.
        assert!(
            get_formatting_instructions_for_context(&TemplateContext::EpisodeFile).len()
                > instructions.len()
        );
    }
}
