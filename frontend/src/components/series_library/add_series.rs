use crate::components::common::form_fields::{
    CheckboxInput, Select, SelectOption, TextInput, field_validation,
};
use crate::components::common::icons::{ArrowLeftIcon, PlusIcon};
use crate::components::common::toast::{show_error, show_success, show_warning};
use crate::hooks::use_config::{ConfigContext, use_config};
use crate::routes::path;
use jumbie_shared::config::PluginsConfig;
use jumbie_shared::plugin::{Capability, PluginInstanceInfo};
use jumbie_shared::types::{CreateSeriesRequest, DEFAULT_MONITOR_MODE, MonitorMode};
use leptos::control_flow::Show;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::*;
use std::collections::HashMap;

/// Decode a URL component for production (WASM).
fn decode_url_component(s: &str) -> String {
    // Replace form-urlencoded spaces BEFORE percent-decode
    // so that %2B (literal +) is distinct from + (space).
    js_sys::decode_uri_component(&s.replace('+', " "))
        .ok()
        .and_then(|s| s.as_string())
        .unwrap_or_default()
}

/// Parse the query string from `?title=X&tvdb=Y` into (title, metadata_key→value map).
///
/// Uses `decode` to decode each component — injected for testability.
fn parse_add_series_query_with(
    query: &str,
    decode: &dyn Fn(&str) -> String,
) -> (String, HashMap<String, String>) {
    let mut title = String::new();
    let mut meta = HashMap::new();

    let search = query.trim_start_matches('?');
    for pair in search.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        let raw_key = parts.next().unwrap_or("");
        let raw_val = parts.next().unwrap_or("");

        let key = decode(raw_key);
        if key.is_empty() {
            continue;
        }
        let val = decode(raw_val);
        if val.is_empty() {
            continue;
        }

        if key == "title" {
            title = val;
        } else {
            meta.insert(key, val);
        }
    }

    (title, meta)
}

/// Convenience wrapper using the real WASM URL decoder.
fn parse_add_series_query(query: &str) -> (String, HashMap<String, String>) {
    parse_add_series_query_with(query, &decode_url_component)
}

// Survives page refresh so the user doesn't have to re-select profiles and root on every visit.
const ADD_SERIES_LAST_VALUES_KEY: &str = "jb_add_series_last_values";

#[derive(serde::Serialize, serde::Deserialize)]
struct AddSeriesLastValues {
    quality_profile: Option<String>,
    release_profile: Option<String>,
    monitor_mode: Option<String>,
    destination_root: Option<String>,
    search_missing_on_add: Option<bool>,
}

fn save_last_values(qp: &str, rp: &str, mm: &str, dr: &str, sm: bool) {
    // Load previous values to preserve destination_root when using a custom path.
    let prev = load_last_values().unwrap_or(AddSeriesLastValues {
        quality_profile: None,
        release_profile: None,
        monitor_mode: None,
        destination_root: None,
        search_missing_on_add: None,
    });

    let values = AddSeriesLastValues {
        quality_profile: Some(qp.to_string()),
        release_profile: Some(rp.to_string()),
        monitor_mode: Some(mm.to_string()),
        // Don't remember custom paths — preserve the last valid root instead.
        destination_root: if dr == "__CUSTOM__" {
            prev.destination_root
        } else {
            Some(dr.to_string())
        },
        search_missing_on_add: Some(sm),
    };
    if let Ok(json) = serde_json::to_string(&values)
        && let Some(window) = web_sys::window()
        && let Ok(Some(storage)) = window.local_storage()
    {
        let _ = storage.set_item(ADD_SERIES_LAST_VALUES_KEY, &json);
    }
}

fn load_last_values() -> Option<AddSeriesLastValues> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(ADD_SERIES_LAST_VALUES_KEY).ok().flatten())
        .and_then(|json| serde_json::from_str::<AddSeriesLastValues>(&json).ok())
}

#[component]
pub fn AddSeries() -> impl IntoView {
    let ConfigContext { config, .. } = use_config();
    let navigate = use_navigate();
    let (series_name, set_series_name) = signal(String::new());

    // Read directly from window.location so it works reliably on full page loads.
    let (query_title, query_metadata_ids) = {
        let search = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .unwrap_or_default();
        parse_add_series_query(&search)
    };

    if !query_title.is_empty() {
        set_series_name.set(query_title.clone());
    }

    let saved = load_last_values();
    let (quality_profile, set_quality_profile) = signal(
        saved
            .as_ref()
            .and_then(|s| s.quality_profile.clone())
            .unwrap_or_default(),
    );
    let (release_profile, set_release_profile) = signal(
        saved
            .as_ref()
            .and_then(|s| s.release_profile.clone())
            .unwrap_or_default(),
    );
    let (selected_dest_root, set_selected_dest_root) = signal(
        saved
            .as_ref()
            .and_then(|s| s.destination_root.clone())
            .unwrap_or_default(),
    );
    let (custom_path, set_custom_path) = signal(String::new());
    let is_windows = crate::hooks::use_server_os();
    // SSoT: the shared path policy engine sanitizes for the SERVER's OS, so
    // the preview matches the folder the backend will create.
    let platform_os = crate::hooks::use_platform_os();

    let (monitor_mode_str, set_monitor_mode_str) = signal(
        saved
            .as_ref()
            .and_then(|s| s.monitor_mode.clone())
            .unwrap_or_else(|| DEFAULT_MONITOR_MODE.as_str().to_string()),
    );

    let monitor_mode = Memo::new(move |_| {
        crate::utils::parse_monitor_mode(&monitor_mode_str.get()).unwrap_or(MonitorMode::None)
    });

    let monitor_help_text = Signal::derive(move || monitor_mode.get().help_text().to_string());

    let monitor_options = Signal::derive(|| {
        crate::constants::MONITOR_OPTIONS
            .iter()
            .map(|(v, l)| SelectOption::from((v.to_string(), l.to_string())))
            .collect::<Vec<_>>()
    });

    let (quality_profiles, set_quality_profiles) = signal(std::collections::HashMap::new());
    let (release_profiles, set_release_profiles) = signal(std::collections::HashMap::new());

    crate::utils::create_mount_resource(move || async move {
        if let Ok(profiles) = crate::api::fetch_quality_profiles().await {
            set_quality_profiles.set(profiles);
        }
        if let Ok(profiles) = crate::api::fetch_release_profiles().await {
            set_release_profiles.set(profiles);
        }
    });

    let (metadata_plugins, set_metadata_plugins) = signal(Vec::<PluginInstanceInfo>::new());
    let (plugins_cfg, set_plugins_cfg) = signal(None::<PluginsConfig>);
    let (metadata_ids, set_metadata_ids) = signal(HashMap::<String, String>::new());

    crate::utils::use_api_cache(
        "fetch_plugins".to_string(),
        || crate::api::fetch_plugins(),
        move |plugins: Vec<PluginInstanceInfo>| {
            let meta = plugins
                .into_iter()
                .filter(|p| {
                    p.capabilities.contains(&Capability::MetadataProviderNormal)
                        || p.capabilities
                            .contains(&Capability::MetadataProviderAbsolute)
                })
                .collect();
            set_metadata_plugins.set(meta);
        },
    );

    crate::utils::use_api_cache(
        "fetch_plugins_cfg".to_string(),
        || crate::api::fetch_plugins_cfg(),
        move |c| {
            set_plugins_cfg.set(Some(c));
        },
    );

    // One PluginInstanceInfo per enabled metadata INSTANCE (not per plugin TYPE).
    // Identity is backend-stamped: `plugin_id` is the type id (e.g. "jumbie.tvdb")
    // and `instance_id` is used as the key in `metadata_ids`. See
    // `resolve_active_metadata_plugins`.
    let active_metadata_plugins = Signal::derive(move || {
        let all_plugins = metadata_plugins.get();
        if let Some(cfg) = plugins_cfg.get() {
            crate::utils::resolve_active_metadata_plugins(all_plugins, &cfg)
        } else {
            Vec::new()
        }
    });

    // Any query param whose key matches a plugin's type key (e.g. `tvdb` matches
    // `plugin_id = "jumbie.tvdb"`) pre-fills that plugin's ID field. Unknown
    // params are ignored — forward compatible with future plugins.
    let pending_meta = query_metadata_ids.clone();
    if !pending_meta.is_empty() {
        Effect::new(move |_| {
            let plugins = active_metadata_plugins.get();
            if plugins.is_empty() {
                return;
            }
            for (qkey, qval) in &pending_meta {
                // SSoT: derive the short type name from the backend-stamped
                // plugin_id (type id), e.g. "tvdb" from "jumbie.tvdb".
                if let Some(plugin) = plugins.iter().find(|p| {
                    p.plugin_id
                        .as_deref()
                        .map(jumbie_shared::plugin::plugin_type_key)
                        == Some(qkey.as_str())
                }) {
                    let key = plugin
                        .instance_id
                        .clone()
                        .unwrap_or_else(|| plugin.display_name.clone());
                    set_metadata_ids.update(|ids| {
                        ids.entry(key).or_insert_with(|| qval.clone());
                    });
                }
            }
        });
    }

    let quality_options = crate::utils::derive_quality_options(quality_profiles.into());
    let release_options = crate::utils::derive_release_options(release_profiles.into());

    let has_active_metadata = Signal::derive(move || !active_metadata_plugins.get().is_empty());
    // The checkbox is only meaningful once at least one metadata ID is entered
    // (a search requires a metadata sync first). Without an ID it is disabled,
    // and a remembered `true` must not be sent to the backend — see
    // `search_missing_effective`, which gates on an ID without discarding the
    // remembered preference. The emptiness rule is applied once here; both the
    // checkbox gate (`has_any_metadata_id`) and the request payload derive from
    // this filtered collection.
    let non_empty_metadata_ids = Memo::new(move |_| {
        metadata_ids
            .get()
            .into_iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .collect::<HashMap<String, String>>()
    });
    let has_any_metadata_id = Signal::derive(move || !non_empty_metadata_ids.get().is_empty());
    let (search_missing_on_add, set_search_missing_on_add) = signal(
        saved
            .as_ref()
            .and_then(|s| s.search_missing_on_add)
            .unwrap_or(false),
    );
    // The remembered preference stays intact while the checkbox is disabled.
    // `search_missing_effective` — what the user sees and what we submit — also
    // requires a metadata ID, so a remembered `true` is never shown active (or
    // sent) without one, yet it is restored once an ID is entered.
    let search_missing_effective =
        Signal::derive(move || search_missing_on_add.get() && has_any_metadata_id.get());

    Effect::new(move |_| {
        if quality_profile.get_untracked().is_empty()
            && let Some(first) = quality_options.get().first()
            && let Some(val) = first.first_value()
        {
            set_quality_profile.set(val);
        }
        if release_profile.get_untracked().is_empty()
            && let Some(first) = release_options.get().first()
            && let Some(val) = first.first_value()
        {
            set_release_profile.set(val);
        }
    });

    let dest_root_options = Signal::derive(move || {
        let mut opts = Vec::new();
        if let Some(c) = config.get() {
            for root in &c.organization.destination_roots {
                let p = root.path.to_string_lossy().to_string();
                opts.push(SelectOption::from((p.clone(), p)));
            }
        }
        opts.push(SelectOption::from((
            "__CUSTOM__".to_string(),
            "Custom Path...".to_string(),
        )));
        opts
    });

    Effect::new(move |_| {
        if selected_dest_root.get_untracked().is_empty()
            && let Some(first) = dest_root_options.get().first()
            && let Some(val) = first.first_value()
        {
            set_selected_dest_root.set(val);
        }
    });

    // If a saved profile/root no longer exists (e.g. deleted since last visit),
    // clear it so the fallback effects above pick the first option.
    //
    // `len > 1` guards the validation: `build_sorted_profile_options` always
    // adds a "None" placeholder and `dest_root_options` always adds "Custom
    // Path...", so before real data arrives these are the only entries
    // (`len == 1`). We skip validation until real options arrive so the saved
    // value isn't spuriously cleared before the API call completes.
    Effect::new(move |_| {
        let qp = quality_profile.get_untracked();
        let q_opts = quality_options.get();
        if !qp.is_empty() && q_opts.len() > 1 {
            let exists = q_opts.iter().any(|o| o.first_value() == Some(qp.clone()));
            if !exists {
                // Saved profile is gone — fall back here, because the fallback
                // effect already ran this cycle and saw the signal as non-empty.
                set_quality_profile.set(
                    q_opts
                        .first()
                        .and_then(|o| o.first_value())
                        .unwrap_or_default(),
                );
            }
        }
    });

    Effect::new(move |_| {
        let rp = release_profile.get_untracked();
        let r_opts = release_options.get();
        if !rp.is_empty() && r_opts.len() > 1 {
            let exists = r_opts.iter().any(|o| o.first_value() == Some(rp.clone()));
            if !exists {
                set_release_profile.set(
                    r_opts
                        .first()
                        .and_then(|o| o.first_value())
                        .unwrap_or_default(),
                );
            }
        }
    });

    Effect::new(move |_| {
        let dr = selected_dest_root.get_untracked();
        if !dr.is_empty() && dr != "__CUSTOM__" {
            let opts = dest_root_options.get();
            // Only validate after real destination roots have been loaded
            // (at least one option beyond "Custom Path...").
            if opts.len() > 1 {
                let exists = opts.iter().any(|o| o.first_value() == Some(dr.clone()));
                if !exists {
                    // Root removed — fall back to first root ourselves for
                    // the same reason as the profile effects above.
                    set_selected_dest_root.set(
                        opts.first()
                            .and_then(|o| o.first_value())
                            .unwrap_or_default(),
                    );
                }
            } else if opts.len() == 1 && config.get().is_some() {
                // Config has loaded but there are no destination roots.
                // The saved root is stale (it no longer exists), so force
                // "Custom Path..." — the only valid choice in the options.
                if let Some(val) = opts.first().and_then(|o| o.first_value()) {
                    set_selected_dest_root.set(val);
                }
            }
        }
    });

    // Generate the default path for the series name.
    //
    // Surrounding whitespace is treated as accidental, so the preview shows the
    // trimmed path the backend will receive; the input field itself is left
    // untouched mid-typing.
    //
    // Sanitisation goes through `jumbie_shared::paths::sanitize_with_policy` so
    // the preview and the backend share one engine (the org illegal-char policy
    // for the server's OS); `validate-path` remains the SSoT for the final
    // collision-resolved name.
    let default_path_fn = move || {
        let name = series_name.get();
        let trimmed = name.trim();
        if trimmed.is_empty() {
            String::new()
        } else {
            // Config fallback mirrors the backend's OrganizationConfig::default()
            // so the preview is still meaningful before config loads.
            let (policy, allow_platform_specific) = match config.get() {
                Some(c) => (
                    c.organization.illegal_char_policy,
                    c.organization.allow_platform_specific_chars,
                ),
                None => (
                    jumbie_shared::config::organization::InvalidCharPolicy::Underscore,
                    false,
                ),
            };
            let sanitized = jumbie_shared::paths::sanitize_with_policy(
                trimmed,
                &policy,
                allow_platform_specific,
                platform_os.get(),
            );

            let root = selected_dest_root.get();
            if root == "__CUSTOM__" || root.is_empty() {
                String::new()
            } else {
                let p = std::path::PathBuf::from(root).join(&sanitized);
                p.to_string_lossy().to_string()
            }
        }
    };

    // Backend validation (rather than client-side string comparison) performs
    // canonical path comparison, catching symlink-equivalent directories and
    // bind mounts. The same endpoint powers the Edit Series Path modal — the
    // SSoT for path validity and collision resolution.
    let (path_validation_msg, set_path_validation_msg) = signal(String::new());
    let (path_validating, set_path_validating) = signal(false);
    // Effective path reported by the backend after folder-level collision
    // resolution (rename → suffixed folder). Keyed by the candidate path it was
    // computed for so a stale response can never leak into the preview for a
    // different candidate.
    let (resolved_path_for, set_resolved_path_for) = signal((String::new(), None::<String>));

    let candidate_path = Signal::derive(move || {
        let is_custom = selected_dest_root.get() == "__CUSTOM__";
        if is_custom {
            custom_path.get()
        } else {
            default_path_fn()
        }
    });

    // The path that will ACTUALLY be created: the backend's collision-resolved
    // path when a fresh validation response exists for the current candidate,
    // else the locally-derived candidate. Preview, submitted request, and the
    // optimistic library cache all derive from this single path-generation logic;
    // the backend remains the SSoT for collision resolution.
    let effective_path = Signal::derive(move || {
        let candidate = candidate_path.get();
        if candidate.is_empty() {
            return String::new();
        }
        let (for_path, resolved) = resolved_path_for.get();
        if for_path == candidate {
            resolved.unwrap_or(candidate)
        } else {
            candidate
        }
    });

    let submit_action = Action::new_local(move |req: &CreateSeriesRequest| {
        let req = req.clone();
        async move { crate::api::create_series_with_request(req).await }
    });

    let pending = submit_action.pending();

    // Busy while the single create request is in-flight; the backend performs
    // the metadata sync (and optional search-on-add) inside the same request.
    let busy = Signal::derive(move || pending.get());

    let nav_success = navigate.clone();
    Effect::new(move |_| {
        if let Some(res) = submit_action.value().get() {
            let name = series_name.get_untracked();
            let qp = quality_profile.get_untracked();
            let rp = release_profile.get_untracked();
            match res {
                Ok(uuid) => {
                    // Remember last used values (except title) as defaults for next time.
                    save_last_values(
                        &quality_profile.get_untracked(),
                        &release_profile.get_untracked(),
                        &monitor_mode_str.get_untracked(),
                        &selected_dest_root.get_untracked(),
                        search_missing_on_add.get_untracked(),
                    );

                    // The path the series actually landed at, reusing the preview's
                    // path-generation logic: `effective_path` is the candidate
                    // collision-resolved by the backend's validate-path response.
                    // Keeps the optimistic cache entry truthful even when rename
                    // handling suffixed the folder.
                    let submitted_path = effective_path.get_untracked();

                    // Insert the new series directly into the cached list so the
                    // library shows it immediately without a network refetch.
                    if let Some(mut series_list) = crate::utils::read_cache::<
                        Vec<jumbie_shared::types::SeriesInfo>,
                    >("fetch_series")
                    {
                        let new_series = jumbie_shared::types::SeriesInfo {
                            id: uuid.clone(),
                            title: name.clone(),
                            seasons: Vec::new(),
                            season_count: 0,
                            release_profile: rp,
                            quality_profile: qp,
                            episodes_counts: (0, 0),
                            monitored_missing_count: 0,
                            queued_count: 0,
                            size: 0,
                            has_not_found_files: false,
                            path: submitted_path,
                            scan_queue_count: 0,
                            absolute_numbering: false,
                            aliases: Vec::new(),
                        };
                        series_list.push(new_series);
                        crate::utils::write_cache("fetch_series", &series_list);
                    }

                    // The backend handled metadata sync (and optional search-on-add)
                    // inside the create request, so the response means the flow completed.
                    show_success(format!("'{}' added to library successfully!", name));
                    nav_success(
                        &format!("/{}/{}/edit", path::SERIES, uuid),
                        Default::default(),
                    );
                }
                Err(e) => {
                    if let Some(id) = e.series_id() {
                        // The add actually succeeded server-side — most commonly
                        // the previous request timed out after the series was
                        // created, and the visible-series claim rejects the
                        // re-submit. Take the user to the existing series
                        // instead of a dead-end "failed" toast.
                        show_warning(format!(
                            "'{}' already exists at this path — opening it",
                            name
                        ));
                        nav_success(
                            &format!("/{}/{}/edit", path::SERIES, id),
                            Default::default(),
                        );
                    } else {
                        crate::components::common::toast::show_error(format!(
                            "Failed to add series: {}",
                            e.user_message()
                        ));
                    }
                }
            }
        }
    });

    // Debounced validation: call the backend whenever the candidate path changes
    Effect::new(move |_| {
        // Track config so a settings change (collision strategy / illegal-char
        // policy) re-validates the path and updates the preview live.
        let _ = config.get();
        let path = candidate_path.get();
        if path.is_empty() {
            set_path_validation_msg.set(String::new());
            return;
        }

        set_path_validating.set(true);
        // resolve_collisions only for non-custom paths: root-derived folder names
        // go through the org collision config (rename/skip/overwrite), whereas
        // custom paths are explicit user choices and keep claim-or-reject behavior
        // (matching the batch-move modal).
        let resolve_collisions = selected_dest_root.get_untracked() != "__CUSTOM__";
        // Clone: `path` is moved into the request payload, and the response must
        // be keyed by the exact candidate it was computed for.
        let key_path = path.clone();
        // spawn_local: the Effect fires synchronously on each signal change, so
        // spawning lets the validation HTTP call run without blocking the reactive graph.
        spawn_local(async move {
            match crate::api::validate_path(jumbie_shared::types::ValidatePathPayload {
                path,
                series_id: None, // New series — no existing ID to exclude
                resolve_collisions,
            })
            .await
            {
                Ok(res) => {
                    set_path_validation_msg.set(if res.is_valid {
                        String::new()
                    } else {
                        res.message
                    });
                    set_resolved_path_for.set((key_path, res.resolved_path));
                }
                Err(_) => {
                    set_path_validation_msg.set(String::new());
                    set_resolved_path_for.set((key_path, None));
                }
            }
            set_path_validating.set(false);
        });
    });

    let path_conflict = Signal::derive(move || !path_validation_msg.get().is_empty());

    view! {
        <div class="view active" id="addSeries">
            <div class="edit-header">
                <div class="back-btn" on:click={
                    let nav = navigate.clone();
                    move |_| nav(&format!("/{}", path::SERIES), Default::default())
                }>
                    <span class="icon"><ArrowLeftIcon /></span>
                    <span>"Back to Library"</span>
                </div>
            </div>

            <div class="edit-content active" id="add-series-content">
                <div class="info-section">
                    <p class="text-muted-color mb-xl">
                        "Enter the series name. The system will create a folder and set up the series for monitoring and downloading."
                    </p>

                    <TextInput
                        id="seriesName".to_string()
                        label="Series Name *".to_string()
                        value=series_name
                        set_value=move |v| set_series_name.set(v)
                        placeholder="Example Series".to_string()
                    />

                    {move || {
                        let plugins = active_metadata_plugins.get();
                        if plugins.is_empty() {
                            return ().into_any();
                        }
                        plugins.into_iter().map(|plugin| {
                            let plugin_name = plugin.display_name.clone();
                            let label = plugin.series_identifier_label.clone()
                                .unwrap_or_else(|| format!("{} ID", plugin.display_name));
                            let field_id = format!("add_metadata_{}", plugin_name);
                            let key = plugin.instance_id.clone()
                                .unwrap_or_else(|| plugin_name.clone());
                            let value_sig = Signal::derive({
                                let k = key.clone();
                                move || metadata_ids.with(|ids| ids.get(&k).cloned().unwrap_or_default())
                            });
                            view! {
                                <TextInput
                                    label=label
                                    id=field_id
                                    value=value_sig
                                    placeholder=plugin.series_identifier_placeholder.clone().unwrap_or_default()
                                    set_value=move |v: String| {
                                        set_metadata_ids.update(|ids| {
                                            if v.trim().is_empty() {
                                                ids.remove(&key);
                                            } else {
                                                ids.insert(key.clone(), v);
                                            }
                                        });
                                    }
                                />
                            }.into_any()
                        }).collect::<Vec<_>>().into_any()
                    }}

                    <Select
                        id="monitorMode".to_string()
                        label="Monitor Mode".to_string()
                        value=monitor_mode_str
                        set_value=move |v| set_monitor_mode_str.set(v)
                        options=monitor_options
                        help_text=monitor_help_text
                    />

                    <Select
                        id="addQualityProfile".to_string()
                        label="Quality Profile".to_string()
                        value=quality_profile
                        set_value=move |v| set_quality_profile.set(v)
                        options=quality_options
                    />

                    <Select
                        id="addReleaseProfile".to_string()
                        label="Release Profile".to_string()
                        value=release_profile
                        set_value=move |v| set_release_profile.set(v)
                        options=release_options
                    />

                    <Select
                        id="destinationRoot".to_string()
                        label="Destination Root".to_string()
                        value=selected_dest_root
                        set_value=move |v| set_selected_dest_root.set(v)
                        options=dest_root_options
                    />

                    <Show when=move || selected_dest_root.get() == "__CUSTOM__">
                    <TextInput
                        id="customPath".to_string()
                        label="Custom Path *".to_string()
                        value=custom_path
                        set_value=move |v| set_custom_path.set(v)
                        placeholder=move || crate::hooks::path_placeholder(is_windows.get(), crate::hooks::EXAMPLE_SERIES_PATH)
                        error=path_validation_msg
                    />
                </Show>

                    <Show when=move || selected_dest_root.get() != "__CUSTOM__" && !series_name.get().is_empty()>
                        <div class="text-muted-color">
                            <span>"Will create: "</span>
                            <code class="text-primary-color path-preview">{effective_path}</code>
                            <div id="add-series-validation-placeholder" style:height=move || if path_validation_msg.get().is_empty() { "1.25rem" } else { "" }>
                            {field_validation(path_validation_msg)}
                                                        </div>
                            </div>

                    </Show>

                    <Show when=move || has_active_metadata.get()>
                            <CheckboxInput
                            id="searchMissingOnAdd".to_string()
                            label="Search Missing When Added".to_string()
                            checked=search_missing_effective
                            set_checked=move |v| set_search_missing_on_add.set(v)
                            disabled=Signal::derive(move || !has_any_metadata_id.get())
                            help_text="After adding and syncing metadata, auto-search for episodes that are monitored and missing from disk (requires a metadata ID)".to_string()
                        />
                    </Show>

                    <button
                        class="btn btn-primary mt-md"
                        prop:disabled=move || busy.get() || series_name.get().is_empty() || path_conflict.get() || path_validating.get() || (selected_dest_root.get() == "__CUSTOM__" && custom_path.get().is_empty())
                        on:click=move |_| {
                            let name = series_name.get();
                            let trimmed_name = name.trim().to_string();
                            let is_custom_root = selected_dest_root.get() == "__CUSTOM__";
                            let custom = custom_path.get();
                            let trimmed_custom = custom.trim().to_string();

                            if trimmed_name.is_empty() {
                                show_warning("Series name is required");
                                return;
                            }

                            if is_custom_root && trimmed_custom.is_empty() {
                                show_warning("Custom path is required when using custom directory");
                                return;
                            }

                            // Validate the trimmed name — the actual value that will be stored.
                            if let Err(e) = crate::validation::validate_title(&trimmed_name) {
                                show_error(&e);
                                return;
                            }

                            let path = if is_custom_root {
                                trimmed_custom
                            } else {
                                // default_path_fn trims internally, keeping path
                                // generation in one place. Send the raw candidate (not
                                // `effective_path`): create_series re-resolves folder
                                // collisions via the same SSoT helper at submit time,
                                // avoiding a double-suffix race if the resolved name was
                                // taken between validation and submission.
                                default_path_fn()
                            };

                            let request = CreateSeriesRequest {
                                series_name: Some(trimmed_name.clone()),
                                path,
                                scan_for_existing: Some(true),
                                monitor_mode: Some(monitor_mode.get()),
                                quality_profile: Some(quality_profile.get()),
                                release_profile: Some(release_profile.get()),
                                metadata_ids: non_empty_metadata_ids.get(),
                                search_missing_on_add: search_missing_effective.get(),
                                // Mirrors the validate-path flag: root-derived
                                // folder names go through the backend policy
                                // (sanitize + collision resolve); explicit custom
                                // paths are honored verbatim.
                                resolve_collisions: !is_custom_root,
                                settings: Default::default(),
                            };

                            submit_action.dispatch(request);
                        }
                    >
                        <Show when=move || !busy.get() fallback=|| view! { "Adding..." }>
                            <span class="icon"><PlusIcon /></span>
                            "Add Series"
                        </Show>
                    </button>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pure-Rust URL-decoder so tests work outside WASM.
    fn decode(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '+' => result.push(' '),
                '%' => {
                    let hi = chars.next().and_then(|c| c.to_digit(16)).unwrap_or(0);
                    let lo = chars.next().and_then(|c| c.to_digit(16)).unwrap_or(0);
                    result.push(char::from_u32((hi << 4) | lo).unwrap_or('\u{FFFD}'));
                }
                other => result.push(other),
            }
        }
        result
    }

    fn parse(query: &str) -> (String, HashMap<String, String>) {
        parse_add_series_query_with(query, &decode)
    }

    #[test]
    fn empty_query_returns_defaults() {
        let (title, meta) = parse("");
        assert!(title.is_empty());
        assert!(meta.is_empty());
    }

    #[test]
    fn title_only() {
        let (title, meta) = parse("?title=The+Series");
        assert_eq!(title, "The Series");
        assert!(meta.is_empty());
    }

    #[test]
    fn title_and_metadata_key() {
        let (title, meta) = parse("?title=A+Show&tvdb=12345");
        assert_eq!(title, "A Show");
        assert_eq!(meta.get("tvdb").map(|s| s.as_str()), Some("12345"));
        assert_eq!(meta.len(), 1);
    }

    #[test]
    fn multiple_metadata_keys() {
        let (title, meta) = parse("?title=Test&tvdb=100&tvmaze=200");
        assert_eq!(title, "Test");
        assert_eq!(meta.get("tvdb").map(|s| s.as_str()), Some("100"));
        assert_eq!(meta.get("tvmaze").map(|s| s.as_str()), Some("200"));
        assert_eq!(meta.len(), 2);
    }

    #[test]
    fn encoded_spaces_in_value() {
        let (title, meta) = parse("?title=Long+Name+Here&tag=value+with+spaces");
        assert_eq!(title, "Long Name Here");
        assert_eq!(
            meta.get("tag").map(|s| s.as_str()),
            Some("value with spaces")
        );
    }

    #[test]
    fn percent_encoded_characters() {
        let (title, meta) = parse("?title=Special%26%3D%2B&id=a%2Fb%3Fc");
        assert_eq!(title, "Special&=+");
        assert_eq!(meta.get("id").map(|s| s.as_str()), Some("a/b?c"));
    }

    #[test]
    fn literal_plus_not_corrupted_by_form_encoding() {
        // %2B is encoded +, while + is form-urlencoded space
        let (title, _) = parse("?title=C%2B%2B+Language");
        assert_eq!(title, "C++ Language");
    }

    #[test]
    fn no_question_mark_prefix() {
        let (title, meta) = parse("title=NoPrefix&tvdb=1");
        assert_eq!(title, "NoPrefix");
        assert_eq!(meta.get("tvdb").map(|s| s.as_str()), Some("1"));
    }

    #[test]
    fn missing_value_skipped() {
        let (title, meta) = parse("?title=Val&empty=&tvdb=99");
        assert_eq!(title, "Val");
        assert_eq!(meta.len(), 1);
        assert_eq!(meta.get("tvdb").map(|s| s.as_str()), Some("99"));
    }

    #[test]
    fn only_meta_no_title() {
        let (title, meta) = parse("?tvdb=42");
        assert!(title.is_empty());
        assert_eq!(meta.get("tvdb").map(|s| s.as_str()), Some("42"));
    }

    #[test]
    fn encoded_key() {
        let (_, meta) = parse("?%74%76%64%62=1");
        assert_eq!(meta.get("tvdb").map(|s| s.as_str()), Some("1"));
    }

    #[test]
    fn unknown_keys_are_collected() {
        let (title, meta) = parse("?title=X&unknown_key=val&foo=bar");
        assert_eq!(title, "X");
        assert_eq!(meta.len(), 2);
        assert_eq!(meta.get("unknown_key").map(|s| s.as_str()), Some("val"));
        assert_eq!(meta.get("foo").map(|s| s.as_str()), Some("bar"));
    }

    #[test]
    fn duplicate_key_last_wins() {
        let (title, meta) = parse("?title=First&title=Second&tvdb=1&tvdb=2");
        assert_eq!(title, "Second");
        assert_eq!(meta.get("tvdb").map(|s| s.as_str()), Some("2"));
    }

    #[test]
    fn stray_question_mark_only() {
        let (title, meta) = parse("?");
        assert!(title.is_empty());
        assert!(meta.is_empty());
    }

    #[test]
    fn leading_ampersand_skipped() {
        let (title, meta) = parse("?&title=Val&tvdb=1");
        assert_eq!(title, "Val");
        assert_eq!(meta.get("tvdb").map(|s| s.as_str()), Some("1"));
        assert_eq!(meta.len(), 1);
    }

    #[test]
    fn all_params_empty_returns_defaults() {
        let (title, meta) = parse("?title=&tvdb=&extra=");
        assert!(title.is_empty());
        assert!(meta.is_empty());
    }

    #[test]
    fn no_equals_sign_is_skipped() {
        let (title, meta) = parse("?title&tvdb=1");
        // "title" has no value → splitn returns ["title"] only → val = "" → skipped
        assert!(title.is_empty());
        assert_eq!(meta.get("tvdb").map(|s| s.as_str()), Some("1"));
        assert_eq!(meta.len(), 1);
    }

    #[test]
    fn synthetic_plugin_type_key_returns_tvdb() {
        // Backend-stamped instance infos carry both plugin_id (type id) and
        // instance_id (instance id); type_key() correctly returns "tvdb" from
        // the plugin_id = "jumbie.tvdb".
        let synthetic = PluginInstanceInfo {
            plugin_id: Some("jumbie.tvdb".to_string()),
            instance_id: Some("550e8400-e29b-41d4-a716-446655440000".to_string()),
            display_name: "My TVDB".to_string(),
            version: String::new(),
            author: String::new(),
            description: String::new(),
            capabilities: Vec::new(),
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        };
        assert_eq!(
            synthetic
                .plugin_id
                .as_deref()
                .map(jumbie_shared::plugin::plugin_type_key),
            Some("tvdb")
        );
        assert_eq!(
            synthetic.instance_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn type_key_matching_works_on_synthetic_plugins() {
        // Simulates the same logic the Effect uses: match "tvdb" via type_key(),
        // then use instance_id as the key for metadata_ids.
        let synthetic = PluginInstanceInfo {
            plugin_id: Some("jumbie.tvdb".to_string()),
            instance_id: Some("uuid-tvdb-1".to_string()),
            display_name: "TVDB".to_string(),
            version: String::new(),
            author: String::new(),
            description: String::new(),
            capabilities: Vec::new(),
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        };
        let synthetics = [synthetic];

        let pending: HashMap<String, String> = [("tvdb".to_string(), "12345".to_string())]
            .into_iter()
            .collect();

        let mut result = HashMap::new();
        for (qkey, qval) in &pending {
            if let Some(plugin) = synthetics.iter().find(|p| {
                p.plugin_id
                    .as_deref()
                    .map(jumbie_shared::plugin::plugin_type_key)
                    == Some(qkey.as_str())
            }) {
                let key = plugin
                    .instance_id
                    .clone()
                    .unwrap_or_else(|| plugin.display_name.clone());
                result.insert(key, qval.clone());
            }
        }

        assert_eq!(result.len(), 1);
        assert_eq!(result.get("uuid-tvdb-1").map(|s| s.as_str()), Some("12345"));
    }

    #[test]
    fn unknown_keys_do_not_match_any_plugin() {
        // The parsing itself collects unknown keys; the Effect silenty ignores
        // keys that don't match any plugin's type key (derived from plugin_id).
        // This test validates the parsing layer: unknown keys are collected,
        // but won't cause errors when no matching plugin exists.
        let (title, meta) = parse("?title=Test&nonexistent_plugin=123");
        assert_eq!(title, "Test");
        assert_eq!(meta.len(), 1);
        assert_eq!(
            meta.get("nonexistent_plugin").map(|s| s.as_str()),
            Some("123")
        );
        // The Effect will iterate meta — no match → no metadata_ids set.
        // This is correct: unknown keys are silently ignored, not crashing.
    }
}
