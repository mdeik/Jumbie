use super::auto_config::AutoProfilesConfigModal;
use crate::components::common::form_fields::{NumberInput, NumberInputMode, TextInput};
use crate::components::common::icons::*;
use crate::components::common::modal_wrapper::modal_footer_save;
use crate::components::common::standard_modal::StandardModal;
use crate::components::common::table_builder::{ManagedColumn, TableBuilder};
use crate::components::common::toast;
use crate::components::profiles::RankingList;
use crate::components::profiles::profile_list::ProfileList;
use crate::components::profiles::profile_utils::{generate_unique_name, score_class};
use crate::hooks::use_persistent_table_state;
use crate::hooks::use_ui_config::use_time_format;
use crate::utils::format_datetime_local;
use jumbie_shared::config;
use jumbie_shared::types::{AutomaticProfile, ReleaseProfile};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

#[component]
pub fn ReleaseProfileEditor() -> impl IntoView {
    let (release_profiles, set_release_profiles) = signal(HashMap::<String, ReleaseProfile>::new());
    let selected_profile = RwSignal::new(None::<String>);

    let (cfg, set_cfg) = signal(None::<config::Config>);
    let (auto_profiles, set_auto_profiles) = signal(Vec::<AutomaticProfile>::new());
    let selected_auto_profile = RwSignal::new(None::<String>);
    let (show_auto_config, set_show_auto_config) = signal(false);
    let (auto_records, set_auto_records) =
        signal(Vec::<jumbie_shared::types::AutomaticProfileRecord>::new());
    let (recalculating, set_recalculating) = signal(false);

    // User's 12h/24h preference for the auto-record "Date Added" column.
    let time_format = use_time_format();

    let on_remove_auto = Callback::new(move |submitter: String| {
        set_auto_profiles.update(|p| p.retain(|a| a.submitter != submitter));
        let s_for_api = submitter.clone();
        crate::utils::spawn_api_toast(
            crate::api::delete_automatic_profile(s_for_api),
            None,
            move |_| {
                toast::show_success("Profile deleted");
                let updated = auto_profiles.get_untracked();
                crate::utils::write_cache("fetch_automatic_profiles", &updated);
            },
        );
    });

    let fetch_auto_data = move || {
        crate::utils::use_api_cache(
            "fetch_config".to_string(),
            || crate::api::fetch_config(),
            move |c| {
                set_cfg.try_update(|cfg| *cfg = Some(c));
            },
        );
        crate::utils::use_api_cache(
            "fetch_automatic_profiles".to_string(),
            || crate::api::fetch_automatic_profiles(),
            move |c| {
                set_auto_profiles.try_update(|p| *p = c);
            },
        );
    };

    Effect::new(move |_| {
        fetch_auto_data();
    });

    crate::utils::use_api_cache(
        "fetch_release_profiles".to_string(),
        || crate::api::fetch_release_profiles(),
        move |c| {
            set_release_profiles.try_update(|p| *p = c);
        },
    );

    let get_names = Signal::derive(move || {
        let mut pairs: Vec<(String, Vec<String>)> = release_profiles
            .get()
            .iter()
            .map(|(k, p)| {
                let term_count = (p.terms.len() + p.regex_terms.len()).to_string();
                (
                    k.clone(),
                    vec![p.name.clone(), p.min_score.to_string(), term_count],
                )
            })
            .collect();
        pairs.sort_by(|a, b| a.1[0].cmp(&b.1[0]));
        pairs
    });

    let (draft_name, set_draft_name) = signal(String::new());
    let (draft_min_score, set_draft_min_score) = signal(0i32);
    let (draft_size_score, set_draft_size_score) = signal(0i32);
    let (draft_peers_score, set_draft_peers_score) = signal(0i32);
    let (draft_age_score, set_draft_age_score) = signal(0i32);
    let (draft_terms, set_draft_terms) = signal(Vec::<(usize, String, i32)>::new());
    let (draft_next_id, set_draft_next_id) = signal(0usize);
    let (draft_term_flags, set_draft_term_flags) = signal(HashMap::<usize, bool>::new());
    let (draft_submitters, set_draft_submitters) = signal(Vec::<(usize, String, i32)>::new());
    let (draft_submitter_next_id, set_draft_submitter_next_id) = signal(0usize);
    let (draft_case_insensitive_terms, set_draft_case_insensitive_terms) = signal(false);
    let (draft_case_insensitive_submitters, set_draft_case_insensitive_submitters) = signal(false);

    let health = LocalResource::new(move || async move { crate::api::fetch_health().await.ok() });

    let on_add = Callback::new(move |()| {
        let existing: std::collections::HashSet<String> = release_profiles
            .get()
            .values()
            .map(|p| p.name.clone())
            .collect();
        let name = generate_unique_name(&existing, "New Release");

        set_draft_name.set(name);
        set_draft_min_score.set(0);
        set_draft_size_score.set(0);
        set_draft_peers_score.set(0);
        set_draft_age_score.set(0);
        set_draft_terms.set(Vec::new());
        set_draft_next_id.set(0);
        set_draft_term_flags.set(HashMap::new());
        set_draft_submitters.set(Vec::new());
        set_draft_submitter_next_id.set(0);
        set_draft_case_insensitive_terms.set(false);
        set_draft_case_insensitive_submitters.set(false);

        selected_profile.set(Some("new".to_string()));
    });

    let on_remove = Callback::new(move |id: String| {
        set_release_profiles.update(|c| {
            c.remove(&id);
        });
        let c_save = release_profiles.get();
        let c_for_cache = c_save.clone();
        crate::utils::spawn_api_toast(
            async move { crate::api::save_release_profiles(&c_save).await },
            None,
            move |_| {
                crate::utils::write_cache("fetch_release_profiles", &c_for_cache);
            },
        );
    });

    let on_select = Callback::new(move |id: String| {
        selected_profile.set(Some(id));
    });

    // Sync draft when selected profile changes
    Effect::new(move |_| {
        if let Some(key) = selected_profile.get()
            && let Some(p) = release_profiles.get().get(&key).cloned()
        {
            set_draft_name.set(p.name.clone());
            set_draft_min_score.set(p.min_score);
            set_draft_case_insensitive_terms.set(p.case_insensitive_terms);
            set_draft_case_insensitive_submitters.set(p.case_insensitive_submitters);

            set_draft_size_score.set(p.size_score_per_gb);
            set_draft_peers_score.set(p.peers_score_per_peer);
            set_draft_age_score.set(p.age_score_per_day);

            let mut terms_vec = Vec::new();
            let mut term_flags = HashMap::new();
            let mut id = draft_next_id.get_untracked();
            for (k, v) in &p.terms {
                terms_vec.push((id, k.clone(), *v));
                id += 1;
            }
            for (k, v) in &p.regex_terms {
                terms_vec.push((id, k.clone(), *v));
                term_flags.insert(id, true);
                id += 1;
            }
            terms_vec.sort_by(|a, b| a.1.cmp(&b.1));
            set_draft_terms.set(terms_vec);
            set_draft_term_flags.set(term_flags);
            set_draft_next_id.set(id);

            let mut submitters_vec = Vec::new();
            let mut s_id = draft_submitter_next_id.get_untracked();
            for (k, v) in &p.submitters {
                submitters_vec.push((s_id, k.clone(), *v));
                s_id += 1;
            }
            submitters_vec.sort_by(|a, b| a.1.cmp(&b.1));
            set_draft_submitters.set(submitters_vec);
            set_draft_submitter_next_id.set(s_id);
        }
    });

    view! {
        <div class="view active">
            <div class="settings-content">
                <ProfileList
                    title="Release Profiles".to_string()
                    add_label="Add Profile".to_string()
                    columns=vec!["Name", "Min Score", "Terms"]
                    items=get_names
                    on_add=on_add
                    on_remove=on_remove
                    on_select=on_select
                    persistence_id="release_profiles".to_string()
                    detail_formatter=|cells: &[String]| {
                        let min_score = cells.get(1).map(|s| s.as_str()).unwrap_or("0");
                        let terms_count = cells.get(2).map(|s| s.as_str()).unwrap_or("0");
                        format!("Min Score: {}  •  {} terms", min_score, terms_count)
                    }
                />

                {
                    view! {
                        <div class="mt-xl">
                            <div class=move || {
                                "table-container".to_string()
                            }>
                                <div class="table-header flex justify-between mb-sm">
                                    <h3 class="m-0 mr-md">"Automatic Profiles"</h3>
                                     <div id="auto-profiles-buttons" class="flex gap-md table-actions">
                                          <button class="btn btn-ghost btn-md flex items-center gap-xs"
                                              disabled=move || recalculating.get()
                                              on:click=move |_| {
                                          set_recalculating.set(true);
                                          spawn_local(async move {
                                              let result = crate::api::recalculate_automatic_scores().await;
                                              match result {
                                                  Ok(_) => {
                                                      toast::show_success("Scores recalculated");
                                                      // Bust the stale cache so fetch_auto_data
                                                      // actually calls the API instead of
                                                      // returning cached empty data.
                                                      crate::utils::invalidate_cache_prefix("fetch_automatic_profiles");
                                                      fetch_auto_data();
                                                  }
                                                  Err(e) => {
                                                      toast::show_error(format!("Recalculation failed: {}", e));
                                                  }
                                              }
                                              set_recalculating.set(false);
                                          });
                                      }>
                                      <RefreshIcon />
                                      "Recalculate"
                                  </button>
                                  <button class="btn btn-primary btn-md flex items-center gap-xs" on:click=move |_| set_show_auto_config.set(true)>
                                      <SettingsIcon />
                                      "Configure"
                                  </button>
                                 <label class="flex items-center gap-xs cursor-pointer m-0" for="cb-auto-profiles">
                                     <input
                                         type="checkbox"
                                         id="cb-auto-profiles"
                                         prop:checked=Signal::derive(move || cfg.get().map(|c| c.general.automatic_profiles.enabled).unwrap_or(false))
                                         on:change=move |ev| {
                                             let is_checked = event_target_checked(&ev);
                                             if let Some(mut c) = cfg.get() {
                                                 c.general.automatic_profiles.enabled = is_checked;
                                                 set_cfg.set(Some(c.clone()));
                                                 let c_for_cache = c.clone();
                                                 let fetch_auto_data = fetch_auto_data.clone();
                                                 crate::utils::spawn_api_toast(
                                                         crate::api::save_config(c.clone()),
                                                         None,
                                                         move |_| {
                                                             crate::utils::write_cache("fetch_config", &c_for_cache);
                                                             toast::show_success("Automatic profiles setting saved");
                                                             // Refresh profiles to pick up any created by
                                                             // the backend's automatic recalculation on enable.
                                                             crate::utils::invalidate_cache_prefix("fetch_automatic_profiles");
                                                             fetch_auto_data();
                                                         }
                                                     );
                                             }
                                         }
                                     />
                                     "Enable Automatic Profiles"
                                 </label>
                             </div>
                        </div>

                        {
                            let columns: Vec<ManagedColumn<AutomaticProfile>> = vec![
                                ManagedColumn {
                                    id: "submitter".into(), label: "Submitter".into(), sortable: true, class: "".into(),
                                    cell_render: Callback::new(|p: AutomaticProfile| view! { <span>{p.submitter}</span> }.into_any())
                                },
                                ManagedColumn {
                                    id: "score".into(), label: "Score".into(), sortable: true, class: "".into(),
                                    cell_render: Callback::new(|p: AutomaticProfile| {
                                        view! { <span class=score_class(p.score)>{p.score.to_string()}</span> }.into_any()
                                    })
                                },
                                ManagedColumn {
                                    id: "actions".into(), label: "Actions".into(), sortable: false, class: "col-actions".into(),
                                    cell_render: Callback::new(move |p: AutomaticProfile| {
                                        let s = p.submitter.clone();
                                        let on_remove = on_remove_auto.clone();
                                        view! {
                                            <div class="action-cell">
                                                <button class="btn-danger btn-sm" on:click=move |e| {
                                                    e.stop_propagation();
                                                    on_remove.run(s.clone());
                                                }>"×"</button>
                                            </div>
                                        }.into_any()
                                    })
                                },
                            ];
                            view! {
                                {
                                    let (sc, sa, on_sort) = use_persistent_table_state("automatic_profiles".to_string(), "submitter".to_string(), true);
                                    TableBuilder::new(sc, sa)
                                        .table_class("table hover w-full text-left unselectable")
                                        .loading(Signal::derive(move || recalculating.get()))
                                        .empty_message("No automatic profiles defined.")
                                        .on_sort(on_sort)
                                        .build_managed(
                                            auto_profiles.into(),
                                            columns,
                                            Callback::new(|(a, b, col): (AutomaticProfile, AutomaticProfile, String)| {
                                                match col.as_str() {
                                                    "submitter" => jumbie_shared::formatting::natural_cmp(&a.submitter, &b.submitter),
                                                    "score" => a.score.cmp(&b.score),
                                                    _ => std::cmp::Ordering::Equal,
                                                }
                                            }),
                                            Some(Callback::new(move |p: AutomaticProfile| {
                                                let submitter = p.submitter.clone();
                                                selected_auto_profile.set(Some(submitter.clone()));
                                                spawn_local(async move {
                                                    if let Ok(r) = crate::api::fetch_automatic_profile_records(submitter).await {
                                                        set_auto_records.try_update(|rec| *rec = r);
                                                    } else {
                                                        set_auto_records.try_update(|rec| *rec = Vec::new());
                                                    }
                                                });
                                            })),
                                            None::<Callback<AutomaticProfile, bool>>,
                                            Some(Callback::new(move |p: AutomaticProfile| {
                                                let s = p.submitter.clone();
                                                let s_for_click = p.submitter.clone();
                                                view! {
                                                    <div class="card p-md flex justify-between items-center" on:click=move |_| {
                                                         let s = s_for_click.clone();
                                                         selected_auto_profile.set(Some(s.clone()));
                                                         spawn_local(async move {
                                                             if let Ok(r) = crate::api::fetch_automatic_profile_records(s).await {
                                                                     set_auto_records.try_update(|rec| *rec = r);
                                                                 } else {
                                                                     set_auto_records.try_update(|rec| *rec = Vec::new());
                                                                 }
                                                         });
                                                    }>
                                                        <div class="flex flex-col gap-xs min-w-0">
                                                            <span class="font-bold truncate">{p.submitter.clone()}</span>
                                                            <span class=move || format!("text-sm text-muted truncate {}", score_class(p.score))>"Score: " {p.score}</span>
                                                        </div>
                                                        <button class="btn-danger btn-sm" on:click=move |e| {
                                                            e.stop_propagation();
                                                            on_remove_auto.run(s.clone());
                                                         }>"×"</button>
                                                    </div>
                                                }.into_any()
                                            })),
                                        )
                                }
                            }.into_any()
                        }
                    </div>
                </div>
                    }
                }
            </div>

            <StandardModal
                show=Signal::derive(move || selected_profile.get().is_some())
                on_close=move |_| selected_profile.set(None)
                title="Edit Release Profile"
                size="modal-lg"
                footer={
                    let on_save = Callback::new(move |_| {
                        if let Some(key) = selected_profile.get() {
                            let new_name = draft_name.get();
                            if new_name.trim().is_empty() {
                                toast::show_toast("Name cannot be empty", toast::NotificationType::Error);
                                return;
                            }
                            if let Err(e) = crate::validation::validate_name(&new_name, "Release profile name", 100) {
                                toast::show_toast(&e, toast::NotificationType::Error);
                                return;
                            }

                            let mut new_terms = HashMap::new();
                            let mut new_regex = HashMap::new();
                            let term_flags = draft_term_flags.get();
                            for (id, k, score) in draft_terms.get() {
                                if k.trim().is_empty() { continue; }
                                if term_flags.get(&id).copied().unwrap_or(false) {
                                    new_regex.insert(k.clone(), score);
                                } else {
                                    new_terms.insert(k.clone(), score);
                                }
                            }

                            let mut new_submitters = HashMap::new();
                            for (_, k, score) in draft_submitters.get() {
                                let submitter_key = if k.trim().is_empty() { "Unknown".to_string() } else { k.clone() };
                                new_submitters.insert(submitter_key, score);
                            }

                            set_release_profiles.update(|c| {
                                    if key == "new" {
                                        c.insert(config::generate_uuid(), ReleaseProfile {
                                            name: new_name,
                                            case_insensitive_terms: draft_case_insensitive_terms.get(),
                                            case_insensitive_submitters: draft_case_insensitive_submitters.get(),
                                            min_score: draft_min_score.get(),
                                            size_score_per_gb: draft_size_score.get(),
                                            peers_score_per_peer: draft_peers_score.get(),
                                            age_score_per_day: draft_age_score.get(),
                                            terms: new_terms,
                                            regex_terms: new_regex,
                                            submitters: new_submitters,
                                            ..Default::default()
                                        });
                                    } else if let Some(p) = c.get_mut(&key) {
                                        p.name = new_name;
                                        p.case_insensitive_terms = draft_case_insensitive_terms.get();
                                        p.case_insensitive_submitters = draft_case_insensitive_submitters.get();
                                        p.min_score = draft_min_score.get();

                                        p.size_score_per_gb = draft_size_score.get();
                                        p.peers_score_per_peer = draft_peers_score.get();
                                        p.age_score_per_day = draft_age_score.get();

                                        p.terms = new_terms;
                                        p.regex_terms = new_regex;
                                        p.submitters = new_submitters;
                                    }
                            });

                            let c_save = release_profiles.get();
                            let c_for_cache = c_save.clone();
                            crate::utils::spawn_api_toast(
                                async move { crate::api::save_release_profiles(&c_save).await },
                                None,
                                move |_| {
                                    crate::utils::write_cache("fetch_release_profiles", &c_for_cache);
                                }
                            );
                            selected_profile.set(None);
                        }
                    });
                    modal_footer_save(on_save, Signal::derive(|| false)).into_any()
                }
            >
                {move || {
                    view! {
                        <TextInput
                            label="Profile Name".to_string()
                            id="releaseProfileName".to_string()
                            value=draft_name
                            set_value=move |v| set_draft_name.set(v)
                        />
                        <NumberInput
                            label="Minimum Score".to_string()
                            id="minimumScore".to_string()
                            mode=NumberInputMode::SignedInteger
                            value=Signal::derive(move || draft_min_score.get().to_string())
                            set_value=Callback::new(move |v: String| {
                                                            if let Ok(val) = v.parse::<i32>() {
                                    set_draft_min_score.set(val);
                                }
                            })
                        />

                        <div class="mt-lg">
                            <h3 class="mb-sm block">"Metadata Scoring"</h3>

                            <div class="form-group mb-md flex gap-sm items-center">
                                <span class="flex-none col-score">"Score per GB"</span>
                                <input
                                    type="number" id="scorePerGb" class="form-control flex-1" placeholder="10"
                                    prop:value=move || draft_size_score.get().to_string()
                                    on:input=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse() { set_draft_size_score.set(v); }
                                    }
                                />
                            </div>

                            <div class="form-group mb-md flex gap-sm items-center">
                                <span class="flex-none col-score">"Score per Seeder"</span>
                                <input
                                    type="number" id="scorePerSeeder" class="form-control flex-1" placeholder="2"
                                    prop:value=move || draft_peers_score.get().to_string()
                                    on:input=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse() { set_draft_peers_score.set(v); }
                                    }
                                />
                            </div>

                            <div class="form-group mb-md flex gap-sm items-center">
                                <span class="flex-none col-score">"Score per Day Age"</span>
                                <input
                                    type="number" id="scorePerDayAge" class="form-control flex-1" placeholder="-5 for modifier"
                                    prop:value=move || draft_age_score.get().to_string()
                                    on:input=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse() { set_draft_age_score.set(v); }
                                    }
                                />
                            </div>
                        </div>





                        <RankingList
                            title="Submitter Rankings"
                            list_id="releaseProfileSubmitters"
                            placeholder="Submitter Group (Blank = Unknown)"
                            items=draft_submitters
                            set_items=set_draft_submitters
                            next_id=draft_submitter_next_id
                            set_next_id=set_draft_submitter_next_id
                            case_insensitive=draft_case_insensitive_submitters
                            set_case_insensitive=set_draft_case_insensitive_submitters
                        />

                        <RankingList
                            title="Terms & Regex Rankings"
                            list_id="releaseProfileTerms"
                            placeholder="Keyword or Regex"
                            items=draft_terms
                            set_items=set_draft_terms
                            next_id=draft_next_id
                            set_next_id=set_draft_next_id
                            case_insensitive=draft_case_insensitive_terms
                            set_case_insensitive=set_draft_case_insensitive_terms
                            extra=std::sync::Arc::new(move |row_id| {
                                view! {
                                    <label class="flex gap-xs items-center m-0 text-sm">
                                        <input
                                            type="checkbox"
                                            id=format!("regex-{}", row_id)
                                            prop:checked=move || draft_term_flags.get().get(&row_id).copied().unwrap_or(false)
                                            on:change=move |ev| {
                                                let regex_checked = event_target_checked(&ev);
                                                set_draft_term_flags.update(|f| {
                                                    f.insert(row_id, regex_checked);
                                                });
                                            }
                                        /> "Regex"
                                    </label>
                                }.into_any()
                            })
                        />
                    }
                }}
            </StandardModal>

            <StandardModal
                show=Signal::derive(move || selected_auto_profile.get().is_some())
                on_close=move |_| selected_auto_profile.set(None)
                title=Signal::derive(move || format!("Records for {}", selected_auto_profile.get().unwrap_or_default()))
                subtitle=Signal::derive(move || {
                    selected_auto_profile.get().and_then(|sub| {
                        auto_profiles.get().iter().find(|p| p.submitter == sub).map(|p| p.media_scan_count)
                    }).map(|count| {
                        format!("{} file{} analyzed", count, if count == 1 { "" } else { "s" })
                    }).unwrap_or_default()
                })
                size="modal-xl"
                body_class="p-0"
            >
                {
                    let (sc, sa, on_sort) = use_persistent_table_state(
                        "auto_records".to_string(),
                        "date_added".to_string(),
                        false,
                    );
                    let columns = vec![
                        ManagedColumn {
                            id: "category".into(),
                            label: "Category".into(),
                            sortable: true,
                            class: "".into(),
                            cell_render: Callback::new(
                                move |o: jumbie_shared::types::AutomaticProfileRecord| {
                                    view! {
                                        <span class="badge badge-secondary text-xs">
                                            {o.category.clone()}
                                        </span>
                                    }
                                    .into_any()
                                },
                            ),
                        },
                        ManagedColumn {
                            id: "date_added".into(),
                            label: "Date Added".into(),
                            sortable: true,
                            class: "".into(),
                            cell_render: Callback::new(
                                move |o: jumbie_shared::types::AutomaticProfileRecord| {
                                    view! {
                                        <span class="text-xs text-secondary-color">
                                            {format_datetime_local(&o.date_added, &time_format.get())}
                                        </span>
                                    }
                                    .into_any()
                                },
                            ),
                        },
                        ManagedColumn {
                            id: "description".into(),
                            label: "Description".into(),
                            sortable: true,
                            class: "".into(),
                            cell_render: Callback::new(
                                move |o: jumbie_shared::types::AutomaticProfileRecord| {
                                    view! { <span>{o.description.clone()}</span> }.into_any()
                                },
                            ),
                        },
                        ManagedColumn {
                            id: "score".into(),
                            label: "Score".into(),
                            sortable: true,
                            class: "".into(),
                            cell_render: Callback::new(
                                move |o: jumbie_shared::types::AutomaticProfileRecord| {
                                    let cls = score_class(o.score);
                                    view! {
                                        <span class=format!("{} font-bold", cls)>
                                            {o.score.to_string()}
                                        </span>
                                    }
                                    .into_any()
                                },
                            ),
                        },
                    ];
                    TableBuilder::new(sc, sa)
                        .table_class("table hover w-full text-left unselectable")
                        .empty_message("No records found.")
                        .on_sort(on_sort)
                        .build_managed(
                            auto_records.into(),
                            columns,
                            Callback::new(
                                |(
                                    a,
                                    b,
                                    col,
                                ): (
                                    jumbie_shared::types::AutomaticProfileRecord,
                                    jumbie_shared::types::AutomaticProfileRecord,
                                    String,
                                )| {
                                    match col.as_str() {
                                        "category" => jumbie_shared::formatting::natural_cmp(&a.category, &b.category),
                                        "date_added" => jumbie_shared::formatting::natural_cmp(&a.date_added, &b.date_added),
                                        "description" => jumbie_shared::formatting::natural_cmp(&a.description, &b.description),
                                        "score" => a.score.cmp(&b.score),
                                        _ => std::cmp::Ordering::Equal,
                                    }
                                },
                            ),
                            None::<Callback<jumbie_shared::types::AutomaticProfileRecord>>,
                            None::<Callback<jumbie_shared::types::AutomaticProfileRecord, bool>>,
                            None::<Callback<jumbie_shared::types::AutomaticProfileRecord, AnyView>>,
                        )
                }
            </StandardModal>

            <AutoProfilesConfigModal
                show=Signal::derive(move || show_auto_config.get())
                cfg=cfg
                set_cfg=set_cfg
                health=health
                on_close=Callback::new(move |_| set_show_auto_config.set(false))
            />
        </div>
    }
}
