use crate::components::common::form_fields::{HelpContext, TooltipBuilder};
use leptos::prelude::*;
use std::time::Duration;

/// Alias and pattern config textareas used in both series-level (advanced tab)
/// and season-level (season manage modal) contexts.
///
/// Pass `context = "season"` to switch labels and hints to season-specific wording.
#[component]
pub fn AliasConfig(
    aliases: RwSignal<String>,
    patterns: RwSignal<String>,
    #[prop(into)] on_save: Callback<()>,
    /// "series" for series-level (default), "season" for season-level
    #[prop(optional)]
    context: &'static str,
) -> impl IntoView {
    let debounce_timer = StoredValue::new_local(None::<TimeoutHandle>);

    let trigger_save = move || on_save.run(());

    let debounced_save = move || {
        if let Some(handle) = *debounce_timer.read_value() {
            handle.clear();
        }
        if let Ok(handle) =
            set_timeout_with_handle(move || on_save.run(()), Duration::from_millis(500))
        {
            *debounce_timer.write_value() = Some(handle);
        }
    };

    let is_season = context == "season";

    let (
        section_heading,
        section_desc,
        aliases_label,
        aliases_id,
        aliases_suffix,
        patterns_label,
        patterns_id,
        patterns_suffix,
    ) = if is_season {
        (
            "Season-Specific Aliases & Patterns",
            "Applies to this season only. Overrides the series-level aliases and \
             patterns when set — leave empty to inherit them.",
            "Season-Specific Aliases",
            "seasonAliases",
            "",
            "Season-Specific Regular Expression Patterns",
            "seasonRegexPatterns",
            "",
        )
    } else {
        (
            "Series-Level Aliases & Patterns",
            "Applies to all seasons. Season-level aliases and patterns override these \
             when set.",
            "Series Aliases",
            "seriesAliases",
            "",
            "Series Regular Expression Title Patterns",
            "seriesRegexPatterns",
            "",
        )
    };

    // Hints describe behavior/purpose; syntax details and how-to guidance live
    // behind the ? help icons.
    //
    // Aliases set what is searched for and are also matched against releases
    // found automatically (polling, auto-search) — but only when no patterns are
    // set. Patterns are only checked against releases found automatically,
    // never what is searched.
    let aliases_hint = if is_season {
        "Alternate titles used when searching for this season. While set, they \
         replace the series-level aliases for this season's searches; leave empty \
         to inherit them."
    } else {
        "Alternate titles releases of this series may use. Sources are searched \
         using these titles; when no patterns are set, releases found automatically \
         (polling, auto-search) are also matched against them. Leave empty to use the \
         series title."
    };

    let patterns_hint = build_patterns_hint(is_season);

    view! {
        <div class="alias-config-container">
            <div class="season-override-section mb-xl">
                <div class="season-override-header">
                    <div>
                        <div class="font-semibold mb-xs">{section_heading}</div>
                        {if !section_desc.is_empty() {
                            view! { <div class="text-xs text-muted-color">{section_desc}</div> }.into_any()
                        } else {
                            ().into_any()
                        }}
                    </div>
                </div>

                {render_alias_field(AliasFieldParams {
                    id: aliases_id,
                    label: aliases_label,
                    default_suffix: aliases_suffix,
                    tooltip_ctx: HelpContext::SourceAlias,
                    value: aliases,
                    placeholder: "Enter aliases, one per line...",
                    hint: aliases_hint.to_string(),
                    on_input: move |val: String| {
                        let current = aliases.get();
                        if val != current {
                            for (i, alias) in val.split('\n').enumerate() {
                                let trimmed = alias.trim();
                                if !trimmed.is_empty()
                                    && let Err(e) = crate::validation::validate_alias(trimmed) {
                                        crate::components::common::toast::show_toast(
                                            format!("Alias at line {}: {}", i + 1, e),
                                            crate::components::common::toast::NotificationType::Error,
                                        );
                                        return;
                                    }
                            }
                            aliases.set(val);
                            debounced_save();
                        }
                    },
                    on_blur: move || trigger_save(),
                    container_class: "form-group",
                })}

                {render_alias_field(AliasFieldParams {
                    id: patterns_id,
                    label: patterns_label,
                    default_suffix: patterns_suffix,
                    tooltip_ctx: HelpContext::SourcePattern,
                    value: patterns,
                    placeholder: "Enter regex patterns, one per line...",
                    hint: patterns_hint.to_string(),
                    on_input: move |val: String| {
                        let current = patterns.get();
                        if val != current {
                            patterns.set(val);
                            debounced_save();
                        }
                    },
                    on_blur: move || trigger_save(),
                    container_class: "form-group mt-lg",
                })}
            </div>
        </div>
    }
}

struct AliasFieldParams<F1: Fn(String) + 'static + Copy, F2: Fn() + 'static + Copy> {
    id: &'static str,
    label: &'static str,
    default_suffix: &'static str,
    tooltip_ctx: HelpContext,
    value: RwSignal<String>,
    placeholder: &'static str,
    hint: String,
    on_input: F1,
    on_blur: F2,
    container_class: &'static str,
}

/// Renders a single alias/pattern textarea with label, tooltip, and hint.
fn render_alias_field<F1, F2>(params: AliasFieldParams<F1, F2>) -> impl IntoView
where
    F1: Fn(String) + 'static + Copy,
    F2: Fn() + 'static + Copy,
{
    view! {
        <div class=params.container_class>
            <div class="flex">
                <label class="form-label mr-xs" for=params.id>{params.label}</label>
                {if !params.default_suffix.is_empty() {
                    view! { <span class="text-muted-color font-normal">{params.default_suffix}</span> }.into_any()
                } else {
                    ().into_any()
                }}
                {TooltipBuilder::new().with_help_context(params.tooltip_ctx).build()}
            </div>
            <textarea
                rows="4"
                id=params.id
                placeholder=params.placeholder
                on:input=move |ev| (params.on_input)(event_target_value(&ev))
                on:blur=move |_| (params.on_blur)()
                prop:value=move || params.value.get()
            ></textarea>
            <div class="form-hint">{params.hint}</div>
        </div>
    }
}

fn build_patterns_hint(is_season: bool) -> String {
    // Patterns only gate releases found automatically (polling, auto-search) —
    // they never change what is searched for and don't affect manual searches.
    if is_season {
        "Optional rules for releases of this season found automatically (polling, \
         auto-search). While set, they replace the series-level patterns and a \
         release must match one to be accepted; named groups can also extract the \
         episode/season. Manual searches are unaffected. Leave empty to inherit the \
         series patterns."
            .to_string()
    } else {
        "Optional rules for releases of this series found automatically (polling, \
         auto-search). Once set, a release must match at least one pattern to be \
         accepted, and named groups can also extract the episode/season. Manual \
         searches are unaffected. If left empty, releases are matched by the series \
         title and aliases."
            .to_string()
    }
}
