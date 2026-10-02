use crate::components::common::empty_state::EmptyState;
use crate::components::common::loading_spinner::LoadingSpinner;
use crate::components::common::standard_modal::StandardModal;
use crate::components::common::table_builder::TableBuilder;
use crate::components::common::toast::NotificationType;
use crate::components::search::search_result_item::SearchResultItem;
use crate::hooks::use_ui_config::use_time_format;
use crate::utils::format_datetime_local;
use jumbie_shared::types::SearchResult;
use leptos::control_flow::Show;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::cell::RefCell;
use std::collections::HashMap;

/// Derives a stable unique key from a `SearchResult` for use with `<For />` keyed rendering.
///
/// Uses `download_id` if available (most stable), then `link`, then a composite
/// of deterministic fields that uniquely identifies the result in practice.
fn search_result_key(result: &SearchResult) -> String {
    if let Some(ref id) = result.download_id {
        return id.clone();
    }
    if let Some(ref link) = result.link {
        return link.clone();
    }
    // Fallback: source + title + size is unique enough for list diffing.
    format!("{}|{}|{}", result.source, result.title, result.size)
}

/// Tie-break keys applied after the user's selected sort column: the canonical
/// release order (score ↓ → date ↓ → seeders ↓) with the sorted-on column removed,
/// so no key is compared twice.
fn tiebreak_keys(primary: &str) -> Vec<jumbie_shared::types::ReleaseRankKey> {
    use jumbie_shared::types::ReleaseRankKey;
    jumbie_shared::types::RELEASE_RANK_KEYS
        .iter()
        .copied()
        .filter(|key| {
            !matches!(
                (*key, primary),
                (ReleaseRankKey::Score, "score")
                    | (ReleaseRankKey::Date, "upload_date")
                    | (ReleaseRankKey::Seeders, "peers")
            )
        })
        .collect()
}

// Cache format: CacheKey -> (ExpirationTimestampMs, Results)
thread_local! {
    static SEARCH_CACHE: RefCell<HashMap<String, (f64, Vec<SearchResult>)>> = RefCell::new(HashMap::new());
}

#[component]
pub fn SearchModal(
    show: ReadSignal<bool>,
    set_show: WriteSignal<bool>,
    initial_query: Signal<String>,
    #[prop(into)] series_id: Signal<Option<String>>,
    #[prop(into)] episode_id: Signal<Option<String>>,
    #[prop(into, optional)] id: Option<String>,
) -> impl IntoView {
    let input_id = id.unwrap_or_else(|| "searchQueryInput".to_string());
    let (query, set_query) = signal(initial_query.get_untracked());
    let (results, set_results) = signal(Vec::<SearchResult>::new());
    let (is_loading, set_loading) = signal(false);
    let (has_searched, set_has_searched) = signal(false);
    let (no_search_plugins, set_no_search_plugins) = signal(false);

    let (sort_column, set_sort_column) = signal("score".to_string());
    let (sort_ascending, set_sort_ascending) = signal(false);

    let handle_sort_click = move |col: String| {
        let mut asc = true;
        if sort_column.get() == col {
            asc = !sort_ascending.get();
        } else if col == "score" || col == "peers" || col == "size" || col == "upload_date" {
            asc = false;
        }
        set_sort_column.set(col);
        set_sort_ascending.set(asc);
    };

    // Time format from UI config (SSoT: `use_time_format`)
    let time_format = use_time_format();

    Effect::new(move |_| {
        if show.get() {
            set_query.set(initial_query.get());
            set_results.set(Vec::new());
            set_has_searched.set(false);
            set_loading.set(false);
            set_no_search_plugins.set(false);
        }
    });

    let do_search = move |_| {
        let q = query.get();
        if q.trim().is_empty() {
            crate::components::common::toast::show_toast(
                "Please enter a search query",
                NotificationType::Error,
            );
            return;
        }

        set_loading.set(true);
        set_has_searched.set(true);
        set_results.set(Vec::new());

        let s_id = untrack(move || series_id.get());
        let e_id = untrack(move || episode_id.get());

        spawn_local(async move {
            let cache_key = format!(
                "{}_{}_{}",
                q.trim(),
                s_id.as_deref().unwrap_or(""),
                e_id.as_deref().unwrap_or("")
            );
            let now = js_sys::Date::now();

            let cached_result = SEARCH_CACHE.with(|cache| {
                if let Some((expires, res)) = cache.borrow().get(&cache_key)
                    && *expires > now
                {
                    return Some(res.clone());
                }
                None
            });

            if let Some(res) = cached_result {
                set_results.set(res);
                set_loading.set(false);
                return;
            }

            match crate::api::search_media(q.clone(), "interactive".to_string(), s_id, e_id).await {
                Ok(res) => {
                    set_results.set(res.clone());

                    // Update cache (5 minutes = 300_000 ms)
                    SEARCH_CACHE.with(|cache| {
                        cache.borrow_mut().insert(cache_key, (now + 300_000.0, res));
                    });
                }
                Err(e) => {
                    let err_msg = format!("{}", e);
                    if err_msg.contains("No Search Plugins are available.") {
                        set_no_search_plugins.set(true);
                    } else {
                        crate::components::common::toast::show_toast(
                            format!("Search failed: {}", e),
                            NotificationType::Error,
                        );
                    }
                }
            }
            set_loading.set(false);
        });
    };

    let download_item = {
        let set_show = set_show.clone();
        move |item: SearchResult| {
            let title = item.title.clone();
            let e_id = episode_id.get();
            let s_id = series_id.get();
            let score = item.score;
            let is_pack = item.is_season_pack;
            crate::utils::spawn_api_toast(
                crate::api::download_media(jumbie_shared::types::DownloadMediaPayload {
                    link: item.link.unwrap_or_default(),
                    download_id: item.download_id.unwrap_or_default(),
                    category: None,
                    episode_id: e_id,
                    tag: None,
                    title: Some(title.clone()),
                    score: Some(score),
                    series_id: s_id,
                    is_season_pack: Some(is_pack),
                    is_user_requested: true,
                    size: Some(item.size),
                    seeders: item.seeders,
                    upload_date: item.published.map(|d| d.to_rfc3339()),
                }),
                None,
                move |_| {
                    crate::components::common::toast::show_success(format!(
                        "Download started: {}",
                        title
                    ));
                },
            );
            set_show.set(false);
        }
    };

    // Memoize sorted results — only re-computes when `results`,
    // `sort_column`, or `sort_ascending` actually change.
    let sorted_results = Signal::derive(move || {
        let mut items = results.get();
        let col = sort_column.get();
        let asc = sort_ascending.get();
        // Canonical tie-break minus the primary column; computed once per recompute.
        let keys = tiebreak_keys(&col);

        items.sort_by(|a, b| {
            let primary = match col.as_str() {
                "source" => jumbie_shared::formatting::natural_cmp(&a.source, &b.source),
                "title" => jumbie_shared::formatting::natural_cmp(&a.title, &b.title),
                "size" => a.size.cmp(&b.size),
                "upload_date" => a.published.cmp(&b.published),
                "peers" => a.seeders.unwrap_or(0).cmp(&b.seeders.unwrap_or(0)),
                _ => a.score.cmp(&b.score),
            };
            let directed = if asc { primary } else { primary.reverse() };
            directed.then_with(|| {
                jumbie_shared::types::compare_release_keys_desc(a.rank(), b.rank(), &keys)
            })
        });

        items
    });

    view! {
        <StandardModal
            show=show
            on_close=move |_| set_show.set(false)
            title="Manual Search"
            class="search-modal-container"
        >
            <div class="search-bar-row">
                <input
                    type="text"
                    id=input_id
                    aria-label="Search query"
                    class="form-input search-input"
                    inputmode="search"
                    placeholder="Search query..."
                    prop:value=move || query.get()
                    on:input=move |ev| set_query.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            do_search(());
                        }
                    }
                />
                <button class="btn btn-primary" disabled=move || is_loading.get() on:click=move |_| do_search(())>
                    {move || if is_loading.get() { "Searching..." } else { "Search" }}
                </button>
            </div>

            <Show when=move || episode_id.get().is_some()>
                <div class="text-xs text-muted-color mt-sm mb-md search-note">
                    "Note: Any release downloaded from these results will be mapped directly to this episode, regardless of the release's original season or episode numbers."
                </div>
            </Show>

            <Show when=move || is_loading.get()>
                <LoadingSpinner />
            </Show>

            <Show when=move || !is_loading.get() && has_searched.get()>
                <Show when=move || results.get().is_empty() fallback=move || {
                    view! {
                        <div>
                        {
                            TableBuilder::new(sort_column.into(), sort_ascending.into())
                            .container_class(Signal::derive(move || "results-table-container".to_string()))
                            .table_class("results-table")
                            .column("source", "Source", true)
                            .column("title", "Release Name", true)
                            .column("size", "Size", true)
                                                            .column("upload_date", "Date", true)
                                                            .column("peers", "Peers", true)
                            .column("score", "Score", true)
                            .column("status", "Status", false)
                            .column_with_class("action", "Actions", false, "col-actions")
                            .on_sort(Callback::new(handle_sort_click))
                            .build(view! {
                                // Use <For /> for keyed rendering — Leptos will only
                                // create/remove/reorder DOM nodes for items whose key
                                // actually changed, instead of rebuilding every row.
                                <For
                                    each=move || sorted_results.get()
                                    key=move |item| search_result_key(item)
                                    children=move |item| {
                                        let download_item = download_item.clone();
                                        let tf = time_format.get();
                                        let formatted_date = item
                                            .published
                                            .map(|d| format_datetime_local(&d.to_rfc3339(), &tf))
                                            .unwrap_or_else(|| "-".to_string());
                                        view! {
                                            <SearchResultItem
                                                item=item
                                                on_download=Callback::new(move |i| download_item(i))
                                                formatted_date
                                            />
                                        }
                                    }
                                />
                            }.into_any())
                        }
                        </div>
                    }
                }>
                    <Show when=move || no_search_plugins.get() fallback=move || {
                        view! { <EmptyState message="No results found." /> }
                    }>
                        <EmptyState message="No Search Plugins are available." />
                    </Show>
                </Show>
            </Show>
        </StandardModal>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::types::ReleaseRankKey;

    /// The selected column is dropped from the tie-break so it is never compared
    /// twice; the remaining canonical keys are kept in order.
    #[test]
    fn tiebreak_keys_drop_the_primary_column() {
        assert_eq!(
            tiebreak_keys("score"),
            vec![ReleaseRankKey::Date, ReleaseRankKey::Seeders]
        );
        assert_eq!(
            tiebreak_keys("upload_date"),
            vec![ReleaseRankKey::Score, ReleaseRankKey::Seeders]
        );
        assert_eq!(
            tiebreak_keys("peers"),
            vec![ReleaseRankKey::Score, ReleaseRankKey::Date]
        );
    }

    #[test]
    fn tiebreak_keys_keep_all_for_unrelated_columns() {
        assert_eq!(
            tiebreak_keys("title"),
            vec![
                ReleaseRankKey::Score,
                ReleaseRankKey::Date,
                ReleaseRankKey::Seeders
            ]
        );
    }
}
