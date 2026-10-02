use crate::api::generate_calendar_token;
use crate::components::common::form_fields::{CheckboxField, FormGroup, TextInput};
use crate::components::common::modal_wrapper::{ModalWrapper, modal_footer_save};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Shared modal for creating a calendar (iCal) link: owns the form state and
/// token-generation logic.
#[component]
pub fn CreateCalendarLinkModal(
    #[prop(into)] show: Signal<bool>,
    set_show: WriteSignal<bool>,
    /// Optional callback invoked after a successful token generation; receives
    /// the raw token string.
    #[prop(optional)]
    on_success: Option<Callback<String>>,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (hide_unmonitored, set_hide_unmonitored) = signal(false);
    let (show_as_all_day, set_show_as_all_day) = signal(false);

    let reset = move || {
        set_name.set(String::new());
        set_hide_unmonitored.set(false);
        set_show_as_all_day.set(false);
    };

    let handle_close = move |_| {
        set_show.set(false);
        reset();
    };

    let handle_generate = Callback::new(move |_: ()| {
        let link_name = name.get_untracked().trim().to_string();
        if link_name.is_empty() {
            crate::components::common::toast::show_error("Link name cannot be empty");
            return;
        }

        crate::debug_log!("CreateCalendarLinkModal: generating token '{}'", link_name);

        let hide_unmonitored = hide_unmonitored.get_untracked();
        let show_as_all_day = show_as_all_day.get_untracked();
        let set_show = set_show.clone();
        let on_success = on_success.clone();

        spawn_local(async move {
            match generate_calendar_token(link_name, hide_unmonitored, show_as_all_day).await {
                Ok(resp) => {
                    crate::debug_log!(
                        "CreateCalendarLinkModal: token created with prefix {}",
                        &resp.token[..resp.token.len().min(8)]
                    );

                    if let Some(window) = web_sys::window()
                        && let Ok(origin) = window.location().origin()
                    {
                        let url = crate::utils::build_calendar_link_url(&origin, &resp.token);
                        if crate::utils::write_clipboard(&url) {
                            crate::components::common::toast::show_success(
                                "Calendar link created and copied to clipboard!",
                            );
                        } else {
                            crate::components::common::toast::show_info(
                                "Calendar link created! Copy the URL from the 'Calendar Feeds' section in API Settings.",
                            );
                        }
                    }

                    set_show.set(false);
                    reset();

                    if let Some(ref cb) = on_success {
                        cb.run(resp.token);
                    }
                }
                Err(e) => {
                    crate::debug_error!(
                        "CreateCalendarLinkModal: failed to create calendar link: {}",
                        e
                    );
                    crate::components::common::toast::show_error(format!(
                        "Failed to create calendar link: {}",
                        e
                    ));
                }
            }
        });
    });

    view! {
        <ModalWrapper
            show=show
            on_close=handle_close
            title="Create Calendar Link"
            size="modal-sm"
            footer=(move || modal_footer_save(
                handle_generate.clone(),
                Signal::derive(|| false),
            ).into_any()).into_any()
        >
            <FormGroup
                label="Link Name".to_string()
                label_for="newCalLinkName"
            >
                <TextInput
                    id="newCalLinkName".to_string()
                    value=name
                    set_value=move |v| set_name.set(v)
                    placeholder="Link Name"
                    help_text="A descriptive name for this calendar feed, e.g., 'My Phone'".to_string()
                    required=true
                    autofocus=true
                />
            </FormGroup>
            <div class="flex flex-col gap-xs mt-md">
                <CheckboxField
                    label="Hide Unmonitored"
                    checked=Signal::from(hide_unmonitored)
                    set_checked=Callback::new(move |v| set_hide_unmonitored.set(v))
                    help_text=MaybeProp::from(Some(
                        "Hide episodes that are not currently being monitored from the feed".to_string()
                    ))
                />
                <CheckboxField
                    label="Show as All-Day Events"
                    checked=Signal::from(show_as_all_day)
                    set_checked=Callback::new(move |v| set_show_as_all_day.set(v))
                    help_text=MaybeProp::from(Some(
                        "Display events as all-day rather than time-specific".to_string()
                    ))
                />
            </div>
        </ModalWrapper>
    }
}
