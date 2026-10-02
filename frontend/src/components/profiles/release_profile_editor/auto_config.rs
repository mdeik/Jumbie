use crate::components::common::form_fields::{
    NumberInput, NumberInputMode, Select, SelectOption, TextInput,
};
use crate::components::common::icons::*;
use crate::components::common::standard_modal::StandardModal;
use crate::components::common::structs::SystemHealth;
use crate::components::common::toast;
use crate::utils::*;
use jumbie_shared::config;
use leptos::control_flow::Show;
use leptos::prelude::*;

/// Describes a single editable field on a rule variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum RuleField {
    Number(&'static str, &'static str),
    Text(&'static str, &'static str),
    ThresholdCondition,
    RuleCondition,
    TrackType,
    UnexpectedFilesMode,
    FileExtensions(&'static str, &'static str),
}

/// Maps a rule variant to its list of editable fields.
fn describe_rule_fields(rule: &config::AutomaticProfileRule) -> Vec<RuleField> {
    match rule {
        config::AutomaticProfileRule::Chapters { .. } => vec![RuleField::RuleCondition],
        config::AutomaticProfileRule::TrackCount { .. } => vec![
            RuleField::Number("count", "Count"),
            RuleField::TrackType,
            RuleField::ThresholdCondition,
        ],
        config::AutomaticProfileRule::Language { .. } => vec![
            RuleField::Text("language", "Language ID"),
            RuleField::TrackType,
            RuleField::RuleCondition,
        ],
        config::AutomaticProfileRule::Resolution { .. } => vec![
            RuleField::Number("width", "Width"),
            RuleField::Number("height", "Height"),
            RuleField::ThresholdCondition,
        ],
        config::AutomaticProfileRule::Bitrate { .. } => vec![
            RuleField::Number("kbps", "Bitrate (kbps)"),
            RuleField::ThresholdCondition,
        ],
        config::AutomaticProfileRule::Codec { .. } => {
            vec![RuleField::Text("codec", "Codec"), RuleField::RuleCondition]
        }
        config::AutomaticProfileRule::AudioChannels { .. } => vec![
            RuleField::Number("channels", "Channels"),
            RuleField::ThresholdCondition,
        ],
        config::AutomaticProfileRule::UnexpectedFiles { .. } => vec![
            RuleField::UnexpectedFilesMode,
            RuleField::FileExtensions("file_patterns", "File Extensions"),
        ],
    }
}

/// Skeleton rule for a given add-form rule type string.
fn dummy_rule_for_type(rule_type: &str) -> Option<config::AutomaticProfileRule> {
    Some(match rule_type {
        "chapters" => config::AutomaticProfileRule::Chapters {
            condition: config::RuleCondition::MustBePresent,
        },
        "track_count" => config::AutomaticProfileRule::TrackCount {
            count: 0,
            track_type: config::TrackType::Any,
            condition: config::ThresholdCondition::Minimum,
        },
        "language" => config::AutomaticProfileRule::Language {
            language: String::new(),
            track_type: config::TrackType::Any,
            condition: config::RuleCondition::MustBePresent,
        },
        "resolution" => config::AutomaticProfileRule::Resolution {
            width: 0,
            height: 0,
            condition: config::ThresholdCondition::Minimum,
        },
        "bitrate" => config::AutomaticProfileRule::Bitrate {
            kbps: 0,
            condition: config::ThresholdCondition::Minimum,
        },
        "codec" => config::AutomaticProfileRule::Codec {
            codec: String::new(),
            condition: config::RuleCondition::MustBePresent,
        },
        "audio_channels" => config::AutomaticProfileRule::AudioChannels {
            channels: 0,
            condition: config::ThresholdCondition::Minimum,
        },
        "unexpected_files" => config::AutomaticProfileRule::UnexpectedFiles {
            file_patterns: vec![],
            mode: config::UnexpectedFilesMode::Blacklist,
        },
        _ => return None,
    })
}

/// Maps string-based rule type identifiers to their field descriptors.
#[cfg(test)]
fn rule_type_to_fields(rule_type: &str) -> Vec<RuleField> {
    dummy_rule_for_type(rule_type)
        .as_ref()
        .map(|dummy| describe_rule_fields(dummy))
        .unwrap_or_default()
}

/// Read the string value of a field from a rule by its field id.
fn read_rule_field(rule: &config::AutomaticProfileRule, id: &str) -> String {
    match (rule, id) {
        (config::AutomaticProfileRule::TrackCount { count, .. }, "count") => count.to_string(),
        (config::AutomaticProfileRule::Resolution { width, .. }, "width") => width.to_string(),
        (config::AutomaticProfileRule::Resolution { height, .. }, "height") => height.to_string(),
        (config::AutomaticProfileRule::Bitrate { kbps, .. }, "kbps") => kbps.to_string(),
        (config::AutomaticProfileRule::AudioChannels { channels, .. }, "channels") => {
            channels.to_string()
        }
        (config::AutomaticProfileRule::Codec { codec, .. }, "codec") => codec.clone(),
        (config::AutomaticProfileRule::Language { language, .. }, "language") => language.clone(),
        _ => String::new(),
    }
}

// local macro to collapse 5 near-identical parse-match arms in write_rule_field.
macro_rules! write_num {
    ($val:expr, $field:expr, $ty:ty) => {
        if let Ok(v) = $val.parse::<$ty>() {
            *$field = v;
            true
        } else {
            false
        }
    };
}

/// Write a string value into a rule field by its field id.
/// Returns true on success, false on parse failure.
fn write_rule_field(rule: &mut config::AutomaticProfileRule, id: &str, val: &str) -> bool {
    match (rule, id) {
        (config::AutomaticProfileRule::TrackCount { count, .. }, "count") => {
            write_num!(val, count, i32)
        }
        (config::AutomaticProfileRule::Resolution { width, .. }, "width") => {
            write_num!(val, width, u32)
        }
        (config::AutomaticProfileRule::Resolution { height, .. }, "height") => {
            write_num!(val, height, u32)
        }
        (config::AutomaticProfileRule::Bitrate { kbps, .. }, "kbps") => {
            write_num!(val, kbps, u64)
        }
        (config::AutomaticProfileRule::AudioChannels { channels, .. }, "channels") => {
            write_num!(val, channels, u32)
        }
        (config::AutomaticProfileRule::Codec { codec, .. }, "codec") => {
            *codec = val.to_string();
            true
        }
        (config::AutomaticProfileRule::Language { language, .. }, "language") => {
            *language = val.to_string();
            true
        }
        _ => false,
    }
}

/// Which type of input to render for a value field.
#[derive(Clone, Copy)]
enum ValueInputType {
    Number,
    Text,
}

/// Render a number or text field bound to a rule stored in the config map.
fn render_edit_value(
    input_type: ValueInputType,
    id: &'static str,
    label: &'static str,
    cat_name: &str,
    cfg: ReadSignal<Option<config::Config>>,
    set_cfg: WriteSignal<Option<config::Config>>,
) -> impl IntoView {
    let n = cat_name.to_string();
    let value = Signal::derive({
        let n = n.clone();
        move || {
            cfg.get()
                .and_then(|c| c.general.automatic_profiles.categories.get(&n).cloned())
                .map(|cat| read_rule_field(&cat.rule, id))
                .unwrap_or_default()
        }
    });
    let set_value = Callback::new({
        let n = n.clone();
        move |val: String| {
            if let Some(mut c) = cfg.get_untracked() {
                if let Some(cat_ref) = c.general.automatic_profiles.categories.get_mut(&n) {
                    write_rule_field(&mut cat_ref.rule, id, &val);
                }
                let c_for_cache = c.clone();
                set_cfg.set(Some(c));
                spawn_api_toast(
                    crate::api::save_config(c_for_cache.clone()),
                    None,
                    move |_| {
                        write_cache("fetch_config", &c_for_cache);
                    },
                );
            }
        }
    });
    match input_type {
        ValueInputType::Number => view! {
            <NumberInput
                id=format!("{}-{}", id, cat_name)
                label=label.to_string()
                value=value
                set_value=set_value
                class="flex-1"
                mode=if matches!(id, "count" | "width" | "height" | "kbps") { NumberInputMode::Integer } else { NumberInputMode::SignedInteger }
            />
        }
        .into_any(),
        ValueInputType::Text => view! {
            <TextInput
                id=format!("{}-{}", id, cat_name)
                label=label.to_string()
                value=value
                set_value=set_value
                class="flex-1"
            />
        }
        .into_any(),
    }
}

struct EditSelectParams {
    id: &'static str,
    label: &'static str,
    cat_name: String,
    config_sig: ReadSignal<Option<config::Config>>,
    set_config_sig: WriteSignal<Option<config::Config>>,
    read_val: fn(&config::AutomaticProfileRule) -> String,
    write_val: fn(&mut config::AutomaticProfileRule, &str),
    options: Signal<Vec<SelectOption>>,
}

fn render_edit_select(params: EditSelectParams) -> impl IntoView {
    let n = params.cat_name.clone();
    view! {
        <Select
            id=format!("{}-{}", params.id, params.cat_name)
            label=params.label.to_string()
            value=Signal::derive({
                let n = n.clone();
                move || {
                    params.config_sig.get()
                        .and_then(|c| c.general.automatic_profiles.categories.get(&n).cloned())
                        .map(|cat| (params.read_val)(&cat.rule))
                        .unwrap_or_default()
                }
            })
            set_value=Callback::new({
                let n = n.clone();
                move |val: String| {
                    if let Some(mut c) = params.config_sig.get_untracked() {
                        if let Some(cat_ref) = c.general.automatic_profiles.categories.get_mut(&n) {
                            (params.write_val)(&mut cat_ref.rule, &val);
                        }
                        let c_for_cache = c.clone();
                        params.set_config_sig.set(Some(c));
                        spawn_api_toast(crate::api::save_config(c_for_cache.clone()), None, move |_| {
                            write_cache("fetch_config", &c_for_cache);
                        });
                    }
                }
            })
            options=params.options
            class="flex-1"
        />
    }
    .into_any()
}

fn threshold_cond_value(rule: &config::AutomaticProfileRule) -> String {
    match rule {
        config::AutomaticProfileRule::TrackCount { condition, .. }
        | config::AutomaticProfileRule::Resolution { condition, .. }
        | config::AutomaticProfileRule::Bitrate { condition, .. }
        | config::AutomaticProfileRule::AudioChannels { condition, .. } => match condition {
            config::ThresholdCondition::Minimum => "minimum",
            config::ThresholdCondition::Maximum => "maximum",
            config::ThresholdCondition::Exact => "exact",
        },
        _ => "minimum",
    }
    .to_string()
}

fn threshold_cond_write(rule: &mut config::AutomaticProfileRule, val: &str) {
    let c = match val {
        "maximum" => config::ThresholdCondition::Maximum,
        "exact" => config::ThresholdCondition::Exact,
        _ => config::ThresholdCondition::Minimum,
    };
    if let config::AutomaticProfileRule::TrackCount { condition, .. }
    | config::AutomaticProfileRule::Resolution { condition, .. }
    | config::AutomaticProfileRule::Bitrate { condition, .. }
    | config::AutomaticProfileRule::AudioChannels { condition, .. } = rule
    {
        *condition = c;
    }
}

fn rule_cond_value(rule: &config::AutomaticProfileRule) -> String {
    match rule {
        config::AutomaticProfileRule::Chapters { condition, .. }
        | config::AutomaticProfileRule::Codec { condition, .. }
        | config::AutomaticProfileRule::Language { condition, .. } => match condition {
            config::RuleCondition::MustBePresent => "must_be_present",
            config::RuleCondition::MustNotBePresent => "must_not_be_present",
        },
        _ => "must_be_present",
    }
    .to_string()
}

fn rule_cond_write(rule: &mut config::AutomaticProfileRule, val: &str) {
    let c = match val {
        "must_not_be_present" => config::RuleCondition::MustNotBePresent,
        _ => config::RuleCondition::MustBePresent,
    };
    if let config::AutomaticProfileRule::Chapters { condition, .. }
    | config::AutomaticProfileRule::Codec { condition, .. }
    | config::AutomaticProfileRule::Language { condition, .. } = rule
    {
        *condition = c;
    }
}

fn track_type_value(rule: &config::AutomaticProfileRule) -> String {
    match rule {
        config::AutomaticProfileRule::TrackCount { track_type, .. }
        | config::AutomaticProfileRule::Language { track_type, .. } => match track_type {
            config::TrackType::Any => "any",
            config::TrackType::Audio => "audio",
            config::TrackType::Subtitle => "subtitle",
            config::TrackType::Video => "video",
        },
        _ => "any",
    }
    .to_string()
}

fn track_type_write(rule: &mut config::AutomaticProfileRule, val: &str) {
    let t = match val {
        "audio" => config::TrackType::Audio,
        "subtitle" => config::TrackType::Subtitle,
        "video" => config::TrackType::Video,
        _ => config::TrackType::Any,
    };
    if let config::AutomaticProfileRule::TrackCount { track_type, .. }
    | config::AutomaticProfileRule::Language { track_type, .. } = rule
    {
        *track_type = t;
    }
}

fn unexpected_mode_value(rule: &config::AutomaticProfileRule) -> String {
    match rule {
        config::AutomaticProfileRule::UnexpectedFiles { mode, .. } => match mode {
            config::UnexpectedFilesMode::Blacklist => "blacklist",
            config::UnexpectedFilesMode::Whitelist => "whitelist",
        },
        _ => "blacklist",
    }
    .to_string()
}

fn unexpected_mode_write(rule: &mut config::AutomaticProfileRule, val: &str) {
    if let config::AutomaticProfileRule::UnexpectedFiles { mode, .. } = rule {
        *mode = match val {
            "whitelist" => config::UnexpectedFilesMode::Whitelist,
            _ => config::UnexpectedFilesMode::Blacklist,
        };
    }
}

/// Renders all editable fields for a rule in the existing-categories display,
/// using `describe_rule_fields` to determine which fields to render.
struct EditFieldsParams<'a> {
    rule: &'a config::AutomaticProfileRule,
    cat_name: String,
    config_sig: ReadSignal<Option<config::Config>>,
    set_config_sig: WriteSignal<Option<config::Config>>,
    track_type_options: Signal<Vec<SelectOption>>,
    threshold_options: Signal<Vec<SelectOption>>,
    rule_options: Signal<Vec<SelectOption>>,
    unexpected_mode_options: Signal<Vec<SelectOption>>,
}

fn render_edit_fields(params: EditFieldsParams) -> impl IntoView {
    let EditFieldsParams {
        rule,
        cat_name,
        config_sig,
        set_config_sig,
        track_type_options,
        threshold_options,
        rule_options,
        unexpected_mode_options,
    } = params;

    let cat_name = cat_name.clone();
    let fields = describe_rule_fields(rule);

    // For UnexpectedFiles, render mode + patterns in a flex row, then the info hint below.
    if matches!(rule, config::AutomaticProfileRule::UnexpectedFiles { .. }) {
        let n = cat_name.clone();
        return view! {
            <div class="flex gap-md items-end">
                <Select
                    id=format!("uf-mode-{}", cat_name)
                    label="Mode".to_string()
                    value=Signal::derive({
                        let n = n.clone();
                        move || {
                            config_sig.get()
                                .and_then(|c| c.general.automatic_profiles.categories.get(&n).cloned())
                                .map(|cat| unexpected_mode_value(&cat.rule))
                                .unwrap_or_default()
                        }
                    })
                    set_value=Callback::new({
                        let n = n.clone();
                        move |val: String| {
                            if let Some(mut c) = config_sig.get_untracked() {
                                if let Some(cat_ref) = c.general.automatic_profiles.categories.get_mut(&n) {
                                    unexpected_mode_write(&mut cat_ref.rule, &val);
                                }
                                let c_for_cache = c.clone();
                                set_config_sig.set(Some(c));
                                spawn_api_toast(crate::api::save_config(c_for_cache.clone()), None, move |_| {
                                    write_cache("fetch_config", &c_for_cache);
                                });
                            }
                        }
                    })
                    options=unexpected_mode_options.clone()
                    class="flex-1"
                />
                <TextInput
                    id=format!("uf-files-{}", cat_name)
                    label="File Extensions".to_string()
                    placeholder="Comma-separated, e.g. srt, idx, sub"
                    value=Signal::derive({
                        let n = n.clone();
                        move || {
                            config_sig.get()
                                .and_then(|c| c.general.automatic_profiles.categories.get(&n).cloned())
                                .and_then(|cat| {
                                    if let config::AutomaticProfileRule::UnexpectedFiles { file_patterns, .. } = &cat.rule {
                                        Some(file_patterns.join(", "))
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or_default()
                        }
                    })
                    set_value=Callback::new({
                        let n = n.clone();
                        move |val: String| {
                            let patterns: Vec<String> = val
                                .split([',', ';'])
                                .map(|s| s.trim().trim_start_matches('.').to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            if let Some(mut c) = config_sig.get_untracked() {
                                if let Some(cat_ref) = c.general.automatic_profiles.categories.get_mut(&n)
                                    && let config::AutomaticProfileRule::UnexpectedFiles { ref mut file_patterns, .. } = cat_ref.rule {
                                        *file_patterns = patterns;
                                    }
                                let c_for_cache = c.clone();
                                set_config_sig.set(Some(c));
                                spawn_api_toast(crate::api::save_config(c_for_cache.clone()), None, move |_| {
                                    write_cache("fetch_config", &c_for_cache);
                                });
                            }
                        }
                    })
                    class="flex-1"
                />
            </div>
            <div class="form-hint text-xs">
                {move || {
                    if let Some(c) = config_sig.get()
                        && let Some(cat) = c.general.automatic_profiles.categories.get(&n)
                            && let config::AutomaticProfileRule::UnexpectedFiles { file_patterns, mode } = &cat.rule {
                                return match mode {
                                    config::UnexpectedFilesMode::Blacklist => view! {
                                        <div>
                                            <div class="font-medium mb-xs">"Globally ignored (override by adding to list above):"</div>
                                            <div class="flex flex-wrap gap-xs">
                                                {jumbie_shared::media_format::IGNORED_UNKNOWN_EXTS.iter().map(|ext| {
                                                    let is_overridden = file_patterns.iter().any(|p| p.eq_ignore_ascii_case(ext));
                                                    let class = if is_overridden { "line-through text-danger" } else { "" };
                                                    view! { <span class=class>{format!(".{}", ext)}</span> }
                                                }).collect_view()}
                                            </div>
                                            <div class="mt-xs text-muted-color">
                                                "Video and subtitle files are always allowed regardless of patterns (only applies to unrecognized files)."
                                            </div>
                                        </div>
                                    }.into_any(),
                                    config::UnexpectedFilesMode::Whitelist => view! {
                                        <div class="text-muted-color">
                                            "Video and subtitle files are always allowed regardless of patterns."
                                        </div>
                                    }.into_any(),
                                };
                            }
                    view! {}.into_any()
                }}
            </div>
        }.into_any();
    }

    // Non-UnexpectedFiles: render each field from the descriptor.
    view! {
        <For
            each=move || fields.clone()
            key=|f| *f
            children=move |field| {
                match field {
                    RuleField::Number(id, label) => {
                        render_edit_value(ValueInputType::Number, id, label, &cat_name, config_sig, set_config_sig).into_any()
                    }
                    RuleField::Text(id, label) => {
                        render_edit_value(ValueInputType::Text, id, label, &cat_name, config_sig, set_config_sig).into_any()
                    }
                    RuleField::ThresholdCondition => {
                        render_edit_select(EditSelectParams {
                            id: "cond",
                            label: "Condition",
                            cat_name: cat_name.clone(),
                            config_sig,
                            set_config_sig,
                            read_val: threshold_cond_value,
                            write_val: threshold_cond_write,
                            options: threshold_options.clone(),
                        }).into_any()
                    }
                    RuleField::RuleCondition => {
                        render_edit_select(EditSelectParams {
                            id: "cond",
                            label: "Condition",
                            cat_name: cat_name.clone(),
                            config_sig,
                            set_config_sig,
                            read_val: rule_cond_value,
                            write_val: rule_cond_write,
                            options: rule_options.clone(),
                        }).into_any()
                    }
                    RuleField::TrackType => {
                        render_edit_select(EditSelectParams {
                            id: "tracktype",
                            label: "Track Type",
                            cat_name: cat_name.clone(),
                            config_sig,
                            set_config_sig,
                            read_val: track_type_value,
                            write_val: track_type_write,
                            options: track_type_options.clone(),
                        }).into_any()
                    }
                    _ => view! {}.into_any()
                }
            }
        />
    }
    .into_any()
}

/// Add-form helper: renders editable fields for a draft rule using RwSignal and
/// the same field descriptors as the edit display.
struct DraftFieldsParams {
    rule_type: Signal<String>,
    draft: RwSignal<config::AutomaticProfileRule>,
    draft_patterns: RwSignal<String>,
    draft_unexpected_mode: RwSignal<String>,
    rule_condition_options: Signal<Vec<SelectOption>>,
    threshold_condition_options: Signal<Vec<SelectOption>>,
    track_type_options: Signal<Vec<SelectOption>>,
    unexpected_mode_options: Signal<Vec<SelectOption>>,
}

fn render_draft_fields(params: DraftFieldsParams) -> impl IntoView {
    let fields = Signal::derive(move || {
        dummy_rule_for_type(&params.rule_type.get())
            .as_ref()
            .map(describe_rule_fields)
            .unwrap_or_default()
    });

    view! {
        <For
            each=move || fields.get()
            key=|f| *f
            children=move |field| {
                let d = params.draft;
                match field {
                    RuleField::Number(id, label) => {
                        let value = Signal::derive(move || read_rule_field(&d.get(), id));
                        let set_value = Callback::new(move |v: String| {
                            d.update(|r| { write_rule_field(r, id, &v); });
                        });
                        view! {
                            // inline id check avoids a bool field on the enum; upgrade to a flag on RuleField::Number if more fields diverge
                            <NumberInput
                                id=format!("new-cat-{}", id)
                                label=label.to_string()
                                value=value
                                set_value=set_value
                                mode=if matches!(id, "count" | "width" | "height" | "kbps") { NumberInputMode::Text } else { NumberInputMode::SignedInteger }
                            />
                        }.into_any()
                    }
                    RuleField::Text(id, label) => {
                        let value = Signal::derive(move || read_rule_field(&d.get(), id));
                        let set_value = Callback::new(move |v: String| {
                            d.update(|r| { write_rule_field(r, id, &v); });
                        });
                        view! {
                            <TextInput
                                id=format!("new-cat-{}", id)
                                label=label.to_string()
                                value=value
                                set_value=set_value
                            />
                        }.into_any()
                    }
                    RuleField::ThresholdCondition => {
                        let value = Signal::derive(move || threshold_cond_value(&d.get()));
                        let set_value = Callback::new(move |v: String| {
                            d.update(|r| threshold_cond_write(r, &v));
                        });
                        view! {
                            <Select
                                id="new-cat-thresh-cond"
                                label="Condition".to_string()
                                value=value
                                set_value=set_value
                                options=params.threshold_condition_options.clone()
                            />
                        }.into_any()
                    }
                    RuleField::RuleCondition => {
                        let value = Signal::derive(move || rule_cond_value(&d.get()));
                        let set_value = Callback::new(move |v: String| {
                            d.update(|r| rule_cond_write(r, &v));
                        });
                        view! {
                            <Select
                                id="new-cat-rule-cond"
                                label="Condition".to_string()
                                value=value
                                set_value=set_value
                                options=params.rule_condition_options.clone()
                            />
                        }.into_any()
                    }
                    RuleField::TrackType => {
                        let value = Signal::derive(move || track_type_value(&d.get()));
                        let set_value = Callback::new(move |v: String| {
                            d.update(|r| track_type_write(r, &v));
                        });
                        view! {
                            <Select
                                id="new-cat-track-type"
                                label="Track Type".to_string()
                                value=value
                                set_value=set_value
                                options=params.track_type_options.clone()
                            />
                        }.into_any()
                    }
                    RuleField::UnexpectedFilesMode => {
                        let set_value = Callback::new(move |v: String| {
                            params.draft_unexpected_mode.set(v);
                        });
                        view! {
                            <Select
                                id="new-cat-unexpected-mode"
                                label="Mode".to_string()
                                value=Signal::derive(move || params.draft_unexpected_mode.get())
                                set_value=set_value
                                options=params.unexpected_mode_options.clone()
                                help_text=Signal::derive(move || {
                                    if params.draft_unexpected_mode.get() == "whitelist" {
                                        "Whitelist: only the listed extensions are flagged (empty = flag nothing)."
                                    } else {
                                        "Blacklist: the listed extensions are flagged (empty = flag all unexpected files)."
                                    }.to_string()
                                })
                            />
                        }.into_any()
                    }
                    RuleField::FileExtensions(_, _) => {
                        view! {
                            <TextInput
                                id="new-cat-files"
                                label="File Extensions".to_string()
                                placeholder=Signal::derive(move || {
                                    if params.draft_unexpected_mode.get() == "whitelist" {
                                        "Specify extensions to flag (empty = flag none), e.g. srt, idx, sub"
                                    } else {
                                        "Leave empty for all files, or specify e.g. srt, idx, sub"
                                    }
                                })
                                value=Signal::derive(move || params.draft_patterns.get())
                                set_value=Callback::new(move |v: String| params.draft_patterns.set(v))
                                help_text="Comma-separated list of file extensions."
                            />
                        }.into_any()
                    }
                }
            }
        />
        <Show when=move || dummy_rule_for_type(&params.rule_type.get()).is_some_and(|r| matches!(r, config::AutomaticProfileRule::UnexpectedFiles { .. }))>
            {move || if params.draft_unexpected_mode.get() == "blacklist" {
                view! {
                    <div class="form-hint mt-sm text-xs">
                        <div class="font-medium mb-xs">"Globally ignored (override by adding to list above):"</div>
                        <div class="flex flex-wrap gap-xs">
                            {let patterns: Vec<String> = params.draft_patterns.get()
                                .split([',', ';'])
                                .map(|s| s.trim().trim_start_matches('.').to_lowercase())
                                .filter(|s| !s.is_empty())
                                .collect();
                            jumbie_shared::media_format::IGNORED_UNKNOWN_EXTS.iter().map(|ext| {
                                let is_overridden = patterns.iter().any(|p| p == ext);
                                let class = if is_overridden { "line-through text-danger" } else { "" };
                                view! { <span class=class>{format!(".{}", ext)}</span> }
                            }).collect_view()}
                        </div>
                        <div class="mt-xs text-muted-color">
                            "Video and subtitle files are always allowed regardless of patterns (only applies to unrecognized files)."
                        </div>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="form-hint mt-sm text-xs">
                        <div class="font-medium mb-xs">"Default input extensions (can edit):"</div>
                        <div class="flex flex-wrap gap-xs">
                            {jumbie_shared::media_format::IGNORED_UNKNOWN_EXTS.iter().map(|ext| {
                                view! { <span>{format!(".{}", ext)}</span> }
                            }).collect_view()}
                        </div>
                        <div class="mt-xs text-muted-color">
                            "Video and subtitle files are always allowed regardless of patterns (only applies to unrecognized files)."
                        </div>
                    </div>
                }.into_any()
            }}
        </Show>
    }
    .into_any()
}

#[component]
pub fn AutoProfilesConfigModal(
    show: Signal<bool>,
    cfg: ReadSignal<Option<config::Config>>,
    set_cfg: WriteSignal<Option<config::Config>>,
    health: LocalResource<Option<SystemHealth>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let draft_name = RwSignal::new(String::new());
    let draft_modifier = RwSignal::new(String::new());
    let draft_bound = RwSignal::new(String::new());
    let draft_rule_type = RwSignal::new("unexpected_files".to_string());
    let draft_rule: RwSignal<config::AutomaticProfileRule> =
        RwSignal::new(dummy_rule_for_type("unexpected_files").unwrap());
    let draft_patterns = RwSignal::new(String::new());
    let draft_unexpected_mode = RwSignal::new("blacklist".to_string());
    let validation_triggered = RwSignal::new(false);

    let rule_condition_options = Signal::derive(move || {
        vec![
            SelectOption::from(("must_be_present".to_string(), "Must Be Present".to_string())),
            SelectOption::from((
                "must_not_be_present".to_string(),
                "Must Not Be Present".to_string(),
            )),
        ]
    });
    let threshold_condition_options = Signal::derive(move || {
        vec![
            SelectOption::from(("minimum".to_string(), "Minimum".to_string())),
            SelectOption::from(("maximum".to_string(), "Maximum".to_string())),
            SelectOption::from(("exact".to_string(), "Exact".to_string())),
        ]
    });
    let unexpected_mode_options = Signal::derive(move || {
        vec![
            SelectOption::from(("blacklist".to_string(), "Blacklist".to_string())),
            SelectOption::from(("whitelist".to_string(), "Whitelist".to_string())),
        ]
    });
    let track_type_options = Signal::derive(move || {
        vec![
            SelectOption::from(("any".to_string(), "Any".to_string())),
            SelectOption::from(("audio".to_string(), "Audio".to_string())),
            SelectOption::from(("subtitle".to_string(), "Subtitle".to_string())),
            SelectOption::from(("video".to_string(), "Video".to_string())),
        ]
    });

    let is_name_empty = move || draft_name.get().trim().is_empty();
    let is_name_duplicate = move || {
        let name = draft_name.get();
        if name.is_empty() {
            return false;
        }
        cfg.get()
            .map(|c| c.general.automatic_profiles.categories.contains_key(&name))
            .unwrap_or(false)
    };

    // A single validation signal derives all field errors, instead of one signal
    // per field with conditionally shown Show blocks.
    let name_error = Signal::derive(move || {
        if !validation_triggered.get() {
            return None;
        }
        if is_name_empty() {
            Some("Name is required".to_string())
        } else if is_name_duplicate() {
            Some("Category already exists".to_string())
        } else {
            None
        }
    });

    let modifier_error = Signal::derive(move || {
        if validation_triggered.get() && draft_modifier.get().parse::<i32>().is_err() {
            Some("Invalid number".to_string())
        } else {
            None
        }
    });

    let bound_error = Signal::derive(move || {
        if validation_triggered.get()
            && !draft_bound.get().trim().is_empty()
            && draft_bound.get().parse::<i32>().is_err()
        {
            Some("Invalid number".to_string())
        } else {
            None
        }
    });

    // Generic validation: try write_rule_field on a clone of the draft rule to
    // check parseability, instead of per-field Show blocks.
    let field_errors = Signal::derive(move || {
        if !validation_triggered.get() {
            return Vec::<(String, String)>::new();
        }
        let mut errs = Vec::new();
        let rule_type = draft_rule_type.get();
        if let Some(dummy) = dummy_rule_for_type(&rule_type) {
            for field in describe_rule_fields(&dummy) {
                match field {
                    RuleField::Number(id, label) => {
                        let val: String = read_rule_field(&draft_rule.get(), id);
                        if val.is_empty() || !write_rule_field(&mut dummy.clone(), id, &val) {
                            errs.push((id.to_string(), format!("Invalid {}", label)));
                        }
                    }
                    RuleField::Text(id, label) => {
                        let val = read_rule_field(&draft_rule.get(), id);
                        if val.trim().is_empty() {
                            errs.push((id.to_string(), format!("{} is required", label)));
                        }
                    }
                    _ => {}
                }
            }
            // Extra: if language or codec is empty
            if rule_type == "language"
                && read_rule_field(&draft_rule.get(), "language")
                    .trim()
                    .is_empty()
            {
                errs.push((
                    "language".to_string(),
                    "Language code is required".to_string(),
                ));
            }
            if rule_type == "codec"
                && read_rule_field(&draft_rule.get(), "codec")
                    .trim()
                    .is_empty()
            {
                errs.push(("codec".to_string(), "Codec is required".to_string()));
            }
        }
        errs
    });

    // Tracks only whether any automatic-profile categories exist, so the grid
    // below is not rebuilt (and inputs not remounted) on every config edit.
    let has_categories = Memo::new(move |_| {
        cfg.get()
            .map(|c| !c.general.automatic_profiles.categories.is_empty())
            .unwrap_or(false)
    });

    view! {
        <StandardModal
            show=show
            on_close=move |_| on_close.run(())
            title=Signal::derive(move || "Automatic Profiles Configuration".to_string())
            size="modal-xl"
        >
            {move || {
                if !has_categories.get() {
                    return view! {}.into_any();
                }

                view! {
                    <div class="settings-grid grid grid-md-cols-2 gap-lg mb-xl">
                        <For
                            each=move || {
                                let mut cats = cfg.get().map(|c| c.general.automatic_profiles.categories.clone().into_iter().collect::<Vec<_>>()).unwrap_or_default();
                                cats.sort_by(|a, b| a.0.cmp(&b.0));
                                cats
                            }
                            key=|c| c.0.clone()
                            children=move |(name, cat)| {
                                let n_display = name.clone();
                                let n_delete = name.clone();

                                let is_ffmpeg_reliant = !matches!(cat.rule, config::AutomaticProfileRule::UnexpectedFiles { .. });
                                let ffmpeg_ok = health.get().flatten().map(|h| h.ffmpeg_installed).unwrap_or(true);
                                let is_disabled = is_ffmpeg_reliant && !ffmpeg_ok;

                                let rule_display = match &cat.rule {
                                    config::AutomaticProfileRule::UnexpectedFiles { .. } => "Unexpected Files",
                                    config::AutomaticProfileRule::Chapters { .. } => "Chapters",
                                    config::AutomaticProfileRule::TrackCount { .. } => "Track Count",
                                    config::AutomaticProfileRule::Language { .. } => "Language",
                                    config::AutomaticProfileRule::Resolution { .. } => "Resolution",
                                    config::AutomaticProfileRule::Bitrate { .. } => "Bitrate",
                                    config::AutomaticProfileRule::Codec { .. } => "Codec",
                                    config::AutomaticProfileRule::AudioChannels { .. } => "Audio Channels",
                                };

                                view! {
                                    <div
                                        class="auto-profile-category-card"
                                        class:is-disabled=is_disabled
                                        title=if is_disabled { "FFmpeg could not be found (Rules still active but cannot be modified)" } else { "" }
                                    >
                                        <div class="flex justify-between items-center mb-md">
                                            <h4 class="m-0 text-primary font-bold">{n_display}</h4>
                                            <button class="btn btn-ghost btn-icon text-danger" on:click=move |_| {
                                                if let Some(mut c) = cfg.get() {
                                                    c.general.automatic_profiles.categories.remove(&n_delete);
                                                    set_cfg.set(Some(c.clone()));
                                                    let c_for_cache = c.clone();
                                                    spawn_api_toast(crate::api::save_config(c), None, move |_| {
                                                        write_cache("fetch_config", &c_for_cache);
                                                    });
                                                }
                                            }>
                                                <TrashIcon />
                                            </button>
                                        </div>
                                        <div class="text-xs text-secondary-color mb-md italic">{rule_display}</div>

                                        <div class=if is_disabled { "pointer-events-none" } else { "" }>
                                            <div class="flex gap-md mb-md">
                                                <NumberInput
                                                    id=format!("modifier-{}", name)
                                                    label="Score Modifier".to_string()
                                                    value=Signal::derive(move || cat.modifier.to_string())
                                                    mode=NumberInputMode::SignedInteger
                                                    set_value=Callback::new({
                                                        let n = name.clone();
                                                        move |val: String| {
                                                            if let Ok(val) = val.parse::<i32>()
                                                                && let Some(mut c) = cfg.get() {
                                                                    if let Some(cat_ref) = c.general.automatic_profiles.categories.get_mut(&n) {
                                                                        cat_ref.modifier = val;
                                                                    }
                                                                    let c_for_cache = c.clone();
                                                                    set_cfg.set(Some(c.clone()));
                                                                    spawn_api_toast(crate::api::save_config(c), None, move |_| {
                                                                        write_cache("fetch_config", &c_for_cache);
                                                                    });
                                                                }
                                                        }
                                                    })
                                                    class="flex-1"
                                                />
                                                <NumberInput
                                                    id=format!("max-modifier-{}", name)
                                                    label="Upper/Lower Bound".to_string()
                                                    value=Signal::derive(move || cat.bound.to_string())
                                                    mode=NumberInputMode::SignedInteger
                                                    set_value=Callback::new({
                                                        let n = name.clone();
                                                        move |val: String| {
                                                            if let Ok(val) = val.parse::<i32>()
                                                                && let Some(mut c) = cfg.get() {
                                                                    if let Some(cat_ref) = c.general.automatic_profiles.categories.get_mut(&n) {
                                                                        cat_ref.bound = val;
                                                                    }
                                                                    let c_for_cache = c.clone();
                                                                    set_cfg.set(Some(c.clone()));
                                                                    spawn_api_toast(crate::api::save_config(c), None, move |_| {
                                                                        write_cache("fetch_config", &c_for_cache);
                                                                    });
                                                                }
                                                        }
                                                    })
                                                    class="flex-1"
                                                />
                                            </div>

                                            {render_edit_fields(EditFieldsParams {
                                                rule: &cat.rule,
                                                cat_name: name.clone(),
                                                config_sig: cfg,
                                                set_config_sig: set_cfg,
                                                track_type_options: track_type_options.clone(),
                                                threshold_options: threshold_condition_options.clone(),
                                                rule_options: rule_condition_options.clone(),
                                                unexpected_mode_options: unexpected_mode_options.clone(),
                                            }).into_any()}
                                        </div>
                                    </div>
                                }
                            }
                        />
                    </div>
                }
                .into_any()
            }}

            <div class="auto-profile-add-category">
                <h4 class="mb-md text-primary font-bold">"Add New Category"</h4>
                <div class="grid grid-cols-2 gap-md mb-md">
                    <TextInput
                        id="new-cat-name"
                        label="Category Name".to_string()
                        value=Signal::derive(move || draft_name.get())
                        set_value=Callback::new(move |v| draft_name.set(v))
                        error=name_error
                    />
                    <Select
                        id="new-cat-rule"
                        label="Rule Type".to_string()
                        value=Signal::derive(move || draft_rule_type.get())
                        set_value=Callback::new(move |v: String| {
                            draft_rule_type.set(v.clone());
                            draft_rule.set(dummy_rule_for_type(&v).unwrap_or(
                                config::AutomaticProfileRule::UnexpectedFiles {
                                    file_patterns: vec![],
                                    mode: config::UnexpectedFilesMode::Blacklist,
                                }
                            ));
                            draft_patterns.set(String::new());
                            draft_unexpected_mode.set("blacklist".to_string());
                            validation_triggered.set(false);
                        })
                        options=Signal::derive(move || {
                            let mut opts = vec![
                                SelectOption::from(("unexpected_files".to_string(), "Unexpected Files".to_string())),
                            ];

                            let ffmpeg_ok = health.get().flatten().map(|h| h.ffmpeg_installed).unwrap_or(true);
                            if ffmpeg_ok {
                                opts.push(SelectOption::from(("chapters".to_string(), "Chapters".to_string())));
                                opts.push(SelectOption::from(("track_count".to_string(), "Track Count".to_string())));
                                opts.push(SelectOption::from(("language".to_string(), "Language".to_string())));
                                opts.push(SelectOption::from(("resolution".to_string(), "Resolution".to_string())));
                                opts.push(SelectOption::from(("bitrate".to_string(), "Bitrate".to_string())));
                                opts.push(SelectOption::from(("codec".to_string(), "Codec".to_string())));
                                opts.push(SelectOption::from(("audio_channels".to_string(), "Audio Channels".to_string())));
                            }
                            opts
                        })
                    />
                </div>
                <div class="grid grid-cols-2 gap-md mb-md">
                    <NumberInput
                        id="new-cat-modifier"
                        label="Score Modifier".to_string()
                        placeholder="-5"
                        value=Signal::derive(move || draft_modifier.get())
                        set_value=Callback::new(move |v| draft_modifier.set(v))
                        error=modifier_error
                        mode=NumberInputMode::SignedInteger
                    />
                    <NumberInput
                        id="new-cat-max-modifier"
                        label="Upper/Lower Bound".to_string()
                        placeholder="No Limit"
                        value=Signal::derive(move || draft_bound.get())
                        set_value=Callback::new(move |v| draft_bound.set(v))
                        error=bound_error
                        mode=NumberInputMode::SignedInteger
                    />
                </div>

                {render_draft_fields(DraftFieldsParams {
                    rule_type: Signal::derive(move || draft_rule_type.get()),
                    draft: draft_rule,
                    draft_patterns,
                    draft_unexpected_mode,
                    rule_condition_options,
                    threshold_condition_options,
                    track_type_options,
                    unexpected_mode_options,
                }).into_any()}

                <button
                    class=move || {
                        let has_field_errors = !field_errors.get().is_empty();
                        let valid = !is_name_empty() && !is_name_duplicate()
                            && draft_modifier.get().parse::<i32>().is_ok()
                            && (draft_bound.get().trim().is_empty() || draft_bound.get().parse::<i32>().is_ok())
                            && !has_field_errors;
                        if valid { "btn btn-primary w-full mt-md" } else { "btn w-full mt-md" }
                    }
                    disabled=move || {
                        let has_field_errors = !field_errors.get().is_empty();
                        is_name_empty() || is_name_duplicate()
                            || draft_modifier.get().parse::<i32>().is_err()
                            || (!draft_bound.get().trim().is_empty() && draft_bound.get().parse::<i32>().is_err())
                            || has_field_errors
                    }
                    on:click=move |_| {
                        validation_triggered.set(true);
                        if !field_errors.get().is_empty() || is_name_empty() || is_name_duplicate()
                            || draft_modifier.get().parse::<i32>().is_err()
                            || (!draft_bound.get().trim().is_empty() && draft_bound.get().parse::<i32>().is_err())
                        {
                            toast::show_error("Please fix the errors before adding a category");
                            return;
                        }

                        if let Some(mut c) = cfg.get() {
                            let name = draft_name.get();
                            if name.is_empty() { return; }

                            let mut rule = draft_rule.get();

                            // Fill UnexpectedFiles-specific fields (mode + patterns)
                            if let config::AutomaticProfileRule::UnexpectedFiles { file_patterns, mode } = &mut rule {
                                let raw = draft_patterns.get();
                                *file_patterns = raw
                                    .split([',', ';'])
                                    .map(|s| s.trim().trim_start_matches('.').to_string())
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                *mode = match draft_unexpected_mode.get().as_str() {
                                    "whitelist" => config::UnexpectedFilesMode::Whitelist,
                                    _ => config::UnexpectedFilesMode::Blacklist,
                                };
                            }

                            c.general.automatic_profiles.categories.insert(name, config::AutomaticProfileCategory {
                                modifier: draft_modifier.get().parse().unwrap_or(0),
                                bound: draft_bound.get().parse().unwrap_or(0),
                                rule,
                            });

                            let c_for_cache = c.clone();
                            set_cfg.set(Some(c.clone()));
                            spawn_api_toast(crate::api::save_config(c), None, move |_| {
                                write_cache("fetch_config", &c_for_cache);
                            });

                            draft_name.set(String::new());
                            draft_modifier.set(String::new());
                            draft_bound.set(String::new());
                            draft_rule.set(dummy_rule_for_type("unexpected_files").unwrap());
                            draft_patterns.set(String::new());
                            draft_unexpected_mode.set("blacklist".to_string());
                            validation_triggered.set(false);
                        }
                    }
                >"Add Category"</button>
            </div>
        </StandardModal>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::config::{
        AutomaticProfileRule, RuleCondition, ThresholdCondition, TrackType, UnexpectedFilesMode,
    };

    #[test]
    fn describe_rule_fields_covers_all_variants() {
        let variants: Vec<AutomaticProfileRule> = vec![
            AutomaticProfileRule::Chapters {
                condition: RuleCondition::MustBePresent,
            },
            AutomaticProfileRule::TrackCount {
                count: 0,
                track_type: TrackType::Any,
                condition: ThresholdCondition::Minimum,
            },
            AutomaticProfileRule::Language {
                language: String::new(),
                track_type: TrackType::Any,
                condition: RuleCondition::MustBePresent,
            },
            AutomaticProfileRule::Resolution {
                width: 0,
                height: 0,
                condition: ThresholdCondition::Minimum,
            },
            AutomaticProfileRule::Bitrate {
                kbps: 0,
                condition: ThresholdCondition::Minimum,
            },
            AutomaticProfileRule::Codec {
                codec: String::new(),
                condition: RuleCondition::MustBePresent,
            },
            AutomaticProfileRule::AudioChannels {
                channels: 0,
                condition: ThresholdCondition::Minimum,
            },
            AutomaticProfileRule::UnexpectedFiles {
                file_patterns: vec![],
                mode: UnexpectedFilesMode::Blacklist,
            },
        ];
        for v in &variants {
            assert!(
                !describe_rule_fields(v).is_empty(),
                "empty fields for {:?}",
                v
            );
        }
    }

    #[test]
    fn read_write_roundtrip_for_all_field_ids() {
        let variants: Vec<AutomaticProfileRule> = vec![
            AutomaticProfileRule::TrackCount {
                count: 42,
                track_type: TrackType::Subtitle,
                condition: ThresholdCondition::Minimum,
            },
            AutomaticProfileRule::Language {
                language: "spa".to_string(),
                track_type: TrackType::Audio,
                condition: RuleCondition::MustBePresent,
            },
            AutomaticProfileRule::Resolution {
                width: 1920,
                height: 1080,
                condition: ThresholdCondition::Minimum,
            },
            AutomaticProfileRule::Bitrate {
                kbps: 5000,
                condition: ThresholdCondition::Minimum,
            },
            AutomaticProfileRule::Codec {
                codec: "x265".to_string(),
                condition: RuleCondition::MustBePresent,
            },
            AutomaticProfileRule::AudioChannels {
                channels: 6,
                condition: ThresholdCondition::Minimum,
            },
            AutomaticProfileRule::UnexpectedFiles {
                file_patterns: vec!["srt".to_string()],
                mode: UnexpectedFilesMode::Blacklist,
            },
        ];
        let mut tested: Vec<&str> = Vec::new();
        for variant in &variants {
            for field in describe_rule_fields(variant) {
                if let RuleField::Number(id, _) | RuleField::Text(id, _) = field
                    && !tested.contains(&id)
                {
                    let val = read_rule_field(variant, id);
                    let mut clone = variant.clone();
                    assert!(
                        write_rule_field(&mut clone, id, &val),
                        "write failed for '{}'",
                        id
                    );
                    assert_eq!(
                        read_rule_field(&clone, id),
                        val,
                        "round-trip mismatch for '{}'",
                        id
                    );
                    tested.push(id);
                }
            }
        }
        for id in &[
            "count", "width", "height", "kbps", "channels", "codec", "language",
        ] {
            assert!(tested.contains(id), "field '{}' never tested", id);
        }
    }

    #[test]
    fn write_rule_field_updates_value() {
        let mut rule = AutomaticProfileRule::TrackCount {
            count: 0,
            track_type: TrackType::Any,
            condition: ThresholdCondition::Minimum,
        };
        assert!(write_rule_field(&mut rule, "count", "5"));
        assert_eq!(read_rule_field(&rule, "count"), "5");
        assert!(!write_rule_field(&mut rule, "count", "not_a_number"));
    }

    #[test]
    fn dummy_rule_consistent_with_fields() {
        for t in &[
            "chapters",
            "track_count",
            "language",
            "resolution",
            "bitrate",
            "codec",
            "audio_channels",
            "unexpected_files",
        ] {
            let dummy = dummy_rule_for_type(t).unwrap();
            assert_eq!(
                describe_rule_fields(&dummy),
                rule_type_to_fields(t),
                "mismatch for '{}'",
                t
            );
        }
        assert!(dummy_rule_for_type("nonexistent").is_none());
    }

    #[test]
    fn condition_helpers_roundtrip() {
        let variants: Vec<AutomaticProfileRule> = vec![
            AutomaticProfileRule::TrackCount {
                count: 0,
                track_type: TrackType::Any,
                condition: ThresholdCondition::Maximum,
            },
            AutomaticProfileRule::Resolution {
                width: 0,
                height: 0,
                condition: ThresholdCondition::Exact,
            },
            AutomaticProfileRule::Bitrate {
                kbps: 0,
                condition: ThresholdCondition::Minimum,
            },
            AutomaticProfileRule::AudioChannels {
                channels: 0,
                condition: ThresholdCondition::Maximum,
            },
            AutomaticProfileRule::Chapters {
                condition: RuleCondition::MustNotBePresent,
            },
            AutomaticProfileRule::Codec {
                codec: String::new(),
                condition: RuleCondition::MustBePresent,
            },
            AutomaticProfileRule::Language {
                language: String::new(),
                track_type: TrackType::Any,
                condition: RuleCondition::MustNotBePresent,
            },
        ];
        for v in &variants {
            let val = threshold_cond_value(v);
            let val2 = rule_cond_value(v);
            assert!(
                !val.is_empty() || !val2.is_empty(),
                "both condition values empty for {:?}",
                v
            );
        }
    }
}
