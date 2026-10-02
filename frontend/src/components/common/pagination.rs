use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use wasm_bindgen::JsCast;

static PAGINATION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Reactive handle to the app shell's pagination footer.
///
/// `AppLayout` provides this so `PaginationControl` can teleport its bar into
/// the bottom of `.main-content` (below the scrolling content) instead of
/// rendering it inside the page. When no context is provided, the control
/// falls back to rendering in place.
#[derive(Clone, Copy)]
pub struct PaginationFooterSlot(pub NodeRef<html::Div>);

#[component]
pub fn PaginationControl(
    #[prop(into)] page: Signal<i64>,
    #[prop(into)] total_pages: Signal<i64>,
    #[prop(into)] on_page_change: Callback<i64>,
    #[prop(optional)] id: Option<String>,
) -> impl IntoView {
    let effective_page = move || page.get().max(0).min((total_pages.get() - 1).max(0));
    let has_previous = move || effective_page() > 0;
    let has_next = move || effective_page() + 1 < total_pages.get();

    let (jump_value, set_jump_value) = signal(String::new());

    let execute_jump = move || {
        if let Ok(n) = jump_value.get().trim().parse::<i64>() {
            let zero_based = (n - 1).max(0);
            on_page_change.run(zero_based);
            set_jump_value.set(String::new());
        }
    };

    let on_jump = move |_: web_sys::MouseEvent| {
        execute_jump();
    };

    let on_jump_input = move |ev| {
        set_jump_value.set(event_target_value(&ev));
    };

    let on_jump_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" {
            execute_jump();
        }
    };

    let input_id = StoredValue::new(id.unwrap_or_else(|| {
        let n = PAGINATION_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("page-jump-field-{n}")
    }));

    let has_pages = move || total_pages.get() > 1;

    // The control itself, shared between the teleported (footer) and inline
    // (fallback) render paths.
    let controls = move || {
        view! {
            <Show when=has_pages fallback=|| view! {}>
                <div class="pagination-control">
                    <div class="pagination-info">
                        <span class="text-sm text-muted page-numbers">
                            "Page " {move || effective_page() + 1} " of " {move || total_pages.get()}
                        </span>

                        <div class="pagination-jump">
                            <input
                                type="text"
                                inputmode="numeric"
                                pattern="[0-9]*"
                                placeholder="Go to"
                                aria-label="Go to page"
                                class="form-input"
                                id=move || input_id.get_value()
                                prop:value=move || jump_value.get()
                                on:input=on_jump_input
                                on:keydown=on_jump_keydown
                            />
                            <button class="btn btn-secondary btn-md" on:click=on_jump type="button">
                                "Go"
                            </button>
                        </div>
                    </div>

                    <div class="row gap-sm pagination-btns">
                        <button
                            class="btn btn-secondary btn-md btn-pair btn-pair-l"
                            disabled=move || !has_previous()
                            on:click=move |_| if has_previous() { on_page_change.run(effective_page() - 1) }
                            type="button"
                        >
                            "Previous"
                        </button>
                        <button
                            class="btn btn-secondary btn-md btn-pair"
                            disabled=move || !has_next()
                            on:click=move |_| if has_next() { on_page_change.run(effective_page() + 1) }
                            type="button"
                        >
                            "Next"
                        </button>
                    </div>
                </div>
            </Show>
        }
    };

    // Teleport into the app shell's footer slot when one is provided. The slot's
    // NodeRef is only populated after it mounts, so gate the portal on it to
    // avoid falling back to `document.body`.
    if let Some(PaginationFooterSlot(slot)) = use_context::<PaginationFooterSlot>() {
        view! {
            <Show when=move || slot.get().is_some() fallback=|| ()>
                <Portal mount=slot
                    .get()
                    .map(|el| el.unchecked_into::<web_sys::Element>())
                    .expect("pagination footer slot is mounted")>
                    {controls()}
                </Portal>
            </Show>
        }
        .into_any()
    } else {
        controls().into_any()
    }
}
