// Router structure: one flat `ParentRoute` (path="") that hosts the auth gate
// + layout shell, with each child route mapping 1:1 to a URL path and component.
//
// Most routes wrap their view in a `move || view! { ... }` closure. Leptos
// evaluates the `view=` property at route *definition* time, so the closure defers
// component construction (and its signal setup) until the route is matched — the
// project's lazy-loading strategy, keeping the initial render lean.
//
// leptos_router 0.8 removed ProtectedRoute; the auth gate now lives in AppLayout.

use crate::components::authentication::{
    AccountSettings, ApiSettings, AuthenticationLanding, SecuritySettings,
};
use crate::components::calendar::Calendar;
use crate::components::common::app_layout::AppLayout;
use crate::components::common::error_page::ErrorPage;
use crate::components::common::form_fields::test_page::FormFieldsTestPage;
use crate::components::common::toast::{ToastContainer, provide_notification_context};
use crate::components::edit_series::EditSeries;
use crate::components::management::{
    SystemDownloadQueue, SystemOrganizedSeries, SystemRenameQueue,
};
use crate::components::plugins::{
    DownloadClientSettings, MetadataSettings, NotifierSettings, PluginsLanding, SourcesSettings,
};
use crate::components::profiles::{ProfilesLanding, QualityProfileEditor, ReleaseProfileEditor};
use crate::components::series_library::AddSeries;
use crate::components::series_library::SeriesLibrary;
use crate::components::settings::{GeneralSettings, OrganizationSettings, SettingsLanding};
use crate::components::system::activity::Activity;
use crate::components::system::{
    ManagementLanding, SystemAbout, SystemLanding, SystemLogs, SystemStatus,
};
use crate::components::wanted::Wanted;
use crate::hooks::use_config::ConfigContext;
use crate::routes::path;
use leptos::prelude::*;
use leptos_router::components::*;

#[component]
pub fn App() -> impl IntoView {
    crate::hooks::performance::use_track_mount("App");
    let _ = crate::hooks::error_handler::use_error_handler();

    // Config is fetched asynchronously, so the signal starts as None; consumers
    // handle the None case with a loading state or defaults.
    let (config, set_config) = signal::<Option<jumbie_shared::config::Config>>(None);
    provide_context(ConfigContext { config, set_config });
    crate::hooks::use_ui_config::provide_ui_config_context();

    // Root-level hooks so their state is available to every child via context.
    // They live here rather than inside route views because the auth gate <Show>
    // wraps all routes, and hooks inside conditional branches may not run
    // consistently across navigations.
    crate::utils::episode_state::provide_episode_state();
    crate::hooks::use_theme();
    let auth = crate::hooks::use_auth();

    // Background services that run for the app's lifetime: use_preload eagerly
    // caches data for the default route; use_status_polling checks backend health
    // periodically. Both are gated on auth to avoid wasted requests before login.
    crate::hooks::use_preload(auth.is_authenticated);
    crate::hooks::use_status_polling(auth.is_authenticated);

    provide_notification_context();

    view! {
        <Router>
            <div class="app-container">
                <ToastContainer/>
                <Routes fallback=|| view! {
                    <div class="p-lg text-center">
                        <h1>404 - Not Found</h1>
                        <p>The page you requested was not found.</p>
                    </div>
                }>
                    // Route path literals MUST match `crate::routes::path`. The
                    // leptos_router::path!() macro requires a compile-time string
                    // literal, so these literals are the duplication point — update
                    // the matching constant in routes.rs when changing a route.
                    // Inline comments below show the matching constant.
                    <ParentRoute path=leptos_router::path!("") view=move || view! { <AppLayout/> }>
                        // path::SERIES
                        <Route path=leptos_router::path!("/") view=|| view! { <Redirect path={format!("/{}", path::SERIES)}/> }/>
                        <Route path=leptos_router::path!("/series") view=move || view! { <SeriesLibrary/> }/>
                        // path::SERIES_ADD
                        <Route path=leptos_router::path!("/series/add") view=move || view! { <AddSeries/> }/>
                        // Dynamic route — no constant (uses /series/:id/edit pattern)
                        <Route path=leptos_router::path!("/series/:id/edit") view=move || view! { <EditSeries/> }/>
                        // path::CALENDAR
                        <Route path=leptos_router::path!("/calendar") view=Calendar/>
                        // path::WANTED
                        <Route path=leptos_router::path!("/wanted") view=Wanted/>

                        // Settings (path::SETTINGS_*)
                        <Route path=leptos_router::path!("/settings") view=SettingsLanding/>
                        <Route path=leptos_router::path!("/settings/general") view=move || view! { <GeneralSettings/> }/>
                        <Route path=leptos_router::path!("/settings/organization") view=move || view! { <OrganizationSettings/> }/>

                        // Plugins (path::PLUGINS_*)
                        <Route path=leptos_router::path!("/plugins") view=PluginsLanding/>
                        <Route path=leptos_router::path!("/plugins/sources") view=move || view! { <SourcesSettings/> }/>
                        <Route path=leptos_router::path!("/plugins/clients") view=move || view! { <DownloadClientSettings/> }/>
                        <Route path=leptos_router::path!("/plugins/notifiers") view=move || view! { <NotifierSettings/> }/>
                        <Route path=leptos_router::path!("/plugins/metadata") view=MetadataSettings/>

                        // Authentication (path::AUTH_*)
                        <Route path=leptos_router::path!("/authentication") view=AuthenticationLanding/>
                        <Route path=leptos_router::path!("/authentication/account") view=move || view! { <AccountSettings/> }/>
                        <Route path=leptos_router::path!("/authentication/security") view=move || view! { <SecuritySettings/> }/>
                        <Route path=leptos_router::path!("/authentication/api") view=move || view! { <ApiSettings/> }/>

                        // path::ACTIVITY
                        <Route path=leptos_router::path!("/activity") view=Activity/>

                        // Profiles (path::PROFILES_*)
                        <Route path=leptos_router::path!("/profiles") view=ProfilesLanding/>
                        <Route path=leptos_router::path!("/profiles/release") view=ReleaseProfileEditor/>
                        <Route path=leptos_router::path!("/profiles/quality") view=QualityProfileEditor/>

                        // Management (path::MANAGEMENT_*)
                        <Route path=leptos_router::path!("/management") view=ManagementLanding/>
                        <Route path=leptos_router::path!("/management/rename") view=SystemRenameQueue/>
                        <Route path=leptos_router::path!("/management/download") view=SystemDownloadQueue/>
                        <Route path=leptos_router::path!("/management/organized") view=SystemOrganizedSeries/>

                        // System (path::SYSTEM_*)
                        <Route path=leptos_router::path!("/system") view=SystemLanding/>
                        <Route path=leptos_router::path!("/system/status") view=SystemStatus/>
                        <Route path=leptos_router::path!("/system/logs") view=SystemLogs/>
                        <Route path=leptos_router::path!("/system/about") view=SystemAbout/>

                        // Test / development routes (not in sidebar); MUST be before
                        // the catch-all /*any route.
                        <Route path=leptos_router::path!("/test/form-fields") view=move || view! { <FormFieldsTestPage/> }/>

                        <Route path=leptos_router::path!("/*any") view=move || view! {
                            <ErrorPage
                                title="Page Not Found"
                                message="The page you are looking for does not exist or has been moved."
                                icon="🧭"
                                back_path="/"
                                back_label="Return Home"
                            />
                        }/>
                    </ParentRoute>
                </Routes>
            </div>
        </Router>
    }
}
