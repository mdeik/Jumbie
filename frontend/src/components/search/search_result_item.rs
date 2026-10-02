use crate::components::common::FormattedTimestamp;
use crate::components::common::form_fields::TooltipBuilder;
use crate::components::common::icons::{ArrowDownIcon, XIcon};
use jumbie_shared::types::SearchResult;
use leptos::prelude::*;

#[component]
pub fn SearchResultItem(
    item: SearchResult,
    on_download: Callback<SearchResult>,
    formatted_date: String,
) -> impl IntoView {
    let item_clone = item.clone();
    let score = item.score;
    let size_str = jumbie_shared::parsing::format_bytes(item.size);

    let title_display = item.title.clone();
    let title_attr = title_display.clone();

    // Some = at least one check was evaluated (so a pass message is meaningful).
    let has_checks = !item.release_checks.is_empty();
    // Failures only — passing checks are never shown. Each failure becomes a
    // tooltip section: the check's heading, then its short description beneath.
    let failures: Vec<(String, String)> = item
        .release_checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| (c.label.clone(), c.description.clone()))
        .collect();
    let has_failures = !failures.is_empty();

    view! {
        <tr>
            <td><span class="badge source-badge">{item.source.clone()}</span></td>
            <td class="truncate col-title" title={title_attr}>{title_display}</td>
            <td>{size_str}</td>
            <td>
                <FormattedTimestamp value=formatted_date />
            </td>
            <td>
                <div class="peer-counts">
                    <span class="seeds" title="Seeders">{item.seeders.unwrap_or(0)} "↑"</span>
                    <span class="leechs" title="Leechers">{item.leechers.unwrap_or(0)} "↓"</span>
                </div>
            </td>
            <td>
                <span class={
                    if score > 0 { "text-success font-bold" }
                    else if score < 0 { "text-danger font-bold" }
                    else { "text-muted-color font-bold" }
                }>
                    {if score > 0 { format!("+{}", score) } else { score.to_string() }}
                </span>
            </td>
            <td class="profile-status-col">
                {move || if has_failures {
                    // Sectioned, formatted tooltip via the shared `TooltipBuilder`
                    // (replaces a native `title`, which cannot format sections).
                    let mut tooltip = TooltipBuilder::new().with_title("Not a valid release");
                    for (heading, description) in &failures {
                        tooltip = tooltip.with_section(heading.clone(), description.clone());
                    }
                    tooltip
                        .build_with(
                            "profile-warning-badge",
                            view! {
                                <span class="status-icon"><XIcon/></span>
                            },
                        )
                        .into_any()
                } else if has_checks {
                    view! {
                        <span class="profile-pass-badge" title="Passes all auto-search checks">
                            "✓"
                        </span>
                    }.into_any()
                } else {
                    view! { <span>"-"</span> }.into_any()
                }}
            </td>
            <td class="col-actions">
                <div class="action-cell">
                    <button
                        class="btn btn-sm btn-action"
                        on:click=move |e| {
                            e.stop_propagation();
                            on_download.run(item_clone.clone());
                        }
                    >
                        <span class="icon"><ArrowDownIcon/></span>
                    </button>
                </div>
            </td>
        </tr>
    }
}
