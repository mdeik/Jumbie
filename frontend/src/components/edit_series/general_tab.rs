use crate::components::common::form_fields::{ActionField, FormGroup, Select, TextInput};
use crate::components::common::icons::*;
use crate::components::common::toast::NotificationType;
use crate::components::edit_series::edit_series_path_modal::EDIT_SERIES_PATH_TITLE;
use crate::components::edit_series::{EditSeriesCtx, parse_signal_lines};
use crate::hooks::use_ui_config::use_time_format;
use crate::utils::format_datetime_local;

use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

#[component]
pub fn GeneralTab() -> impl IntoView {
    let ctx = expect_context::<EditSeriesCtx>();

    // User's configured 12h/24h preference, used for all timestamp display.
    let time_format = use_time_format();

    // Per-plugin loading state: keyed by plugin display_name
    let (fetching_map, set_fetching_map) = signal(HashMap::<String, bool>::new());

    // Providers that expose at least one opt-in fetch action.
    //
    // SSoT: the provider TYPE decides availability (`FetchSeriesTitle` /
    // `FetchSeriesAliases` are type-level — there is no per-instance toggle for
    // them). The instance id is used only to target the call, because the fetch
    // depends on that instance's config (language, credentials).
    let fetch_providers = Signal::derive(move || {
        ctx.active_metadata_plugins
            .get()
            .into_iter()
            .filter(|p| {
                p.capabilities
                    .contains(&jumbie_shared::plugin::Capability::FetchSeriesTitle)
                    || p.capabilities
                        .contains(&jumbie_shared::plugin::Capability::FetchSeriesAliases)
            })
            .collect::<Vec<_>>()
    });

    // Pulsate the match button when a series ID is present but no sync has been performed yet.
    // Per-provider: pulse if ANY provider field has an ID but no corresponding sync timestamp.
    let should_pulse = Signal::derive(move || {
        if ctx.series_id.get().is_empty() {
            return false;
        }
        ctx.form.with(|s| {
            s.metadata_ids
                .keys()
                .any(|k| !s.metadata_last_synced_at.contains_key(k))
        })
    });

    // Monitor mode string derived from FormState for the Select widget
    let monitor_mode_str: Signal<String> = Signal::derive(move || {
        ctx.form
            .with(|s| s.selected_monitor_mode.as_str().to_string())
    });

    view! {
        <div class="edit-content active" id="tab-general">
            <div class="form-row">
                <FormGroup label="Series Title".to_string() label_for="seriesTitle".to_string()>
                    <TextInput
                        id="seriesTitle".to_string()
                        value=Signal::derive(move || ctx.form.with(|s| s.title.clone()))
                        set_value=move |v| ctx.form.update(|s| s.title = v)
                        on_blur=move |_| {
                                                    let val = ctx.form.with_untracked(|s| s.title.clone());
                                                    if let Err(e) = crate::validation::validate_title(&val) {
                                                        crate::components::common::toast::show_error(e);
                                                        return;
                                                    }
                                                    ctx.save_series.run(None);
                                                }
                    />
                </FormGroup>
                <ActionField label="Path" help_text="">
                    <TextInput
                        id="seriesPath".to_string()
                        value=ctx.root_path_sig
                        set_value=move |_| {}
                        readonly=true
                        class="readonly-input flex-1".to_string()
                    />
                    <button
                        class="btn btn-primary flex-none"
                        on:click=move |_| ctx.set_show_path_modal.set(true)
                        title=EDIT_SERIES_PATH_TITLE
                    >
                        <span class="icon"><FolderIcon/></span>
                    </button>
                </ActionField>
            </div>

            <div class="form-row">
                <FormGroup label="Quality Profile".to_string() label_for="qualityProfile".to_string()>
                    <Select
                        id="qualityProfile".to_string()
                        value=Signal::derive(move || ctx.form.with(|s| s.quality_profile.clone()))
                        set_value=move |v| {
                            ctx.form.update(|s| s.quality_profile = v);
                            ctx.save_series.run(None);
                        }
                        options=ctx.quality_options
                    />
                </FormGroup>

                <FormGroup label="Release Profile".to_string() label_for="releaseProfile".to_string()>
                    <Select
                        id="releaseProfile".to_string()
                        value=Signal::derive(move || ctx.form.with(|s| s.release_profile.clone()))
                        set_value=move |v| {
                            ctx.form.update(|s| s.release_profile = v);
                            ctx.save_series.run(None);
                        }
                        options=ctx.release_options
                    />
                </FormGroup>
                <FormGroup
                    label="Monitor Mode".to_string()
                    label_for="monitorMode".to_string()
                    help_signal=Signal::derive(move || ctx.form.with(|s| s.selected_monitor_mode.help_text().to_string()))
                >
                    <Select
                        id="monitorMode".to_string()
                        value=monitor_mode_str
                        set_value=move |v| ctx.handle_monitor_change.run(v)
                        options=ctx.monitor_options
                    />
                </FormGroup>
            </div>

            <div class="form-row">
                <For
                    each=move || ctx.active_metadata_plugins.get()
                    key=|p| p.instance_id.clone().unwrap_or_default()
                    children=move |plugin| {
                        let plugin_name = plugin.display_name.clone();
                        let plugin_label = plugin.series_identifier_label.clone().unwrap_or(format!("{} ID", plugin.display_name));
                        let field_id = format!("metadata_{}", plugin_name);

                        // Mode-aware visibility
                        let supports_normal = plugin.capabilities.contains(&jumbie_shared::plugin::Capability::MetadataProviderNormal);
                        let supports_absolute = plugin.capabilities.contains(&jumbie_shared::plugin::Capability::MetadataProviderAbsolute);
                        let is_absolute_mode = ctx.absolute_numbering.get();
                        let show_field = if is_absolute_mode { supports_absolute } else { supports_normal };

                        // Use instance_id as the metadata_ids key
                        let key = plugin.instance_id.as_deref()
                            .unwrap_or(&plugin_name)
                            .to_string();
                        let plugin_key = StoredValue::new_local(key.clone());
                        let vk = key.clone();
                        let value_sig = Signal::derive(move || {
                            ctx.form.with(|s| s.metadata_ids.get(&vk).cloned().unwrap_or_default())
                        });

                        // Derive this plugin's fetching state from the shared map
                        let is_fetching = Signal::derive({
                            let pn = key.clone();
                            move || fetching_map.get().get(&pn).copied().unwrap_or(false)
                        });

                        let help_text_sig = Signal::derive({
                            let pn = key.clone();
                            move || {
                                ctx.form.with(|s| s.metadata_last_synced_at.get(&pn).cloned()).map(|ts| {
                                    let tf = time_format.get();
                                    format!("Last synced: {}", format_datetime_local(&ts, &tf))
                                }).unwrap_or_default()
                            }
                        });

                        let set_fetching_map_clone = set_fetching_map.clone();

                        // Whether this plugin supports fetching in the current mode
                        // Disabled when: already fetching, capability mismatch, or field is empty
                        let fetch_disabled = Signal::derive(move || {
                            let abs = ctx.absolute_numbering.get();
                            let supports_now = if abs { supports_absolute } else { supports_normal };
                            is_fetching.get() || !supports_now || value_sig.get().trim().is_empty()
                        });
                        let fetch_title = move || {
                            let abs = ctx.absolute_numbering.get();
                            if abs && !supports_absolute {
                                "This metadata provider does not support absolute numbering mode"
                            } else if !abs && !supports_normal {
                                "This metadata provider does not support normal numbering mode"
                            } else {
                                "Fetch latest episode data from online metadata provider (stores new/updated episodes and titles)"
                            }
                        };

                        if !show_field {
                            return view! {}.into_any();
                        }

                        view! {
                            <ActionField label=plugin_label help_text=help_text_sig>
                                <TextInput
                                    id=field_id
                                    value=value_sig
                                    class="flex-1".to_string()
                                    placeholder=plugin.series_identifier_placeholder.clone().unwrap_or_default()
                                    set_value=move |v: String| {
                                        ctx.form.update(|s| {
                                            let k = plugin_key.read_value().clone();
                                            if v.trim().is_empty() {
                                                s.metadata_ids.remove(&k);
                                            } else {
                                                s.metadata_ids.insert(k, v);
                                            }
                                        });
                                    }
                                    on_blur=move |_| {
                                        ctx.save_series.run(None);
                                    }
                                />
                                <button
                                    class="btn btn-secondary flex gap-xs items-center whitespace-nowrap"
                                    class:disabled=move || fetch_disabled.get()
                                    class:pulsate=move || should_pulse.get()
                                    title=move || fetch_title()
                                    disabled=move || fetch_disabled.get()
                                    on:click=move |_| {
                                        let p_key = plugin_key.read_value().clone();
                                        let id = ctx.form.with(|m| m.metadata_ids.get(&p_key).cloned().unwrap_or_default());
                                        if id.is_empty() {
                                            crate::components::common::toast::show_toast("Please enter an ID first", NotificationType::Warning);
                                            return;
                                        }

                                        // Mark this plugin as fetching (disables button, shows spinner)
                                        let pn_fetch = p_key.clone();
                                        set_fetching_map_clone.update(|m| { m.insert(pn_fetch, true); });

                                        // Debounced save as safety net for other field changes
                                        // Suppress the default toast — the sync flow shows its own.
                                        ctx.save_series.run(Some(String::new()));

                                        spawn_local(async move {
                                            let s_id = ctx.series_id.get_untracked();

                                            // 1. Persist the form (including metadata_id) to DB
                                            //    and await completion before syncing, so the
                                            //    refetch after sync returns correct data.
                                            let form = ctx.form.get_untracked();
                                            let payload = jumbie_shared::types::UpdateSeriesPayload {
                                                quality_profile: form.quality_profile,
                                                release_profile: form.release_profile,
                                                title: Some(form.title),
                                                path_operation: None,
                                                settings: jumbie_shared::mapping::SeriesSettings {
                                                    aliases: parse_signal_lines(&ctx.series_aliases),
                                                    reg_patterns: parse_signal_lines(&ctx.series_reg_patterns),
                                                    season: ctx.season
                                                        .get_untracked()
                                                        .into_iter()
                                                        .map(|o| (o.season.clone(), o))
                                                        .collect(),
                                                    season_absolute: ctx.season_absolute
                                                        .get_untracked()
                                                        .into_iter()
                                                        .map(|o| (o.season.clone(), o))
                                                        .collect(),
                                                    path: Some(ctx.series_path.get_untracked()),
                                                    ..jumbie_shared::mapping::SeriesSettings::from_form(
                                                        jumbie_shared::mapping::SeriesSettingsForm {
                                                            season_folder_format: form.season_folder_format,
                                                            episode_file_format: form.episode_file_format,
                                                            season_folder_format_absolute: form.season_folder_format_absolute,
                                                            episode_file_format_absolute: form.episode_file_format_absolute,
                                                            flatten_season_folders: form.flatten_season_folders,
                                                            absolute_numbering: form.absolute_numbering,
                                                            rename_episodes: form.rename_episodes,
                                                            search_format: form.search_format,
                                                            search_format_absolute: form.search_format_absolute,
                                                            metadata_ids: form.metadata_ids,
                                                        },
                                                    )
                                                },
                                            };
                                            if let Err(e) = crate::api::update_series(s_id.clone(), payload).await {
                                                crate::components::common::toast::show_error(
                                                    format!("Failed to save series: {}", e.user_message()),
                                                );
                                                set_fetching_map_clone.update(|m| { m.insert(p_key, false); });
                                                return;
                                            }

                                            // 2. Sync episodes from the metadata provider
                                            match crate::api::sync_metadata(s_id, Some(id)).await {
                                                Ok(status) => {
                                                    ctx.series_details.refetch();
                                                    if let Some(synced_at) = status.synced_at {
                                                        ctx.form.update(|s| {
                                                            s.metadata_last_synced_at
                                                                .insert(p_key.clone(), synced_at);
                                                        });
                                                    }
                                                    crate::components::common::toast::show_success("Synchronized metadata episodes!");
                                                }
                                                Err(e) => crate::components::common::toast::show_error(e.to_string()),
                                            }

                                            let pn_done = plugin_key.read_value().clone();
                                            set_fetching_map_clone.update(|m| { m.insert(pn_done, false); });
                                        });
                                    }
                                >
                                    {move || if is_fetching.get() {
                                        view! {
                                            <span class="icon">
                                                <svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                                    <circle cx="12" cy="12" r="8" stroke-dasharray="40" stroke-linecap="round">
                                                        <animateTransform attributeName="transform" type="rotate" from="0 12 12" to="360 12 12" dur="0.8s" repeatCount="indefinite"/>
                                                    </circle>
                                                </svg>
                                            </span>
                                        }.into_any()
                                    } else {
                                        view! { <span class="icon"><WandSparklesIcon/></span> }.into_any()
                                    }}
                                </button>
                            </ActionField>
                        }.into_any()
                    }
                />
            </div>

            <Show when=move || !fetch_providers.get().is_empty()>
            <div class="metadata-actions-section">
                <div class="metadata-actions-header">
                    <div>
                        <div class="font-semibold mb-xs">"Online Metadata Actions"</div>
                        <div class="text-xs text-muted-color">
                            "Fetch the canonical title and alternative titles (aliases) from a metadata provider. Actions are targeted per provider — that provider's language and credentials are used."
                        </div>
                    </div>
                </div>
                <For
                    each=move || fetch_providers.get()
                    key=|p| p.instance_id.clone().unwrap_or_default()
                    children=move |plugin| {
                        let instance_id = plugin.instance_id.clone().unwrap_or_default();
                        let provider_label = plugin.display_name.clone();
                        let can_title = plugin
                            .capabilities
                            .contains(&jumbie_shared::plugin::Capability::FetchSeriesTitle);
                        let can_aliases = plugin
                            .capabilities
                            .contains(&jumbie_shared::plugin::Capability::FetchSeriesAliases);

                        let (fetching_title, set_fetching_title) = signal(false);
                        let (fetching_aliases, set_fetching_aliases) = signal(false);

                        // Non-`Copy` values that reactive closures need must live in
                        // `StoredValue` (Copy) handles — otherwise the `<For>` children
                        // closure becomes `FnOnce` and stops compiling.
                        let id_key_sv = StoredValue::new_local(instance_id.clone());
                        let label_sv = StoredValue::new_local(provider_label);
                        let title_pid_sv = StoredValue::new_local(instance_id.clone());
                        let aliases_pid_sv = StoredValue::new_local(instance_id);

                        let has_id = Signal::derive(move || {
                            let k = id_key_sv.read_value().clone();
                            ctx.form.with(|m| {
                                m.metadata_ids
                                    .get(&k)
                                    .map(|v| !v.trim().is_empty())
                                    .unwrap_or(false)
                            })
                        });

                        view! {
                            <div class="metadata-actions-provider">
                                <div class="text-xs font-semibold text-muted-color mb-xs">{label_sv.read_value().clone()}</div>
                                <div class="metadata-actions-buttons">
                                    <Show when=move || can_title>
                                        <button
                                            class="btn btn-secondary flex gap-xs items-center"
                                            class:disabled=move || fetching_title.get() || !has_id.get()
                                            title=move || if !has_id.get() {
                                                "Enter a metadata ID first".to_string()
                                            } else if fetching_title.get() {
                                                "Fetching title...".to_string()
                                            } else {
                                                format!("Fetch the canonical series title from {} and update it", label_sv.read_value().clone())
                                            }
                                            disabled=move || fetching_title.get() || !has_id.get()
                                            on:click=move |_| {
                                                let s_id = ctx.series_id.get_untracked();
                                                if s_id.is_empty() { return; }

                                                set_fetching_title.set(true);
                                                let pid = title_pid_sv.read_value().clone();

                                                spawn_local(async move {
                                                    match crate::api::fetch_series_info(s_id.clone(), true, false, Some(pid)).await {
                                                        Ok(resp) => {
                                                            if let Some(name) = resp.get("name").and_then(|n| n.as_str()) {
                                                                ctx.form.update(|s| s.title = name.to_string());
                                                            }
                                                            ctx.series_details.refetch();
                                                            crate::components::common::toast::show_toast(
                                                                "Series title updated from metadata provider",
                                                                crate::components::common::toast::NotificationType::Success,
                                                            );
                                                        }
                                                        Err(e) => {
                                                            crate::components::common::toast::show_error(
                                                                format!("Failed to fetch series info: {}", e.user_message()),
                                                            );
                                                        }
                                                    }
                                                    set_fetching_title.set(false);
                                                });
                                            }
                                        >
                                            {move || if fetching_title.get() {
                                                view! {
                                                    <span class="icon">
                                                        <svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                                            <circle cx="12" cy="12" r="8" stroke-dasharray="40" stroke-linecap="round">
                                                                <animateTransform attributeName="transform" type="rotate" from="0 12 12" to="360 12 12" dur="0.8s" repeatCount="indefinite"/>
                                                            </circle>
                                                        </svg>
                                                    </span>
                                                }.into_any()
                                            } else {
                                                view! { <span class="icon"><RefreshIcon/></span> }.into_any()
                                            }}
                                            "Fetch Title"
                                        </button>
                                    </Show>

                                    <Show when=move || can_aliases>
                                        <button
                                            class="btn btn-secondary flex gap-xs items-center"
                                            class:disabled=move || fetching_aliases.get() || !has_id.get()
                                            title=move || if !has_id.get() {
                                                "Enter a metadata ID first".to_string()
                                            } else if fetching_aliases.get() {
                                                "Fetching aliases...".to_string()
                                            } else {
                                                format!("Fetch the canonical title and alternative titles (aliases) from {} — existing aliases are preserved, the canonical title is appended, followed by new aliases", label_sv.read_value().clone())
                                            }
                                            disabled=move || fetching_aliases.get() || !has_id.get()
                                            on:click=move |_| {
                                                let s_id = ctx.series_id.get_untracked();
                                                if s_id.is_empty() { return; }

                                                set_fetching_aliases.set(true);
                                                let pid = aliases_pid_sv.read_value().clone();

                                                spawn_local(async move {
                                                    // 1. Fetch series aliases from the provider.
                                                    //    Backend returns { title, aliases, cached } where
                                                    //    `title` is the provider's canonical title and
                                                    //    `aliases` is the merged list (new + existing DB)
                                                    //    WITHOUT the title — no parsing needed.
                                                    match crate::api::fetch_series_aliases(s_id.clone(), Some(pid)).await {
                                                        Ok(resp) => {
                                                            let provider_title: String = resp
                                                                .get("title")
                                                                .and_then(|t| t.as_str())
                                                                .unwrap_or("")
                                                                .to_string();
                                                            let response_aliases: Vec<String> = resp
                                                                .get("aliases")
                                                                .and_then(|a| a.as_array())
                                                                .map(|arr| {
                                                                    arr.iter()
                                                                        .filter_map(|v| v.as_str().map(String::from))
                                                                        .collect()
                                                                })
                                                                .unwrap_or_default();

                                                            // 2. Read existing in-memory aliases (may have unsaved edits).
                                                            let existing_lowercase: std::collections::HashSet<String> =
                                                                ctx.series_aliases
                                                                    .get_untracked()
                                                                    .split('\n')
                                                                    .map(|s| s.trim().to_lowercase())
                                                                    .filter(|a| !a.is_empty())
                                                                    .collect();
                                                            let mut merged: Vec<String> = ctx
                                                                .series_aliases
                                                                .get_untracked()
                                                                .split('\n')
                                                                .map(|s| s.trim().to_string())
                                                                .filter(|a| !a.is_empty())
                                                                .collect();

                                                            // 3. Append provider title (after existing) if:
                                                            //    - it resolved (non-empty), AND
                                                            //    - not already in existing, AND
                                                            //    - there are actually new aliases to accompany it
                                                            let mut new_alias_count = 0;
                                                            if !provider_title.is_empty()
                                                                && !existing_lowercase.contains(&provider_title.to_lowercase())
                                                                && response_aliases.iter().any(|a| {
                                                                    !existing_lowercase.contains(&a.to_lowercase())
                                                                })
                                                            {
                                                                merged.push(provider_title.clone());
                                                                // Title is NOT counted in new_alias_count
                                                            }

                                                            // 4. Append response aliases that are truly new
                                                            let title_lower = provider_title.to_lowercase();
                                                            for alias in &response_aliases {
                                                                let lower = alias.to_lowercase();
                                                                if !existing_lowercase.contains(&lower)
                                                                    && lower != title_lower
                                                                {
                                                                    merged.push(alias.clone());
                                                                    new_alias_count += 1;
                                                                }
                                                            }

                                                            ctx.series_aliases.set(merged.join("\n"));
                                                            ctx.series_details.refetch();

                                                            crate::components::common::toast::show_toast(
                                                                format!("Fetched and merged {} aliases", new_alias_count),
                                                                crate::components::common::toast::NotificationType::Success,
                                                            );
                                                        }
                                                        Err(e) => {
                                                            crate::components::common::toast::show_error(
                                                                format!("Failed to fetch series aliases: {}", e.user_message()),
                                                            );
                                                        }
                                                    }
                                                    set_fetching_aliases.set(false);
                                                });
                                            }
                                        >
                                            {move || if fetching_aliases.get() {
                                                view! {
                                                    <span class="icon">
                                                        <svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                                            <circle cx="12" cy="12" r="8" stroke-dasharray="40" stroke-linecap="round">
                                                                <animateTransform attributeName="transform" type="rotate" from="0 12 12" to="360 12 12" dur="0.8s" repeatCount="indefinite"/>
                                                            </circle>
                                                        </svg>
                                                    </span>
                                                }.into_any()
                                            } else {
                                                view! { <span class="icon"><TagsIcon/></span> }.into_any()
                                            }}
                                            "Fetch Aliases"
                                        </button>
                                    </Show>
                                </div>
                            </div>
                        }.into_any()
                    }
                />
            </div>
            </Show>

            <div class="danger-zone">
                <div class="danger-zone-title">"Danger Zone"</div>
                <div class="action-row-body">
                    <p class="action-row-desc">
                        "Permanently remove this series from the library. Deleting files from disk is optional."
                    </p>
                    <button class="btn btn-danger" on:click=move |_| {
                        ctx.set_show_delete_modal.set(true);
                    }>"Delete Series"</button>
                </div>
            </div>
        </div>
    }
}
