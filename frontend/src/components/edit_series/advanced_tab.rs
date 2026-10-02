use crate::components::common::form_fields::{
    FormatFieldBuilder, TooltipBuilder, TriStateCheckbox,
};
use crate::components::common::nested_checkboxes::{NestedCheckboxGroup, NestedCheckboxItem};
use crate::components::common::standard_modal::{ModalConfirmationFooter, StandardModal};
use crate::components::edit_series::EditSeriesCtx;
use jumbie_shared::variables::{TemplateContext, get_allowed_vars};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The backend's default organization config. The format/search fields below are
/// seeded from it so the UI never drifts from the shipped defaults.
fn org_defaults() -> jumbie_shared::config::OrganizationConfig {
    jumbie_shared::config::OrganizationConfig::default()
}

#[component]
pub fn AdvancedTab() -> impl IntoView {
    let ctx = expect_context::<EditSeriesCtx>();

    let (show_clear_modal, set_show_clear_modal) = signal(false);
    let (clear_all, set_clear_all) = signal(false);

    // Resolve the list of metadata providers configured for this series.
    // Each entry is (instance_id, display_name) so we can show a named
    // checkbox per provider and send the UUID to the API.
    let series_providers = Signal::derive(move || {
        let metadata_ids = ctx.form.with(|s| s.metadata_ids.clone());
        let plugins = ctx.active_metadata_plugins.get();
        metadata_ids
            .into_keys()
            .map(|uuid| {
                let display_name = plugins
                    .iter()
                    .find(|p| p.instance_id.as_deref() == Some(&uuid))
                    .map(|p| p.display_name.clone())
                    .unwrap_or_else(|| uuid.clone());
                (uuid, display_name)
            })
            .collect::<Vec<_>>()
    });

    // Per-provider toggle signals, rebuilt each time the modal opens.
    let (provider_toggles, set_provider_toggles) =
        signal::<Vec<(String, RwSignal<bool>)>>(Vec::new());

    let open_clear_modal = move |_| {
        let providers = series_providers.get();
        set_provider_toggles.set(
            providers
                .into_iter()
                .map(|(uuid, _name)| (uuid, RwSignal::new(false)))
                .collect(),
        );
        set_clear_all.set(false);
        set_show_clear_modal.set(true);
    };

    let close_clear_modal = move || {
        set_show_clear_modal.set(false);
    };

    // Global search templates, used to seed the series fields when they stop
    // inheriting (the row is disabled while inheriting).
    let global_search_format = Signal::derive(move || {
        ctx.global_config
            .get()
            .flatten()
            .map(|c| c.organization.search_format.clone())
            .unwrap_or_else(|| org_defaults().search_format)
    });
    let global_search_format_absolute = Signal::derive(move || {
        ctx.global_config
            .get()
            .flatten()
            .map(|c| c.organization.search_format_absolute.clone())
            .unwrap_or_else(|| org_defaults().search_format_absolute)
    });

    // Global naming formats, used to seed/display the format row while it inherits.
    let global_season_folder_format = Signal::derive(move || {
        ctx.global_config
            .get()
            .flatten()
            .map(|c| c.organization.season_folder_format.clone())
            .unwrap_or_else(|| org_defaults().season_folder_format)
    });
    let global_episode_file_format = Signal::derive(move || {
        ctx.global_config
            .get()
            .flatten()
            .map(|c| c.organization.episode_file_format.clone())
            .unwrap_or_else(|| org_defaults().episode_file_format)
    });
    let global_season_folder_format_absolute = Signal::derive(move || {
        ctx.global_config
            .get()
            .flatten()
            .map(|c| c.organization.season_folder_format_absolute.clone())
            .unwrap_or_else(|| org_defaults().season_folder_format_absolute)
    });
    let global_episode_file_format_absolute = Signal::derive(move || {
        ctx.global_config
            .get()
            .flatten()
            .map(|c| c.organization.episode_file_format_absolute.clone())
            .unwrap_or_else(|| org_defaults().episode_file_format_absolute)
    });

    view! {
        <div class="edit-content active" id="tab-advanced">
           <crate::components::edit_series::alias_config::AliasConfig
               aliases=ctx.series_aliases
               patterns=ctx.series_reg_patterns
               on_save=move |_| ctx.save_series.run(None)
           />


           <div class="season-config-panel active" id="search-config">
               <div
                   class="inheritance-toggle"
                   on:click=move |_| {
                       let use_defaults = !ctx.form.with(|s| s.search_use_defaults());
                       if use_defaults {
                           ctx.form.update(|s| {
                               s.search_format = None;
                               s.search_format_absolute = None;
                           });
                       } else {
                           let normal = global_search_format.get();
                           let absolute = global_search_format_absolute.get();
                           ctx.form.update(|s| {
                               s.search_format = Some(s.search_format.clone().unwrap_or(normal));
                               s.search_format_absolute = Some(s.search_format_absolute.clone().unwrap_or(absolute));
                           });
                       }
                       ctx.save_series.run(None);
                   }
               >
                   <input class="pointer-events-none" type="checkbox" id="inheritSearch" prop:checked={move || ctx.form.with(|s| s.search_use_defaults())} />
                   <label class="pointer-events-none" for="inheritSearch" >"Use Defaults"</label>
                   <span class="inheritance-hint pointer-events-none">
                       {move || if ctx.form.with(|s| s.search_use_defaults()) {
                           "Inheriting from global settings"
                       } else {
                           "Overriding global settings"
                       }}
                   </span>
               </div>
               <p class="form-hint mb-md">{crate::validation::SEARCH_FORMAT_FIELD_HELP}</p>
           </div>

           <div class="form-row" id="search-settings-row" class:panel-disabled=move || ctx.form.with(|s| s.search_use_defaults())>
               <FormatFieldBuilder
                   id="series-search-format".to_string()
                   label="S/E Search Format".to_string()
                   value=Signal::derive(move || ctx.form.with(|s| s.search_format.clone()).unwrap_or_else(|| global_search_format.get()))
                   on_change=Callback::new(move |v: String| ctx.form.update(|s| s.search_format = Some(v)))
                   tooltip=TooltipBuilder::new().with_context(TemplateContext::SearchFormat).with_formatting()
                   disabled=Signal::derive(move || ctx.form.with(|s| s.search_use_defaults()))
                   on_blur=Callback::new(move |_| {
                       let val = ctx.form.with_untracked(|s| s.search_format.clone());
                       if let Some(v) = val
                           && crate::validation::validate_search_format_field(&v, true, "search format") {
                           return;
                       }
                       ctx.save_series.run(None);
                   })
               />
               <FormatFieldBuilder
                   id="series-search-format-absolute".to_string()
                   label="S/E Search Format (Absolute)".to_string()
                   value=Signal::derive(move || ctx.form.with(|s| s.search_format_absolute.clone()).unwrap_or_else(|| global_search_format_absolute.get()))
                   on_change=Callback::new(move |v: String| ctx.form.update(|s| s.search_format_absolute = Some(v)))
                   tooltip=TooltipBuilder::new().with_context(TemplateContext::SearchFormat).with_formatting()
                   disabled=Signal::derive(move || ctx.form.with(|s| s.search_use_defaults()))
                   on_blur=Callback::new(move |_| {
                       let val = ctx.form.with_untracked(|s| s.search_format_absolute.clone());
                       if let Some(v) = val
                           && crate::validation::validate_search_format_field(&v, true, "absolute search format") {
                           return;
                       }
                       ctx.save_series.run(None);
                   })
               />
           </div>


           <div class="season-config-panel active" id="season-config-2">
               <div
                   class="inheritance-toggle"
                   on:click=move |_| {
                       let use_defaults = !ctx.form.with(|s| s.formats_use_defaults());
                       if use_defaults {
                           ctx.form.update(|s| {
                               s.season_folder_format = None;
                               s.episode_file_format = None;
                               s.season_folder_format_absolute = None;
                               s.episode_file_format_absolute = None;
                           });
                       } else {
                           let season = global_season_folder_format.get();
                           let episode = global_episode_file_format.get();
                           let season_abs = global_season_folder_format_absolute.get();
                           let episode_abs = global_episode_file_format_absolute.get();
                           ctx.form.update(|s| {
                               s.season_folder_format = Some(s.season_folder_format.clone().unwrap_or(season));
                               s.episode_file_format = Some(s.episode_file_format.clone().unwrap_or(episode));
                               s.season_folder_format_absolute = Some(s.season_folder_format_absolute.clone().unwrap_or(season_abs));
                               s.episode_file_format_absolute = Some(s.episode_file_format_absolute.clone().unwrap_or(episode_abs));
                           });
                       }
                       ctx.save_series.run(None);
                   }
               >
                   <input class="pointer-events-none" type="checkbox" id="inheritSeries-2" prop:checked={move || ctx.form.with(|s| s.formats_use_defaults())} />
                   <label class="pointer-events-none" for="inheritSeries-2" >"Use Defaults"</label>
                   <span class="inheritance-hint pointer-events-none">
                       {move || if ctx.form.with(|s| s.formats_use_defaults()) {
                           "Inheriting from global settings"
                       } else {
                           "Overriding global settings"
                       }}
                   </span>
               </div>
               <p class="form-hint mb-md">{crate::validation::NAMING_FORMATS_FIELD_HELP}</p>
           </div>

           <div class="form-row" id="format-settings-row" class:panel-disabled=move || ctx.form.with(|s| s.formats_use_defaults())>

           <FormatFieldBuilder
               id="series-season-folder-format".to_string()
               label="Season Folder Format".to_string()
               value=Signal::derive(move || ctx.form.with(|s| s.season_folder_format.clone()).unwrap_or_else(|| global_season_folder_format.get()))
               on_change=Callback::new(move |v: String| ctx.form.update(|s| s.season_folder_format = Some(v)))
               tooltip=TooltipBuilder::new().with_context(TemplateContext::SeasonFolder).with_formatting()
               disabled=Signal::derive(move || ctx.form.with(|s| s.formats_use_defaults()))
               on_blur=Callback::new(move |_| {
                   let val = ctx.form.with_untracked(|s| s.season_folder_format.clone()).unwrap_or_else(|| global_season_folder_format.get_untracked());
                   let allowed_vars = get_allowed_vars(&TemplateContext::SeasonFolder);
                   if crate::validation::validate_template_field(&val, &allowed_vars, true, "season folder format") {
                       return;
                   }
                   ctx.save_series.run(None);
               })
           />

           <FormatFieldBuilder
               id="series-episode-file-format".to_string()
               label="Episode File Format".to_string()
               value=Signal::derive(move || ctx.form.with(|s| s.episode_file_format.clone()).unwrap_or_else(|| global_episode_file_format.get()))
               on_change=Callback::new(move |v: String| ctx.form.update(|s| s.episode_file_format = Some(v)))
               tooltip=TooltipBuilder::new().with_context(TemplateContext::EpisodeFile).with_formatting()
               disabled=Signal::derive(move || ctx.form.with(|s| s.formats_use_defaults()))
               on_blur=Callback::new(move |_| {
                   let val = ctx.form.with_untracked(|s| s.episode_file_format.clone()).unwrap_or_else(|| global_episode_file_format.get_untracked());
                   let allowed_vars = get_allowed_vars(&TemplateContext::EpisodeFile);
                   if crate::validation::validate_template_field(&val, &allowed_vars, false, "episode file format") {
                       return;
                   }
                   ctx.save_series.run(None);
               })
           />

           <FormatFieldBuilder
               id="series-season-folder-format-absolute".to_string()
               label="Season Folder Format (Absolute)".to_string()
               value=Signal::derive(move || ctx.form.with(|s| s.season_folder_format_absolute.clone()).unwrap_or_else(|| global_season_folder_format_absolute.get()))
               on_change=Callback::new(move |v: String| ctx.form.update(|s| s.season_folder_format_absolute = Some(v)))
               tooltip=TooltipBuilder::new().with_context(TemplateContext::SeasonFolder).with_formatting()
               disabled=Signal::derive(move || ctx.form.with(|s| s.formats_use_defaults()))
               on_blur=Callback::new(move |_| {
                   let val = ctx.form.with_untracked(|s| s.season_folder_format_absolute.clone()).unwrap_or_else(|| global_season_folder_format_absolute.get_untracked());
                   let allowed_vars = get_allowed_vars(&TemplateContext::SeasonFolder);
                   if crate::validation::validate_template_field(&val, &allowed_vars, true, "absolute season folder format") {
                       return;
                   }
                   ctx.save_series.run(None);
               })
           />

           <FormatFieldBuilder
               id="series-episode-file-format-absolute".to_string()
               label="Episode File Format (Absolute)".to_string()
               value=Signal::derive(move || ctx.form.with(|s| s.episode_file_format_absolute.clone()).unwrap_or_else(|| global_episode_file_format_absolute.get()))
               on_change=Callback::new(move |v: String| ctx.form.update(|s| s.episode_file_format_absolute = Some(v)))
               tooltip=TooltipBuilder::new().with_context(TemplateContext::EpisodeFile).with_formatting()
               disabled=Signal::derive(move || ctx.form.with(|s| s.formats_use_defaults()))
               on_blur=Callback::new(move |_| {
                   let val = ctx.form.with_untracked(|s| s.episode_file_format_absolute.clone()).unwrap_or_else(|| global_episode_file_format_absolute.get_untracked());
                   let allowed_vars = get_allowed_vars(&TemplateContext::EpisodeFile);
                   if crate::validation::validate_template_field(&val, &allowed_vars, false, "absolute episode file format") {
                       return;
                   }
                   ctx.save_series.run(None);
               })
           />
           </div>

           <div class="tri-state-checkboxes">
           <TriStateCheckbox
               label="Rename Episodes"
               help_text="When disabled, episode files will be organized into the correct folder but their original filenames will be preserved. Leave indeterminate to inherit the global default.".to_string()
               value=Signal::derive(move || ctx.form.with(|s| s.rename_episodes))
               set_value=Callback::new(move |v| {
                   ctx.form.update(|s| s.rename_episodes = v);
                   ctx.save_series.run(None);
               })
           />
               <TriStateCheckbox
                   label="Flatten Seasons"
                   help_text="When enabled, all episodes are placed directly in the series folder without season sub-folders. Leave indeterminate to inherit the global default.".to_string()
                   value=Signal::derive(move || ctx.form.with(|s| s.flatten_season_folders))
                   set_value=Callback::new(move |v| {
                       ctx.form.update(|s| s.flatten_season_folders = v);
                       ctx.save_series.run(None);
                   })
               />
               <TriStateCheckbox
                   label="Absolute Numbering Mapping"
                   help_text="When enabled, episodes are tracked by absolute numbers rather than reseting the episode count each season.".to_string()
                   value=Signal::derive(move || ctx.form.with(|s| s.absolute_numbering))
                   set_value=Callback::new(move |v| {
                       ctx.form.update(|s| s.absolute_numbering = v);
                       ctx.save_series.run(None);
                   })
               />
           </div>
            <Show when=move || ctx.series_details_view.get().map(|d| !d.suppressed_seasons.is_empty()).unwrap_or(false)>
                <div class="action-row-body">
                    <p class="action-row-desc">
                        "Seasons you removed. They stay deleted across metadata resync. Restore re-adds the season from cached provider metadata — available only while that cache is present."
                    </p>
                    <div class="flex flex-col gap-sm">
                        <For
                            each=move || ctx.series_details_view.get().map(|d| d.suppressed_seasons).unwrap_or_default()
                            key=|s| s.season
                            children=move |s| {
                                let season = s.season;
                                let cache_available = s.cache_available;
                                view! {
                                    <div class="flex items-center justify-between gap-md">
                                        <span>{format!("Season {}", season)}</span>
                                        <Show when=move || cache_available>
                                            <button class="btn btn-secondary" on:click=move |_| {
                                                let sid = ctx.series_id.get_untracked();
                                                let details = ctx.series_details.clone();
                                                spawn_local(async move {
                                                    crate::utils::spawn_api_toast(
                                                        crate::api::restore_season_metadata(sid, season.to_string()),
                                                        None,
                                                        move |_| {
                                                            crate::components::common::toast::show_success("Season restored");
                                                            details.refetch();
                                                        },
                                                    );
                                                });
                                            }>"Restore"</button>
                                        </Show>
                                    </div>
                                }
                            }
                        />
                    </div>
                </div>
            </Show>

            <div class="danger-zone">
                <div class="danger-zone-title">"Danger Zone"</div>
                <div class="action-row-body">
                    <p class="action-row-desc">
                        "Delete all episode data (metadata, media scans) for this series from the database. Episode files remain on disk."
                    </p>
                    <button class="btn btn-danger" on:click={
                        let ctx_clone = ctx.clone();
                        move |_| {
                            let sid = ctx_clone.series_id.get_untracked();
                            let details = ctx_clone.series_details.clone();
                            spawn_local(async move {
                                crate::utils::spawn_api_toast(crate::api::delete_series_data(sid), None, move |_| {
                                    crate::components::common::toast::show_success("Episode data deleted");
                                    details.refetch();
                                });
                            });
                        }
                    }>"Delete Episode Data"</button>
                </div>
                <div class="action-row-body">
                    <p class="action-row-desc">
                        "Reset all configuration for this series (quality profile, release profile, season settings, format overrides) to defaults. Episode data is kept."
                    </p>
                    <button class="btn btn-danger" on:click={
                        let ctx_clone = ctx.clone();
                        move |_| {
                            let sid = ctx_clone.series_id.get_untracked();
                            let details = ctx_clone.series_details.clone();
                            spawn_local(async move {
                                crate::utils::spawn_api_toast(crate::api::reset_series_configuration(sid), None, move |_| {
                                    crate::components::common::toast::show_success("Configuration reset");
                                    details.refetch();
                                });
                            });
                        }
                    }>"Reset Configuration"</button>
                </div>
                <div class="action-row-body">
                    <p class="action-row-desc">
                        "Remove cached metadata fetched from the online provider. Metadata will be re-fetched on the next sync."
                    </p>
                    <button class="btn btn-danger" on:click=open_clear_modal>"Clear Cached Metadata"</button>
                </div>
            </div>

            {move || {
                if show_clear_modal.get() {
                    let toggles = provider_toggles.get();

                    let children: Vec<NestedCheckboxItem> = toggles
                        .iter()
                        .map(|(uuid, sig)| {
                            let display_name = series_providers
                                .get()
                                .into_iter()
                                .find(|(u, _)| u == uuid)
                                .map(|(_, name)| name)
                                .unwrap_or_else(|| uuid.clone());

                            // Stable id based on uuid prefix
                            let id = format!("clear-provider-{}", &uuid[..8.min(uuid.len())]);

                            NestedCheckboxItem {
                                id,
                                label: format!("Only clear metadata from: {}", display_name),
                                checked: Signal::from(sig.read_only()),
                                set_checked: sig.write_only(),
                            }
                        })
                        .collect();

                    let on_confirm = {
                        let ctx_clone = ctx.clone();
                        let provider_toggles = provider_toggles;
                        move || {
                            let sid = ctx_clone.series_id.get_untracked();
                            let clear_all_val = clear_all.get_untracked();

                            if clear_all_val {
                                let ctx_for = ctx_clone.clone();
                                spawn_local(async move {
                                    match crate::api::clear_metadata_cache(sid, None).await {
                                        Ok(_) => {
                                            // Remove all provider entries from the form state so
                                            // no providers appear as options in the modal.
                                            ctx_for.form.update(|f| f.metadata_ids.clear());
                                            crate::components::common::toast::show_success(
                                                "Metadata cache cleared",
                                            );
                                        }
                                        Err(e) => {
                                            crate::components::common::toast::show_error(
                                                format!("Failed to clear metadata cache: {}", e),
                                            );
                                        }
                                    }
                                });
                            } else {
                                let toggles = provider_toggles.get_untracked();
                                let selected_uuids: Vec<String> = toggles
                                    .into_iter()
                                    .filter(|(_, sig)| sig.get_untracked())
                                    .map(|(uuid, _)| uuid)
                                    .collect();

                                if selected_uuids.is_empty() {
                                    return;
                                }

                                let ctx_for = ctx_clone.clone();
                                spawn_local(async move {
                                    let mut all_ok = true;
                                    for provider in &selected_uuids {
                                        if crate::api::clear_metadata_cache(
                                            sid.clone(),
                                            Some(provider.clone()),
                                        )
                                        .await
                                        .is_err()
                                        {
                                            all_ok = false;
                                            break;
                                        }
                                    }

                                    if all_ok {
                                        // Only remove from form state when all API calls succeed.
                                        ctx_for.form.update(|f| {
                                            f.metadata_ids
                                                .retain(|k, _| !selected_uuids.contains(k));
                                        });
                                        crate::components::common::toast::show_success(
                                            "Metadata cache cleared",
                                        );
                                    } else {
                                        crate::components::common::toast::show_error(
                                            "Failed to clear metadata cache for one or more providers",
                                        );
                                    }
                                });
                            }
                            close_clear_modal();
                        }
                    };

                    let on_cancel = {
                        let cc = close_clear_modal.clone();
                        move || cc()
                    };

                    view! {
                        <StandardModal
                            show=show_clear_modal
                            on_close=on_cancel
                            title="Clear Cached Metadata"
                            size="modal-sm"
                            footer=view! {
                                <ModalConfirmationFooter
                                    on_confirm=on_confirm
                                    confirm_label="Clear Metadata"
                                    confirm_class=Signal::derive(move || {
                                        let parent = clear_all.get();
                                        let any_child = provider_toggles
                                            .get()
                                            .iter()
                                            .any(|(_, sig)| sig.get());
                                        let enabled = parent || any_child;
                                        if enabled { "btn btn-danger".to_string() } else { "btn btn-danger disabled".to_string() }
                                    })
                                />
                            }.into_any()
                        >
                            <NestedCheckboxGroup
                                parent_id="clear-all-checkbox"
                                parent_label="Clear All Cached Metadata for Series"
                                parent_checked=Signal::from(clear_all)
                                set_parent_checked=set_clear_all
                                children
                            />
                        </StandardModal>
                    }.into_any()
                } else {
                    view! { <span class="hidden"></span> }.into_any()
                }
            }}
         </div>
    }
}
