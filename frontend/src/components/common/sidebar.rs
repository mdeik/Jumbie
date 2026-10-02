use crate::components::common::icons::*;
use crate::routes::{NAV_GROUPS, NAV_STANDALONE, nav_icon};
use crate::utils::indicators::{IndicatorLevels, StateSnapshot, compute_indicator_levels};
use gloo_timers::callback::Timeout;
use leptos::prelude::*;
use leptos_router::hooks::*;
use std::cell::RefCell;
use std::rc::Rc;

// Indicator priority (lowest → highest): Warning (yellow dot, `should_warn`,
// `text-warning`), Danger (red dot, `should_danger`, `text-danger`), Critical
// (red pulsing, `should_critical`, `sys-breathing-danger`). Only the highest
// active level is ever `true` — `compute_indicator_levels` enforces this.

/// Leptos-specific wrapper: a reactive Memo over the pure StateSnapshot.
type IndicatorState = Memo<StateSnapshot>;

fn item_indicator_levels(state: IndicatorState, id: &'static str) -> impl Fn() -> IndicatorLevels {
    let state = state.clone();
    move || compute_indicator_levels(id, state.get())
}

type NavGroupItem = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
);

#[component]
pub fn Sidebar() -> impl IntoView {
    let navigate = use_navigate();
    let location = use_location();

    let layout_ctx = use_context::<crate::hooks::LayoutContext>().expect("LayoutContext missing");
    let sidebar_open = layout_ctx.sidebar_open;
    let set_sidebar_open = layout_ctx.set_sidebar_open;

    // Single reactive source for the active route. Every nav item reads this Memo
    // instead of calling `location.pathname.get()` independently, collapsing ~220
    // signal subscriptions and String allocations per navigation into one node.
    let active_path =
        Memo::new(move |_| location.pathname.get().trim_start_matches('/').to_string());

    // Indicator context signals, read once here and captured by value in closures
    // so nav items don't each walk the context tree (`use_context` is O(n)).
    let rename_populated = use_context::<crate::hooks::RenameQueuePopulated>().map(|c| c.0);
    let rename_failed = use_context::<crate::hooks::RenameQueueFailed>().map(|c| c.0);
    let downloader_disabled = use_context::<crate::hooks::DownloaderDisabled>().map(|c| c.0);
    let download_queue_failed = use_context::<crate::hooks::DownloadQueueFailed>().map(|c| c.0);
    let download_queue_populated =
        use_context::<crate::hooks::DownloadQueuePopulated>().map(|c| c.0);
    let wanted_has_items = use_context::<crate::hooks::WantedHasItems>().map(|c| c.0);

    // One Memo combining all indicator signals so nav items share a single
    // reactive root. `StateSnapshot` is Copy, so cloning the Memo is a cheap Rc bump.
    let indicator_state = Memo::new(move |_| StateSnapshot {
        rename_populated: rename_populated.map(|s| s.get()).unwrap_or(false),
        rename_failed: rename_failed.map(|s| s.get()).unwrap_or(false),
        downloader_disabled: downloader_disabled.map(|s| s.get()).unwrap_or(false),
        download_queue_failed: download_queue_failed.map(|s| s.get()).unwrap_or(false),
        download_queue_populated: download_queue_populated.map(|s| s.get()).unwrap_or(false),
        wanted_has_items: wanted_has_items.map(|s| s.get()).unwrap_or(false),
    });

    // Preloading is a reactive side-effect of the current route, not an imperative
    // call in the click handler: Effects run after the reactive update, so
    // navigation completes before any spawn_local is enqueued, and rapid clicks
    // batch into one run with the final route.
    //
    // Debounced preload: rapid navigation only preloads the final destination.
    // Rc<RefCell> because gloo's `Timeout` is !Clone and `on_cleanup` needs Send+Sync.
    let debounce_timeout: Rc<RefCell<Option<Timeout>>> = Rc::new(RefCell::new(None));

    Effect::new(move |_| {
        let path = active_path.get();
        if !path.is_empty() {
            if let Some(t) = debounce_timeout.borrow_mut().take() {
                t.cancel();
            }
            let group = path.split('/').next().unwrap_or("").to_string();
            let handle = Timeout::new(50, move || {
                crate::utils::preload_group_data(&group);
            });
            *debounce_timeout.borrow_mut() = Some(handle);
        }
    });

    let nav_item = {
        let navigate_base = navigate.clone();
        move |id: &'static str, label: &'static str, icon: AnyView, indent: bool| {
            let navigate_inner = navigate_base.clone();
            let is_active = move || active_path.get() == id;
            let levels = std::sync::Arc::new(item_indicator_levels(indicator_state.clone(), id));

            view! {
                <button
                    data-testid={format!("nav-item-{}", id)}
                    class="nav-item"
                    class:active=is_active
                    class:sub-item=indent
                    on:click={
                        let nav = navigate_inner.clone();
                        move |_| {
                            nav(&format!("/{}", id), Default::default());
                            set_sidebar_open.set(false);
                        }
                    }
                >
                    <span
                        class="nav-icon"
                        class:text-warning={
                            let l = levels.clone();
                            move || l().should_warn
                        }
                        class:text-danger={
                            let l = levels.clone();
                            move || l().should_danger
                        }
                        class:sys-breathing-danger={
                            let l = levels.clone();
                            move || l().should_critical
                        }
                    >{icon}</span>
                    {label}
                </button>
            }
        }
    };

    let nav_group_item = {
        let navigate_base = navigate.clone();
        move |id: &'static str,
              label: &'static str,
              icon: AnyView,
              children: Box<dyn Fn() -> AnyView + Send>| {
            let navigate_inner = navigate_base.clone();
            let is_active_tree = move || {
                let current = active_path.get();
                current == id || current.starts_with(&format!("{}/", id))
            };
            let is_self_active = move || active_path.get() == id;

            let indicator_state_clone = indicator_state.clone();
            let levels = std::sync::Arc::new(move || {
                if is_active_tree() {
                    return IndicatorLevels::default();
                }
                // Delegate to compute_indicator_levels so parent groups and leaves
                // share one implementation.
                compute_indicator_levels(id, indicator_state_clone.get())
            });

            view! {
                <div class="nav-tree">
                    <button
                        data-testid={format!("nav-group-{}", id)}
                        class="nav-item"
                        class:active=is_self_active
                        class:active-parent=is_active_tree
                        on:click={
                            let nav = navigate_inner.clone();
                            move |_| {
                                nav(&format!("/{}", id), Default::default());
                            }
                        }
                    >
                        <span
                            class="nav-icon"
                            class:text-warning={
                                let l = levels.clone();
                                move || l().should_warn
                            }
                            class:text-danger={
                                let l = levels.clone();
                                move || l().should_danger
                            }
                            class:sys-breathing-danger={
                                let l = levels.clone();
                                move || l().should_critical
                            }
                        >{icon}</span>
                        {label}
                        <span class="nav-chevron" class:expanded=is_active_tree></span>
                    </button>
                    <div class="nav-children" class:expanded=is_active_tree>
                        {move || if is_active_tree() {
                            children().into_any()
                        } else {
                            view! { <div></div> }.into_any()
                        }}
                    </div>
                </div>
            }
        }
    };

    let nav_item = std::sync::Arc::new(nav_item);

    // Extract static nav data for capture in closures.
    let standalone_items: Vec<(&'static str, &'static str)> = NAV_STANDALONE
        .iter()
        .filter(|item| item.show_in_sidebar)
        .map(|item| (item.id, item.label))
        .collect();

    let parent_items: Vec<NavGroupItem> = NAV_GROUPS
        .iter()
        .map(|g| {
            let children: Vec<(&'static str, &'static str)> =
                g.children.iter().map(|c| (c.id, c.label)).collect();
            (g.id, g.label, children)
        })
        .collect();

    view! {
        <aside class="sidebar" class:open=move || sidebar_open.get()>
            <div class="brand">
                <div class="brand-icon">"JB"</div>
                <div class="brand-text">"Jumbie"</div>
            </div>

            <nav class="nav-section">
                <div class="nav-group">
                    <div class="nav-label">"Main"</div>
                    {standalone_items
                        .into_iter()
                        .map(|(id, label)| {
                            let ni = nav_item.clone();
                            let icon: AnyView = nav_icon(id).into_any();
                            ni(id, label, icon, false)
                        })
                        .collect_view()}
                </div>

                <div class="nav-group">
                    <div class="nav-label">"Configuration"</div>
                    {parent_items
                        .into_iter()
                        .map(|(pid, plabel, children)| {
                            let ni = nav_item.clone();
                            let icon: AnyView = nav_icon(pid).into_any();
                            nav_group_item(
                                pid,
                                plabel,
                                icon,
                                Box::new(move || {
                                    children
                                        .iter()
                                        .map(|(cid, clabel)| {
                                            ni(cid, clabel, view! { <DotIcon/> }.into_any(), true)
                                        })
                                        .collect::<Vec<_>>()
                                        .into_any()
                                }),
                            )
                        })
                        .collect_view()}
                </div>
            </nav>
        </aside>
    }
}
