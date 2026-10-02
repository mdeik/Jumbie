use crate::components::common::landing::NavLanding;
use crate::routes::{find_group, path};
use leptos::prelude::*;

#[component]
pub fn ProfilesLanding() -> impl IntoView {
    let parent = find_group(path::PROFILES).unwrap();
    view! { <NavLanding parent=parent /> }
}
