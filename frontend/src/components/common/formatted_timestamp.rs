/// Renders a formatted timestamp string (e.g. `"07-15-26 14:30"`) as time then
/// date in separate `<span>` elements. SSoT shared by `logs.rs` and
/// `search_result_item.rs`.
use leptos::prelude::*;

#[component]
pub fn FormattedTimestamp(
    /// A formatted timestamp string, such as `"07-15-26 14:30"` or `"-"`.
    #[prop(into)]
    value: String,
) -> impl IntoView {
    match value.split_once(' ') {
        Some((date, time)) => {
            view! { <p class="timestamp-cell">{time}</p> <p class="timestamp-cell">{date}</p> }
                .into_any()
        }
        None => view! { <p class="timestamp-cell">{value}</p> }.into_any(),
    }
}
