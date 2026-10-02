// use_pagination — reactive pagination with URL hash sync, cache, preloading.
//
// Every paginated table needs the same wiring: page/total_pages signals,
// cache-seeding on mount, fetching on page change, URL hash synchronization, and
// adjacent-page preloading. The hook owns all reactive state internally and exposes
// a lightweight PaginationState with read-only signals and callbacks.

use crate::api_client::ApiError;
use gloo_timers::future::TimeoutFuture;
use jumbie_shared::types::PaginatedResponse;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;

/// How many pages in each direction to preload after a successful fetch.
const PRELOAD_RANGE: i64 = 3;

/// The decision the fetch effect makes for one run.
///
/// Extracted as a pure function ([`plan_fetch`]) so the trickiest part of the
/// hook — the query-change reset, the refresh tick, and the no-op guard — is
/// unit-testable without a reactive runtime or a DOM.
#[derive(Debug, PartialEq)]
struct FetchPlan {
    /// Page to request.
    fetch_page: i64,
    /// The non-page query (sort/filters) changed since the last run.
    query_changed: bool,
    /// An explicit `refresh()` was requested since the last run.
    refreshing: bool,
    /// Revalidate even on a cache hit (mount, or an explicit refresh).
    do_stale: bool,
    /// Also write `page` back to 0 (a query change from a later page).
    reset_page: bool,
    /// No-op run that must not fetch (the guard).
    skip: bool,
}

/// Compute the [`FetchPlan`] for one effect run.
///
/// `prev` is `(last_fetch_page, last_query_key, last_refresh_tick)` from the
/// previous run, or `None` on the first (mount) run.
fn plan_fetch(
    prev: Option<&(i64, String, u64)>,
    current_page: i64,
    key: &str,
    tick: u64,
    total_pages: i64,
    stale_while_revalidate: bool,
) -> FetchPlan {
    let query_changed = prev.is_some_and(|(_, pk, _)| pk != key);
    let refreshing = prev.is_some_and(|(_, _, pt)| *pt != tick);

    // A query change resets to the first page.
    let requested_page = if query_changed { 0 } else { current_page };
    // Clamp to a valid page when the count is known. `total_pages` is 1 until
    // the first response, so the URL's page passes through on mount and the
    // real total is discovered from the response.
    let fetch_page = if total_pages > 1 {
        requested_page.min((total_pages - 1).max(0))
    } else {
        requested_page
    };

    // `RwSignal::set` notifies unconditionally, so writing `page` back to 0
    // produces an extra run with the same target — skip it rather than fetching
    // twice. The refresh tick is part of the identity, so a refresh is never a
    // no-op.
    let skip = prev.is_some_and(|(fp, pk, pt)| *fp == fetch_page && pk == key && *pt == tick);

    FetchPlan {
        fetch_page,
        query_changed,
        refreshing,
        do_stale: (stale_while_revalidate && prev.is_none()) || refreshing,
        reset_page: query_changed && current_page != 0,
        skip,
    }
}

/// Reactive state bundle returned by `use_pagination`.
pub struct PaginationState<T>
where
    T: Send + Sync + 'static,
{
    /// Items for the current page.
    pub items: Signal<Vec<T>>,
    /// Whether a fetch is in progress AND no cached data is available.
    pub loading: Signal<bool>,
    /// Current page number (0‑indexed).
    pub page: Signal<i64>,
    /// Total number of pages.
    pub total_pages: Signal<i64>,
    /// Callback to navigate to a specific page (0‑indexed).
    pub set_page: Callback<i64>,
    /// Callback to re‑fetch the current page (e.g. after a mutation).
    pub refresh: Callback<()>,
}

/// Creates a self‑contained pagination state machine.
///
/// * `cache_key_base` — prefix used for page‑aware cache keys
///   (`"{cache_key_base}:{page}"`). Must be `'static` so it can live in
///   effects and spawned tasks.
/// * `fetcher` — a closure that takes a 0‑indexed page number and returns a
///   future yielding `PaginatedResponse<T>`.
/// * `items_per_page` — used to compute `total_pages` from `response.total`.
/// * `stale_while_revalidate` — if `true`, re‑fetch the current page in the
///   background on mount even when cached data is available. The cached data
///   is shown instantly, then silently replaced when fresh data arrives.
/// * `auto_refresh_ms` — optional interval (ms) for periodic background
///   refreshes while the component is mounted. Behaves like
///   `stale_while_revalidate` on each tick — no loading flash.
/// * `query_key` — a signal encoding every non‑page parameter of the query
///   (sort column + direction, active filters, …).  When it changes the hook
///   invalidates the cache prefix, resets to page 0, and re‑fetches.  This is
///   the SSoT for "a sort/filter changed".
///
///   `Signal::derive` has no equality check, so writing an override that yields
///   the *same* key still re‑runs the effect.  Dedupe at the source — do not
///   write an override that is already set (see `SearchInput` and the filter
///   setters).  This is deliberately not an HTTP‑layer request gate: a repeated
///   *distinct* query (e.g. a re-run) must still hit the server.
pub fn use_pagination<T, Fut>(
    cache_key_base: &'static str,
    fetcher: impl Fn(i64) -> Fut + Clone + 'static,
    items_per_page: i64,
    stale_while_revalidate: bool,
    auto_refresh_ms: Option<u64>,
    query_key: impl Into<Signal<String>>,
) -> PaginationState<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<PaginatedResponse<T>, ApiError>> + 'static,
{
    let query_key: Signal<String> = query_key.into();

    // 1. Initial page from URL hash. The hash stores a 1-based page number
    // (#page_3 = page index 2 internally) so bookmarks/shared URLs match the
    // 1-based display in the UI.
    let initial_page = web_sys::window()
        .and_then(|w| w.location().hash().ok())
        .and_then(|hash| {
            if hash.starts_with("#page_") {
                hash.trim_start_matches("#page_").parse::<i64>().ok()
            } else {
                None
            }
        })
        // Convert from 1-based (URL) to 0-based (internal).
        .map(|n| n - 1)
        .unwrap_or(0)
        .max(0);

    // 2. Internal reactive state
    let items: RwSignal<Vec<T>> = RwSignal::new(Vec::new());
    let loading: RwSignal<bool> = RwSignal::new(true);
    let page: RwSignal<i64> = RwSignal::new(initial_page);
    let total_pages: RwSignal<i64> = RwSignal::new(1);
    // A generation counter prevents stale responses from overwriting newer ones
    // when the user clicks through pages faster than the network responds.
    let fetch_gen: RwSignal<u64> = RwSignal::new(0);
    // Bumped by `refresh` to force an immediate re-fetch that bypasses both the
    // page cache and the effect's no-op guard.
    let refresh_tick: RwSignal<u64> = RwSignal::new(0);
    // Tracks the last-known `total` from PaginatedResponse. When a re-fetch reports
    // a different total, adjacent cached pages are invalidated so they are re-fetched
    // on the next visit (handles prepended data).
    let known_total: RwSignal<Option<i64>> = RwSignal::new(None);

    // 3. Seed from cache on mount — skipped when stale_while_revalidate is true,
    // since the effect handles cache population as part of its initial run.
    if !stale_while_revalidate {
        {
            let cache_key = format!("{}:{}", cache_key_base, initial_page);
            if let Some(cached) = crate::utils::read_cache::<PaginatedResponse<T>>(&cache_key) {
                items.set(cached.items);
                total_pages.set(((cached.total as f64) / (items_per_page as f64)).ceil() as i64);
                loading.set(false);
            }
        }
    }

    // Shared fetch response handler — the single place that processes a successful
    // fetch: invalidates adjacent cached pages on total change, updates known_total,
    // writes fresh data to cache, and updates items/total_pages. Used by both the
    // effect (page nav / mount) and the auto-refresh interval. All captures are Copy,
    // so the closure is Copy and can be freely cloned into async blocks.
    let apply_fetch_result = move |payload: PaginatedResponse<T>, cache_key: &str| -> i64 {
        let total = payload.total;
        let prev_total = known_total.get_untracked();
        if prev_total.is_some() && prev_total != Some(total) {
            for page_idx in 0..=total_pages.get_untracked().max(0) {
                let ck = format!("{}:{}", cache_key_base, page_idx);
                if ck != cache_key {
                    crate::utils::invalidate_cache_key(&ck);
                }
            }
        }
        known_total.set(Some(total));
        crate::utils::write_cache(cache_key, &payload);
        items.set(payload.items);
        let new_total_pages = ((total as f64) / (items_per_page as f64)).ceil() as i64;
        total_pages.set(new_total_pages);
        new_total_pages
    };

    // 4. Effect: fetch on page change, refresh, or query change. Arc (not Rc) so the
    // fetcher is Send+Sync, as required by the auto-refresh interval callback.
    let fetcher_rc = std::sync::Arc::new(fetcher);

    // The effect returns `(last_fetched_page, last_query_key, last_refresh_tick)` so
    // the next run can tell which input changed. Tracking `query_key` here is what
    // makes a persisted sort/filter (delivered asynchronously via `ui_config` after
    // mount) trigger a re-fetch.
    Effect::new({
        let fetcher_rc = fetcher_rc.clone();
        move |prev: Option<(i64, String, u64)>| {
            let key = query_key.get();
            // Read `page` and the refresh tick unconditionally (tracked) so the
            // effect stays subscribed to them on *every* run.  Reading `page`
            // only inside the fetch path would drop the subscription on a
            // query-change run, after which pagination clicks would stop
            // triggering a fetch.
            let current_page = page.get();
            let tick = refresh_tick.get();

            let plan = plan_fetch(
                prev.as_ref(),
                current_page,
                &key,
                tick,
                total_pages.get_untracked(),
                stale_while_revalidate,
            );

            // Query (sort/filter) changed → invalidate. The page-aware cache keys do
            // not encode the query parameters, so every cached page is stale once the
            // query changes. Bump the generation first so any in-flight fetch for the
            // old query is discarded when it completes.
            if plan.query_changed {
                crate::utils::invalidate_cache_prefix(&format!("{}:", cache_key_base));
                fetch_gen.update(|g| *g += 1);
            }

            // No-op guard (see `plan_fetch`)
            if plan.skip {
                return (plan.fetch_page, key, tick);
            }

            // A query change resets to the first page; write it back so the
            // pagination control agrees.  The guard above swallows the extra
            // effect run this write triggers.
            if plan.reset_page {
                page.set(0);
            }

            let fetch_page = plan.fetch_page;
            let do_stale = plan.do_stale;
            // Bump generation so in‑flight requests from previous pages are
            // ignored when they eventually complete.
            let generation = {
                let g = fetch_gen.get_untracked() + 1;
                fetch_gen.set(g);
                g
            };

            let cache_key = format!("{}:{}", cache_key_base, fetch_page);

            // 4a. Cache hit → populate from cache (instant render). If this page's
            // data is cached and still fresh (within the 5-minute TTL — see
            // CACHE_TTL_MS), populate the signals from cache. When
            // `stale_while_revalidate` is true and this is the first effect run
            // (mount), we still fall through to fetch fresh data (no loading flash).
            let cache_fresh = crate::utils::is_cache_fresh(&cache_key);
            if cache_fresh {
                if let Some(cached) = crate::utils::read_cache::<PaginatedResponse<T>>(&cache_key) {
                    items.set(cached.items);
                    total_pages
                        .set(((cached.total as f64) / (items_per_page as f64)).ceil() as i64);
                    loading.set(false);
                }
                if !do_stale {
                    // Cache hit and no revalidation requested — nothing was
                    // fetched, so report the current inputs unchanged.
                    return (fetch_page, key, tick);
                }
                // Stale-while-revalidate: keep cached data visible and
                // silently fetch fresh data in the background.
            } else {
                // 4b. No cache → show loading skeleton. Clear the items signal so the
                // table renders a clean skeleton instead of the previous page's data
                // (which is not just stale — it's wrong for the new page number).
                items.set(Vec::new());
                loading.set(true);
            }

            // Clone fetcher_rc BEFORE entering the async block so the outer
            // closure stays FnMut (the inner async move won't steal it).
            let fetcher = fetcher_rc.clone();
            spawn_local(async move {
                let resp = fetcher(fetch_page).await;
                // Use get_untracked() — this code runs inside spawn_local,
                // which is outside any reactive tracking context, so a
                // tracked .get() would trigger a false-positive warning.
                if fetch_gen.get_untracked() != generation {
                    return;
                }
                match resp {
                    Ok(payload) => {
                        let new_total_pages = apply_fetch_result(payload, &cache_key);

                        // Page clamping: after the first successful fetch we know the
                        // real total_pages, so clamp a page beyond the valid range
                        // (e.g. URL hash #page_100) to avoid sitting on an out-of-bounds
                        // page with empty data forever.
                        let max_page = (new_total_pages - 1).max(0);
                        let current_clamp = page.get_untracked();
                        if current_clamp > max_page {
                            page.set(max_page);
                            return;
                        }

                        loading.set(false);

                        // 5. Adjacent-page preloading (best-effort). Debounce rapid
                        // clicks with a short pause, then preload up to PRELOAD_RANGE
                        // pages in each direction, skipping pages already fresh in cache.
                        TimeoutFuture::new(100).await;

                        // Preload up to PRELOAD_RANGE pages in each direction.
                        let fetcher_for_preload = fetcher.clone();
                        let start = (fetch_page - PRELOAD_RANGE).max(0);
                        let end = (fetch_page + PRELOAD_RANGE).min(new_total_pages - 1);
                        for adj in start..=end {
                            if adj == fetch_page {
                                continue;
                            }
                            let adj_key = format!("{}:{}", cache_key_base, adj);
                            if !crate::utils::is_cache_fresh(&adj_key) {
                                let f = fetcher_for_preload.clone();
                                spawn_local(async move {
                                    let _ = f(adj).await.map(|resp| {
                                        crate::utils::write_cache(&adj_key, &resp);
                                    });
                                });
                            }
                        }
                    }
                    Err(_) => {
                        loading.set(false);
                        // Errors are intentionally swallowed here.  Components
                        // that need user‑visible error handling wrap the
                        // fetcher argument with their own toast logic.
                    }
                }
            });

            // Hand the inputs of this run back to the effect so the next run can
            // detect a query change versus a page change versus a refresh.
            (fetch_page, key, tick)
        }
    });

    // 6. URL hash ↔ page synchronisation. The hash stores a 1-based page number
    // (#page_3) so the address bar matches the "Page 3 of 12" label in the UI.
    let set_page = Callback::new(move |new_page: i64| {
        let raw = new_page.max(0);
        // Clamp the internal page signal to the valid range so the API
        // request fetches a real page, but write the raw value (1‑based)
        // to the URL hash so the address bar preserves what the user typed.
        let effective = raw.min((total_pages.get_untracked() - 1).max(0));
        page.set(effective);
        if let Some(window) = web_sys::window() {
            // Write 1‑based to the URL hash.
            let _ = window.location().set_hash(&format!("page_{}", raw + 1));
        }
    });

    // 7. Listen for cross-tab hash changes.
    //
    // on_cleanup MUST remove this listener on unmount: otherwise every mount of a
    // paginated view leaks a listener that keeps firing, saturating the WASM
    // executor and freezing the UI. The js_sys::Function reference is cloned before
    // `handler` is moved into the cleanup closure so both hold the same JS function.
    if let Some(window) = web_sys::window() {
        let page_clone = page;
        let handler = Closure::<dyn FnMut()>::new(move || {
            if let Some(hash) = web_sys::window().and_then(|w| w.location().hash().ok())
                && hash.starts_with("#page_")
                && let Ok(p) = hash.trim_start_matches("#page_").parse::<i64>()
            {
                // Hash is 1-based → convert to 0-based internal.
                page_clone.set((p - 1).max(0));
            }
        });
        // Clone the JS Function reference before moving `handler` so the
        // cleanup closure can call remove_event_listener_with_callback.
        let handler_fn = handler.as_ref().unchecked_ref::<js_sys::Function>().clone();
        let _ = window.add_event_listener_with_callback("hashchange", &handler_fn);

        // Remove listener and clear the stale page hash on unmount, so a leftover
        // "#page_N" is not read as the initial page by the next paginated view.
        //
        // `handler.forget()` keeps the Wasm closure alive in JS memory; we only
        // capture `handler_fn` (a js_sys::Function, which is Send+Sync) in the
        // cleanup closure — once the listener is removed the forgotten closure is
        // permanently unreachable. This is the idiomatic wasm-bindgen cleanup pattern.
        handler.forget();
        on_cleanup(move || {
            if let Some(w) = web_sys::window() {
                let _ = w.remove_event_listener_with_callback("hashchange", &handler_fn);
                // Clear stale page hash so the next paginated view starts fresh.
                let _ = w.location().set_hash("");
            }
        });
    }

    // 8. Refresh callback
    let refresh = Callback::new(move |()| {
        // Bump the refresh tick: the effect re-runs, bypasses the page cache
        // (see `do_stale`), and is never treated as a no-op because the tick is
        // part of its comparison key.
        refresh_tick.update(|t| *t += 1);
    });

    // 9. Auto-refresh while mounted (silent background polling): periodically
    // re-fetches the current page without showing a loading indicator.
    if let Some(interval_ms) = auto_refresh_ms {
        match set_interval_with_handle(
            move || {
                let snapshot_page = page.get_untracked();
                // Only auto-refresh on the first page to avoid disrupting the
                // user while they browse deeper pages.
                if snapshot_page != 0 {
                    return;
                }
                let f = fetcher_rc.clone();
                spawn_local(async move {
                    if let Ok(resp) = f(snapshot_page).await {
                        // Discard if the user navigated to a different page
                        // while this request was in-flight.
                        if page.get_untracked() != snapshot_page {
                            return;
                        }

                        let cache_key = format!("{}:{}", cache_key_base, snapshot_page);
                        apply_fetch_result(resp, &cache_key);
                    }
                });
            },
            std::time::Duration::from_millis(interval_ms),
        ) {
            Ok(handle) => on_cleanup(move || {
                handle.clear();
            }),
            Err(_) => {
                crate::debug_warn!("Failed to set auto-refresh interval for {}", cache_key_base)
            }
        }
    }

    PaginationState {
        items: items.into(),
        loading: loading.into(),
        page: page.into(),
        total_pages: total_pages.into(),
        set_page,
        refresh,
    }
}

#[cfg(test)]
mod tests {
    use super::{FetchPlan, plan_fetch};

    fn prev(page: i64, key: &str, tick: u64) -> (i64, String, u64) {
        (page, key.to_string(), tick)
    }

    // Mount

    #[test]
    fn mount_fetches_the_page_from_the_url_and_revalidates() {
        let plan = plan_fetch(None, 0, "k", 0, 1, true);
        assert_eq!(
            plan,
            FetchPlan {
                fetch_page: 0,
                query_changed: false,
                refreshing: false,
                do_stale: true, // stale-while-revalidate on mount
                reset_page: false,
                skip: false,
            }
        );
    }

    #[test]
    fn mount_passes_through_the_url_page_before_the_total_is_known() {
        // total_pages is 1 until the first response, so no clamping yet.
        let plan = plan_fetch(None, 2, "k", 0, 1, false);
        assert_eq!(plan.fetch_page, 2);
        assert!(!plan.do_stale); // no stale-while-revalidate → no forced fetch
        assert!(!plan.skip);
    }

    // The no-op guard

    #[test]
    fn identical_inputs_are_a_no_op() {
        let plan = plan_fetch(Some(&prev(0, "k", 0)), 0, "k", 0, 5, true);
        assert!(plan.skip, "re-running with the same target must not fetch");
        assert!(!plan.do_stale);
        assert!(!plan.query_changed);
    }

    // Page navigation

    #[test]
    fn page_change_fetches_the_new_page() {
        let plan = plan_fetch(Some(&prev(0, "k", 0)), 3, "k", 0, 5, true);
        assert_eq!(plan.fetch_page, 3);
        assert!(!plan.skip);
        assert!(!plan.query_changed);
        assert!(!plan.reset_page);
    }

    #[test]
    fn a_page_beyond_the_total_is_clamped() {
        let plan = plan_fetch(Some(&prev(0, "k", 0)), 9, "k", 0, 3, true);
        assert_eq!(plan.fetch_page, 2);
        assert!(!plan.skip);
    }

    // Query (sort/filter) changes

    #[test]
    fn query_change_from_page_zero_fetches_page_zero_without_a_page_reset() {
        let plan = plan_fetch(Some(&prev(0, "old", 0)), 0, "new", 0, 5, true);
        assert!(plan.query_changed);
        assert_eq!(plan.fetch_page, 0);
        assert!(!plan.reset_page, "already on page 0 — nothing to reset");
        assert!(!plan.skip, "a query change must always fetch");
        assert!(!plan.do_stale, "the cache was invalidated, not revalidated");
    }

    #[test]
    fn query_change_from_a_later_page_resets_to_zero() {
        let plan = plan_fetch(Some(&prev(2, "old", 0)), 2, "new", 0, 5, true);
        assert!(plan.query_changed);
        assert_eq!(plan.fetch_page, 0);
        assert!(plan.reset_page, "must also write `page` back to 0");
        assert!(!plan.skip);
    }

    #[test]
    fn the_run_triggered_by_the_page_reset_is_a_no_op() {
        // After the query-change run returns `(0, "new", tick)`, writing
        // `page=0` produces one more run with exactly those inputs. It must not
        // fetch a second time — this is the regression the guard prevents.
        let plan = plan_fetch(Some(&prev(0, "new", 0)), 0, "new", 0, 5, true);
        assert!(plan.skip);
    }

    // Explicit refresh

    #[test]
    fn refresh_fetches_even_when_nothing_else_changed() {
        let plan = plan_fetch(Some(&prev(1, "k", 0)), 1, "k", 1, 5, true);
        assert!(plan.refreshing);
        assert!(!plan.skip, "a refresh must never be treated as a no-op");
        assert!(plan.do_stale, "a refresh must bypass the cache");
        assert_eq!(plan.fetch_page, 1);
    }
}
