pub mod download_client;
pub mod grid;
pub mod metadata;
pub mod notifiers;
pub mod sources;
pub mod utils;

pub use download_client::DownloadClientSettings;
pub use metadata::MetadataSettings;
pub use notifiers::NotifierSettings;
pub use sources::SourcesSettings;

use crate::components::common::landing::NavLanding;
use crate::routes::{find_group, path};
use leptos::prelude::*;

#[component]
pub fn PluginsLanding() -> impl IntoView {
    let parent = find_group(path::PLUGINS).unwrap();
    view! { <NavLanding parent=parent /> }
}
