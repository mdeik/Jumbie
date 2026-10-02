use crate::components::common::form_fields::{NumberInput, NumberInputMode};
use crate::components::common::icons::*;
use crate::components::edit_series::EditSeriesCtx;
use crate::components::edit_series::SeasonAccordionList;
use jumbie_shared::types::EpisodeViewModel;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn EpisodesTab() -> impl IntoView {
    let ctx = expect_context::<EditSeriesCtx>();

    let (new_season_num, set_new_season_num) = signal(String::new());
    let (new_episode_count, set_new_episode_count) = signal(String::new());
    let collapsed_seasons =
        RwSignal::<std::collections::HashSet<String>>::new(std::collections::HashSet::new());

    // SSoT: synced from series_details_view on every server refresh.
    // Local mutations (shift+click toggle) are applied optimistically and
    // overwritten when the server pushes back the canonical state.
    let episodes_rw = RwSignal::new(Vec::<EpisodeViewModel>::new());
    Effect::new(move |_| {
        // Move the episodes out of the freshly-cloned details rather than deep-cloning
        // them a second time (saves one full episode-vec clone per server refresh).
        let eps = ctx
            .series_details_view
            .get()
            .map(|d| d.episodes)
            .unwrap_or_default();
        episodes_rw.set(eps);
    });

    // SSoT: single callback for toggling monitor state — used by shift+click
    // in the accordion. Optimistic = flip locally, then API, then refetch.
    let on_toggle_monitor = Callback::new({
        let sd = ctx.series_details.clone();
        move |(episode_id, new_monitored): (String, bool)| {
            // Optimistic UI update
            episodes_rw.update(|eps| {
                if let Some(ep) = eps.iter_mut().find(|e| e.unique_id == episode_id) {
                    ep.monitored = new_monitored;
                }
            });
            let eid = episode_id.clone();
            spawn_local(async move {
                if let Err(e) = crate::api::toggle_episode_monitor(eid, new_monitored).await {
                    crate::components::common::toast::show_error(format!(
                        "Failed to toggle monitor: {}",
                        e.user_message()
                    ));
                }
                // Refetch to reconcile — on error reverts the optimistic flip.
                sd.refetch();
            });
        }
    });
    // SSoT: group episodes once here; the expand/collapse-all season list and the
    // Season Accordion both derive from this same grouping, so they cannot disagree
    // about which seasons exist or their order.
    let grouped_episodes: Memo<Vec<(String, Vec<EpisodeViewModel>)>> = Memo::new(move |_| {
        crate::utils::episode_state::group_episodes_by_season(
            episodes_rw.get(),
            ctx.absolute_numbering.get(),
        )
    });

    let season_names: Memo<Vec<String>> = Memo::new(move |_| {
        grouped_episodes.with(|groups| groups.iter().map(|(season, _)| season.clone()).collect())
    });

    let all_collapsed = Memo::new(move |_| {
        let names = season_names.get();
        collapsed_seasons.with(|s| names.iter().all(|n| s.contains(n)))
    });

    // Memoized season input validation — parsed once, used by both disabled state
    // and click handler. Empty/invalid input yields None, keeping the button disabled.
    let new_season_valid = Memo::new(move |_| -> Option<Vec<i32>> {
        let input = new_season_num.get();
        if input.trim().is_empty() {
            return None;
        }
        crate::validation::parse_season_input(&input).ok()
    });

    // Mode-aware season overrides — derived directly from the correct signal
    let season_overrides_for_mode = Memo::new(move |_| {
        if ctx.absolute_numbering.get() {
            ctx.season_absolute.get()
        } else {
            ctx.season.get()
        }
    });

    // Memoized season mismatch detection — the comparison itself is the pure,
    // unit-tested SSoT `crate::utils::season_mismatch`; only the reactive input
    // derivation stays here.
    let season_mismatch = Memo::new(move |_| -> Option<crate::utils::SeasonMismatch> {
        let details_opt = ctx.series_details.get().unwrap_or(None);
        let metadata_seasons = details_opt.map(|d| d.metadata_seasons).unwrap_or_default();

        // Seasons from episodes on disk. Built inside the borrow so the episode list
        // isn't cloned just to derive a set of season labels.
        let loaded_seasons: std::collections::HashSet<String> = ctx.series_details_view.with(|d| {
            d.as_ref()
                .map(|sd| {
                    sd.episodes
                        .iter()
                        .map(|ep| {
                            jumbie_shared::mapping::parse_season_num(&ep.season)
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| ep.season.clone())
                        })
                        .collect()
                })
                .unwrap_or_default()
        });

        let source = if ctx.absolute_numbering.get() {
            ctx.season_absolute
        } else {
            ctx.season
        };
        let override_seasons: Vec<String> =
            source.with(|ops| ops.iter().map(|o| o.season.clone()).collect());

        crate::utils::season_mismatch(&metadata_seasons, &loaded_seasons, &override_seasons)
    });

    view! {
        <div class="edit-content active" id="tab-episodes">
            <div class="episodes-header">
                <div class="flex gap-sm items-center flex-wrap">
                    <button
                        class="btn btn-sm episodes-header-btn flex gap-xs items-center"
                        id="expand-collapse-btn"
                        on:click=move |_| {
                            collapsed_seasons.update(|s| {
                                if all_collapsed.get() {
                                    s.clear();
                                } else {
                                    s.clear();
                                    s.extend(season_names.get());
                                }
                            });
                        }
                        title={move || if all_collapsed.get() { "Expand Seasons" } else { "Collapse Seasons" }}
                    >
                        <span class="icon">
                            {move || if all_collapsed.get() {
                                view! { <ChevronDownIcon/> }.into_any()
                            } else {
                                view! { <ChevronUpIcon/> }.into_any()
                            }}
                        </span>
                        <span class="desktop-only text-xs">
                            {move || if all_collapsed.get() { "Expand" } else { "Collapse" }}
                        </span>
                    </button>
                    <button
                        class="btn btn-sm episodes-header-btn flex gap-xs items-center"
                        title="Manage Series Files"
                        on:click=move |_| ctx.set_show_manage.set(true)
                    >
                        <span class="icon"><ManageIcon/></span>
                        <span class="desktop-only text-xs">"Manage"</span>
                    </button>
                    <button
                        class="btn btn-sm episodes-header-btn flex gap-xs items-center"
                        title="Search"
                        on:click=move |_| {
                            ctx.set_search_query_initial.set(ctx.form.with(|s| s.title.clone()));
                            ctx.set_show_search.set(true);
                        }
                    >
                        <span class="icon"><SearchIcon/></span>
                        <span class="desktop-only text-xs">"Search"</span>
                    </button>


                </div>
                <div class="flex gap-md text-xs text-muted-color flex-wrap legend-container">
                    {crate::components::edit_series::season_accordion::render_legend_items(false)}
                </div>
            </div>

            <Show when=move || !ctx.absolute_numbering.get()>
            <div class="add-season-bar">
                <NumberInput
                    id="newSeasonNum"
                    label="Season:"
                    placeholder="1-5, 8, 10-12"
                    class="input input-season-num"
                    value=new_season_num
                    set_value=Callback::new(move |v| set_new_season_num.set(v))
                    mode=NumberInputMode::Custom
                    allow_chars="-, "
                />
                <NumberInput
                    id="newEpisodeCount"
                    label="Episodes:"
                    placeholder="12"
                    min="1"
                    class="input input-episode-num"
                    value=new_episode_count
                    set_value=Callback::new(move |v| set_new_episode_count.set(v))
                    mode=NumberInputMode::Integer
                />
                <button
                    class="btn btn-sm btn-primary"
                    disabled=move || new_season_valid.get().is_none()
                    on:click=move |_| {
                        let seasons = match new_season_valid.get() {
                            Some(s) => s,
                            None => return,
                        };
                        let ep_count_str = new_episode_count.get();
                        let ep_count = ep_count_str.parse::<i32>().unwrap_or(1);

                        if !ep_count_str.is_empty() && ep_count < 1 {
                            crate::components::common::toast::show_error("Season cannot have 0 episodes.");
                            return;
                        }

                        let is_abs = ctx.absolute_numbering.get_untracked();
                        let target = if is_abs { ctx.season_absolute } else { ctx.season };

                        // Snapshot current state for rollback on failure
                        let prev_state = target.get_untracked();

                        // Optimistic UI update — instant feedback
                        target.update(|ops| {
                            for s_num in &seasons {
                                let s_num_str = s_num.to_string();
                                if let Some(existing) = ops.iter_mut().find(|o| o.season == s_num_str) {
                                    existing.cell_count = Some(ep_count);
                                } else {
                                    ops.push(jumbie_shared::types::SeasonOverride {
                                        season: s_num_str,
                                        episode_start: None,
                                        episode_end: None,
                                        cell_count: Some(ep_count),
                                        episode_offset: None,
                                        alias_season_number: None,
                                        search_format: None,
                                        aliases: Vec::new(),
                                        reg_patterns: Vec::new(),
                                    });
                                }
                            }
                        });
                        let series_id = ctx.series_id.get_untracked();
                        let details = ctx.series_details.clone();

                        set_new_season_num.set(String::new());
                        set_new_episode_count.set(String::new());

                        // Dedicated API call — only sends the changed data
                        spawn_local(async move {
                            match crate::api::batch_upsert_seasons(
                                series_id.clone(),
                                jumbie_shared::types::BatchUpsertSeasonsRequest {
                                    seasons,
                                    episode_count: ep_count,
                                    absolute_numbering: is_abs,
                                },
                            )
                            .await
                            {
                                Ok(keys) => {
                                    details.refetch();
                                    crate::components::common::toast::show_success(format!(
                                        "Season config updated for {} season(s).",
                                        keys.len()
                                    ));
                                }
                                Err(e) => {
                                    target.set(prev_state);
                                    crate::components::common::toast::show_error(format!(
                                        "Failed to save seasons: {}",
                                        e.user_message()
                                    ));
                                }
                            }
                        });
                    }
                >
                    "Add Season"
                </button>

                {
                    // Show "Match Seasons" button when provider has seasons not yet loaded or vice versa
                    move || {
                        match season_mismatch.get() {
                            Some(crate::utils::SeasonMismatch { ref missing, ref extra_overrides }) => {
                                let missing_clone = missing.clone();
                                let extra_clone = extra_overrides.clone();
                                let sid_for_click = ctx.series_id.get_untracked();
                                view! {
                                    <button
                                        class="btn btn-sm btn-primary flex items-center gap-xs"
                                        title="Match seasons to provider data already stored locally"
                                        on:click=move |_| {
                                            let missing = missing_clone.clone();
                                            let extra = extra_clone.clone();
                                            let s_id = sid_for_click.clone();
                                            let details = ctx.series_details.clone();

                                            // Step 1: Add cell_count for missing seasons & remove extras
                                            let is_abs_match = ctx.absolute_numbering.get_untracked();
                                            let target = if is_abs_match { ctx.season_absolute } else { ctx.season };
                                            target.update(|ops| {
                                                for (s_num, metadata_count) in &missing {
                                                    if let Some(existing) = ops.iter_mut().find(|o| &o.season == s_num) {
                                                        if existing.cell_count.is_none() {
                                                            existing.cell_count = Some(*metadata_count);
                                                        }
                                                    } else {
                                                        ops.push(jumbie_shared::types::SeasonOverride {
                                                            season: s_num.to_string(),
                                                            episode_start: None,
                                                            episode_end: None,
                                                            cell_count: Some(*metadata_count),
                                                            episode_offset: None,
                                                            alias_season_number: None,
                                                            search_format: None,
                                                            aliases: Vec::new(),
                                                            reg_patterns: Vec::new(),
                                                        });
                                                    }
                                                }
                                                ops.retain(|o| !extra.contains(&o.season));
                                            });
                                            // Suppress the autosave toast — the explicit message at step 3 handles it.
                                            ctx.save_series.run(Some(String::new()));

                                            // Step 2: Restore cached episode metadata for each newly added season
                                            for (s_num, _) in &missing {
                                                crate::utils::spawn_api_toast(
                                                    crate::api::restore_season_metadata(s_id.clone(), s_num.clone()),
                                                    None,
                                                    |_| {},
                                                );
                                            }

                                            // Step 3: Refetch series details to refresh the episode view
                                            details.refetch();

                                            let msg = if !missing.is_empty() && !extra.is_empty() {
                                                format!("Added {} season(s) and removed {} extra.", missing.len(), extra.len())
                                            } else if !missing.is_empty() {
                                                format!("Added {} missing season(s).", missing.len())
                                            } else {
                                                format!("Removed {} extra season(s).", extra.len())
                                            };
                                            crate::components::common::toast::show_success(msg.clone());
                                        }
                                    >
                                        <span class="icon text-xs"><WandSparklesIcon/></span>
                                        <span class="text-xs">"Match Seasons"</span>
                                    </button>
                                }.into_any()
                            }
                            None => view! { <span class="hidden"></span> }.into_any(),
                        }
                    }
                }
            </div>
            </Show>

             <div class="season-accordion">
                 <SeasonAccordionList
                     grouped_episodes=grouped_episodes
                     season_overrides=season_overrides_for_mode
                     series_id=ctx.series_id
                     collapsed_seasons=collapsed_seasons
                     managing_season=ctx.managing_season
                     set_selected_episode=ctx.set_selected_episode
                     set_show_modal=ctx.set_show_modal
                     save_series=ctx.save_series
                     metadata_seasons=Signal::derive(move || ctx.series_details.get().flatten().map(|d| d.metadata_seasons).unwrap_or_default())
                     on_toggle_episode_monitor=on_toggle_monitor
                 />
             </div>
        </div>
    }
}
