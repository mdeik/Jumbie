// In-memory API response cache with stale-while-revalidate semantics.

use crate::api_client::ApiError;
use js_sys;
use jumbie_shared::config::TimeFormat;
use jumbie_shared::config::ui::ReleaseDateDisplayConfig;
use jumbie_shared::types::{CalendarResponse, EpisodeViewModel};
use leptos::prelude::*;
use leptos::task::spawn_local;

// A thread_local in-memory cache for API responses — deliberately invisible to the
// reactive system (a reactive store would re-render every reader on each cache
// update, even when the data is only used to seed initial state). Components seed
// their signals from the cache synchronously, then fire an async fetch that updates
// the signal when it completes.
/// Type alias for the API_PENDING request map.
type PendingRequests = std::collections::HashMap<String, Vec<Box<dyn FnOnce()>>>;

thread_local! {
    pub static API_CACHE: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    // Parallel map tracking when each cache entry was written (ms since epoch),
    // used for stale-while-revalidate.
    pub static API_CACHE_TIMESTAMPS: std::cell::RefCell<std::collections::HashMap<String, f64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    // Tracks in-flight requests so callers can deduplicate: cache key -> callbacks
    // to invoke when the in-flight request completes. An empty Vec means no
    // callbacks registered yet.
    pub static API_PENDING:
        std::cell::RefCell<PendingRequests> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// How long a cached API response is considered fresh (5 minutes). After this
/// window expires, the next page visit re-fetches.
pub const CACHE_TTL_MS: f64 = 300_000.0;

/// Maximum number of entries the cache can hold before the oldest entries are evicted.
pub const MAX_CACHE_ENTRIES: usize = 500;

// Rate limiting / cooldown: last-call timestamp per operation key, so a cooldown
// window can be enforced and rapid-fire API calls (from fast mount/unmount cycles)
// prevented. thread_local + RefCell keeps the state invisible to the reactive
// system and needs no cleanup — stale keys expire on their own.
thread_local! {
    static LAST_CALL_TIMES: std::cell::RefCell<std::collections::HashMap<String, f64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Returns `true` if the operation identified by `key` may proceed (at least
/// `cooldown_ms` elapsed since the last recorded call), updating the timestamp so
/// the next call within the window is blocked.
///
/// Use to gate an immediate `spawn_local` + API call in a mount hook: if the user
/// navigates away and back quickly, the cooldown prevents a redundant request that
/// was already made (or is still in-flight) from the previous mount.
///
/// # Example
///
/// ```ignore
/// if check_cooldown("fetch_media_info_scan_counts", 5_000.0) {
///     spawn_local(async move { /* fetch */ });
/// }
/// ```
pub fn check_cooldown(key: &str, cooldown_ms: f64) -> bool {
    let now = js_sys::Date::now();
    LAST_CALL_TIMES.with(|times| {
        let mut times = times.borrow_mut();
        if let Some(&last) = times.get(key)
            && now - last < cooldown_ms
        {
            return false;
        }
        times.insert(key.to_string(), now);
        true
    })
}

/// Write a value to the cache, evicting the oldest entries if `MAX_CACHE_ENTRIES`
/// is reached and this is a new key.
pub fn write_cache<T: serde::Serialize>(cache_key: &str, value: &T) {
    if let Ok(json) = serde_json::to_string(value) {
        API_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() >= MAX_CACHE_ENTRIES && !cache.contains_key(cache_key) {
                let excess = cache.len() + 1 - MAX_CACHE_ENTRIES;
                let stale: Vec<String> = API_CACHE_TIMESTAMPS.with(|ts| {
                    let mut pairs: Vec<_> =
                        ts.borrow().iter().map(|(k, &v)| (k.clone(), v)).collect();
                    pairs.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                    pairs.into_iter().take(excess).map(|(k, _)| k).collect()
                });
                for k in &stale {
                    cache.remove(k);
                    API_CACHE_TIMESTAMPS.with(|ts| {
                        ts.borrow_mut().remove(k);
                    });
                }
            }
            cache.insert(cache_key.to_string(), json);
        });
        API_CACHE_TIMESTAMPS.with(|ts| {
            ts.borrow_mut()
                .insert(cache_key.to_string(), js_sys::Date::now());
        });
    }
}

/// Read a value from the cache synchronously. Returns `None` if not cached.
pub fn read_cache<T: serde::de::DeserializeOwned>(cache_key: &str) -> Option<T> {
    API_CACHE.with(|cache| {
        cache
            .borrow()
            .get(cache_key)
            .and_then(|json| serde_json::from_str::<T>(json).ok())
    })
}

/// Returns true if a cached entry exists and was written within `CACHE_TTL_MS`.
pub fn is_cache_fresh(key: &str) -> bool {
    let now = js_sys::Date::now();
    let has_data = API_CACHE.with(|c| c.borrow().contains_key(key));
    if !has_data {
        return false;
    }
    API_CACHE_TIMESTAMPS.with(|ts| {
        ts.borrow()
            .get(key)
            .map(|&t| (now - t) < CACHE_TTL_MS)
            .unwrap_or(false)
    })
}

/// Remove a single cache entry by key.
pub fn invalidate_cache_key(key: &str) {
    API_CACHE.with(|cache| {
        cache.borrow_mut().remove(key);
    });
    API_CACHE_TIMESTAMPS.with(|ts| {
        ts.borrow_mut().remove(key);
    });
}

/// Remove all cache entries whose key starts with the given prefix.
/// Used when a filter or sort parameter changes so that stale cached pages
/// don't prevent a fresh API call with the new parameters.
pub fn invalidate_cache_prefix(prefix: &str) {
    API_CACHE.with(|cache| {
        cache.borrow_mut().retain(|k, _| !k.starts_with(prefix));
    });
    API_CACHE_TIMESTAMPS.with(|ts| {
        ts.borrow_mut().retain(|k, _| !k.starts_with(prefix));
    });
}

/// Update the quality/release profiles of a single entry in the cached series list.
///
/// A shared helper (used by the edit-series autosave and reorganize paths) so the
/// cache key and series-list structure live in one place.
pub fn update_cached_series_profiles(
    series_id: &str,
    quality_profile: &str,
    release_profile: &str,
) {
    if let Some(mut series_list) =
        read_cache::<Vec<jumbie_shared::types::SeriesInfo>>("fetch_series")
        && let Some(entry) = series_list.iter_mut().find(|s| s.id == series_id)
    {
        entry.quality_profile = quality_profile.to_string();
        entry.release_profile = release_profile.to_string();
        write_cache("fetch_series", &series_list);
    }
}

/// Invalidate the wanted-list caches (all pages/sorts) — used after any event
/// that can change which episodes are missing/queued.
pub fn invalidate_wanted_caches() {
    invalidate_cache_prefix("fetch_wanted_episodes:");
}

/// Invalidate the sibling list caches (library series list, wanted lists) after a
/// background change is detected by the calendar poll. Per-series details keys are
/// NOT touched — the modal force-revalidates those on every open.
pub fn invalidate_sibling_view_caches() {
    invalidate_cache_prefix("fetch_series");
    invalidate_wanted_caches();
}

/// Surgically remove a deleted/hidden series' episodes from every cached calendar
/// window, leaving the rest of the cache warm (mirrors `patch_calendar_cache`'s
/// scan-and-mutate pattern).
pub fn remove_series_from_calendar_cache(series_ids: &[String]) {
    let ids_set: std::collections::HashSet<&str> = series_ids.iter().map(String::as_str).collect();
    if ids_set.is_empty() {
        return;
    }
    API_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let patch_keys: Vec<String> = cache
            .keys()
            .filter(|k| k.starts_with("fetch_calendar_"))
            .cloned()
            .collect();
        for key in patch_keys {
            if let Some(json) = cache.get(&key)
                && let Ok(mut response) = serde_json::from_str::<CalendarResponse>(json)
            {
                let before = response.episodes.len();
                response
                    .episodes
                    .retain(|ep| !ids_set.contains(ep.series_id.as_str()));
                if response.episodes.len() != before
                    && let Ok(new_json) = serde_json::to_string(&response)
                {
                    cache.insert(key, new_json);
                }
            }
        }
    });
}

/// Invalidate every cache that can hold data for the given series after it is
/// deleted or hidden: per-series details keys, the series' episodes in cached
/// calendar windows (surgically), and the wanted lists. Call from every delete/hide
/// success path so stale entries never resurface.
/// `fetch_series` is deliberately NOT touched: the delete/hide call sites already
/// manage the library list surgically (in-place retain / refetch).
pub fn invalidate_series_caches(series_ids: &[String]) {
    for sid in series_ids {
        invalidate_cache_key(&format!("fetch_series_details_{}", sid));
    }
    remove_series_from_calendar_cache(series_ids);
    invalidate_wanted_caches();
}

/// Surgically propagate a series rename through every cached view that displays the
/// series title — calendar windows, the library series list, and wanted pages —
/// keyed by `series_id` (never by name), so the rest of each cache stays warm.
/// Mirrors the scan-and-mutate pattern of `patch_calendar_cache`.
/// Call from the edit-series autosave after a title change.
pub fn rename_series_in_caches(series_id: &str, new_title: &str) {
    let new_title = new_title.to_string();

    // 1. Calendar windows: update the display title of every episode of the series.
    API_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let keys: Vec<String> = cache
            .keys()
            .filter(|k| k.starts_with("fetch_calendar_"))
            .cloned()
            .collect();
        for key in keys {
            if let Some(json) = cache.get(&key)
                && let Ok(mut response) = serde_json::from_str::<CalendarResponse>(json)
            {
                let mut changed = false;
                for ep in &mut response.episodes {
                    if ep.series_id == series_id && ep.series_title != new_title {
                        ep.series_title = new_title.clone();
                        changed = true;
                    }
                }
                if changed && let Ok(new_json) = serde_json::to_string(&response) {
                    cache.insert(key, new_json);
                }
            }
        }
    });

    // 2. Library series list: update the entry title in place.
    if let Some(mut series_list) =
        read_cache::<Vec<jumbie_shared::types::SeriesInfo>>("fetch_series")
        && let Some(entry) = series_list.iter_mut().find(|s| s.id == series_id)
        && entry.title != new_title
    {
        entry.title = new_title.clone();
        write_cache("fetch_series", &series_list);
    }

    // 3. Wanted pages: update the series title on every cached page.
    let wanted_keys: Vec<String> = API_CACHE.with(|cache| {
        cache
            .borrow()
            .keys()
            .filter(|k| k.starts_with("fetch_wanted_episodes:"))
            .cloned()
            .collect()
    });
    for key in wanted_keys {
        if let Some(mut page) = read_cache::<
            jumbie_shared::types::PaginatedResponse<jumbie_shared::types::WantedEpisode>,
        >(&key)
        {
            let mut changed = false;
            for item in &mut page.items {
                if item.series_id == series_id && item.series_title != new_title {
                    item.series_title = new_title.clone();
                    changed = true;
                }
            }
            if changed {
                write_cache(&key, &page);
            }
        }
    }
}

/// Write a fresh calendar window and detect whether its episodes changed vs the
/// previously cached window. When changed, invalidate the sibling list caches
/// (library, wanted) so they re-fetch on their next mount — background
/// downloads/organizes are only noticed by the calendar's polling, and this
/// propagates the news instead of letting the other views serve TTL-stale data.
pub fn write_calendar_cache_with_change_detection(ck: &str, response: &CalendarResponse) {
    let changed = read_cache::<CalendarResponse>(ck)
        .map(|cached| cached.episodes != response.episodes)
        .unwrap_or(false);
    write_cache(ck, response);
    if changed {
        invalidate_sibling_view_caches();
    }
}

/// Refresh a single episode in every cached calendar window from the freshest
/// episode view model (dates, status, assigned, title, eff_date…).
///
/// Delegates the field projection to
/// [`crate::utils::refresh_calendar_episode_from_view_model`] so the calendar grid
/// and cached windows never drift after a modal save.
///
/// Mutating in-place instead of invalidating + re-fetching costs zero network:
/// the caller already holds the fresh view model, so patching the cached JSON
/// updates the calendar instantly on the next cache read.
pub fn patch_calendar_cache(
    episode: &EpisodeViewModel,
    series_title: &str,
    config: &ReleaseDateDisplayConfig,
    time_format: &TimeFormat,
) {
    let now = js_sys::Date::now();
    API_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let mut touched = false;
        let patch_keys: Vec<String> = cache
            .keys()
            .filter(|k| k.starts_with("fetch_calendar_"))
            .cloned()
            .collect();
        for key in patch_keys {
            if let Some(json) = cache.get(&key)
                && let Ok(mut response) = serde_json::from_str::<CalendarResponse>(json)
            {
                let mut changed = false;
                for ep in &mut response.episodes {
                    if ep.episode_id == episode.unique_id {
                        let before = ep.clone();
                        crate::utils::refresh_calendar_episode_from_view_model(
                            ep,
                            episode,
                            series_title,
                            config,
                            time_format,
                        );
                        changed = *ep != before;
                        break;
                    }
                }
                if changed {
                    touched = true;
                    if let Ok(new_json) = serde_json::to_string(&response) {
                        cache.insert(key, new_json);
                    }
                }
            }
        }
        if touched {
            // Also update the timestamps so the patched entries stay fresh.
            let cal_keys: Vec<String> = API_CACHE_TIMESTAMPS.with(|ts| {
                ts.borrow()
                    .keys()
                    .filter(|k| k.starts_with("fetch_calendar_"))
                    .cloned()
                    .collect()
            });
            API_CACHE_TIMESTAMPS.with(|ts| {
                let mut ts = ts.borrow_mut();
                for key in cal_keys {
                    if let Some(t) = ts.get_mut(&key) {
                        *t = now;
                    }
                }
            });
        }
    });
}

// In-flight request deduplication: tracks which cache keys have a network request
// currently in-flight so a duplicate caller can register a completion callback
// instead of firing a duplicate HTTP request.

/// Returns `true` if a request for this cache key is already in-flight.
pub fn is_pending(key: &str) -> bool {
    API_PENDING.with(|p| p.borrow().contains_key(key))
}

/// Marks a cache key as having a request in-flight.
pub fn mark_pending(key: &str) {
    API_PENDING.with(|p| {
        p.borrow_mut().entry(key.to_string()).or_default();
    });
}

/// Registers a callback to fire when the in-flight request for `key` completes.
/// Does nothing if the key is not currently pending.
pub fn on_pending_complete(key: &str, callback: Box<dyn FnOnce()>) {
    API_PENDING.with(|p| {
        if let Some(callbacks) = p.borrow_mut().get_mut(key) {
            callbacks.push(callback);
        }
    });
}

/// Fires all registered completion callbacks for a key and removes its pending flag.
/// Safe to call even if the key is not pending (no-op).
pub fn resolve_pending(key: &str) {
    let callbacks = API_PENDING.with(|p| p.borrow_mut().remove(key).unwrap_or_default());
    for cb in callbacks {
        cb();
    }
}

// Seeds a reactive signal from the cache, then fires an async fetch behind it.
// The two-phase approach (sync read from cache, then async fetch) lets the UI render
// immediately with stale-but-visible data while the round-trip completes.
// In-flight requests are tracked via `API_PENDING` so concurrent callers never fire
// duplicate HTTP requests for the same key; the second caller registers a callback
// that runs when the first completes.
pub fn use_api_cache<T, F, Fut, S>(cache_key: String, fetcher: F, setter: S)
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = Result<T, ApiError>> + 'static,
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + 'static,
    S: Fn(T) + Clone + 'static,
{
    // Phase 1: synchronously populate the signal from cache (instant render)
    API_CACHE.with(|cache| {
        if let Some(cached_json) = cache.borrow().get(&cache_key)
            && let Ok(parsed) = serde_json::from_str::<T>(cached_json)
        {
            setter(parsed);
        }
    });

    // Phase 2: fetch latest data from the API (eventual consistency)
    // Skip if the cache already has fresh data (stale-while-revalidate).
    if is_cache_fresh(&cache_key) {
        return;
    }

    // Alive-guard: component-scoped flag that is set to false when the
    // component unmounts.  The on_pending_complete callback checks this flag
    // before calling the setter, so that stale callbacks accumulated from
    // repeated rapid navigations become instant no-ops rather than triggering
    // reactive updates on dropped signal owners — which was the root cause of
    // the nav-switching freeze.
    //
    // StoredValue::new_local is used (not a Send wrapper) because the
    // callbacks stored in API_PENDING are Box<dyn FnOnce()> (not Send),
    // matching the single-threaded WASM execution model.
    let alive = StoredValue::new_local(true);
    on_cleanup(move || alive.set_value(false));

    // If a request for this key is already in-flight, register a callback
    // that reads from cache when it completes, avoiding a duplicate HTTP request.
    if is_pending(&cache_key) {
        on_pending_complete(
            &cache_key,
            Box::new({
                let setter = setter.clone();
                let ck = cache_key.clone();
                move || {
                    // No-op if the component that registered this callback
                    // has already been unmounted (navigated away from).
                    if !alive.get_value() {
                        return;
                    }
                    if let Some(data) = read_cache::<T>(&ck) {
                        setter(data);
                    }
                }
            }),
        );
        return;
    }

    mark_pending(&cache_key);
    spawn_local(async move {
        if let Ok(fresh) = fetcher().await {
            write_cache(&cache_key, &fresh);
            // Guard: component may have unmounted while the HTTP request
            // was in-flight.  Skip the setter if so.
            if alive.get_value() {
                setter(fresh);
            }
        }
        resolve_pending(&cache_key);
    });
}

// Pre-fetches data into the cache without touching any signal.
//
// This is the silent counterpart to use_api_cache — it populates the cache so
// that when a component later calls use_api_cache with the same key, it gets
// an instant hit. Useful for preloading data for tabs the user hasn't clicked yet.
pub fn preload_api_cache<T, F, Fut>(cache_key: String, fetcher: F)
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = Result<T, ApiError>> + 'static,
    T: serde::Serialize + 'static,
{
    // Skip if already cached and fresh — avoids duplicating network requests on repeat visits
    if is_cache_fresh(&cache_key) {
        return;
    }

    // If a request is already in-flight (e.g. from a component that mounted
    // before this preloader ran), skip to avoid a duplicate.
    if is_pending(&cache_key) {
        return;
    }

    mark_pending(&cache_key);
    spawn_local(async move {
        if let Ok(fresh) = fetcher().await {
            write_cache(&cache_key, &fresh);
        }
        resolve_pending(&cache_key);
    });
}

/// Fire-and-forget cached fetch with dedup and a data callback.
///
/// Writes result to cache, then calls `on_data` with the result.
/// If already cached and fresh, skips silently (does NOT call `on_data`).
/// If already pending, registers a callback that fires when the in-flight
/// request completes (and reads the result from cache).
///
/// This is the universal lower-level building block for all fire-and-forget
/// API reads. Use it in event handlers (button clicks, callbacks) where you
/// need the data eventually but don't want to block the UI.
///
/// For component-initialization reads (static key), use `use_api_cache` instead.
/// For reactive-key reads (Effect-driven), use `use_api_cache_effect` instead.
/// For silent preloading (no callback), use `spawn_cached` or `preload_api_cache`.
/// For event-driven revalidation that must ALWAYS hit the network (modal open,
/// post-save patch), use `refresh_cached_with`.
pub fn spawn_cached_with<T, F, Fut>(cache_key: String, fetcher: F, on_data: impl Fn(T) + 'static)
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = Result<T, ApiError>> + 'static,
    T: serde::Serialize + serde::de::DeserializeOwned + 'static,
{
    // Skip if the cache already has fresh data — avoids redundant network requests
    if is_cache_fresh(&cache_key) {
        return;
    }

    refresh_cached_with(cache_key, fetcher, on_data);
}

/// Fire-and-forget cached fetch that ALWAYS revalidates (bypasses the TTL).
///
/// Same dedup + cache write + `on_data` semantics as `spawn_cached_with`, but
/// the `is_cache_fresh` gate is skipped: a TTL-fresh-but-stale entry must not
/// suppress the fetch (e.g. the episode-details modal opened right after the
/// backend state changed).
///
/// SSoT for event-driven revalidation — modal opens and post-save patches.
pub fn refresh_cached_with<T, F, Fut>(cache_key: String, fetcher: F, on_data: impl Fn(T) + 'static)
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = Result<T, ApiError>> + 'static,
    T: serde::Serialize + serde::de::DeserializeOwned + 'static,
{
    // Never duplicate an in-flight request — register a completion callback instead.
    if is_pending(&cache_key) {
        let ck = cache_key.clone();
        on_pending_complete(
            &cache_key,
            Box::new(move || {
                if let Some(data) = read_cache::<T>(&ck) {
                    on_data(data);
                }
            }),
        );
        return;
    }

    mark_pending(&cache_key);
    spawn_local(async move {
        if let Ok(fresh) = fetcher().await {
            write_cache(&cache_key, &fresh);
            on_data(fresh);
        }
        resolve_pending(&cache_key);
    });
}

/// Fire-and-forget cached fetch with dedup — silent variant (no callback).
///
/// Writes result to cache without invoking any signal setter.
/// This is exactly `preload_api_cache` under a more descriptive name.
/// Use it when you want to pre-populate the cache for later use by another
/// component that calls `use_api_cache` with the same key.
pub fn spawn_cached<T, F, Fut>(cache_key: String, fetcher: F)
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = Result<T, ApiError>> + 'static,
    T: serde::Serialize + serde::de::DeserializeOwned + 'static,
{
    spawn_cached_with(cache_key, fetcher, |_| {});
}

/// Seeds a reactive signal from the cache, then fires a background fetch
/// whenever the cache key (derived from reactive state) changes.
///
/// This is the Effect-aware counterpart to `use_api_cache` — use it when the
/// cache key depends on reactive state (e.g., a `current_date` signal).
///
/// ## How it works
///
/// An Effect wraps the call so that reactive reads inside `key_fn` are tracked
/// by Leptos. When a dependency changes, the Effect re-runs:
/// 1. **Sync from cache** — populates `setter` instantly if a cached entry exists.
/// 2. **Background refresh** — if the cache is stale or missing, fires a network
///    fetch with full dedup (checks `is_pending`, registers callbacks on duplicate).
///
/// The `fetcher_fn` receives the computed cache key so it can derive parameters
/// (e.g., parsing date strings from a calendar key). For static-key endpoints
/// the fetcher can simply ignore the key parameter.
///
/// ## When to use
///
/// - Calendar component: the cache key changes when `current_date` changes.
/// - Any component where the endpoint parameters are reactive.
///
/// For static keys, use `use_api_cache` instead. For fire-and-forget event
/// handlers, use `spawn_cached_with`.
pub fn use_api_cache_effect<T, F, Fut, S, K>(key_fn: K, fetcher_fn: F, setter: S)
where
    K: Fn() -> String + Clone + 'static,
    F: Fn(String) -> Fut + Clone + 'static,
    Fut: std::future::Future<Output = Result<T, ApiError>> + 'static,
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + 'static,
    S: Fn(T) + Clone + 'static,
{
    // Alive-guard: created OUTSIDE the Effect so it is tied to the component
    // owner, not re-created on every Effect re-run.  StoredValue is Copy, so
    // the Effect closure captures it cheaply.  on_cleanup sets it to false when
    // the component unmounts, making all pending callbacks instant no-ops.
    let alive = StoredValue::new_local(true);
    on_cleanup(move || alive.set_value(false));

    Effect::new(move |_| {
        let cache_key = key_fn();

        // Phase 1: synchronously populate the signal from cache (instant render)
        if let Some(data) = read_cache::<T>(&cache_key) {
            setter(data);
        }

        // Phase 2: fetch latest data from the API (eventual consistency)
        // Skip if the cache already has fresh data (stale-while-revalidate).
        if is_cache_fresh(&cache_key) {
            return;
        }

        // If a request for this key is already in-flight, register a callback
        // that reads from cache when it completes, avoiding a duplicate HTTP request.
        if is_pending(&cache_key) {
            on_pending_complete(
                &cache_key,
                Box::new({
                    let setter = setter.clone();
                    let ck = cache_key.clone();
                    move || {
                        if !alive.get_value() {
                            return;
                        }
                        if let Some(data) = read_cache::<T>(&ck) {
                            setter(data);
                        }
                    }
                }),
            );
            return;
        }

        mark_pending(&cache_key);
        let ck = cache_key.clone();
        let fetcher_fn = fetcher_fn.clone();
        let setter = setter.clone();
        spawn_local(async move {
            if let Ok(data) = fetcher_fn(ck.clone()).await {
                write_cache(&ck, &data);
                if alive.get_value() {
                    setter(data);
                }
            }
            resolve_pending(&ck);
        });
    });
}
