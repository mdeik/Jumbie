use crate::components::common::standard_modal::{ModalConfirmationFooter, StandardModal};
use leptos::prelude::*;

#[component]
pub fn ConfirmationModal(
    #[prop(into)] show: Signal<bool>,
    set_show: WriteSignal<bool>,
    title: &'static str,
    /// Primary message text. When omitted, consumers should pass the message
    /// content via the `children` slot for custom styling.
    #[prop(optional)]
    #[prop(into)]
    message: Option<Signal<String>>,
    on_confirm: Callback<()>,
    /// Optional extra content rendered below the message (e.g. a checkbox).
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    // derive a signal to ensure Copy semantics for closures
    let message_sig = message.map(|m| Memo::new(move |_| m.get()));

    view! {
        <StandardModal
            show=show
            on_close=move |_| set_show.set(false)
            title=title.to_string()
            size="modal-sm"
            footer=view! {
                <ModalConfirmationFooter
                    on_confirm=move |_| {
                        on_confirm.run(());
                        set_show.set(false);
                    }
                    confirm_class="btn-danger"
                />
            }.into_any()
        >
            {message_sig.map(|m| view! { <p>{move || m.get()}</p> })}
            {children.map(|c| c())}
        </StandardModal>
    }
}
