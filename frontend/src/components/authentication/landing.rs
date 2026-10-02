use crate::components::common::landing::NavLanding;
use crate::routes::{find_group, path};
use leptos::prelude::*;

#[component]
pub fn AuthenticationLanding() -> impl IntoView {
    let parent = find_group(path::AUTH).unwrap();
    view! { <NavLanding parent=parent /> }
}
