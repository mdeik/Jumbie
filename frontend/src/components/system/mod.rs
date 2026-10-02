pub mod about;
pub mod activity;
pub mod logs;
pub mod status;

pub use about::SystemAbout;
pub use activity::*;
pub use logs::SystemLogs;
pub use status::SystemStatus;

use crate::components::common::landing::NavLanding;
use crate::routes::{find_group, path};
use leptos::prelude::*;

#[component]
pub fn SystemLanding() -> impl IntoView {
    let parent = find_group(path::SYSTEM).unwrap();
    view! { <NavLanding parent=parent /> }
}

#[component]
pub fn ManagementLanding() -> impl IntoView {
    let parent = find_group(path::MANAGEMENT).unwrap();
    view! { <NavLanding parent=parent /> }
}
