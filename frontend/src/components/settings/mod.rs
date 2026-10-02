pub mod general;
pub mod organization;
pub mod time_input;

pub use general::GeneralSettings;
pub use organization::OrganizationSettings;

use crate::components::common::landing::NavLanding;
use crate::routes::{find_group, path};
use leptos::prelude::*;

#[component]
pub fn SettingsLanding() -> impl IntoView {
    let parent = find_group(path::SETTINGS).unwrap();
    view! { <NavLanding parent=parent /> }
}
