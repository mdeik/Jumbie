use crate::components::plugins::utils::plugin_section;
use jumbie_shared::plugin::Capability;
use leptos::prelude::*;

#[component]
pub fn DownloadClientSettings() -> impl IntoView {
    plugin_section(
        "settings-client",
        Capability::Downloader,
        "downloader",
        "Active Clients",
        "Configure download clients for fetching and managing media files.",
        "downloader",
    )
}
