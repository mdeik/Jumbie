use crate::components::authentication::login::Login;
use crate::components::common::header::Header;
use crate::components::common::pagination::PaginationFooterSlot;
use crate::components::common::sidebar::Sidebar;
use crate::hooks::use_auth::AuthContext;
use crate::hooks::use_config::ConfigContext;
use crate::hooks::use_layout;
use leptos::prelude::*;
use leptos_router::components::Outlet;

#[component]
pub fn AppLayout() -> impl IntoView {
    let auth = use_context::<AuthContext>().expect("AuthContext missing");
    let config = use_context::<ConfigContext>().expect("ConfigContext missing");
    let handle_login_success =
        use_context::<Callback<()>>().expect("Login Success Callback missing");

    let layout = use_layout();
    let sidebar_open = layout.sidebar_open;
    let set_sidebar_open = layout.set_sidebar_open;

    // Pages teleport their pagination control into this footer (below the
    // scrolling content) so the bar stays at the bottom of main-content.
    let pagination_footer = NodeRef::<leptos::html::Div>::new();
    provide_context(PaginationFooterSlot(pagination_footer));

    let show_login = Memo::new(move |_| {
        if auth.is_authenticated.get() {
            return false;
        }
        if auth.force_login.get() {
            return true;
        }
        config.config.with(|c| match c {
            Some(cfg) => cfg
                .auth
                .password
                .as_deref()
                .map(|p| !p.is_empty())
                .unwrap_or(false),
            // Config not loaded yet: assume auth disabled to avoid a flash of
            // login on fresh loads; auth-on is confirmed via force_login once
            // config resolves.
            None => false,
        })
    });

    view! {
        <Show
            when=move || !show_login.get()
            fallback=move || {
                view! { <Login on_success=handle_login_success.clone()/> }
            }
        >
            <Sidebar/>
            <main class="main-content">
                <Header/>
                <div class="content">
                    <div class="max-w-content">
                        <Outlet/>
                    </div>
                </div>
                <div class="pagination-footer-slot" node_ref=pagination_footer></div>
                <Show when=move || sidebar_open.get()>
                    <div
                        class="sidebar-overlay"
                        on:click=move |_| set_sidebar_open.set(false)
                    ></div>
                </Show>
            </main>
        </Show>
    }
}
