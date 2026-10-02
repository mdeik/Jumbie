use crate::components::common::modal_wrapper::ModalWrapper;
use leptos::prelude::*;

/// A high-level standardized modal that handles commonly repeated logic like
/// close signals, default footer buttons, and standard formatting.
///
/// The two `ModalWrapper` branches below differ only in `footer=f`; the `view!`
/// macro can't pass `Option<AnyView>` to an `#[prop(optional)]`, so we branch.
/// Keep them visually aligned — shared props must be replicated in both.
#[component]
pub fn StandardModal(
    #[prop(into)] show: Signal<bool>,
    #[prop(into)] on_close: Callback<()>,
    #[prop(into)] title: Signal<String>,
    #[prop(into, optional)] subtitle: Signal<String>,
    #[prop(into, optional)] size: Signal<String>,
    #[prop(into, optional)] id: Signal<String>,
    #[prop(into, optional)] class: Signal<String>,
    #[prop(into, optional)] body_class: Signal<String>,
    #[prop(optional)] footer: Option<AnyView>,
    children: Children,
) -> impl IntoView {
    // Only pass `footer` when `Some` — the `view!` macro can't pass `Option<AnyView>` to an optional prop.
    if let Some(f) = footer {
        view! {
            <ModalWrapper
                show=show
                on_close=on_close
                title=title
                subtitle=subtitle
                size=size
                id=id
                class=class
                body_class=body_class
                footer=f
            >
                {children()}
            </ModalWrapper>
        }
        .into_any()
    } else {
        view! {
            <ModalWrapper
                show=show
                on_close=on_close
                title=title
                subtitle=subtitle
                size=size
                id=id
                class=class
                body_class=body_class
            >
                {children()}
            </ModalWrapper>
        }
        .into_any()
    }
}

/// A standardized footer for confirmation-style modals. When `on_cancel` is
/// omitted, only the confirm button is rendered and dismissal relies on the
/// modal's X button or clicking off.
#[component]
pub fn ModalConfirmationFooter(
    #[prop(optional, into)] on_cancel: Option<Callback<()>>,
    #[prop(into)] on_confirm: Callback<()>,
    #[prop(optional, into)] cancel_label: MaybeProp<String>,
    #[prop(optional, into)] confirm_label: MaybeProp<String>,
    #[prop(optional, into)] confirm_class: MaybeProp<String>,
) -> impl IntoView {
    let cancel_text = move || cancel_label.get().unwrap_or_else(|| "Cancel".to_string());
    let confirm_text = move || confirm_label.get().unwrap_or_else(|| "Confirm".to_string());
    let confirm_btn_class = move || {
        format!(
            "btn {}",
            confirm_class
                .get()
                .unwrap_or_else(|| "btn-primary".to_string())
        )
    };
    let cancel_button = on_cancel.map(|on_cancel| {
        view! {
            <button class="btn" on:click=move |_| on_cancel.run(()) type="button">
                {cancel_text}
            </button>
        }
    });

    view! {
        <div class="modal-footer flex items-center justify-end gap-md">
            {cancel_button}
            <button class=confirm_btn_class on:click=move |_| on_confirm.run(()) type="button">
                {confirm_text}
            </button>
        </div>
    }
}
