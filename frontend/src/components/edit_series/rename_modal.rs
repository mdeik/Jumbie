use crate::components::common::standard_modal::{ModalConfirmationFooter, StandardModal};
use leptos::prelude::*;

#[component]
pub fn RenameModal(
    #[prop(into)] show: Signal<bool>,
    set_show: WriteSignal<bool>,
    #[prop(into)] on_now: Callback<(), ()>,
    #[prop(into)] on_later: Callback<(), ()>,
    #[prop(optional, into)] message: Signal<Option<String>>,
) -> impl IntoView {
    view! {
        <StandardModal
            show=show
            on_close=move |_| set_show.set(false)
            title="Apply File Rename"
            footer=view! {
                <ModalConfirmationFooter
                    cancel_label="Later"
                    confirm_label="Now"
                    on_cancel=move |_| {
                        set_show.set(false);
                        on_later.run(());
                    }
                    on_confirm=move |_| {
                        set_show.set(false);
                        on_now.run(());
                    }
                />
            }.into_any()
        >
            <p>{move || message.get().unwrap_or_else(|| "The Absolute Numbering Mapping setting has been changed. Would you like to rename and reorganize the existing files for this series to use the new numbering scheme now, or apply it later?".to_string())}</p>
        </StandardModal>
    }
}
