use crate::components::common::SettingsPage;
use crate::routes::{NavGroup, path};
use leptos::prelude::*;
use leptos_router::hooks::*;

/// Generic landing page for any parent nav group.
/// Renders a status-card grid from the parent's children (single source of truth).
#[component]
pub fn NavLanding(parent: &'static NavGroup) -> impl IntoView {
    let navigate = use_navigate();

    // Setup caching for various endpoints that might be needed by child cards
    // Profiles
    let (quality_profiles, set_quality_profiles) = signal::<usize>(0);
    crate::utils::use_api_cache(
        "fetch_quality_profiles".to_string(),
        || crate::api::fetch_quality_profiles(),
        move |c| {
            let _ = set_quality_profiles.try_update(|s| *s = c.len());
        },
    );

    let (release_profiles, set_release_profiles) = signal::<usize>(0);
    crate::utils::use_api_cache(
        "fetch_release_profiles".to_string(),
        || crate::api::fetch_release_profiles(),
        move |c| {
            let _ = set_release_profiles.try_update(|s| *s = c.len());
        },
    );

    // Management
    let (rename_queue, set_rename_queue) = signal::<usize>(0);
    crate::utils::use_api_cache(
        "fetch_rename_queue".to_string(),
        || crate::api::fetch_rename_queue(),
        move |c| {
            let _ = set_rename_queue.try_update(|s| *s = c.total_affected_episodes);
        },
    );

    let (torrent_queue, set_torrent_queue) = signal::<usize>(0);
    crate::utils::use_api_cache(
        "fetch_download_queue".to_string(),
        || crate::api::fetch_download_queue(),
        move |c| {
            let _ = set_torrent_queue.try_update(|s| *s = c.len());
        },
    );

    let (organized, set_organized) = signal::<usize>(0);
    crate::utils::use_api_cache(
        "fetch_organized_series".to_string(),
        || crate::api::fetch_organized_series(),
        move |c| {
            let _ = set_organized.try_update(|s| *s = c.len());
        },
    );

    // System
    let (health, set_health) = signal(String::from("Unknown"));
    crate::utils::use_api_cache(
        "fetch_health".to_string(),
        || crate::api::fetch_health(),
        move |c| {
            let _ = set_health.try_update(|s| {
                *s = if c.status == "Healthy" {
                    "Healthy".to_string()
                } else {
                    "Issues Detected".to_string()
                }
            });
        },
    );

    let (_logs, set_logs) = signal::<usize>(0);
    crate::utils::use_api_cache(
        "fetch_logs".to_string(),
        || crate::api::fetch_logs(0, 100, None, None, None),
        move |c| {
            let _ = set_logs.try_update(|s| *s = c.total as usize);
        },
    );

    // Settings Plugins
    let (plugins_res, set_plugins_res) =
        signal(Vec::<jumbie_shared::plugin::PluginTypeListing>::new());
    crate::utils::use_api_cache(
        "fetch_available_plugins".to_string(),
        || crate::api::fetch_available_plugins(),
        move |c| {
            let _ = set_plugins_res.try_update(|s| *s = c);
        },
    );

    let get_plugin_names = move |capability: jumbie_shared::plugin::Capability,
                                 fallback: &'static str| {
        let plugins = plugins_res.get();
        let names: Vec<String> = plugins
            .iter()
            .filter(|p| p.capabilities.contains(&capability))
            .map(|p| p.display_name.clone())
            .collect();
        if names.is_empty() {
            fallback.to_string()
        } else {
            names.join(", ")
        }
    };

    view! {
        <SettingsPage id="landing">
            <div class="status-grid">
                {parent.children.iter().map(|child| {
                    let route = child.id;
                    let label = child.label;
                    let desc  = child.desc;
                    let navigate_inner = navigate.clone();

                    let dynamic_desc = Signal::derive(move || {
                        match route {
                            path::PROFILES_QUALITY => format!("{} Profiles", quality_profiles.get()),
                            path::PROFILES_RELEASE => format!("{} Release Profiles", release_profiles.get()),
                            path::MANAGEMENT_RENAME => format!("{} Pending", rename_queue.get()),
                            path::MANAGEMENT_DOWNLOAD => format!("{} Active", torrent_queue.get()),
                            path::MANAGEMENT_ORGANIZED => format!("{} Managed", organized.get()),
                            path::SYSTEM_STATUS => health.get(),
                            path::SYSTEM_LOGS => "Recent Logs".to_string(),
                            path::PLUGINS_SOURCES => get_plugin_names(jumbie_shared::plugin::Capability::FeedProvider, "Indexers & Feeds"),
                            path::PLUGINS_CLIENTS => get_plugin_names(jumbie_shared::plugin::Capability::Downloader, "qBittorrent, Transmission"),
                            path::PLUGINS_METADATA => get_plugin_names(jumbie_shared::plugin::Capability::MetadataProviderNormal, "None configured"),
                            path::PLUGINS_NOTIFIERS => get_plugin_names(jumbie_shared::plugin::Capability::Notifier, "Discord, Webhooks"),
                            _ => desc.to_string(),
                        }
                    });

                    view! {
                        <div
                            class="status-card clickable"
                            on:click=move |_| navigate_inner(&format!("/{}", route), Default::default())
                        >
                            <div class="label">{label}</div>
                            <h3 class="value text-base">{move || dynamic_desc.get()}</h3>
                        </div>
                    }
                }).collect_view()}
            </div>
        </SettingsPage>
    }
}
