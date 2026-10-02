use crate::components::common::form_fields::TextInput;
use crate::components::common::icons::{
    ArrowRightIcon, ArrowUpIcon, CheckIcon, ChevronUpIcon, SquareIcon,
};
use crate::components::common::modal_wrapper::modal_footer_save;
use crate::components::common::standard_modal::StandardModal;
use crate::components::common::toast::NotificationType;
use crate::components::profiles::profile_list::ProfileList;
use crate::components::profiles::profile_utils::generate_unique_name;
use jumbie_shared::config::generate_uuid;
use jumbie_shared::types::{Quality, QualityProfile};
use leptos::prelude::*;
use std::collections::HashMap;

#[component]
pub fn QualityProfileEditor() -> impl IntoView {
    let (quality_profiles, set_quality_profiles) = signal(HashMap::<String, QualityProfile>::new());
    let (available_qualities, set_available_qualities) = signal(HashMap::<String, Quality>::new());
    let selected_profile = RwSignal::new(None::<String>);
    let selected_quality = RwSignal::new(None::<String>);

    crate::utils::use_api_cache(
        "fetch_quality_profiles".to_string(),
        || crate::api::fetch_quality_profiles(),
        move |c| {
            set_quality_profiles.try_update(|q| *q = c);
        },
    );
    crate::utils::use_api_cache(
        "fetch_qualities".to_string(),
        || crate::api::fetch_qualities(),
        move |c| {
            set_available_qualities.try_update(|q| *q = c);
        },
    );

    let get_profile_names = Signal::derive(move || {
        let mut pairs: Vec<(String, Vec<String>)> = quality_profiles
            .get()
            .iter()
            .map(|(k, q)| {
                let qual_count = q.qualities.len().to_string();
                (k.clone(), vec![q.name.clone(), qual_count])
            })
            .collect();
        pairs.sort_by(|a, b| a.1[0].cmp(&b.1[0]));
        pairs
    });

    let get_quality_names = Signal::derive(move || {
        let mut pairs: Vec<(String, Vec<String>)> = available_qualities
            .get()
            .iter()
            .map(|(k, q)| {
                let tags_display = if q.tags.is_empty() {
                    "None".to_string()
                } else {
                    q.tags.join(", ")
                };
                (k.clone(), vec![q.name.clone(), tags_display])
            })
            .collect();
        pairs.sort_by(|a, b| a.1[0].cmp(&b.1[0]));
        pairs
    });

    let (draft_name, set_draft_name) = signal(String::new());
    let (draft_qualities, set_draft_qualities) = signal(Vec::<String>::new());
    let (draft_upgrade_only, set_draft_upgrade_only) = signal(Vec::<String>::new());

    let (draft_q_name, set_draft_q_name) = signal(String::new());
    let (draft_q_tags, set_draft_q_tags) = signal(String::new());

    let on_profile_add = Callback::new(move |()| {
        let existing: std::collections::HashSet<String> = quality_profiles
            .get()
            .values()
            .map(|p| p.name.clone())
            .collect();
        let name = generate_unique_name(&existing, "New Quality Profile");

        set_draft_name.set(name);
        set_draft_qualities.set(Vec::new());
        set_draft_upgrade_only.set(Vec::new());

        selected_profile.set(Some("new".to_string()));
    });

    let on_profile_remove = Callback::new(move |id: String| {
        set_quality_profiles.update(|c| {
            c.remove(&id);
        });
        let c_save = quality_profiles.get();
        let c_for_cache = c_save.clone();
        crate::utils::spawn_api_toast(
            async move { crate::api::save_quality_profiles(&c_save).await },
            None,
            move |_| {
                crate::utils::write_cache("fetch_quality_profiles", &c_for_cache);
            },
        );
    });

    let on_profile_select = Callback::new(move |id: String| {
        selected_profile.set(Some(id));
    });

    Effect::new(move |_| {
        if let Some(key) = selected_profile.get()
            && let Some(p) = quality_profiles.get().get(&key).cloned()
        {
            set_draft_name.set(p.name.clone());
            set_draft_qualities.set(p.qualities.clone());
            set_draft_upgrade_only.set(
                p.upgrade_only_qualities
                    .iter()
                    .filter(|id| p.qualities.contains(id))
                    .cloned()
                    .collect(),
            );
        }
    });

    let on_quality_add = Callback::new(move |()| {
        let existing: std::collections::HashSet<String> = available_qualities
            .get()
            .values()
            .map(|q| q.name.clone())
            .collect();
        let name = generate_unique_name(&existing, "New Quality");

        set_draft_q_name.set(name);
        set_draft_q_tags.set(String::new());
        selected_quality.set(Some("new".to_string()));
    });

    let on_quality_remove = Callback::new(move |id: String| {
        set_available_qualities.update(|c| {
            c.remove(&id);
        });
        let c_save = available_qualities.get();
        let c_for_cache = c_save.clone();
        crate::utils::spawn_api_toast(
            async move { crate::api::save_qualities(&c_save).await },
            None,
            move |_| {
                crate::utils::write_cache("fetch_qualities", &c_for_cache);
            },
        );
    });

    let on_quality_select = Callback::new(move |id: String| {
        selected_quality.set(Some(id));
    });

    Effect::new(move |_| {
        if let Some(key) = selected_quality.get()
            && let Some(q) = available_qualities.get().get(&key).cloned()
        {
            set_draft_q_name.set(q.name.clone());
            set_draft_q_tags.set(q.tags.join(", "));
        }
    });

    view! {
        <div class="view active">
            <div class="settings-content">

                <section>
                    <ProfileList
                        title="Qualities".to_string()
                        add_label="Add Quality".to_string()
                        columns=vec!["Name", "Tags"]
                        items=get_quality_names
                        on_add=on_quality_add
                        on_remove=on_quality_remove
                        on_select=on_quality_select
                        persistence_id="qualities".to_string()
                    />
                </section>

                <section>
                    <ProfileList
                        title="Quality Profiles".to_string()
                        add_label="Add Profile".to_string()
                        columns=vec!["Name", "Qualities Count"]
                        items=get_profile_names
                        on_add=on_profile_add
                        on_remove=on_profile_remove
                        on_select=on_profile_select
                        persistence_id="quality_profiles".to_string()
                        detail_formatter=|cells: &[String]| {
                            let count = cells.get(1).map(|s| s.as_str()).unwrap_or("0");
                            format!("{} qualities", count)
                        }
                    />
                </section>
            </div>

            <StandardModal
                show=Signal::derive(move || selected_quality.get().is_some())
                on_close=move |_| selected_quality.set(None)
                title="Edit Quality"
                size="modal-md"
                footer={
                    let on_save = Callback::new(move |_| {
                        if let Some(key) = selected_quality.get() {
                            let new_name = draft_q_name.get();
                            if new_name.trim().is_empty() {
                                crate::components::common::toast::show_toast("Name cannot be empty", crate::components::common::toast::NotificationType::Error);
                                return;
                            }
                            if let Err(e) = crate::validation::validate_name(&new_name, "Quality name", 100) {
                                crate::components::common::toast::show_toast(&e, crate::components::common::toast::NotificationType::Error);
                                return;
                            }

                            let tags_str = draft_q_tags.get();
                            let tags: Vec<String> = tags_str.split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();

                            for tag in &tags {
                                if let Err(e) = crate::validation::validate_tag(tag) {
                                    crate::components::common::toast::show_toast(&e, crate::components::common::toast::NotificationType::Error);
                                    return;
                                }
                            }

                            set_available_qualities.update(|c| {
                                    if key == "new" {
                                        c.insert(generate_uuid(), Quality {
                                            name: new_name,
                                            tags,
                                        });
                                    } else if let Some(q) = c.get_mut(&key) {
                                        q.name = new_name;
                                        q.tags = tags;
                                    }
                            });

                            let c_save = available_qualities.get();
                            let c_for_cache = c_save.clone();
                            crate::utils::spawn_api_toast(
                                async move { crate::api::save_qualities(&c_save).await },
                                None,
                                move |_| {
                                    crate::utils::write_cache("fetch_qualities", &c_for_cache);
                                }
                            );
                            selected_quality.set(None);
                        }
                    });
                    modal_footer_save(on_save, Signal::derive(|| false)).into_any()
                }
            >
                <TextInput
                    label="Name".to_string()
                    id="qualityName".to_string()
                    value=draft_q_name
                    set_value=move |v| set_draft_q_name.set(v)
                />
                <TextInput
                    label="Tags (Comma Separated)".to_string()
                    id="qualityTags".to_string()
                    value=draft_q_tags
                    set_value=move |v| set_draft_q_tags.set(v)
                    help_text="Tags are case-insensitive".to_string()
                />
            </StandardModal>

            <StandardModal
                show=Signal::derive(move || selected_profile.get().is_some())
                on_close=move |_| selected_profile.set(None)
                title="Edit Quality Profile"
                size="modal-md"
                footer={
                    let on_save = Callback::new(move |_| {
                        if let Some(key) = selected_profile.get() {
                            let new_name = draft_name.get();
                            if new_name.trim().is_empty() {
                                crate::components::common::toast::show_toast("Name cannot be empty", crate::components::common::toast::NotificationType::Error);
                                return;
                            }
                            if let Err(e) = crate::validation::validate_name(&new_name, "Quality profile name", 100) {
                                crate::components::common::toast::show_toast(&e, crate::components::common::toast::NotificationType::Error);
                                return;
                            }
                            let qualities = draft_qualities.get();
                            let upgrade_only = draft_upgrade_only.get();

                            set_quality_profiles.update(|c| {
                                    if key == "new" {
                                        c.insert(generate_uuid(), QualityProfile {
                                            name: new_name,
                                            qualities,
                                            upgrade_only_qualities: upgrade_only,
                                        });
                                    } else if let Some(p) = c.get_mut(&key) {
                                        p.name = new_name;
                                        p.qualities = qualities;
                                        p.upgrade_only_qualities = upgrade_only;
                                    }
                            });

                            let c_save = quality_profiles.get();
                            let c_for_cache = c_save.clone();
                            crate::utils::spawn_api_toast_with_level(
                                async move { crate::api::save_quality_profiles(&c_save).await },
                                None,
                                move |_| {
                                    crate::utils::write_cache("fetch_quality_profiles", &c_for_cache);
                                },
                                NotificationType::Warning,
                            );
                            selected_profile.set(None);
                        }
                    });
                    modal_footer_save(on_save, Signal::derive(move || draft_qualities.get().is_empty())).into_any()
                }
            >
                {move || {
                    let mut pairs: Vec<(String, String)> = available_qualities.get().iter().map(|(k, q)| (k.clone(), q.name.clone())).collect();
                    pairs.sort_by(|a, b| a.1.cmp(&b.1));
                    let available_qualities_display = pairs;

                    view! {
                        <TextInput
                            label="Profile Name".to_string()
                            id="qualityProfileName".to_string()
                            value=draft_name
                            set_value=move |v| set_draft_name.set(v)
                        />
                        <div class="form-group mt-md">
                            <h3>"Enabled Qualities"</h3>
                            <p class="text-muted text-sm mb-sm flex items-center gap-xs flex-wrap">
                                "Click to cycle: "
                                <span class="quality-state-off flex items-center gap-xs"><SquareIcon />"off"</span>
                                <ArrowRightIcon />
                                <span class="quality-state-normal flex items-center gap-xs"><CheckIcon />"normal"</span>
                                <ArrowRightIcon />
                                <span class="quality-state-upgrade flex items-center gap-xs"><ChevronUpIcon />"upgrade target"</span>
                            </p>
                            <p class="text-muted text-xs mb-sm">
                                "Normal qualities work as usual. When any upgrade targets are set, "
                                "only those qualities will trigger upgrades for the series."
                            </p>
                            <div class="quality-checkboxes grid grid-cols-2 mt-md gap-sm">
                                {available_qualities_display.into_iter().map(move |(q_id, q_name)| {
                                    let id_for_class = q_id.clone();
                                    let id_for_click = q_id.clone();
                                    let id_for_text = q_id;
                                    let name_for_text = q_name;
                                    view! {
                                        <button
                                            type="button"
                                            class=move || {
                                                let qs = draft_qualities.get();
                                                let uo = draft_upgrade_only.get();
                                                if !qs.contains(&id_for_class) {
                                                    "btn quality-btn quality-btn-off"
                                                } else if uo.contains(&id_for_class) {
                                                    "btn quality-btn quality-btn-upgrade"
                                                } else {
                                                    "btn quality-btn quality-btn-on"
                                                }
                                            }
                                            on:click=move |_| {
                                                let qs = draft_qualities.get_untracked();
                                                let uo = draft_upgrade_only.get_untracked();
                                                let in_qs = qs.contains(&id_for_click);
                                                let in_uo = uo.contains(&id_for_click);
                                                if !in_qs {
                                                    // off \u{2192} on
                                                    set_draft_qualities.update(|qs| qs.push(id_for_click.clone()));
                                                    set_draft_upgrade_only.update(|uo| uo.retain(|q| q != &id_for_click));
                                                } else if !in_uo {
                                                    // on \u{2192} upgrade-only
                                                    set_draft_upgrade_only.update(|uo| uo.push(id_for_click.clone()));
                                                } else {
                                                    // upgrade-only \u{2192} off
                                                    set_draft_qualities.update(|qs| qs.retain(|q| q != &id_for_click));
                                                    set_draft_upgrade_only.update(|uo| uo.retain(|q| q != &id_for_click));
                                                }
                                            }
                                        >
                                            {move || {
                                                let qs = draft_qualities.get();
                                                let uo = draft_upgrade_only.get();
                                                if !qs.contains(&id_for_text) {
                                                    view! { <span class="flex items-center gap-xs"><SquareIcon /> {name_for_text.clone()}</span> }.into_any()
                                                } else if uo.contains(&id_for_text) {
                                                    view! { <span class="flex items-center gap-xs"><ArrowUpIcon /> {name_for_text.clone()}</span> }.into_any()
                                                } else {
                                                    view! { <span class="flex items-center gap-xs"><CheckIcon /> {name_for_text.clone()}</span> }.into_any()
                                                }
                                            }}
                                        </button>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        </div>
                    }
                }}
            </StandardModal>
        </div>
    }
}
