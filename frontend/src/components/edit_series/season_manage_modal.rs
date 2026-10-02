use super::edit_series::SeasonMonitorInfo;
use crate::components::common::form_fields::{
    ActionRow, NumberInput, NumberInputMode, OverrideFormatField, TooltipBuilder, field_validation,
};
use crate::components::common::icons::TrashIcon;
use crate::components::common::modal_wrapper::ModalWrapper;
use crate::components::common::toast::show_success;
use jumbie_shared::types::SeasonOverride;
use jumbie_shared::variables::TemplateContext;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn SeasonManageModal(
    season: RwSignal<Vec<SeasonOverride>>,
    season_absolute: RwSignal<Vec<SeasonOverride>>,
    managing_season: RwSignal<Option<String>>,
    #[prop(into)] series_id: Signal<String>,
    #[prop(into)] on_save: Callback<()>,
    #[prop(into)] on_refresh: Callback<()>,
    absolute_numbering: ReadSignal<bool>,
    #[prop(into)] season_monitor_info: Signal<std::collections::HashMap<String, SeasonMonitorInfo>>,
    /// Whether the provider's episode cache holds this season — the Restore
    /// button is only offered when a cache-only restore is possible.
    #[prop(into)]
    has_cached_metadata: Signal<bool>,
    /// Series-level search templates and their global fallbacks, used to show the
    /// inherited value when the season has no override of its own.
    #[prop(into)]
    series_search_format: Signal<Option<String>>,
    #[prop(into)] series_search_format_absolute: Signal<Option<String>>,
    #[prop(into)] global_search_format: Signal<String>,
    #[prop(into)] global_search_format_absolute: Signal<String>,
) -> impl IntoView {
    let current_season = move || managing_season.get().unwrap_or_default();
    let _active_plugin_name = expect_context::<Signal<String>>();

    let override_data = move || {
        let s = current_season();
        let source = if absolute_numbering.get() {
            season_absolute
        } else {
            season
        };
        source.with(|overrides| overrides.iter().find(|o| o.season == s).cloned())
    };

    // Raw string signals for AliasConfig (SSoT — the textarea is the truth).
    // Split into Vec only at the save boundary.
    let season_aliases_rw = RwSignal::new(String::new());
    let season_patterns_rw = RwSignal::new(String::new());

    let (episode_offset, set_episode_offset) = signal(String::new());
    let (ep_start, set_ep_start) = signal(String::new());
    let (ep_end, set_ep_end) = signal(String::new());

    // Episode Range inline validation (SSoT)
    // Single derived signal computes all validation in one place.
    // The parse+validate logic and feedback-derivation rules are defined once.
    #[derive(Clone)]
    struct EpRangeFeedback {
        start: Option<String>,
        end: Option<String>,
    }
    let ep_range_feedback = Signal::derive(move || {
        let start = ep_start.get();
        let end = ep_end.get();

        // Per-field validation: same logic, applied to each value (not duplicated).
        let validate_field = |v: &str| -> Option<String> {
            if v.is_empty() {
                return None;
            }
            match v.parse::<i32>() {
                Ok(n) => crate::validation::validate_episode_number(n).err(),
                Err(_) => Some("Must be a valid positive number".to_string()),
            }
        };
        let start_error = validate_field(&start);
        let end_error = validate_field(&end);

        // Range relationship: only matters when both fields are individually valid.
        let range_error =
            if start_error.is_none() && end_error.is_none() && !start.is_empty() && !end.is_empty()
            {
                // Safety: both have already passed parse+validate above.
                let s = start.parse::<i32>().unwrap();
                let e = end.parse::<i32>().unwrap();
                if e <= s {
                    Some("End must be greater than Start".to_string())
                } else {
                    None
                }
            } else {
                None
            };

        // Per-field feedback: field-level error first, then range error.
        EpRangeFeedback {
            start: start_error.or_else(|| range_error.clone()),
            end: end_error.or_else(|| range_error.clone()),
        }
    });
    // Thin accessors so the template only subscribes to what it uses.
    let ep_start_feedback = Signal::derive(move || ep_range_feedback.get().start);
    let ep_end_feedback = Signal::derive(move || ep_range_feedback.get().end);
    let (alias_season_number, set_alias_season_number) = signal(String::new());
    let (search_format, set_search_format) = signal(None::<String>);
    let (episode_count, set_episode_count) = signal(String::new());

    // SSoT: reduce the stored override for the current season/mode — or a blank
    // default when there is none — to the form signals in a SINGLE pass. One code
    // path means a field can't be seeded in one branch and forgotten in another,
    // which is exactly how `cell_count` drifted from the normal to absolute view.
    Effect::new(move |_| {
        let o = override_data().unwrap_or_default();

        set_episode_offset.set(o.episode_offset.map(|v| v.to_string()).unwrap_or_default());
        set_ep_start.set(o.episode_start.map(|v| v.to_string()).unwrap_or_default());
        set_ep_end.set(o.episode_end.map(|v| v.to_string()).unwrap_or_default());
        set_episode_count.set(o.cell_count.map(|v| v.to_string()).unwrap_or_default());
        set_alias_season_number.set(
            o.alias_season_number
                .map(|v| v.to_string())
                .unwrap_or_default(),
        );
        set_search_format.set(o.search_format);
        season_aliases_rw.set(o.aliases.join("\n"));
        season_patterns_rw.set(o.reg_patterns.join("\n"));
    });

    let count_placeholder =
        Signal::derive(move || "Leave empty to show database count".to_string());

    let delete_season = move || {
        let s = current_season();
        let sid = series_id.get_untracked();
        let is_absolute = absolute_numbering.get_untracked();
        let target = if is_absolute { season_absolute } else { season };
        if is_absolute {
            // Absolute numbering has one season, so this is a reset: keep the
            // season key with a blank override instead of removing it.
            target.update(|overrides| {
                if let Some(o) = overrides.iter_mut().find(|o| o.season == s) {
                    *o = SeasonOverride {
                        season: o.season.clone(),
                        ..SeasonOverride::default()
                    };
                } else {
                    overrides.push(SeasonOverride {
                        season: s.clone(),
                        ..SeasonOverride::default()
                    });
                }
            });
        } else {
            target.update(|overrides| overrides.retain(|o| o.season != s));
        }
        on_save.run(());
        managing_season.set(None);
        // Also delete downloaded episode records from DB via the API
        let refresh = on_refresh.clone();
        let message = if is_absolute {
            "Season reset."
        } else {
            "Season deleted."
        };
        crate::utils::spawn_api_toast(crate::api::delete_season(sid, s), None, move |_| {
            show_success(message);
            refresh.run(());
        });
    };

    let save_season = move || {
        let s = current_season();
        let offset = episode_offset.get();
        let start = ep_start.get();
        let end = ep_end.get();
        let count_str = episode_count.get();
        let aliases_str = season_aliases_rw.get_untracked();
        let patterns_str = season_patterns_rw.get_untracked();

        let mut offset_val = None;
        if !offset.is_empty() {
            match offset.parse::<i32>() {
                Ok(val) => {
                    if let Err(e) = crate::validation::validate_episode_offset(val) {
                        crate::components::common::toast::show_error(e);
                        return;
                    }
                    offset_val = Some(val);
                }
                Err(_) => {
                    crate::components::common::toast::show_error(
                        "Episode offset must be a valid number",
                    );
                    return;
                }
            }
        }

        let mut start_val = None;
        if !start.is_empty() {
            match start.parse::<i32>() {
                Ok(val) => {
                    if let Err(e) = crate::validation::validate_episode_number(val) {
                        crate::components::common::toast::show_error(e);
                        return;
                    }
                    start_val = Some(val);
                }
                Err(_) => {
                    crate::components::common::toast::show_error(
                        "Episode start must be a valid positive number",
                    );
                    return;
                }
            }
        }

        let mut end_val = None;
        if !end.is_empty() {
            match end.parse::<i32>() {
                Ok(val) => {
                    if let Err(e) = crate::validation::validate_episode_number(val) {
                        crate::components::common::toast::show_error(e);
                        return;
                    }
                    end_val = Some(val);
                }
                Err(_) => {
                    crate::components::common::toast::show_error(
                        "Episode end must be a valid positive number",
                    );
                    return;
                }
            }
        }

        if let (Some(s), Some(e)) = (start_val, end_val)
            && e <= s
        {
            crate::components::common::toast::show_error(
                "Episode end must be greater than episode start",
            );
            return;
        }

        let mut count_val = None;
        if !count_str.is_empty() {
            match count_str.parse::<i32>() {
                Ok(val) => {
                    if let Err(e) = crate::validation::validate_episode_number(val) {
                        crate::components::common::toast::show_error(e);
                        return;
                    }
                    count_val = Some(val);
                }
                Err(_) => {
                    crate::components::common::toast::show_error(
                        "Episode count must be a valid positive number",
                    );
                    return;
                }
            }
        }

        let mut alias_season_val = None;
        if !alias_season_number.get().is_empty() {
            match crate::validation::validate_season_number(&alias_season_number.get()) {
                Ok(val) => alias_season_val = Some(val as u32),
                Err(e) => {
                    crate::components::common::toast::show_error(e);
                    return;
                }
            }
        }

        let search_format_val = search_format.get();
        if let Some(fmt) = &search_format_val
            && crate::validation::validate_search_format_field(fmt, true, "search format")
        {
            return;
        }

        // Validate each alias (non-empty, no control chars, max 255)
        for (i, alias) in aliases_str.split('\n').enumerate() {
            let trimmed = alias.trim();
            if !trimmed.is_empty()
                && let Err(e) = crate::validation::validate_alias(trimmed)
            {
                crate::components::common::toast::show_error(format!(
                    "Alias at line {}: {}",
                    i + 1,
                    e
                ));
                return;
            }
        }

        let aliases: Vec<String> = aliases_str
            .split('\n')
            .map(|s| s.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect();
        let reg_patterns: Vec<String> = patterns_str
            .split('\n')
            .map(|s| s.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();

        let target = if absolute_numbering.get_untracked() {
            season_absolute
        } else {
            season
        };
        target.update(|overrides| {
            if let Some(o) = overrides.iter_mut().find(|o| o.season == s) {
                o.episode_offset = offset_val;
                o.episode_start = start_val;
                o.episode_end = end_val;
                o.cell_count = count_val;
                o.alias_season_number = alias_season_val;
                o.search_format = search_format_val.clone();
                o.aliases = aliases;
                o.reg_patterns = reg_patterns;
            } else {
                overrides.push(SeasonOverride {
                    season: s.clone(),
                    aliases,
                    reg_patterns,
                    episode_start: start_val,
                    episode_end: end_val,
                    cell_count: count_val,
                    episode_offset: offset_val,
                    alias_season_number: alias_season_val,
                    search_format: search_format_val,
                });
            }
        });
        on_save.run(());
        show_success("Season configuration updated.");
        managing_season.set(None);
    };

    view! {
        <ModalWrapper
            show=move || managing_season.get().is_some()
            on_close=move |_| managing_season.set(None)
            title=Signal::derive(move || format!("Manage Season {}", current_season()))
            footer={
            view! {
                <div class="modal-footer flex items-center justify-between gap-md">
                    <div>
                            <button
                                class="btn btn-danger"
                                title=move || if absolute_numbering.get() {
                                    "Clears the season's episode data. Absolute numbering always has one season, so the season is kept."
                                } else {
                                    "Deletes the season and its episode data"
                                }
                                on:click=move |_| delete_season()
                            >
                                <span class="icon"><TrashIcon /></span>
                                {move || if absolute_numbering.get() { "Reset Season" } else { "Season" }}
                            </button>
                    </div>
                    <div class="flex items-center gap-md">
                        <button class="btn btn-primary" on:click=move |_| save_season()>"Save"</button>
                    </div>
                </div>
            }.into_any()
        }
        >
            <div class="form-group mb-lg">
                <crate::components::edit_series::alias_config::AliasConfig
                    aliases={season_aliases_rw}
                    patterns={season_patterns_rw}
                    on_save={Callback::new(move |_| {})}
                    context="season"
                />
            </div>
                <a class="form-label form-section-header">"Episode Mapping"</a>
                <div class="mt-md">
                    <div class="form-group mb-sm">
                    <OverrideFormatField
                        id="seasonSearchFormat".to_string()
                        label="S/E Search Format".to_string()
                        override_label="Override S/E Search Format".to_string()
                        help_text="Season-level search template. Unchecked inherits the series format; clear the field to search by title alone for this season.".to_string()
                        field_help_text=crate::validation::SEARCH_FORMAT_FIELD_HELP.to_string()
                        value=Signal::derive(move || search_format.get())
                        set_value=Callback::new(move |v| set_search_format.set(v))
                        inherited=Signal::derive(move || {
                            let absolute = absolute_numbering.get();
                            let series = if absolute {
                                series_search_format_absolute.get()
                            } else {
                                series_search_format.get()
                            };
                            series.unwrap_or_else(|| {
                                if absolute {
                                    global_search_format_absolute.get()
                                } else {
                                    global_search_format.get()
                                }
                            })
                        })
                        tooltip=TooltipBuilder::new().with_context(TemplateContext::SearchFormat).with_formatting()
                    />
                    </div>

                    <NumberInput
                        label="Season Number Alias".to_string()
                        id="aliasSeasonNumber".to_string()
                        mode=NumberInputMode::Integer
                        placeholder="Alias Season".to_string()
                        value=Signal::derive(move || alias_season_number.get().to_string())
                        set_value=Callback::new(move |v| set_alias_season_number.set(v))
                        help_text="Allows searching and downloading an alias season for this particular season view (e.g. Setting alias to 4 for Season 6 will look for Season 4 releases)".to_string()
                    />

                    <NumberInput
                        label="Episode Offset".to_string()
                        id="episodeOffset".to_string()
                        mode=NumberInputMode::SignedInteger
                        placeholder="Offset Amount".to_string()
                        value=Signal::derive(move || episode_offset.get().to_string())
                        set_value=Callback::new(move |v| set_episode_offset.set(v))
                        help_text="Offset added to episode numbers (e.g., +5 makes ep1 become ep6)".to_string()
                    />

                    <NumberInput
                        label="Cell Count".to_string()
                        id="episodeCount".to_string()
                        mode=NumberInputMode::Integer
                        placeholder=Signal::derive(move || count_placeholder.get().to_string())
                        value=Signal::derive(move || episode_count.get().to_string())
                        set_value=Callback::new(move |v| set_episode_count.set(v))
                        help_text="Total number of cells to render in the UI for this season".to_string()
                    />

                    <div class="form-group">
                        <div class="form-label mb-sm">"Episode Range"</div>
                        <div class="flex gap-sm items-center">
                            <input
                                type="number"
                                id="episodeStart"
                                aria-label="Episode Start"
                                placeholder="Start"
                                class="flex-1"
                                prop:value=ep_start
                                on:input=move |ev| set_ep_start.set(event_target_value(&ev))
                            />
                            <span>"to"</span>
                            <input
                                type="number"
                                id="episodeEnd"
                                aria-label="Episode End"
                                placeholder="End"
                                class="flex-1"
                                prop:value=ep_end
                                on:input=move |ev| set_ep_end.set(event_target_value(&ev))
                            />
                        </div>
                        {field_validation(Signal::derive(move || ep_start_feedback.get().unwrap_or_default()))}
                                                {field_validation(Signal::derive(move || ep_end_feedback.get().unwrap_or_default()))}
                        <div class="form-hint">"Limit processing to specific episode range"</div>
                    </div>
                </div>



            <div class="danger-zone">
                <div class="danger-zone-title">"Danger Zone"</div>
                <ActionRow description=Signal::derive(move ||
                    "Set monitored flag for every episode in this season to the same state.".to_string()
                )>
                    {
                        let season_info = move || {
                            let s = current_season();
                            season_monitor_info.with(|map| map.get(&s).cloned())
                        };
                        view! {
                            <button
                                class="btn btn-primary"
                                on:click={
                                    let on_refresh = on_refresh.clone();
                                    move |_| {
                                        let s = current_season();
                                        let info = season_monitor_info.with(|map| map.get(&s).cloned());
                                        let on_refresh = on_refresh.clone();
                                        if let Some(info) = info {
                                            let target = !info.all_monitored;
                                            spawn_local(async move {
                                                crate::utils::spawn_api_toast(
                                                    crate::api::batch_monitor_episodes(info.episode_ids, target),
                                                    None,
                                                    move |_| {
                                                        crate::components::common::toast::show_success(
                                                            if target { "All episodes monitored" } else { "All episodes unmonitored" }
                                                        );
                                                        on_refresh.run(());
                                                    },
                                                );
                                            });
                                        }
                                    }
                                }
                            >
                                {move || {
                                    if season_info().map(|info| info.all_monitored).unwrap_or(false) { "Unmonitor All" } else { "Monitor All" }
                                }}
                            </button>
                        }.into_any()
                    }
                </ActionRow>
                <Show when=move || has_cached_metadata.get()>
                <ActionRow description=Signal::derive(move ||
                    "Restore episode metadata from the provider's cached data (titles, descriptions, runtime, images) without re-fetching from the API.".to_string()
                )>
                    <button class="btn btn-primary" on:click=move |_| {
                            let sid = series_id.get_untracked();
                            let s = current_season();
                            let on_save = on_save.clone();
                            let on_refresh = on_refresh.clone();
                            spawn_local(async move {
                                crate::utils::spawn_api_toast(
                                    crate::api::restore_season_metadata(sid, s),
                                    None,
                                    move |_| {
                                        crate::components::common::toast::show_success("Season metadata restored");
                                        on_refresh.run(());
                                        on_save.run(());
                                    },
                                );
                            });
                        }>"Restore Metadata"</button>
                </ActionRow>
                </Show>
                <ActionRow description=Signal::derive(move ||
                    "Delete all episode data (metadata, media scans) for this season from the database. Episode files remain on disk.".to_string()
                )>
                    <button class="btn btn-danger" on:click=move |_| {
                            let sid = series_id.get_untracked();
                            let s = current_season();
                            let on_save = on_save.clone();
                            spawn_local(async move {
                                crate::utils::spawn_api_toast(
                                    crate::api::delete_season_episode_data(sid, s),
                                    None,
                                    move |_| {
                                        crate::components::common::toast::show_success("Season episode data deleted");
                                        on_save.run(());
                                    },
                                );
                            });
                        }>"Delete Episode Data"</button>
                </ActionRow>

                <ActionRow description=Signal::derive(move ||
                    "Reset all season-specific settings (aliases, offsets, episode range) to defaults. The season accordion is kept. Episode data is preserved.".to_string()
                )>
                    <button class="btn btn-danger" on:click=move |_| {
                            let s = current_season();
                            // Reset all season-specific fields to defaults, but keep
                            // the season override entry so the accordion remains.
                            let target = if absolute_numbering.get_untracked() { season_absolute } else { season };
                            target.update(|overrides| {
                                if let Some(o) = overrides.iter_mut().find(|o| o.season == s) {
                                    // SSoT: replace with a blank override (keeping the
                                    // season key) so newly added fields reset too, rather
                                    // than hand-listing each field here.
                                    *o = SeasonOverride {
                                        season: o.season.clone(),
                                        ..SeasonOverride::default()
                                    };
                                }
                            });
                            // Save the cleared override — modal stays open so the user
                            // can see the fields reset and continue editing if desired.
                            on_save.run(());
                            crate::components::common::toast::show_success("Season configuration reset");
                        }>"Reset Configuration"</button>
                </ActionRow>
            </div>

        </ModalWrapper>
    }
}
