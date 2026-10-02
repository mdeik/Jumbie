use crate::api::{fetch_config, save_config};
use crate::components::common::form_fields::{
    CheckboxInput, FormatFieldBuilder, Select, SelectOption, TooltipBuilder,
};
use crate::components::common::{SettingsBuilder, SettingsCheckbox};
use crate::hooks::ConfigBinding;
use crate::hooks::use_config::{ConfigContext, use_config};
use crate::validation;
use jumbie_shared::config::Config;
use jumbie_shared::config::organization::{
    CollisionRenameSuffix, InvalidCharPolicy, PartNumberFormat,
};
use jumbie_shared::variables::{TemplateContext, get_allowed_vars};
use leptos::prelude::*;
use leptos::task::spawn_local;

fn suffix_to_string(s: &CollisionRenameSuffix) -> String {
    match s {
        CollisionRenameSuffix::DotNumeric => "dot_numeric",
        CollisionRenameSuffix::ParenNumeric => "paren_numeric",
        CollisionRenameSuffix::UnderscoreNumeric => "underscore_numeric",
        CollisionRenameSuffix::DashNumeric => "dash_numeric",
    }
    .to_string()
}

fn string_to_suffix(s: &str) -> CollisionRenameSuffix {
    match s {
        "paren_numeric" => CollisionRenameSuffix::ParenNumeric,
        "underscore_numeric" => CollisionRenameSuffix::UnderscoreNumeric,
        "dash_numeric" => CollisionRenameSuffix::DashNumeric,
        _ => CollisionRenameSuffix::DotNumeric,
    }
}

fn part_fmt_to_string(f: &PartNumberFormat) -> String {
    match f {
        PartNumberFormat::Pt => "pt",
        PartNumberFormat::Cd => "cd",
        PartNumberFormat::Part => "part",
        PartNumberFormat::Disc => "disc",
        PartNumberFormat::DashNumeric => "dash_numeric",
        PartNumberFormat::ParenNumeric => "paren_numeric",
    }
    .to_string()
}

fn string_to_part_fmt(s: &str) -> PartNumberFormat {
    match s {
        "cd" => PartNumberFormat::Cd,
        "part" => PartNumberFormat::Part,
        "disc" => PartNumberFormat::Disc,
        "dash_numeric" => PartNumberFormat::DashNumeric,
        "paren_numeric" => PartNumberFormat::ParenNumeric,
        _ => PartNumberFormat::Pt,
    }
}

fn policy_to_string(p: &InvalidCharPolicy) -> String {
    match p {
        InvalidCharPolicy::Underscore => "underscore",
        InvalidCharPolicy::Hyphen => "hyphen",
        InvalidCharPolicy::Unicode => "unicode",
        InvalidCharPolicy::Space => "space",
        InvalidCharPolicy::Remove => "remove",
    }
    .to_string()
}

fn string_to_policy(s: &str) -> InvalidCharPolicy {
    match s {
        "underscore" => InvalidCharPolicy::Underscore,
        "hyphen" => InvalidCharPolicy::Hyphen,
        "unicode" => InvalidCharPolicy::Unicode,
        "space" => InvalidCharPolicy::Space,
        "remove" => InvalidCharPolicy::Remove,
        _ => InvalidCharPolicy::Underscore,
    }
}

#[component]
pub fn OrganizationSettings() -> impl IntoView {
    let ConfigContext { config, set_config } = use_config();
    let (original_season_folder_format, set_original_season_folder_format) = signal(String::new());
    let (original_episode_file_format, set_original_episode_file_format) = signal(String::new());
    let (original_season_folder_format_absolute, set_original_season_folder_format_absolute) =
        signal(String::new());
    let (original_episode_file_format_absolute, set_original_episode_file_format_absolute) =
        signal(String::new());
    let (show_rename_modal, set_show_rename_modal) = signal(false);
    let (_show_apply_rename_btn, set_show_apply_rename_btn) = signal(false);

    let trigger_save = crate::utils::use_autosave(config, |c| save_config(c));

    crate::utils::use_api_cache(
        "fetch_config".to_string(),
        || fetch_config(),
        move |c| {
            set_original_season_folder_format
                .try_update(|f| *f = c.organization.season_folder_format.clone());
            set_original_episode_file_format
                .try_update(|f| *f = c.organization.episode_file_format.clone());
            set_original_season_folder_format_absolute
                .try_update(|f| *f = c.organization.season_folder_format_absolute.clone());
            set_original_episode_file_format_absolute
                .try_update(|f| *f = c.organization.episode_file_format_absolute.clone());
            set_config.try_update(|conf| *conf = Some(c));
        },
    );

    let trigger_save = StoredValue::new_local(move || trigger_save(true));

    SettingsBuilder::new("settings-organization")
        .section("Global Policy", |section| {
            section.field(global_policy_section(config, set_config, trigger_save))
        })
        .section("Search", |section| {
            section.field(search_section(config, set_config, trigger_save))
        })
        .section("Conflict Resolution", |section| {
            section.field(conflict_resolution_section(config, set_config))
        })
        .section("Part Numbering", |section| {
            section.field(part_numbering_section(config, set_config))
        })
        .section("Filename Sanitization", |section| {
            section.field(filename_sanitization_section(config, set_config))
        })
        .section("File Handling", |section| {
            section.field(file_handling_section(config, set_config))
        })
        .section("Library Formats", |section| {
            section.field(library_format_section(config, set_config, trigger_save))
        })
        .raw_section(rename_modal_section(
            config,
            show_rename_modal,
            set_show_rename_modal,
            set_show_apply_rename_btn,
            RenameModalOriginalFormats {
                season_folder_format: original_season_folder_format,
                set_season_folder_format: set_original_season_folder_format,
                episode_file_format: original_episode_file_format,
                set_episode_file_format: set_original_episode_file_format,
                season_folder_format_absolute: original_season_folder_format_absolute,
                set_season_folder_format_absolute: set_original_season_folder_format_absolute,
                episode_file_format_absolute: original_episode_file_format_absolute,
                set_episode_file_format_absolute: set_original_episode_file_format_absolute,
            },
        ))
        .build()
}

fn global_policy_section(
    config: ReadSignal<Option<Config>>,
    set_config: WriteSignal<Option<Config>>,
    trigger_save: StoredValue<impl Fn() + Clone + 'static, LocalStorage>,
) -> impl IntoView {
    view! {
        <CheckboxInput
            label="Rename Episodes".to_string()
            checked=Signal::derive(move || config.get().map(|c| c.organization.rename_episodes).unwrap_or(true))
            set_checked=Callback::new(move |v| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.rename_episodes = v;
                });
                trigger_save.with_value(|t| t());
            })
            help_text="When disabled, downloaded files will be organized into the correct series folder but their original filenames will be preserved.".to_string()
            class="form-label checkbox-group".to_string()
        />
        <SettingsCheckbox
            label="Flatten Seasons"
            binding=ConfigBinding {
                value: Signal::derive(move || config.get().map(|c| c.general.flatten_season_folders).unwrap_or(false)),
                set_value: Callback::new(move |v| {
                    crate::utils::write_config_field(&set_config, |c| {
                        c.general.flatten_season_folders = v;
                    });
                    trigger_save.with_value(|t| t());
                }),
            }
            help_text=Signal::derive(move || "When enabled, all episodes are placed directly in the series folder without season sub-folders.".to_string())
        />
        <SettingsCheckbox
            label="Use Absolute Numbering"
            binding=ConfigBinding {
                value: Signal::derive(move || config.get().map(|c| c.general.absolute_numbering).unwrap_or(false)),
                set_value: Callback::new(move |v| {
                    crate::utils::write_config_field(&set_config, |c| {
                        c.general.absolute_numbering = v;
                    });
                    trigger_save.with_value(|t| t());
                }),
            }
            help_text=Signal::derive(move || "When enabled, new series will track episodes by absolute numbers rather than resetting the episode count each season.".to_string())
        />
    }
}

// Search templates applied to every search query (manual + automatic). Blank
// deletes the season/episode key entirely, searching by title alone.
fn search_section(
    config: ReadSignal<Option<Config>>,
    set_config: WriteSignal<Option<Config>>,
    trigger_save: StoredValue<impl Fn() + Clone + 'static, LocalStorage>,
) -> impl IntoView {
    view! {
        <FormatFieldBuilder
            id="global-search-format".to_string()
            label="S/E Search Format".to_string()
            value=Signal::derive(move || config.get().map(|c| c.organization.search_format.clone()).unwrap_or_default())
            on_change=Callback::new(move |v: String| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.search_format = v;
                });
            })
            tooltip=TooltipBuilder::new().with_context(TemplateContext::SearchFormat).with_formatting()
            help_text=validation::SEARCH_FORMAT_FIELD_HELP.to_string()
            on_blur=Callback::new(move |_| {
                let val = config.get().map(|c| c.organization.search_format.clone()).unwrap_or_default();
                if validation::validate_search_format_field(&val, true, "search format") {
                    return;
                }
                trigger_save.with_value(|t| t());
            })
        />
        <FormatFieldBuilder
            id="global-search-format-absolute".to_string()
            label="S/E Search Format (Absolute)".to_string()
            value=Signal::derive(move || config.get().map(|c| c.organization.search_format_absolute.clone()).unwrap_or_default())
            on_change=Callback::new(move |v: String| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.search_format_absolute = v;
                });
            })
            tooltip=TooltipBuilder::new().with_context(TemplateContext::SearchFormat).with_formatting()
            help_text=validation::SEARCH_FORMAT_FIELD_HELP.to_string()
            on_blur=Callback::new(move |_| {
                let val = config.get().map(|c| c.organization.search_format_absolute.clone()).unwrap_or_default();
                if validation::validate_search_format_field(&val, true, "absolute search format") {
                    return;
                }
                trigger_save.with_value(|t| t());
            })
        />
    }
}

fn conflict_resolution_section(
    config: ReadSignal<Option<Config>>,
    set_config: WriteSignal<Option<Config>>,
) -> impl IntoView {
    let collision_help_text: Signal<String> = Signal::derive(move || {
        let mode = config
            .get()
            .map(|c| c.organization.collision_handling.clone())
            .unwrap_or("rename".to_string());
        match mode.as_str() {
            "rename" => "When a file already exists at the destination, the new file gets a numeric suffix to avoid conflict.".to_string(),
            "skip" => "When a file already exists at the destination, the new file is skipped and not moved.".to_string(),
            "overwrite" => "When a file already exists at the destination, it is overwritten by the new file.".to_string(),
            _ => String::new(),
        }
    });

    let rename_suffix_help_text: Signal<String> = Signal::derive(move || {
        let suffix = config
            .get()
            .map(|c| c.organization.collision_rename_suffix)
            .unwrap_or_default();
        match suffix {
            CollisionRenameSuffix::DotNumeric => {
                "Uses a dot before the number (e.g., `Episode.001.mkv`).".to_string()
            }
            CollisionRenameSuffix::ParenNumeric => {
                "Uses parentheses around the number (e.g., `Episode (1).mkv`).".to_string()
            }
            CollisionRenameSuffix::UnderscoreNumeric => {
                "Uses an underscore before the number (e.g., `Episode_001.mkv`).".to_string()
            }
            CollisionRenameSuffix::DashNumeric => {
                "Uses a dash before the number (e.g., `Episode-1.mkv`).".to_string()
            }
        }
    });

    view! {
        <Select
            label="Collision Handling".to_string()
            id="orgCollisionHandling".to_string()
            value=Signal::derive(move || config.get().map(|c| c.organization.collision_handling.clone()).unwrap_or("rename".to_string()))
            set_value=move |v| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.collision_handling = v;
                });
            }
            help_signal=collision_help_text
            options=Signal::derive(move || vec![
                SelectOption::from(("rename".to_string(), "Rename".to_string())),
                SelectOption::from(("skip".to_string(), "Skip".to_string())),
                SelectOption::from(("overwrite".to_string(), "Overwrite".to_string())),
            ])
        />
        <Select
            label="Collision Rename Suffix".to_string()
            id="orgCollisionRenameSuffix".to_string()
            value=Signal::derive(move || config.get().map(|c| suffix_to_string(&c.organization.collision_rename_suffix)).unwrap_or("dot_numeric".to_string()))
            set_value=Callback::new(move |v: String| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.collision_rename_suffix = string_to_suffix(&v);
                });
            })
            help_signal=rename_suffix_help_text
            options=Signal::derive(move || vec![
                SelectOption::from(("dot_numeric".to_string(), ".001".to_string())),
                SelectOption::from(("paren_numeric".to_string(), " (1)".to_string())),
                SelectOption::from(("underscore_numeric".to_string(), "_001".to_string())),
                SelectOption::from(("dash_numeric".to_string(), "-1".to_string())),
            ])
        />
    }
}

fn part_numbering_section(
    config: ReadSignal<Option<Config>>,
    set_config: WriteSignal<Option<Config>>,
) -> impl IntoView {
    let part_fmt_help_text: Signal<String> = Signal::derive(move || {
        let fmt = config
            .get()
            .map(|c| c.organization.part_number_format)
            .unwrap_or_default();
        match fmt {
            PartNumberFormat::Pt => "Uses `-pt{n}` format (e.g., `Episode - pt2.mkv`).".to_string(),
            PartNumberFormat::Cd => "Uses `-cd{n}` format (e.g., `Episode - cd2.mkv`).".to_string(),
            PartNumberFormat::Part => {
                "Uses `-part{n}` format (e.g., `Episode - part2.mkv`).".to_string()
            }
            PartNumberFormat::Disc => {
                "Uses `-disc{n}` format (e.g., `Episode - disc2.mkv`).".to_string()
            }
            PartNumberFormat::DashNumeric => {
                "Uses `-{n}` format (e.g., `Episode - 2.mkv`).".to_string()
            }
            PartNumberFormat::ParenNumeric => {
                "Uses ` ({n})` format (e.g., `Episode (2).mkv`).".to_string()
            }
        }
    });

    view! {
        <Select
            label="Part Number Format".to_string()
            id="orgPartNumberFormat".to_string()
            value=Signal::derive(move || config.get().map(|c| part_fmt_to_string(&c.organization.part_number_format)).unwrap_or("pt".to_string()))
            set_value=Callback::new(move |v: String| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.part_number_format = string_to_part_fmt(&v);
                });
            })
            help_signal=part_fmt_help_text
            options=Signal::derive(move || vec![
                SelectOption::from(("pt".to_string(), "-pt{n}".to_string())),
                SelectOption::from(("cd".to_string(), "-cd{n}".to_string())),
                SelectOption::from(("part".to_string(), "-part{n}".to_string())),
                SelectOption::from(("disc".to_string(), "-disc{n}".to_string())),
                SelectOption::from(("dash_numeric".to_string(), "-{n}".to_string())),
                SelectOption::from(("paren_numeric".to_string(), " ({n})".to_string())),
            ])
        />
    }
}

fn file_handling_section(
    config: ReadSignal<Option<Config>>,
    set_config: WriteSignal<Option<Config>>,
) -> impl IntoView {
    view! {
        <Select
            label="Unexpected Files".to_string()
            id="unexpectedFiles".to_string()
            value=Signal::derive(move || config.get().map(|c| c.general.unexpected_files_handling.clone()).unwrap_or("keep".to_string()))
            set_value=move |v| {
                crate::utils::write_config_field(&set_config, |c| { c.general.unexpected_files_handling = v; });
            }
            help_signal=Signal::derive(move || {
                match config.get().map(|c| c.general.unexpected_files_handling.clone()).unwrap_or("keep".to_string()).as_str() {
                    "delete" => "Files that can't be linked to an episode (non-media files like .nfo/.jpg, or unparseable videos like samples/trailers) are permanently deleted from disk.",
                    "keep" => "Files that can't be linked to an episode are left in the download directory. Non-media files (.nfo, .jpg, etc.) remain on disk and require manual cleanup.",
                    _ => "",
                }.to_string()
            })
            options=Signal::derive(move || vec![
                SelectOption::from(("delete".to_string(), "Delete".to_string())),
                SelectOption::from(("keep".to_string(), "Keep for Review".to_string())),
            ])
        />
        <Select
            label="Unneeded Episodes".to_string()
            id="unneededEpisodes".to_string()
            value=Signal::derive(move || config.get().map(|c| c.general.unneeded_episodes_handling.clone()).unwrap_or("keep".to_string()))
            set_value=move |v| {
                crate::utils::write_config_field(&set_config, |c| { c.general.unneeded_episodes_handling = v; });
            }
            help_signal=Signal::derive(move || {
                match config.get().map(|c| c.general.unneeded_episodes_handling.clone()).unwrap_or("keep".to_string()).as_str() {
                    "delete" => "Episodes from pack downloads that are not needed (already downloaded or not monitored) are permanently deleted.",
                    "keep" => "Episodes from pack downloads that are not needed are left in the download directory for manual review.",
                    _ => "",
                }.to_string()
            })
            options=Signal::derive(move || vec![
                SelectOption::from(("delete".to_string(), "Delete".to_string())),
                SelectOption::from(("keep".to_string(), "Keep for Review".to_string())),
            ])
        />
    }
}

fn filename_sanitization_section(
    config: ReadSignal<Option<Config>>,
    set_config: WriteSignal<Option<Config>>,
) -> impl IntoView {
    let illegal_help_text: Signal<String> = Signal::derive(move || {
        let policy = config
            .get()
            .map(|c| c.organization.illegal_char_policy)
            .unwrap_or(InvalidCharPolicy::Underscore);
        match policy {
            InvalidCharPolicy::Underscore => "Replaces invalid characters with underscores (`_`) \u{2014} the safest and most compatible choice, guaranteed valid on all filesystems.".to_string(),
            InvalidCharPolicy::Hyphen => "Replaces invalid characters with hyphens (`-`).".to_string(),
            InvalidCharPolicy::Unicode => "Replaces invalid characters with Unicode look-alikes (e.g., `?` \u{2192} `\u{FF1F}`, `:` \u{2192} `\u{FF1A}`) to preserve visual intent without using actual invalid bytes.".to_string(),
            InvalidCharPolicy::Space => "Replaces invalid characters with spaces (` `).".to_string(),
            InvalidCharPolicy::Remove => "Removes invalid characters entirely. Caution: this can concatenate words (e.g., `file:name` becomes `filename`).".to_string(),
        }
    });

    let os = crate::hooks::use_server_os_name();
    let platform_help_text: Signal<String> = Signal::derive(move || {
        let enabled = config
            .get()
            .map(|c| c.organization.allow_platform_specific_chars)
            .unwrap_or(false);
        match (os.get().as_deref(), enabled) {
            (Some("linux"), true) => {
                "Linux-only characters (`?`, `*`, `:`, `\"`, `<`, `>`, `|`) are allowed. Files with these names will fail on Windows.".to_string()
            }
            (Some("macos") | Some("darwin"), true) => {
                "macOS-only characters (`?`, `*`, `\"`, `<`, `>`, `|`) are allowed. Files with these names will fail on Windows.".to_string()
            }
            (Some("windows"), true) => {
                "Windows already enforces the strictest filename character restrictions, so this setting has no effect.".to_string()
            }
            (_, true) => {
                "Characters valid on this OS but not on others are allowed. Files may fail on different platforms.".to_string()
            }
            (_, false) => {
                "Characters that are not cross-platform compatible will be replaced.".to_string()
            }
        }
    });

    view! {
        <Select
            label="Illegal Character Policy".to_string()
            id="orgIllegalCharPolicy".to_string()
            value=Signal::derive(move || config.get().map(|c| policy_to_string(&c.organization.illegal_char_policy)).unwrap_or("underscore".to_string()))
            set_value=Callback::new(move |v: String| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.illegal_char_policy = string_to_policy(&v);
                });
            })
            help_signal=illegal_help_text
            options=Signal::derive(move || vec![
                SelectOption::from(("underscore".to_string(), "Underscore".to_string())),
                SelectOption::from(("hyphen".to_string(), "Hyphen".to_string())),
                SelectOption::from(("unicode".to_string(), "Unicode Look-alike".to_string())),
                SelectOption::from(("space".to_string(), "Space".to_string())),
                SelectOption::from(("remove".to_string(), "Remove".to_string())),
            ])
        />
        <CheckboxInput
            label="Allow Platform-Specific Characters".to_string()
            checked=Signal::derive(move || config.get().map(|c| c.organization.allow_platform_specific_chars).unwrap_or(false))
            set_checked=Callback::new(move |v| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.allow_platform_specific_chars = v;
                });
            })
            help_text=platform_help_text
            class="form-label checkbox-group".to_string()
        />
    }
}

fn library_format_section(
    config: ReadSignal<Option<Config>>,
    set_config: WriteSignal<Option<Config>>,
    trigger_save: StoredValue<impl Fn() + Clone + 'static, LocalStorage>,
) -> impl IntoView {
    view! {
        <FormatFieldBuilder
            id="global-season-folder-format".to_string()
            label="Season Folder Format".to_string()
            value=Signal::derive(move || config.get().map(|c| c.organization.season_folder_format.clone()).unwrap_or_default())
            on_change=Callback::new(move |v: String| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.season_folder_format = v;
                });
            })
            tooltip=TooltipBuilder::new().with_context(TemplateContext::SeasonFolder).with_formatting()
            help_text=validation::SEASON_FOLDER_FORMAT_FIELD_HELP.to_string()
            on_blur=Callback::new(move |_| {
                let val = config.get().map(|c| c.organization.season_folder_format.clone()).unwrap_or_default();
                let allowed_vars = get_allowed_vars(&TemplateContext::SeasonFolder);
                if validation::validate_template_field(&val, &allowed_vars, true, "season folder format") {
                    return;
                }
                trigger_save.with_value(|t| t());
            })
        />
        <FormatFieldBuilder
            id="global-episode-file-format".to_string()
            label="Episode File Format".to_string()
            value=Signal::derive(move || config.get().map(|c| c.organization.episode_file_format.clone()).unwrap_or_default())
            on_change=Callback::new(move |v: String| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.episode_file_format = v;
                });
            })
            tooltip=TooltipBuilder::new().with_context(TemplateContext::EpisodeFile).with_formatting()
            help_text=validation::EPISODE_FILE_FORMAT_FIELD_HELP.to_string()
            on_blur=Callback::new(move |_| {
                let val = config.get().map(|c| c.organization.episode_file_format.clone()).unwrap_or_default();
                let allowed_vars = get_allowed_vars(&TemplateContext::EpisodeFile);
                if validation::validate_template_field(&val, &allowed_vars, false, "episode file format") {
                    return;
                }
                trigger_save.with_value(|t| t());
            })
        />
        <FormatFieldBuilder
            id="global-season-folder-format-absolute".to_string()
            label="Season Folder Format (Absolute)".to_string()
            value=Signal::derive(move || config.get().map(|c| c.organization.season_folder_format_absolute.clone()).unwrap_or_default())
            on_change=Callback::new(move |v: String| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.season_folder_format_absolute = v;
                });
            })
            tooltip=TooltipBuilder::new().with_context(TemplateContext::SeasonFolder).with_formatting()
            help_text=validation::SEASON_FOLDER_FORMAT_FIELD_HELP.to_string()
            on_blur=Callback::new(move |_| {
                let val = config.get().map(|c| c.organization.season_folder_format_absolute.clone()).unwrap_or_default();
                let allowed_vars = get_allowed_vars(&TemplateContext::SeasonFolder);
                if validation::validate_template_field(&val, &allowed_vars, true, "absolute season folder format") {
                    return;
                }
                trigger_save.with_value(|t| t());
            })
        />
        <FormatFieldBuilder
            id="global-episode-file-format-absolute".to_string()
            label="Episode File Format (Absolute)".to_string()
            value=Signal::derive(move || config.get().map(|c| c.organization.episode_file_format_absolute.clone()).unwrap_or_default())
            on_change=Callback::new(move |v: String| {
                crate::utils::write_config_field(&set_config, |c| {
                    c.organization.episode_file_format_absolute = v;
                });
            })
            tooltip=TooltipBuilder::new().with_context(TemplateContext::EpisodeFile).with_formatting()
            help_text=validation::EPISODE_FILE_FORMAT_FIELD_HELP.to_string()
            on_blur=Callback::new(move |_| {
                let val = config.get().map(|c| c.organization.episode_file_format_absolute.clone()).unwrap_or_default();
                let allowed_vars = get_allowed_vars(&TemplateContext::EpisodeFile);
                if validation::validate_template_field(&val, &allowed_vars, false, "absolute episode file format") {
                    return;
                }
                trigger_save.with_value(|t| t());
            })
        />
    }
}

#[derive(Clone, Copy)]
struct RenameModalOriginalFormats {
    season_folder_format: ReadSignal<String>,
    set_season_folder_format: WriteSignal<String>,
    episode_file_format: ReadSignal<String>,
    set_episode_file_format: WriteSignal<String>,
    season_folder_format_absolute: ReadSignal<String>,
    set_season_folder_format_absolute: WriteSignal<String>,
    episode_file_format_absolute: ReadSignal<String>,
    set_episode_file_format_absolute: WriteSignal<String>,
}

fn rename_modal_section(
    config: ReadSignal<Option<Config>>,
    show_rename_modal: ReadSignal<bool>,
    set_show_rename_modal: WriteSignal<bool>,
    set_show_apply_rename_btn: WriteSignal<bool>,
    original_formats: RenameModalOriginalFormats,
) -> impl IntoView {
    view! {
        <crate::components::edit_series::rename_modal::RenameModal
            show=Signal::from(show_rename_modal)
            set_show=set_show_rename_modal
            message=Signal::derive(move || {
                let mut changes = Vec::new();
                let current_season_format = config.get().map(|c| c.organization.season_folder_format).unwrap_or_default();
                let current_episode_format = config.get().map(|c| c.organization.episode_file_format).unwrap_or_default();
                let current_season_format_absolute = config.get().map(|c| c.organization.season_folder_format_absolute).unwrap_or_default();
                let current_episode_format_absolute = config.get().map(|c| c.organization.episode_file_format_absolute).unwrap_or_default();

                if current_season_format != original_formats.season_folder_format.get() { changes.push("Season Folder Format"); }
                if current_episode_format != original_formats.episode_file_format.get() { changes.push("Episode File Format"); }
                if current_season_format_absolute != original_formats.season_folder_format_absolute.get() { changes.push("Season Folder Format (Absolute)"); }
                if current_episode_format_absolute != original_formats.episode_file_format_absolute.get() { changes.push("Episode File Format (Absolute)"); }

                let fields_str = if changes.len() > 1 {
                    let last = changes.pop().unwrap();
                    format!("{} and {}", changes.join(", "), last)
                } else if changes.len() == 1 {
                    changes[0].to_string()
                } else {
                    "File formatting".to_string()
                };

                Some(format!("The global {} setting has been changed. Would you like to reorganize all existing series to use the new file naming scheme now, or apply it later?", fields_str))
            })
            on_now=Callback::new(move |_| {
                set_show_rename_modal.set(false);
                set_show_apply_rename_btn.set(false);
                let current_season_format = config.get().map(|c| c.organization.season_folder_format).unwrap_or_default();
                let current_episode_format = config.get().map(|c| c.organization.episode_file_format).unwrap_or_default();
                let current_season_format_absolute = config.get().map(|c| c.organization.season_folder_format_absolute).unwrap_or_default();
                let current_episode_format_absolute = config.get().map(|c| c.organization.episode_file_format_absolute).unwrap_or_default();

                let refresh_rename_queue = use_context::<crate::hooks::RenameQueueRefresh>();
                let current_season = current_season_format.clone();
                let current_ep = current_episode_format.clone();
                let current_season_abs = current_season_format_absolute.clone();
                let current_ep_abs = current_episode_format_absolute.clone();
                spawn_local(async move {
                    match crate::api::reorganize_all_series_async().await {
                        Ok(result) => {
                            if let Some(task_id) = &result.task_id {
                                let ctx = crate::components::common::toast::use_notification();
                                ctx.show_operation_progress(
                                    "Reorganize all series".to_string(),
                                    task_id.clone(),
                                );
                                let mut polls: u32 = 0;
                                loop {
                                    if polls >= 600 { break; }
                                    polls += 1;
                                    gloo_timers::future::TimeoutFuture::new(500).await;
                                    if let Ok(progress) = crate::api::reorganize_all_status(task_id).await {
                                        ctx.update_progress(
                                            "Reorganize all series",
                                            progress.completed,
                                            progress.total,
                                        );
                                        if progress.finished {
                                            // SSoT: transform the progress toast into the
                                            // single result notification (no second toast).
                                            let op = crate::components::common::structs::ActiveOperation {
                                                id: task_id.clone(),
                                                operation_type: "reorganize".to_string(),
                                                total: progress.total,
                                                completed: progress.completed,
                                                finished: true,
                                                finished_at_ms: None,
                                                success_count: progress.success_count,
                                                failed: progress.failed,
                                                errors: progress.errors.clone(),
                                            };
                                            ctx.finalize_operation(task_id, &op);
                                            break;
                                        }
                                    }
                                }
                            }
                            original_formats.set_season_folder_format.try_update(|f| *f = current_season);
                            original_formats.set_episode_file_format.try_update(|f| *f = current_ep);
                            original_formats.set_season_folder_format_absolute.try_update(|f| *f = current_season_abs);
                            original_formats.set_episode_file_format_absolute.try_update(|f| *f = current_ep_abs);
                            if let Some(r) = refresh_rename_queue {
                                r.1.run(());
                            }
                        }
                        Err(e) => {
                            crate::components::common::toast::show_toast(
                                format!("Failed to start reorganization: {}", e),
                                crate::components::common::toast::NotificationType::Error,
                            );
                        }
                    }
                });
            })
            on_later=Callback::new(move |_| {
                set_show_rename_modal.set(false);
                set_show_apply_rename_btn.set(true);
            })
        />
    }
}
