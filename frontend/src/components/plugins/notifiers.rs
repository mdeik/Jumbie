use crate::components::plugins::utils::plugin_section;
use jumbie_shared::plugin::Capability;
use leptos::prelude::*;

#[component]
pub fn NotifierSettings() -> impl IntoView {
    plugin_section(
        "settings-notifiers",
        Capability::Notifier,
        "notifier",
        "Active Notifiers",
        "Configure notification services for events and alerts.",
        "notifier",
    )
}
