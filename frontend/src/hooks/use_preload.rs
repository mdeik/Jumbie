use leptos::prelude::*;
use leptos::task::spawn_local;

/// Preloads all API data once on first connect, in prioritized waves.
pub fn use_preload(is_authenticated: ReadSignal<bool>) {
    Effect::new(move |_| {
        if is_authenticated.get() {
            let current_path = web_sys::window()
                .and_then(|w| w.location().pathname().ok())
                .unwrap_or_else(|| format!("/{}", crate::routes::path::SERIES));

            spawn_local(async move {
                background_preload_all(current_path).await;
            });
        }
    });
}

async fn background_preload_all(current_path: String) {
    // Wave 0: current page group
    let current_group = current_path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or(crate::routes::path::SERIES)
        .to_string();
    crate::utils::preload_group_data(&current_group);

    // Small yield so wave 0 requests are in flight before we continue.
    gloo_timers::future::TimeoutFuture::new(0).await;

    // Wave 1A: series details fire at highest priority. Wait 300ms for fetch_series
    // (started in Wave 0) to complete, then fetch ALL series details in one batch
    // request to avoid N individual round-trips. The batch endpoint returns a HashMap
    // keyed by series ID; each entry is cached under fetch_series_details_{id} so the
    // existing use_series_details hook finds them instantly. Skipped on other pages.
    if current_group == crate::routes::path::SERIES {
        // Give the user's initial page time to load and render before background
        // preloading consumes bandwidth.
        gloo_timers::future::TimeoutFuture::new(300).await;
        if let Some(list) =
            crate::utils::read_cache::<Vec<jumbie_shared::types::SeriesInfo>>("fetch_series")
        {
            let ids: Vec<String> = list.into_iter().map(|s| s.id).collect();
            // Only fire the batch if at least one series isn't cached yet.
            let needs_fetch = ids.iter().any(|id| {
                let ck = format!("fetch_series_details_{}", id);
                !crate::utils::API_CACHE.with(|c| c.borrow().contains_key(&ck))
            });
            if needs_fetch {
                let batch = crate::api::fetch_series_details_batch(ids).await;
                if let Ok(details_map) = batch {
                    for (id, details) in details_map {
                        let cache_key = format!("fetch_series_details_{}", id);
                        crate::utils::write_cache(&cache_key, &details);
                    }
                }
            }
        }
    }

    // Wave 1B: cheap globals needed on nearly every page. Delayed so series details
    // get uncontested bandwidth. fetch_series and fetch_quality_profiles are already
    // handled by preload_group_data in Wave 0 — do not duplicate them here.
    gloo_timers::future::TimeoutFuture::new(500).await;

    // Bootstrap: single round-trip for all settings data
    if let Ok(bootstrap) = crate::api::fetch_bootstrap().await {
        crate::utils::write_cache("fetch_config", &bootstrap.config);
        crate::utils::write_cache("fetch_qualities", &bootstrap.qualities);
        crate::utils::write_cache("fetch_quality_profiles", &bootstrap.quality_profiles);
        crate::utils::write_cache("fetch_release_profiles", &bootstrap.release_profiles);
        crate::utils::write_cache("fetch_ui_preferences", &bootstrap.ui_preferences);
        crate::utils::write_cache("fetch_plugins_cfg", &bootstrap.plugins_cfg);
        crate::utils::write_cache("fetch_plugins", &bootstrap.plugins);
        crate::utils::write_cache("fetch_available_plugins", &bootstrap.available_plugins);
        crate::utils::write_cache("fetch_plugin_status", &bootstrap.plugin_status);
    }

    // Sort/filter params are omitted so the endpoints apply the stored
    // preferences — matching the tables' own first request.
    crate::utils::preload_api_cache("fetch_wanted_episodes:0".to_string(), || {
        crate::api::fetch_wanted_episodes(0, 50, None, None)
    });
    crate::utils::preload_api_cache("fetch_activity:0".to_string(), || {
        crate::api::fetch_activity(0, 50, None, None, None)
    });
    crate::utils::preload_api_cache("fetch_health".to_string(), || crate::api::fetch_health());

    // Wait for wave 1
    gloo_timers::future::TimeoutFuture::new(200).await;

    // Plugin schema preloading: batch-fetch all schemas in one request, then unpack
    // each into fetch_plugin_schema_{type} so DynamicPluginForm finds them instantly.
    {
        if let Ok(schemas) = crate::api::fetch_all_plugin_schemas().await {
            for (plugin_type, schema) in schemas {
                let cache_key = format!("fetch_plugin_schema_{}", plugin_type);
                crate::utils::write_cache(&cache_key, &schema);
            }
        }
    }

    // Wave 2: heavier / less-frequently visited endpoints
    crate::utils::preload_api_cache("fetch_qualities".to_string(), || {
        crate::api::fetch_qualities()
    });
    crate::utils::preload_api_cache("fetch_rename_queue".to_string(), || {
        crate::api::fetch_rename_queue()
    });
    crate::utils::preload_api_cache("fetch_download_queue".to_string(), || {
        crate::api::fetch_download_queue()
    });
    crate::utils::preload_api_cache("fetch_organized_series".to_string(), || {
        crate::api::fetch_organized_series()
    });
    crate::utils::preload_api_cache("fetch_plugin_status".to_string(), || {
        crate::api::fetch_plugin_status()
    });
    crate::utils::preload_api_cache("fetch_logs:0".to_string(), || {
        crate::api::fetch_logs(0, 100, None, None, None)
    });
    crate::utils::preload_api_cache("fetch_about".to_string(), || crate::api::fetch_about());

    // Calendar — preload a ~3-month window, key aligned with calendar.rs's Effect
    crate::utils::preload::preload_calendar();
}
