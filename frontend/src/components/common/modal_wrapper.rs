use crate::components::common::icons::XIcon;
use leptos::ev;
use leptos::html;
use leptos::prelude::*;

/// Detent fractions (share of `innerHeight`) for the mobile bottom sheet.
///
/// The two detents are the only resting heights on mobile; `DETENT_HIGH_FRACTION`
/// is the cap (taller content scrolls internally). Dragging above the high detent
/// rubber-bands toward 100vh (`RUBBER_UP_DAMPING`); dragging below the low detent
/// rubber-bands until released far enough to dismiss (`DISMISS_PULL_PX`). These
/// constants are the only knobs for sheet feel.
const DETENT_LOW_FRACTION: f64 = 0.65;
const DETENT_HIGH_FRACTION: f64 = 0.90;
const RUBBER_UP_DAMPING: f64 = 0.25;
const RUBBER_DOWN_DAMPING: f64 = 0.50;
const DISMISS_PULL_PX: f64 = 80.0;

/// Viewport heights below this (px) get a single detent: the low detent is
/// collapsed into the high one, because a 65% peek wastes too much vertical
/// space on short screens (small phones, split-screen, browser chrome).
const SINGLE_DETENT_BELOW_VIEWPORT_PX: f64 = 700.0;

/// Drag-dismiss safety net: if the `.modal-exiting` animation never fires
/// `animationend` (e.g. CSS is disabled), close anyway. This is deliberately
/// NOT synced to the CSS duration — exit timing lives in `frontend/style.css`.
const EXIT_SAFETY_NET_MS: u64 = 1000;

/// How long after the last window `resize` event the sheet keeps its transition
/// suppressed. Resizing emits events every frame; leaving the 0.5s height
/// transition live would restart it on every event, so the sheet visibly trails
/// the viewport. Re-arming this timer on each event keeps the sheet glued to the
/// viewport while it moves and restores smooth animation once it settles.
const RESIZE_SETTLE_MS: u64 = 150;

/// Current viewport height in px. Uses `innerHeight` (not CSS `vh`) so the detents
/// track the visible viewport, including mobile URL-bar show/hide.
fn viewport_height_px() -> f64 {
    window()
        .inner_height()
        .ok()
        .and_then(|h| h.as_f64())
        .unwrap_or(1000.0)
}

/// Effective resting detent fractions (low, high) for the given viewport
/// height. On short viewports the low fraction equals the high one, so the
/// only resting height is `DETENT_HIGH_FRACTION`.
fn detent_fractions(vh: f64) -> (f64, f64) {
    if vh < SINGLE_DETENT_BELOW_VIEWPORT_PX {
        (DETENT_HIGH_FRACTION, DETENT_HIGH_FRACTION)
    } else {
        (DETENT_LOW_FRACTION, DETENT_HIGH_FRACTION)
    }
}

/// The two sheet detents (low, high) in px for the given viewport height.
/// Takes `vh` explicitly so callers can read `innerHeight` once per event
/// (drag moves fire at high frequency).
fn detents_at(vh: f64) -> (f64, f64) {
    let (f_low, f_high) = detent_fractions(vh);
    (vh * f_low, vh * f_high)
}

/// Standard Save footer for modals. Dismissal is handled by the modal's X button
/// or clicking off, so no Cancel button is rendered.
pub fn modal_footer_save(on_save: Callback<()>, save_disabled: Signal<bool>) -> impl IntoView {
    view! {
        <div class="modal-footer flex items-center justify-end gap-md">
            <button
                class="btn btn-primary"
                disabled=move || save_disabled.get()
                on:click=move |_| on_save.run(())
                type="button"
            >"Save"</button>
        </div>
    }
}

#[component]
pub fn ModalWrapper(
    #[prop(into)] show: Signal<bool>,
    #[prop(into)] on_close: Callback<()>,
    #[prop(into)] title: Signal<String>,
    #[prop(into, optional)] subtitle: Signal<String>,
    #[prop(into, optional)] id: Signal<String>,
    #[prop(into, optional)] size: Signal<String>,
    #[prop(into, optional)] class: Signal<String>,
    #[prop(into, optional)] body_class: Signal<String>,
    #[prop(optional)] footer: Option<AnyView>,
    children: Children,
) -> impl IntoView {
    let should_close = StoredValue::new_local(false);
    let initial_children = children();
    let initial_footer = footer;

    // Draggable Sheet Logic — iOS-style bottom sheet with one or two detents
    // (90vh always; 65vh additionally on tall viewports). On mobile the sheet
    // height is always explicit (px); `None` means `auto` (desktop, or when
    // closed). The sheet always opens at the low detent — the only detent on
    // short viewports — and taller content scrolls internally; dragging follows
    // the finger, rubber-bands past the detents, and snaps to the nearest
    // detent on release.
    let (sheet_height, set_sheet_height) = signal(None::<f64>);

    let (is_dragging, set_is_dragging) = signal(false);
    let start_pointer_y = StoredValue::new_local(0.0);
    let start_height = StoredValue::new_local(0.0);

    // Effective detent fraction the sheet is currently resting at. Tracked as a
    // fraction (not px) so a viewport resize can re-apply the same detent at
    // the new viewport height.
    let resting_fraction = StoredValue::new_local(None::<f64>);

    // Rest at `fraction`: normalize it to the effective detents for the current
    // viewport (on short screens a LOW request becomes the HIGH detent), record
    // it, and apply it as px against the viewport height. The only writer of a
    // resting height, so the px always stays in lockstep with the recorded
    // fraction.
    let snap_to = move |fraction: f64| {
        let vh = viewport_height_px();
        let (f_low, f_high) = detent_fractions(vh);
        let fraction = fraction.clamp(f_low, f_high);
        *resting_fraction.write_value() = Some(fraction);
        set_sheet_height.set(Some(vh * fraction));
    };

    // Drag-to-dismiss exit: play the slide-down (reverse of the open slide-up)
    // while keeping the overlay mounted, then run `on_close` once the CSS
    // animation finishes. `is_exiting` gates the CSS class; the close is driven
    // by the container's `animationend` (see the view), with a safety-net
    // timeout in case the animation never runs (see `EXIT_SAFETY_NET_MS`).
    let (is_exiting, set_is_exiting) = signal(false);
    let exit_timer = StoredValue::new_local(None::<TimeoutHandle>);

    let cancel_exit_timer = move || {
        if let Some(h) = exit_timer.write_value().take() {
            h.clear();
        }
    };

    let finish_exit = move || {
        cancel_exit_timer();
        set_is_exiting.set(false);
        on_close.run(());
    };

    // Animated close (drag past the dismiss threshold).
    let start_exit = move || {
        if !is_exiting.get() {
            set_is_exiting.set(true);
            if let Ok(h) = set_timeout_with_handle(
                finish_exit,
                std::time::Duration::from_millis(EXIT_SAFETY_NET_MS),
            ) {
                *exit_timer.write_value() = Some(h);
            }
        }
    };

    // Instant close (X, ESC, overlay click): cancels any pending animated
    // close so `on_close` never fires twice.
    let close_now = move || {
        cancel_exit_timer();
        set_is_exiting.set(false);
        on_close.run(());
    };

    // While the viewport is actively resizing, the sheet's height must track it
    // instantly; the 0.5s height transition is for open/snap/dismiss, not for
    // window resizes. `is_resizing` suppresses it until `RESIZE_SETTLE_MS` after
    // the last resize event.
    let (is_resizing, set_is_resizing) = signal(false);
    let resize_timer = StoredValue::new_local(None::<TimeoutHandle>);

    let cancel_resize_settle = move || {
        if let Some(h) = resize_timer.write_value().take() {
            h.clear();
        }
    };

    let mark_resizing = move || {
        cancel_resize_settle();
        set_is_resizing.set(true);
        if let Ok(h) = set_timeout_with_handle(
            move || set_is_resizing.set(false),
            std::time::Duration::from_millis(RESIZE_SETTLE_MS),
        ) {
            *resize_timer.write_value() = Some(h);
        }
    };

    let container_ref = NodeRef::<html::Div>::new();

    let is_mobile = move || {
        window()
            .inner_width()
            .ok()
            .and_then(|w| w.as_f64())
            .map(|w| w <= crate::hooks::use_media_query::MOBILE_BREAKPOINT_PX)
            .unwrap_or(false)
    };

    Effect::new(move |_| {
        if show.get() && is_mobile() {
            // Open resting at the low detent; taller content scrolls internally
            // until the user drags up to the high detent.
            snap_to(DETENT_LOW_FRACTION);

            // Pointer Move: follow the finger, rubber-band past the detents.
            let move_handle =
                window_event_listener(ev::pointermove, move |ev: ev::PointerEvent| {
                    if is_dragging.get() {
                        let delta_y = ev.client_y() as f64 - *start_pointer_y.read_value();
                        let target = *start_height.read_value() - delta_y;
                        // One viewport read per event; drag moves fire at high
                        // frequency.
                        let vh = viewport_height_px();
                        let (n_low, n_high) = detents_at(vh);
                        let h = if target > n_high {
                            // Above the top detent: resistance toward 100vh.
                            n_high + (target - n_high) * RUBBER_UP_DAMPING
                        } else if target < n_low {
                            // Below the bottom detent: resistance (dismissal on release).
                            n_low - (n_low - target) * RUBBER_DOWN_DAMPING
                        } else {
                            target
                        };
                        set_sheet_height.set(Some(h.clamp(0.0, vh)));
                    }
                });

            // Pointer Up: snap to the nearest detent, or dismiss if pulled far
            // past the bottom.
            let up_handle = window_event_listener(ev::pointerup, move |_| {
                if is_dragging.get() {
                    set_is_dragging.set(false);
                    let vh = viewport_height_px();
                    let (n_low, n_high) = detents_at(vh);
                    if let Some(h) = sheet_height.get() {
                        if h < n_low - DISMISS_PULL_PX {
                            // Slide the sheet back down (reverse of the open
                            // slide-up), then close once it's off screen.
                            start_exit();
                        } else {
                            let mid = (n_low + n_high) / 2.0;
                            let snapped_fraction = if h >= mid {
                                DETENT_HIGH_FRACTION
                            } else {
                                DETENT_LOW_FRACTION
                            };
                            snap_to(snapped_fraction);
                        }
                    }
                }
            });

            // Resize (URL-bar show/hide, orientation): keep the resting detent
            // re-applied at the new viewport height, or revert to auto when
            // rotated to desktop width.
            let resize_handle = window_event_listener(ev::resize, move |_| {
                // Track the viewport instantly; animate again once it settles.
                mark_resizing();
                if !is_mobile() {
                    if is_exiting.get() {
                        // Rotated to desktop width mid-exit: the slide-down
                        // animation no longer applies, so close right away.
                        close_now();
                    } else {
                        set_sheet_height.set(None);
                    }
                } else if !is_dragging.get() && !is_exiting.get() {
                    // Keep the resting detent (defaulting to low) at the new
                    // viewport height. `get_value` copies the fraction out and
                    // releases its read lock; a `read_value` guard would still
                    // be held when `snap_to` takes the write lock on the same
                    // value, and the resulting panic aborts (and leaks the lock)
                    // on wasm.
                    snap_to(resting_fraction.get_value().unwrap_or(DETENT_LOW_FRACTION));
                }
            });

            let esc_handle = window_event_listener(ev::keydown, move |ev| {
                if ev.key() == "Escape" {
                    close_now();
                }
            });

            on_cleanup(move || {
                move_handle.remove();
                up_handle.remove();
                esc_handle.remove();
                resize_handle.remove();
                cancel_resize_settle();
            });
        } else {
            set_sheet_height.set(None);
            set_is_dragging.set(false);
            *resting_fraction.write_value() = None;
            cancel_exit_timer();
            set_is_exiting.set(false);
            cancel_resize_settle();
            set_is_resizing.set(false);
        }
    });

    view! {
        <div
            class=move || {
                let mut cls =
                    if show.get() { "modal-overlay active" } else { "modal-overlay" }.to_string();
                if is_exiting.get() {
                    cls.push_str(" modal-exiting");
                }
                cls
            }
            on:mousedown=move |_| *should_close.write_value() = true
            on:click=move |_| {
                if *should_close.read_value() {
                    close_now();
                }
                *should_close.write_value() = false;
            }
        >
            <div
                node_ref=container_ref
                id=id.clone()
                class={
                    let size = size.clone();
                    let class = class.clone();
                    move || format!("modal-container {} {} {} {}",
                        size.get(),
                        class.get(),
                        if is_dragging.get() { "is-dragging" } else { "" },
                        if is_exiting.get() { "modal-exiting" } else { "" }
                    )
                }
                style:height={move || {
                    sheet_height.get().map(|px| format!("{}px", px)).unwrap_or_else(|| "auto".to_string())
                }}
                style:transition={move ||
                    if is_dragging.get() || is_resizing.get() {
                        "none".to_string()
                    } else {
                        "height 0.5s cubic-bezier(0.32, 0.72, 0, 1)".to_string()
                    }
                }
                // Matches the `.modal-container.modal-exiting` animation in style.css.
                on:animationend=move |ev: ev::AnimationEvent| {
                    if is_exiting.get() && ev.animation_name() == "sheetSlideOut" {
                        finish_exit();
                    }
                }
                on:click=move |e| e.stop_propagation()
                on:mousedown=move |e| e.stop_propagation()
            >
                <div
                    class="modal-header"
                    on:pointerdown=move |ev| {
                        if is_mobile()
                            && !is_exiting.get()
                            && let Some(el) = container_ref.get() {
                                use wasm_bindgen::JsCast;
                                let height = sheet_height.get().or_else(|| {
                                    let rect = el.unchecked_ref::<web_sys::Element>().get_bounding_client_rect();
                                    (rect.height() > 0.0).then_some(rect.height())
                                });
                                if let Some(height) = height {
                                    *start_height.write_value() = height;
                                    *start_pointer_y.write_value() = ev.client_y() as f64;
                                    set_is_dragging.set(true);
                                }
                            }
                    }
                >
                    <div class="modal-title-container">
                        <div class="modal-title font-semibold" title=move || title.get()>
                            {title}
                        </div>
                        <Show when=move || !subtitle.get().is_empty() fallback=|| ()>
                            <div class="modal-subtitle">
                                {subtitle}
                            </div>
                        </Show>
                    </div>
                    <button class="btn btn-ghost btn-icon" on:click=move |_| close_now() type="button" aria-label="Close" title="Close">
                        <span class="icon text-xl"><XIcon /></span>
                    </button>
                </div>
                <div class={
                    let body_class = body_class.clone();
                    move || format!("modal-body {}", body_class.get())
                }>
                    {initial_children}
                </div>
                {match initial_footer {
                    Some(f) => view! { <div class="modal-footer-wrapper">{f}</div> }.into_any(),
                    None => ().into_any()
                }}
            </div>
        </div>
    }
}
