use crate::components::plugins::grid::{PluginConfigWrapper, PluginDisplayItem, PluginGrid};
use jumbie_shared::config::PluginsConfig;
use jumbie_shared::plugin::{Capability, plugin_type_key};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::utils::write_cache;
use std::collections::HashMap;
use std::sync::Arc; // For sharing the editor closure if needed

/// Get the plugin map for a config section; SSoT for section-name lookups into
/// `PluginsConfig`.
pub fn get_plugin_section_map<'a>(
    cfg: &'a PluginsConfig,
    section: &str,
) -> Option<&'a HashMap<String, HashMap<String, serde_json::Value>>> {
    match section {
        "downloader" => Some(&cfg.downloader),
        "notifier" => Some(&cfg.notifier),
        "source" => Some(&cfg.source),
        "metadata" => Some(&cfg.metadata),
        _ => None,
    }
}

/// Mutable version of `get_plugin_section_map` for in-place updates.
pub fn get_plugin_section_map_mut<'a>(
    cfg: &'a mut PluginsConfig,
    section: &str,
) -> Option<&'a mut HashMap<String, HashMap<String, serde_json::Value>>> {
    match section {
        "downloader" => Some(&mut cfg.downloader),
        "notifier" => Some(&mut cfg.notifier),
        "source" => Some(&mut cfg.source),
        "metadata" => Some(&mut cfg.metadata),
        _ => None,
    }
}

#[component]
pub fn PluginSettings<IV>(
    capability: Capability,
    // Config section key in PluginsConfig (e.g. "downloader", "notifier", "source", "metadata")
    config_section: &'static str,
    #[prop(default = "".to_string())] header: String,
    #[prop(default = "".to_string())] description: String,
    // Factory for default settings for new plugins
    default_config: Arc<dyn Fn(String) -> serde_json::Value + Send + Sync>,
    // Optional Test Handler (plugin_type, current_value)
    #[prop(optional)] test_handler: Option<Arc<dyn Fn(String, serde_json::Value) + Send + Sync>>,
    // Render function for the editor form: (plugin_type, current_value, update_callback)
    editor: Arc<dyn Fn(String, serde_json::Value, Callback<serde_json::Value>) -> IV + Send + Sync>,
) -> impl IntoView
where
    IV: IntoView + 'static,
{
    // Seed signals from cache synchronously (instant render when preloaded)
    let (plugins_cfg, set_plugins_cfg) = signal(crate::utils::read_cache::<
        jumbie_shared::config::PluginsConfig,
    >("fetch_plugins_cfg"));

    // Background refresh with dedup if cache is stale or missing.
    // The preload system populates this cache on app start, so in practice
    // the signal is already populated and no fetch occurs on mount.
    crate::utils::spawn_cached_with(
        "fetch_plugins_cfg".to_string(),
        || crate::api::fetch_plugins_cfg(),
        move |cfg| {
            set_plugins_cfg.set(Some(cfg));
        },
    );

    // Fetch available plugins to check supports_test per plugin
    let (available_plugins, set_available_plugins) = signal(
        crate::utils::read_cache::<Vec<jumbie_shared::plugin::PluginTypeListing>>(
            "fetch_available_plugins",
        )
        .unwrap_or_default(),
    );

    crate::utils::spawn_cached_with(
        "fetch_available_plugins".to_string(),
        || crate::api::fetch_available_plugins(),
        move |plugins| {
            set_available_plugins.set(plugins);
        },
    );

    let save_section = config_section;
    let save_action = Action::new_local(move |cfg: &jumbie_shared::config::PluginsConfig| {
        let cfg = cfg.clone();
        let section = save_section;
        async move {
            let section_data = get_plugin_section_map(&cfg, section)
                .cloned()
                .unwrap_or_default();
            match crate::api::save_plugins_section(section, &section_data).await {
                Ok(resp) => (Ok(resp.warnings), Some(resp.config)),
                Err(e) => (Err(e.to_string()), None),
            }
        }
    });

    Effect::new(move |_| {
        if let Some((res, real_cfg)) = save_action.value().get() {
            match res {
                Ok(warnings) => {
                    if warnings.is_empty() {
                        crate::components::common::toast::show_success("Plugin settings saved.");
                    } else {
                        for warning in warnings {
                            crate::components::common::toast::show_warning(warning);
                        }
                    }
                }
                Err(e) => crate::components::common::toast::show_error(format!(
                    "Failed to save plugin settings: {}",
                    e
                )),
            }
            if let Some(ref real_cfg) = real_cfg {
                set_plugins_cfg.try_update(|c| *c = Some(real_cfg.clone()));
                write_cache("fetch_plugins_cfg", real_cfg);
            }
        }
    });

    let trigger_save = move || {
        if let Some(cfg) = plugins_cfg.get() {
            save_action.dispatch(cfg);
        }
    };
    let (editing_id, set_editing_id) = signal(None::<String>); // (plugin_type:instance_id)

    let display_items = Memo::new(move |_| {
        let mut items = Vec::new();
        if let Some(c) = plugins_cfg.get()
            && let Some(map) = get_plugin_section_map(&c, config_section)
        {
            for (plugin_type, instances) in map {
                for (id, value) in instances {
                    let config_name = value
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    let enabled = value
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    // Derive short type for display — the full plugin_id is
                    // preserved in the `id` field for edit lookups.
                    let short_name = plugin_type_key(&plugin_type);
                    items.push(PluginDisplayItem {
                        id: format!("{}:{}", plugin_type, id),
                        name: config_name,
                        plugin_id: short_name.to_string(),
                        enabled,
                    });
                }
            }
        }
        items.sort_by(|a, b| {
            jumbie_shared::formatting::natural_cmp(&a.name, &b.name)
                .then_with(|| jumbie_shared::formatting::natural_cmp(&a.id, &b.id))
        });
        items
    });

    let on_add = Callback::new(move |plugin_type: String| {
        // Use a sentinel value — the backend will generate the real instance ID
        // when the user clicks Save on a new (non-existing) instance.
        set_editing_id.set(Some(format!("{}:__new__", plugin_type)));
    });

    let on_edit = Callback::new(move |id: String| {
        set_editing_id.set(Some(id));
    });

    let active_plugin_data = move || {
        editing_id.get().and_then(|composite_id| {
            let parts: Vec<&str> = composite_id.splitn(2, ':').collect();
            if parts.len() != 2 {
                return None;
            }
            let p_type = parts[0];
            let p_id = parts[1];

            if let Some(c) = plugins_cfg.get()
                && let Some(map) = get_plugin_section_map(&c, config_section)
                && let Some(instances) = map.get(p_type)
                && let Some(val) = instances.get(p_id)
            {
                return Some((p_type.to_string(), p_id.to_string(), val.clone(), true));
            }

            let default = default_config(p_type.to_string());
            Some((p_type.to_string(), p_id.to_string(), default, false))
        })
    };

    let delete_plugin = move || {
        if let Some(composite_id) = editing_id.get() {
            let parts: Vec<&str> = composite_id.splitn(2, ':').collect();
            if parts.len() == 2 {
                let p_type = parts[0].to_string();
                let p_id = parts[1].to_string();
                let section = config_section;
                let set_plugins_cfg = set_plugins_cfg.clone();
                let set_editing_id = set_editing_id.clone();

                spawn_local(async move {
                    match crate::api::delete_plugin_instance(section, &p_type, &p_id).await {
                        Ok(full_cfg) => {
                            set_plugins_cfg.set(Some(full_cfg.clone()));
                            write_cache("fetch_plugins_cfg", &full_cfg);
                            set_editing_id.set(None);
                            crate::components::common::toast::show_success(
                                "Plugin instance deleted.",
                            );
                        }
                        Err(e) => {
                            crate::components::common::toast::show_error(format!(
                                "Failed to delete plugin instance: {}",
                                e
                            ));
                        }
                    }
                });
            }
        }
    };

    let trigger_save_save = trigger_save.clone();
    let category_grid = capability.clone();

    view! {
        <div class="gap-0">
        {if !header.is_empty() {
            view! { <h3>{header.clone()}</h3> }.into_any()
        } else {
            view! { <></> }.into_any()
        }}

        {if !description.is_empty() {
            view! { <p class="settings-page-description">{description}</p> }.into_any()
        } else {
            view! { <></> }.into_any()
        }}

        <PluginGrid
            items=display_items.into()
            capability_filter=category_grid
            on_add=on_add
            on_edit=on_edit
        />

        {move || {
            let trigger_save = trigger_save_save.clone();
            let delete_plugin = delete_plugin.clone();
            let test_handler = test_handler.clone();
            let editor = editor.clone();

            active_plugin_data().map(move |(p_type, p_id, initial_val, is_existing)| {
                // SSoT: Derive short_type once ("qbittorrent" from "downloader.qbittorrent")
                // for display/editor/test purposes.  p_type (full plugin_id) is kept for
                // the save path where it serves as the PluginsConfig HashMap key.
                let short_name = plugin_type_key(&p_type).to_string();
                let p_type_clone_save = p_type.clone();
                let p_type_clone_test = short_name.clone();
                let p_id_clone = p_id.clone();

                let draft = StoredValue::new_local(initial_val.clone());

                let update_draft = Callback::new(move |new_val: serde_json::Value| {
                    *draft.write_value() = new_val;
                });

                let trigger_save = trigger_save.clone();
                let delete_plugin = delete_plugin.clone();
                let on_cancel = Callback::new(move |_| set_editing_id.set(None));

                let on_save = if is_existing {
                    // Existing instance: update local state and trigger full section save.
                    let trigger_save = trigger_save.clone();
                    let set_plugins_cfg = set_plugins_cfg.clone();
                    let p_type = p_type_clone_save.clone();
                    let p_id = p_id_clone.clone();
                    let section = config_section;
                    Callback::new(move |_| {
                        let new_val = (*draft.read_value()).clone();
                        let mut switched_off: Vec<String> = Vec::new();
                        set_plugins_cfg.update(|c| {
                            if let Some(c) = c {
                                if let Some(map) = get_plugin_section_map_mut(c, section) {
                                    let instances = map.entry(p_type.clone()).or_insert_with(HashMap::new);
                                    instances.insert(p_id.clone(), new_val.clone());
                                }
                                // Single-active metadata (SSoT: shared helper): make
                                // the outgoing payload valid so the UI and save agree.
                                if section == "metadata" {
                                    switched_off = crate::utils::plugins::enforce_single_metadata(
                                        c,
                                        Some((&p_type, &p_id)),
                                    );
                                }
                            }
                        });
                        if !switched_off.is_empty() {
                            crate::components::common::toast::show_info(format!(
                                "Only one metadata provider can be active — disabled: {}",
                                switched_off.join(", ")
                            ));
                        }
                        trigger_save();
                        set_editing_id.set(None);
                    })
                } else {
                    // New instance: the backend generates the ID atomically on save.
                    let set_plugins_cfg = set_plugins_cfg.clone();
                    let cfg_read = plugins_cfg;
                    let p_type = p_type_clone_save.clone();
                    let section = config_section;
                    Callback::new(move |_| {
                        let new_val = (*draft.read_value()).clone();
                        let sp = set_plugins_cfg.clone();
                        let sec = section;
                        let pt = p_type.clone();
                        spawn_local(async move {
                            // Snapshot before the request so we can explain which
                            // provider the backend switched off (single-active policy).
                            let before = cfg_read.get_untracked();
                            match crate::api::create_plugin_instance(sec, &pt, &new_val).await {
                                Ok(full_cfg) => {
                                    let switched_off = before
                                        .as_ref()
                                        .map(|b| {
                                            crate::utils::plugins::newly_disabled_metadata_providers(
                                                b, &full_cfg,
                                            )
                                        })
                                        .unwrap_or_default();
                                    crate::utils::write_cache("fetch_plugins_cfg", &full_cfg);
                                    sp.set(Some(full_cfg));
                                    if !switched_off.is_empty() {
                                        crate::components::common::toast::show_info(format!(
                                            "Only one metadata provider can be active — disabled: {}",
                                            switched_off.join(", ")
                                        ));
                                    }
                                }
                                Err(e) => {
                                    crate::components::common::toast::show_error(format!(
                                        "Failed to create plugin instance: {}",
                                        e
                                    ));
                                }
                            }
                        });
                        set_editing_id.set(None);
                    })
                };

                // Only show the test button if the plugin declares supports_test: true
                let on_test = test_handler.clone().and_then(move |handler| {
                    let p_type_full = p_type_clone_save.clone();
                    let supports = available_plugins.with(|plugins| {
                        plugins.iter().any(|p| {
                            p.supports_test
                                && p.plugin_id.as_deref() == Some(&p_type_full)
                        })
                    });
                    if !supports {
                        return None;
                    }
                    Some(Callback::new(move |_| {
                        let val = (*draft.read_value()).clone();
                        handler(p_type_clone_test.clone(), val);
                    }))
                });

                let on_delete = if is_existing {
                    Some(Callback::new(move |_| delete_plugin()))
                } else {
                    None
                };

                view! {
                    <PluginConfigWrapper
                        title=format!("Edit {}", short_name)
                        subtitle=Signal::derive(move || {
                            if is_existing {
                                format!("instance_id: {}", p_id.clone())
                            } else {
                                "New instance".to_string()
                            }
                        })
                        on_save=on_save
                        on_delete=on_delete
                        on_cancel=on_cancel
                        on_test=on_test
                    >
                        {(editor)(short_name, initial_val, update_draft)}
                    </PluginConfigWrapper>
                }
            })
        }}
        </div>
    }
}
