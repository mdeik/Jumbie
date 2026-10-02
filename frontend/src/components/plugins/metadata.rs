use crate::components::plugins::utils::plugin_section;
use jumbie_shared::plugin::Capability;
use leptos::prelude::*;

#[component]
pub fn MetadataSettings() -> impl IntoView {
    plugin_section(
        "settings-metadata",
        Capability::MetadataProviderNormal,
        "metadata",
        "Configurations",
        "Configure metadata providers for enriching series and episode information.",
        "metadata",
    )
}
