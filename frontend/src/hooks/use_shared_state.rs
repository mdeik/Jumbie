use crate::api_client::ApiError;
use leptos::prelude::*;

#[derive(Clone, Debug)]
pub struct SharedState<T: Send + Sync + 'static> {
    pub data: Signal<Option<T>>,
    pub loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    pub refresh: Callback<()>,
}

/// A hook that provides a unified pattern for data fetching and state
/// synchronization. Seeds from `API_CACHE` for immediate data, then fetches fresh
/// data through the shared cache/dedup system in utils.rs.
pub fn use_shared_state<T, Fut>(
    cache_key: String,
    fetcher: impl Fn() -> Fut + Clone + Send + Sync + 'static,
) -> SharedState<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<T, ApiError>> + 'static,
{
    let (data, set_data) = signal(crate::utils::read_cache::<T>(&cache_key));
    let (loading, set_is_loading) = signal(data.get_untracked().is_none());
    let (error, set_error) = signal(None::<String>);

    let ck = cache_key.clone();
    let f = fetcher.clone();

    // Effect-driven background refresh with full dedup. On mount (and re-visit), fire
    // a background fetch if the cache is stale or missing. spawn_cached_with ensures
    // an in-flight preloader request is not duplicated, and registers a callback that
    // reads from cache when the in-flight request completes.
    //
    // Alive-guard: `alive` is a component-scoped StoredValue (dropped when the owner
    // is cleaned up). Every on_pending_complete callback checks it so stale callbacks
    // from unmounted components become instant no-ops. Without this, rapid nav
    // switching accumulates N callbacks in API_PENDING that all fire synchronously
    // when the response arrives, freezing the UI.
    let alive = StoredValue::new_local(true);
    on_cleanup(move || alive.set_value(false));

    Effect::new(move |_| {
        if crate::utils::is_cache_fresh(&ck) {
            return;
        }

        crate::utils::spawn_cached_with(
            ck.clone(),
            {
                let f = f.clone();
                move || {
                    let f = f.clone();
                    async move { f().await }
                }
            },
            {
                let set_data = set_data.clone();
                let set_error = set_error.clone();
                let set_is_loading = set_is_loading.clone();
                move |fresh: T| {
                    // Guard: skip if the component that registered this
                    // callback has already been unmounted.
                    if !alive.get_value() {
                        return;
                    }
                    set_data.set(Some(fresh));
                    set_error.set(None);
                    set_is_loading.set(false);
                }
            },
        );

        // Clear loading when the pending request resolves (even on error),
        // since spawn_cached_with's on_data callback only fires on success.
        let ck2 = ck.clone();
        let set_is_loading = set_is_loading.clone();
        if crate::utils::is_pending(&ck2) {
            crate::utils::on_pending_complete(
                &ck2,
                Box::new(move || {
                    // Guard: skip if the component has unmounted.
                    if alive.get_value() {
                        set_is_loading.set(false);
                    }
                }),
            );
        }
    });

    SharedState {
        data: data.into(),
        loading: loading.into(),
        error: error.into(),
        refresh: Callback::new(move |_| {
            crate::utils::invalidate_cache_prefix(&cache_key);
            crate::utils::spawn_cached_with(
                cache_key.clone(),
                {
                    let f = fetcher.clone();
                    move || {
                        let f = f.clone();
                        async move { f().await }
                    }
                },
                {
                    let set_data = set_data.clone();
                    let set_error = set_error.clone();
                    move |fresh: T| {
                        set_data.set(Some(fresh));
                        set_error.set(None);
                    }
                },
            );
        }),
    }
}
