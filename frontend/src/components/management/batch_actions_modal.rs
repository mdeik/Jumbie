use crate::components::common::form_fields::{NumberInput, NumberInputMode, TextInput};
use crate::components::common::modal_wrapper::ModalWrapper;
use leptos::prelude::*;
use web_sys::SubmitEvent;

#[component]
pub fn BatchActionsModal(
    show: ReadSignal<bool>,
    set_show: WriteSignal<bool>,
    #[prop(into)] selected_count: Signal<usize>,
    on_confirm: Callback<(String, i32)>,
) -> impl IntoView {
    let (start_season, set_start_season) = signal("1".to_string());
    let (start_episode, set_start_episode) = signal(1);

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        on_confirm.run((start_season.get(), start_episode.get()));
        set_show.set(false);
    };

    view! {
        <ModalWrapper
            show=show
            on_close=move |_| set_show.set(false)
            title="Batch Assign Files"
            size="modal-sm"
        >
            <form on:submit=on_submit>
                <p class="mb-md">
                    {move || format!("Assigning {} selected files sequentially.", selected_count.get())}
                </p>

                <TextInput
                    label="Start Season".to_string()
                    id="batchStartSeason".to_string()
                    value=start_season
                    set_value=move |v| set_start_season.set(v)
                    required=true
                />

                <NumberInput
                    label="Start Episode".to_string()
                    id="batchStartEpisode".to_string()
                    mode=NumberInputMode::Integer
                    value=Signal::derive(move || start_episode.get().to_string())
                    set_value=Callback::new(move |v: String| set_start_episode.set(v.parse().unwrap_or(1)))
                    min="0".to_string()
                />

                <div class="alert alert-info mt-md text-sm">
                    "Files will be assigned sequentially starting from this episode number."
                    <br/>
                    {move || format!("Example: S{}E{} -> S{}E{}",
                        start_season.get(), start_episode.get(),
                        start_season.get(), start_episode.get() + selected_count.get() as i32 - 1
                    )}
                </div>

                <div class="modal-footer">
                    <button type="submit" class="btn btn-primary">"Assign"</button>
                </div>
            </form>
        </ModalWrapper>
    }
}
