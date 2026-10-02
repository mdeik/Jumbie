use leptos::prelude::*;

/// Client-side search/filter for managed tables.
///
/// Owns the search query and derives the filtered view: for each item,
/// `haystack` returns the text to match, and a case-insensitive substring test
/// decides inclusion.
///
/// Callers should drive "select all" from the returned `filtered` signal so the
/// selection matches exactly what is on screen, not the unfiltered source.
pub fn use_table_search<T, H>(
    items: Signal<Vec<T>>,
    haystack: H,
) -> (WriteSignal<String>, Signal<Vec<T>>)
where
    T: Clone + Send + Sync + 'static,
    H: Fn(&T) -> String + Copy + Send + Sync + 'static,
{
    let (query, set_query) = signal(String::new());

    let filtered = Signal::derive(move || {
        let query = query.get().trim().to_lowercase();
        let mut items = items.get();
        if !query.is_empty() {
            items.retain(|item| haystack(item).to_lowercase().contains(&query));
        }
        items
    });

    (set_query, filtered)
}
