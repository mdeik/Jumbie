use crate::components::plugins::utils::plugin_section;
use jumbie_shared::plugin::Capability;
use leptos::prelude::*;

#[component]
pub fn SourcesSettings() -> impl IntoView {
    plugin_section(
        "settings-sources",
        Capability::FeedProvider,
        "source",
        "Active Sources",
        "Configure feed sources for tracking and discovering new series.",
        "source",
    )
}
