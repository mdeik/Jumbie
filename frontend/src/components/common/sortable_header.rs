use crate::components::common::icons::ArrowDownIcon;
use leptos::prelude::*;

#[component]
pub fn SortableHeader(
    label: &'static str,
    sort_key: &'static str,
    current_sort: ReadSignal<String>,
    is_desc: ReadSignal<bool>,
    on_sort: Callback<&'static str>,
    #[prop(default = "auto")] width: &'static str,
) -> impl IntoView {
    let is_sorted = move || current_sort.get() == sort_key;
    let desc = move || is_desc.get();

    view! {
        <th
            class=format!("col-{} cursor-pointer", sort_key)
            style=format!("width: {}; cursor: pointer;", width)
            on:click=move |_| {
                on_sort.run(sort_key);
            }
        >
            <div class="flex items-center gap-sm">
                {label}
                <span
                    class="icon"
                    style=move || format!(
                        "opacity: {}; transform: rotate({}deg); transition: transform 0.2s;",
                        if is_sorted() { 1 } else { 0 },
                        if is_sorted() && desc() { 180 } else { 0 }
                    )
                >
                    <ArrowDownIcon/>
                </span>
            </div>
        </th>
    }
}
