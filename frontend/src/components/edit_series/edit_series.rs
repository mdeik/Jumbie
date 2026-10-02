use super::advanced_tab::AdvancedTab;
use super::edit_series_path_modal::EditSeriesPathModal;
use super::episodes_tab::EpisodesTab;
use super::general_tab::GeneralTab;
use crate::components::edit_series::search_modal::SearchModal;
use crate::components::series_library::remove_series_modal::RemoveSeriesModal;

use crate::components::common::error_page::ErrorPage;
use crate::components::common::form_fields::SelectOption;
use crate::components::edit_series::episode_details_modal::EpisodeDetailsModal;
use crate::components::edit_series::manage_episodes_modal::ManageEpisodesModal;
use crate::components::edit_series::season_manage_modal::SeasonManageModal;

use crate::components::common::icons::ArrowLeftIcon;
use crate::components::common::skeleton::EditSeriesSkeleton;
use crate::components::common::toast::NotificationType;
use crate::routes::path;
use jumbie_shared::types::{
    DEFAULT_MONITOR_MODE, EpisodeViewModel, MonitorMode, SeasonOverride, SeriesDetails,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::*;
use std::collections::HashMap;

// SeasonMonitorInfo – per-season monitor state + episode IDs.
// SSoT: Computed once in EditSeries, consumed by SeasonManageModal
// for both the button label and the batch-monitor API call.

#[derive(Clone, Debug, PartialEq)]
pub struct SeasonMonitorInfo {
    pub all_monitored: bool,
    pub episode_ids: Vec<String>,
}

// FormState – single source of truth for all form fields.

#[derive(Clone, Debug)]
pub struct FormState {
    pub title: String,
    pub quality_profile: String,
    pub release_profile: String,
    pub metadata_ids: HashMap<String, String>,
    pub metadata_last_synced_at: HashMap<String, String>,
    pub selected_monitor_mode: MonitorMode,
    pub season_folder_format: Option<String>,
    pub episode_file_format: Option<String>,
    pub season_folder_format_absolute: Option<String>,
    pub episode_file_format_absolute: Option<String>,
    pub flatten_season_folders: Option<bool>,
    pub absolute_numbering: Option<bool>,
    pub rename_episodes: Option<bool>,
    pub search_format: Option<String>,
    pub search_format_absolute: Option<String>,
    pub original_absolute_numbering: Option<bool>,
    pub original_season_folder_format: String,
    pub original_episode_file_format: String,
}

impl FormState {
    /// Whether both search fields inherit the global templates (no series override).
    pub fn search_use_defaults(&self) -> bool {
        self.search_format.is_none() && self.search_format_absolute.is_none()
    }

    /// Whether all four naming formats inherit the global defaults. Any explicit
    /// field (e.g. one set through the API) makes the group appear overridden.
    pub fn formats_use_defaults(&self) -> bool {
        self.season_folder_format.is_none()
            && self.episode_file_format.is_none()
            && self.season_folder_format_absolute.is_none()
            && self.episode_file_format_absolute.is_none()
    }

    fn from_series_details(
        d: &SeriesDetails,
        global_config: Option<&jumbie_shared::config::Config>,
    ) -> Self {
        Self {
            title: d.info.title.clone(),
            quality_profile: d.info.quality_profile.clone(),
            release_profile: d.info.release_profile.clone(),
            metadata_ids: d.config.settings.metadata_ids.clone(),
            metadata_last_synced_at: d.config.settings.metadata_last_synced_at.clone(),
            selected_monitor_mode: d
                .config
                .settings
                .monitor_mode
                .unwrap_or(DEFAULT_MONITOR_MODE),
            season_folder_format: d.config.settings.season_folder_format.clone(),
            episode_file_format: d.config.settings.episode_file_format.clone(),
            season_folder_format_absolute: d.config.settings.season_folder_format_absolute.clone(),
            episode_file_format_absolute: d.config.settings.episode_file_format_absolute.clone(),
            flatten_season_folders: d.config.settings.flatten_season_folders,
            absolute_numbering: d.config.settings.absolute_numbering,
            rename_episodes: d.config.settings.rename_episodes,
            search_format: d.config.settings.search_format.clone(),
            search_format_absolute: d.config.settings.search_format_absolute.clone(),
            original_absolute_numbering: d.config.settings.absolute_numbering,
            original_season_folder_format: d
                .config
                .settings
                .season_folder_format
                .clone()
                .or_else(|| global_config.map(|c| c.organization.season_folder_format.clone()))
                .unwrap_or_default(),
            original_episode_file_format: d
                .config
                .settings
                .episode_file_format
                .clone()
                .or_else(|| global_config.map(|c| c.organization.episode_file_format.clone()))
                .unwrap_or_default(),
        }
    }
}

// EditSeriesCtx – provided via Leptos context so every tab can access
// signals without drilling props.

#[derive(Clone)]
pub struct EditSeriesCtx {
    pub form: RwSignal<FormState>,
    pub series_id: ReadSignal<String>,
    pub series_details: LocalResource<Option<SeriesDetails>>,
    pub series_details_view: Signal<Option<SeriesDetails>>,
    pub quality_options: Signal<Vec<SelectOption>>,
    pub quality_profiles:
        Signal<std::collections::HashMap<String, jumbie_shared::types::QualityProfile>>,
    pub release_options: Signal<Vec<SelectOption>>,
    pub monitor_options: Signal<Vec<SelectOption>>,
    pub active_metadata_plugins: Signal<Vec<jumbie_shared::plugin::PluginInstanceInfo>>,
    /// First argument is an optional custom success message.
    /// - `None`      → show default "Series saved successfully"
    /// - `Some("")`  → suppress toast entirely
    /// - `Some(msg)`  → show `msg`
    pub save_series: Callback<Option<String>, ()>,
    pub handle_monitor_change: Callback<String, ()>,
    pub global_config: LocalResource<Option<jumbie_shared::config::Config>>,
    pub managing_season: RwSignal<Option<String>>,
    pub set_show_modal: WriteSignal<bool>,
    pub set_show_manage: WriteSignal<bool>,
    pub set_show_search: WriteSignal<bool>,
    pub set_search_query_initial: WriteSignal<String>,
    pub set_show_delete_modal: WriteSignal<bool>,
    pub set_show_path_modal: WriteSignal<bool>,
    pub set_selected_episode: WriteSignal<Option<EpisodeViewModel>>,
    pub series_path: ReadSignal<String>,
    pub set_series_path: WriteSignal<String>,
    pub season: RwSignal<Vec<SeasonOverride>>,
    pub season_absolute: RwSignal<Vec<SeasonOverride>>,
    pub series_aliases: RwSignal<String>,
    pub series_reg_patterns: RwSignal<String>,
    pub series_title: ReadSignal<String>,
    pub absolute_numbering: ReadSignal<bool>,
    pub root_path_sig: Signal<String>,
}

/// Parse a newline-separated signal string into a clean Vec
/// (trimmed, empty lines filtered).
///
/// SSoT for reading alias/pattern textarea data at consumption
/// boundaries (save, merge, search). Works with disjoint capture
/// because `RwSignal` is `Copy`.
pub fn parse_signal_lines(signal: &RwSignal<String>) -> Vec<String> {
    signal
        .get_untracked()
        .split('\n')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_multiple_lines() {
        let signal = RwSignal::new("alias one\nalias two\nalias three".into());
        let result = parse_signal_lines(&signal);
        assert_eq!(result, vec!["alias one", "alias two", "alias three"]);
    }

    #[test]
    fn parse_trims_whitespace() {
        let signal = RwSignal::new("  padded  \n\talias with tab\n".into());
        let result = parse_signal_lines(&signal);
        assert_eq!(result, vec!["padded", "alias with tab"]);
    }

    #[test]
    fn parse_skips_empty_lines() {
        let signal = RwSignal::new("first\n\n\nsecond\n".into());
        let result = parse_signal_lines(&signal);
        assert_eq!(result, vec!["first", "second"]);
    }

    #[test]
    fn parse_all_whitespace_is_empty() {
        let signal = RwSignal::new("   \n\t\n   \n".into());
        let result = parse_signal_lines(&signal);
        let empty: Vec<String> = vec![];
        assert_eq!(result, empty);
    }

    #[test]
    fn parse_empty_string() {
        let signal = RwSignal::new(String::new());
        let result = parse_signal_lines(&signal);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_single_line() {
        let signal = RwSignal::new("just one alias".into());
        let result = parse_signal_lines(&signal);
        assert_eq!(result, vec!["just one alias"]);
    }

    /// A form with every format/search field inheriting (no overrides).
    fn form() -> FormState {
        FormState {
            title: String::new(),
            quality_profile: String::new(),
            release_profile: String::new(),
            metadata_ids: HashMap::new(),
            metadata_last_synced_at: HashMap::new(),
            selected_monitor_mode: DEFAULT_MONITOR_MODE,
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            absolute_numbering: None,
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            original_absolute_numbering: None,
            original_season_folder_format: String::new(),
            original_episode_file_format: String::new(),
        }
    }

    #[test]
    fn all_inherited_groups_read_as_defaults() {
        let f = form();
        assert!(f.formats_use_defaults());
        assert!(f.search_use_defaults());
    }

    #[test]
    fn any_single_override_flips_the_group_off_defaults() {
        // Each naming field independently disables the shared checkbox: a value
        // set through the API must read as overridden, not stay "inheriting"
        // just because the other fields were left alone.
        let mut f = form();
        f.season_folder_format = Some("x".into());
        assert!(!f.formats_use_defaults());

        let mut f = form();
        f.episode_file_format = Some("x".into());
        assert!(!f.formats_use_defaults());

        let mut f = form();
        f.season_folder_format_absolute = Some("x".into());
        assert!(!f.formats_use_defaults());

        let mut f = form();
        f.episode_file_format_absolute = Some("x".into());
        assert!(!f.formats_use_defaults());
    }

    #[test]
    fn blank_is_an_override_not_inherit() {
        // `None` = inherit, but `Some("")` is a deliberate blank that still
        // counts as an override, so it must clear the "Use Defaults" checkbox.
        let mut f = form();
        f.search_format = Some(String::new());
        assert!(!f.search_use_defaults());

        let mut g = form();
        g.search_format_absolute = Some(String::new());
        assert!(!g.search_use_defaults());

        let mut h = form();
        h.season_folder_format = Some(String::new());
        assert!(!h.formats_use_defaults());
    }

    #[test]
    fn format_and_search_groups_are_independent() {
        let mut f = form();
        f.episode_file_format = Some("x".into());
        assert!(!f.formats_use_defaults());
        assert!(f.search_use_defaults());
    }
}

// EditSeries component

#[component]
pub fn EditSeries() -> impl IntoView {
    let navigate = use_navigate();
    let params = use_params_map();
    let (series_id, set_series_id) =
        signal(params.with_untracked(|p| p.get("id").clone().unwrap_or_default()));
    Effect::new(move |_| {
        let new_id = params.with(|p| p.get("id").clone().unwrap_or_default());
        if new_id != series_id.get_untracked() {
            set_series_id.set(new_id);
        }
    });

    // Derive the active tab from the URL hash (e.g. #episodes → "episodes").
    let hash_to_tab = |hash: &str| -> String {
        match hash.trim_start_matches('#') {
            "episodes" => "episodes".to_string(),
            "advanced" => "advanced".to_string(),
            _ => "general".to_string(),
        }
    };
    let initial_tab = web_sys::window()
        .map(|w| w.location().hash().unwrap_or_default())
        .map(|h| hash_to_tab(&h))
        .unwrap_or_else(|| "general".to_string());
    let (active_tab, set_active_tab) = signal(initial_tab);

    // Keep active_tab in sync with hash changes (browser back/forward or direct edits).
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
        let closure = Closure::<dyn Fn()>::new(move || {
            if let Some(hash) = web_sys::window().and_then(|w| w.location().hash().ok()) {
                set_active_tab.set(hash_to_tab(&hash));
            }
        });
        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("hashchange", closure.as_ref().unchecked_ref());
        }
        closure.forget();
    }

    // Modal visibility signals
    let (show_modal, set_show_modal) = signal(false);
    let (show_search, set_show_search) = signal(false);
    let (show_manage, set_show_manage) = signal(false);
    let (show_delete_modal, set_show_delete_modal) = signal(false);
    let (delete_configurations, set_delete_configurations) = signal(false);
    let (delete_episodes, set_delete_episodes) = signal(false);
    let (delete_episode_data, set_delete_episode_data) = signal(false);
    let (show_rename_modal, set_show_rename_modal) = signal(false);
    let (show_path_modal, set_show_path_modal) = signal(false);

    let (selected_episode, set_selected_episode) = signal(None::<EpisodeViewModel>);
    let (search_query_initial, set_search_query_initial) = signal(String::new());
    let (path_operation, set_path_operation) = signal(None::<jumbie_shared::types::PathOperation>);

    // Initialize cache-primed signal synchronously if data is available for instant first render.
    let (series_details_cached, set_series_details_cached) = signal({
        let id = series_id.get_untracked();
        if id.is_empty() {
            None
        } else {
            let cache_key = format!("fetch_series_details_{}", id);
            crate::utils::read_cache::<SeriesDetails>(&cache_key)
        }
    });

    // When series_id changes (navigation), check cache and populate instantly.
    Effect::new(move |_| {
        let id = series_id.get();
        if id.is_empty() {
            return;
        }
        let cache_key = format!("fetch_series_details_{}", id);
        if let Some(cached) = crate::utils::read_cache::<SeriesDetails>(&cache_key) {
            set_series_details_cached.set(Some(cached));
        }
    });

    let initial_details = series_details_cached.get_untracked();

    // Form state
    let form_state = RwSignal::new(FormState {
        title: initial_details
            .as_ref()
            .map(|d| d.info.title.clone())
            .unwrap_or_default(),
        quality_profile: initial_details
            .as_ref()
            .map(|d| d.info.quality_profile.clone())
            .unwrap_or_default(),
        release_profile: initial_details
            .as_ref()
            .map(|d| d.info.release_profile.clone())
            .unwrap_or_default(),
        metadata_ids: initial_details
            .as_ref()
            .map(|d| d.config.settings.metadata_ids.clone())
            .unwrap_or_default(),
        metadata_last_synced_at: initial_details
            .as_ref()
            .map(|d| d.config.settings.metadata_last_synced_at.clone())
            .unwrap_or_default(),
        selected_monitor_mode: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.monitor_mode)
            .unwrap_or(DEFAULT_MONITOR_MODE),
        season_folder_format: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.season_folder_format.clone()),
        episode_file_format: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.episode_file_format.clone()),
        season_folder_format_absolute: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.season_folder_format_absolute.clone()),
        episode_file_format_absolute: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.episode_file_format_absolute.clone()),
        flatten_season_folders: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.flatten_season_folders),
        absolute_numbering: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.absolute_numbering),
        rename_episodes: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.rename_episodes),
        search_format: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.search_format.clone()),
        search_format_absolute: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.search_format_absolute.clone()),
        original_absolute_numbering: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.absolute_numbering),
        original_season_folder_format: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.season_folder_format.clone())
            .unwrap_or_default(),
        original_episode_file_format: initial_details
            .as_ref()
            .and_then(|d| d.config.settings.episode_file_format.clone())
            .unwrap_or_default(),
    });

    // Sub-component signals (kept as signal pair / RwSignal for direct access)
    let (series_path, set_series_path) = signal(
        initial_details
            .as_ref()
            .and_then(|d| d.config.settings.path.clone())
            .unwrap_or_default(),
    );
    let series_aliases = RwSignal::new(
        initial_details
            .as_ref()
            .map(|d| d.config.settings.aliases.join("\n"))
            .unwrap_or_default(),
    );
    let series_reg_patterns = RwSignal::new(
        initial_details
            .as_ref()
            .map(|d| d.config.settings.reg_patterns.join("\n"))
            .unwrap_or_default(),
    );
    let season = RwSignal::new(
        initial_details
            .as_ref()
            .map(|d| {
                d.config
                    .settings
                    .season
                    .clone()
                    .into_values()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    let season_absolute = RwSignal::new(
        initial_details
            .as_ref()
            .map(|d| {
                d.config
                    .settings
                    .season_absolute
                    .clone()
                    .into_values()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );

    let managing_season = RwSignal::new(None::<String>);

    // Push the current series title into the header so it shows the name instead
    // of the generic "Jumbie" title (fixes D-4 / D-13).
    if let Some(layout_ctx) = use_context::<crate::hooks::LayoutContext>() {
        let set_override = layout_ctx.set_header_title_override;
        Effect::new(move |_| {
            let title = form_state.with(|s| s.title.clone());
            if title.is_empty() {
                set_override.set(None);
            } else {
                set_override.set(Some(title));
            }
        });
        on_cleanup(move || {
            set_override.set(None);
        });
    }

    // API data signals (kept separate – not form data)
    let (metadata_plugins, set_metadata_plugins) =
        signal(Vec::<jumbie_shared::plugin::PluginInstanceInfo>::new());
    let (quality_profiles, set_quality_profiles) = signal(HashMap::new());
    let (release_profiles, set_release_profiles) = signal(HashMap::new());
    let (plugins_cfg_sig, set_plugins_cfg_sig) =
        signal(None::<jumbie_shared::config::PluginsConfig>);

    crate::utils::use_api_cache(
        "fetch_quality_profiles".to_string(),
        || crate::api::fetch_quality_profiles(),
        move |p| {
            let _ = set_quality_profiles.try_update(|s| *s = p);
        },
    );

    crate::utils::use_api_cache(
        "fetch_release_profiles".to_string(),
        || crate::api::fetch_release_profiles(),
        move |p| {
            let _ = set_release_profiles.try_update(|s| *s = p);
        },
    );

    crate::utils::use_api_cache(
        "fetch_plugins".to_string(),
        || crate::api::fetch_plugins(),
        move |plugins: Vec<jumbie_shared::plugin::PluginInstanceInfo>| {
            let meta = plugins
                .into_iter()
                .filter(|p| {
                    p.capabilities
                        .contains(&jumbie_shared::plugin::Capability::MetadataProviderNormal)
                        || p.capabilities
                            .contains(&jumbie_shared::plugin::Capability::MetadataProviderAbsolute)
                })
                .collect();
            let _ = set_metadata_plugins.try_update(|s| *s = meta);
        },
    );

    crate::utils::use_api_cache(
        "fetch_plugins_cfg".to_string(),
        || crate::api::fetch_plugins_cfg(),
        move |c| {
            let _ = set_plugins_cfg_sig.try_update(|s| *s = Some(c));
        },
    );

    let quality_options = crate::utils::derive_quality_options(quality_profiles.into());
    let release_options = crate::utils::derive_release_options(release_profiles.into());

    let monitor_options = Signal::derive(|| {
        crate::constants::MONITOR_OPTIONS
            .iter()
            .map(|(v, l)| SelectOption::from((v.to_string(), l.to_string())))
            .collect::<Vec<_>>()
    });

    // Returns one PluginInstanceInfo per enabled metadata INSTANCE (not per plugin
    // TYPE). Identity is backend-stamped: `plugin_id` is the type id and
    // `instance_id` the instance id — the latter keys `metadata_ids`.
    let active_metadata_plugins = Signal::derive(move || {
        let all_plugins = metadata_plugins.get();
        if let Some(cfg) = plugins_cfg_sig.get() {
            crate::utils::resolve_active_metadata_plugins(all_plugins, &cfg)
        } else {
            Vec::new()
        }
    });

    // SSoT: active_metadata_plugin_name returns the instance id of the first
    // enabled metadata plugin.  This is used for:
    //   1. Comparing against metadata_source and provider_instance_id (both UUIDs)
    //   2. Passing as ?provider= param to cache-clearing API calls
    // For display names, use active_metadata_plugins which carries display_name.
    let active_metadata_plugin_name = Signal::derive(move || {
        if let Some(cfg) = plugins_cfg_sig.get() {
            for instances in cfg.metadata.values() {
                for (instance_id, instance_cfg) in instances {
                    let enabled = instance_cfg
                        .as_object()
                        .and_then(|t| t.get("enabled"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if enabled {
                        return instance_id.clone();
                    }
                }
            }
        }
        "None configured".to_string()
    });

    provide_context(active_metadata_plugin_name);

    let series_details = LocalResource::new(move || {
        let id = series_id.get();
        async move {
            if id.is_empty() {
                return None;
            }
            match crate::api::fetch_series_details(id.clone()).await {
                Ok(res) => {
                    if let Some(ref details) = res {
                        let ck = format!("fetch_series_details_{}", id);
                        crate::utils::write_cache(&ck, details);
                    }
                    set_series_details_cached.set(res.clone());
                    res
                }
                Err(e) => {
                    crate::debug_error!("Failed to fetch series details: {}", e);
                    crate::components::common::toast::show_error(format!(
                        "Failed to load series: {}",
                        e
                    ));
                    None
                }
            }
        }
    });

    let series_details_view = Signal::derive(move || {
        if let Some(res) = series_details.get() {
            return res;
        }
        series_details_cached.get()
    });

    // Season-level monitor info
    // Per-season data: whether ALL episodes are monitored, and the list of
    // episode IDs (used by SeasonManageModal to call the generic batch endpoint).
    // SSoT: Both the button label and the click action derive from the same data.
    let season_monitor_info = Memo::new(move |_| {
        let eps = series_details_view
            .get()
            .map(|d| d.episodes)
            .unwrap_or_default();
        let mut map = std::collections::HashMap::<String, SeasonMonitorInfo>::new();
        for ep in &eps {
            let key =
                crate::components::edit_series::season_accordion::normalize_season(&ep.season);
            let entry = map.entry(key).or_insert_with(|| SeasonMonitorInfo {
                all_monitored: true,
                episode_ids: Vec::new(),
            });
            entry.all_monitored = entry.all_monitored && ep.monitored;
            entry.episode_ids.push(ep.unique_id.clone());
        }
        map
    });

    // Auto-refresh episode data
    // Poll the backend every 15 seconds so episode cells automatically
    // reflect completed downloads and organized files.
    let sd = series_details.clone();
    match set_interval_with_handle(
        move || {
            sd.refetch();
        },
        std::time::Duration::from_secs(15),
    ) {
        Ok(handle) => on_cleanup(move || handle.clear()),
        Err(_) => crate::debug_warn!("Failed to set interval for series details refresh"),
    }

    let global_config =
        LocalResource::new(move || async move { crate::api::fetch_config().await.ok() });

    // Derived signals for compat with components that expect ReadSignal.
    let (series_title, set_series_title) = signal(form_state.with_untracked(|s| s.title.clone()));
    let (absolute_numbering, set_absolute_numbering) = signal(
        form_state
            .with_untracked(|s| s.absolute_numbering)
            .unwrap_or(
                global_config
                    .get_untracked()
                    .flatten()
                    .map(|c| c.general.absolute_numbering)
                    .unwrap_or(false),
            ),
    );

    let last_saved_payload =
        StoredValue::new_local(None::<jumbie_shared::types::UpdateSeriesPayload>);
    let last_saved_monitor_mode =
        StoredValue::new_local(form_state.with_untracked(|f| f.selected_monitor_mode));

    // Reusable payload builder — SSoT for the save logic.
    // Captures all signals needed to build the update payload.
    let build_payload = move |with_path_op: bool| {
        let form = form_state.get_untracked();
        let op = if with_path_op {
            let op = path_operation.get_untracked();
            if op.is_some() {
                set_path_operation.set(None);
            }
            op
        } else {
            None
        };
        jumbie_shared::types::UpdateSeriesPayload {
            quality_profile: form.quality_profile,
            release_profile: form.release_profile,
            title: Some(form.title),
            path_operation: op,
            settings: {
                let mut settings = jumbie_shared::mapping::SeriesSettings {
                    aliases: parse_signal_lines(&series_aliases),
                    reg_patterns: parse_signal_lines(&series_reg_patterns),
                    season: season
                        .get_untracked()
                        .into_iter()
                        .map(|o| (o.season.clone(), o))
                        .collect(),
                    season_absolute: season_absolute
                        .get_untracked()
                        .into_iter()
                        .map(|o| (o.season.clone(), o))
                        .collect(),
                    path: Some(series_path.get_untracked()),
                    ..jumbie_shared::mapping::SeriesSettings::from_form(
                        jumbie_shared::mapping::SeriesSettingsForm {
                            season_folder_format: form.season_folder_format,
                            episode_file_format: form.episode_file_format,
                            season_folder_format_absolute: form.season_folder_format_absolute,
                            episode_file_format_absolute: form.episode_file_format_absolute,
                            flatten_season_folders: form.flatten_season_folders,
                            absolute_numbering: form.absolute_numbering,
                            rename_episodes: form.rename_episodes,
                            search_format: form.search_format,
                            search_format_absolute: form.search_format_absolute,
                            metadata_ids: form.metadata_ids,
                        },
                    )
                };
                // Only include monitor_mode when it actually changed
                if form.selected_monitor_mode != last_saved_monitor_mode.get_value() {
                    settings.monitor_mode = Some(form.selected_monitor_mode);
                }
                settings
            },
        }
    };

    // Single hydration effect – populates the form from API data once on first load.
    // After initialization, `form_state` is the single source of truth — the server
    // never overwrites unsaved user edits. This prevents a desync when clicking a
    // tri-state checkbox faster than the save round-trip completes.
    let form_initialized = StoredValue::new_local(false);
    Effect::new(move |_| {
        if form_initialized.get_value() {
            return;
        }
        if let Some(Some(details)) = series_details_view.get().map(Some) {
            let cfg = global_config.get().flatten();
            form_state.set(FormState::from_series_details(&details, cfg.as_ref()));
            form_initialized.set_value(true);

            set_series_path.set(details.config.settings.path.clone().unwrap_or_default());
            series_aliases.set(details.config.settings.aliases.join("\n"));
            series_reg_patterns.set(details.config.settings.reg_patterns.join("\n"));
            season.set(
                details
                    .config
                    .settings
                    .season
                    .clone()
                    .into_values()
                    .collect(),
            );
            season_absolute.set(
                details
                    .config
                    .settings
                    .season_absolute
                    .clone()
                    .into_values()
                    .collect(),
            );

            *last_saved_payload.write_value() = Some(build_payload(false));
            last_saved_monitor_mode
                .set_value(form_state.with_untracked(|f| f.selected_monitor_mode));
        }
    });

    // Keep the selected episode (which feeds `EpisodeDetailsModal`) in sync with
    // server refreshes so an open modal reflects assignments/renames/manual saves.
    //
    // This MUST be its own effect: the hydration effect above is one-shot (it
    // early-returns once `form_initialized` is set), so anything inside it only
    // ever runs on first load. Previously this sync lived there, which is why the
    // modal kept showing the stale episode snapshot after Manage Series Files.
    Effect::new(move |_| {
        if let Some(details) = series_details_view.get()
            && let Some(current) = selected_episode.get_untracked()
            && let Some(updated) = details
                .episodes
                .iter()
                .find(|e| e.unique_id == current.unique_id)
        {
            set_selected_episode.set(Some(updated.clone()));
        }
    });

    // Sync Effect: keep separate signals in lockstep with form_state
    // These signals are used by SeasonAccordionList, EpisodeDetailsModal, etc.
    // which expect a ReadSignal rather than reading from FormState directly.
    Effect::new(move |_| {
        let title = form_state.with(|s| s.title.clone());
        let abs = form_state.with(|s| s.absolute_numbering).unwrap_or(
            global_config
                .get()
                .flatten()
                .map(|c| c.general.absolute_numbering)
                .unwrap_or(false),
        );
        set_series_title.set(title);
        set_absolute_numbering.set(abs);
    });

    let save_timer = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);
    let (save_trigger, set_save_trigger) = signal(0);
    // Tracks whether the user has made any manual changes.
    // Prevents showing "Series saved successfully" on initial form load
    // when no actual modifications were made.
    let has_user_changes = StoredValue::new_local(false);
    // Optional override for the success toast message shown by the
    // debounced autosave. When Some(""), the toast is suppressed entirely.
    let save_success_override = StoredValue::new_local(None::<String>);

    let refresh_rename_queue = use_context::<crate::hooks::RenameQueueRefresh>();

    Effect::new(move |_| {
        let trigger = save_trigger.get();
        if trigger == 0 {
            return;
        }

        *save_timer.write_value() = None;

        let id = series_id.get_untracked();

        let timeout = gloo_timers::callback::Timeout::new(500, move || {
            let payload = build_payload(true);

            if Some(&payload) == last_saved_payload.read_value().as_ref() {
                has_user_changes.set_value(false);
                return;
            }
            *last_saved_payload.write_value() = Some(payload.clone());
            last_saved_monitor_mode
                .set_value(form_state.with_untracked(|f| f.selected_monitor_mode));

            spawn_local(async move {
                match crate::api::update_series(id.clone(), payload).await {
                    Ok(resp) => {
                        crate::debug_log!("Autosaved series {}", id);
                        if resp.mode_switch_warning {
                            let msg = resp.mode_switch_message.unwrap_or_else(|| {
                                "Ordering mode switched. Episode alignments were modified."
                                    .to_string()
                            });
                            crate::components::common::toast::show_toast(
                                &msg,
                                crate::components::common::toast::NotificationType::Warning,
                            );
                            has_user_changes.set_value(false);
                        } else if has_user_changes.get_value() {
                            let overridden = save_success_override.try_get_value().flatten();
                            save_success_override.set_value(None);
                            match overridden {
                                Some(msg) if !msg.is_empty() => {
                                    crate::components::common::toast::show_success(&msg);
                                }
                                None => {
                                    crate::components::common::toast::show_success(
                                        "Series saved successfully",
                                    );
                                }
                                // Some("") → suppress
                                _ => {}
                            }
                            has_user_changes.set_value(false);
                        }
                        series_details.refetch();
                        // Surgically propagate the rename through every cached view
                        // (calendar windows, library list, wanted pages) keyed by
                        // series_id — no wholesale cache wipe.  SSoT helper.
                        if let Some(form) = form_state.try_get_untracked() {
                            crate::utils::rename_series_in_caches(&id, &form.title);
                            crate::utils::update_cached_series_profiles(
                                &id,
                                &form.quality_profile,
                                &form.release_profile,
                            );
                        }
                        if let Some(r) = refresh_rename_queue {
                            r.1.run(());
                        }
                    }
                    Err(e) => {
                        crate::debug_error!("Failed to autosave series {}: {}", id, e);
                        crate::components::common::toast::show_error(format!(
                            "Failed to save series: {}",
                            e.user_message()
                        ));
                    }
                }
            });
        });
        *save_timer.write_value() = Some(timeout);
    });

    let save_series = Callback::new(move |msg: Option<String>| {
        if let Some(m) = &msg {
            save_success_override.set_value(Some(m.clone()));
        }
        has_user_changes.set_value(true);
        set_save_trigger.update(|v| *v += 1);
    });

    let handle_monitor_change = Callback::new(move |val: String| {
        let mode = crate::utils::parse_monitor_mode(&val).unwrap_or(MonitorMode::None);
        form_state.update(|s| s.selected_monitor_mode = mode);
        save_series.run(None);
    });

    let platform_os = crate::hooks::use_platform_os();

    let root_path_sig: Signal<String> = Signal::derive(move || {
        if let Some(Some(details)) = series_details_view.get().map(Some) {
            let path = series_path.get();
            let series_title: String = details.info.title.clone();

            if !path.is_empty() {
                // SSoT: shared template resolution + policy sanitization, so the
                // displayed folder matches the sanitized name on disk (e.g. a
                // `${series}` template for "Show: Part" renders "Show_ Part").
                // Falls back to the raw replacement only while config is loading.
                match global_config.get().flatten() {
                    Some(cfg) => crate::utils::resolve_path_template(
                        &path,
                        &series_title,
                        &cfg.organization,
                        platform_os.get(),
                    ),
                    None => path.replace("${series}", &series_title),
                }
            } else {
                series_title
            }
        } else {
            String::new()
        }
    });

    // Track whether the Episodes tab has been loaded at least once.
    // Once loaded it stays in the DOM so switching away and back is instant.
    let (episodes_loaded, set_episodes_loaded) = signal(false);
    Effect::new(move |_| {
        if active_tab.get() == "episodes" {
            set_episodes_loaded.set(true);
        }
    });

    // Provide context so tabs can access signals without props
    provide_context(EditSeriesCtx {
        form: form_state,
        series_id,
        series_details,
        series_details_view,
        quality_options,
        quality_profiles: quality_profiles.into(),
        release_options,
        monitor_options,
        active_metadata_plugins,
        save_series,
        handle_monitor_change,
        global_config,
        managing_season,
        set_show_modal,
        set_show_manage,
        set_show_search,
        set_search_query_initial,
        set_show_delete_modal,
        set_show_path_modal,
        set_selected_episode,
        series_path,
        set_series_path,
        season,
        season_absolute,
        series_aliases,
        series_reg_patterns,
        series_title,
        absolute_numbering,
        root_path_sig,
    });

    // Derived signals for EpisodeDetailsModal (created outside view! macro)
    let episode_details_all_episodes = Signal::derive(move || {
        series_details_view
            .get()
            .map(|d| d.episodes)
            .unwrap_or_default()
    });
    let episode_details_series_id = Signal::derive(move || series_id.get_untracked());
    let episode_details_season_overrides = Signal::derive(move || {
        if form_state.with(|s| s.absolute_numbering).unwrap_or(
            global_config
                .get()
                .flatten()
                .map(|c| c.general.absolute_numbering)
                .unwrap_or(false),
        ) {
            season_absolute.get()
        } else {
            season.get()
        }
    });

    // Tab UI helpers
    let tab_item = move |id: &'static str, label: &'static str| {
        let is_active = move || active_tab.get() == id;
        view! {
            <a
                data-testid={format!("edit-tab-{}", id)}
                class="edit-tab"
                class:active=is_active
                href={format!("#{}", id)}
                on:click=move |ev| {
                    if active_tab.get_untracked() != id {
                        set_active_tab.set(id.to_string());
                    } else {
                        ev.prevent_default();
                    }
                }
            >
                {label}
            </a>
        }
    };

    // Media info scan progress
    // Poll scan queue progress to show the scanning indicator in the header.
    // The polling filters to only render when count > 0.
    let (scan_queue_count, set_scan_queue_count) = signal(None::<usize>);
    let sid_for_effect = series_id.clone();
    Effect::new(move |_| {
        let sid = sid_for_effect.get();
        if sid.is_empty() {
            return;
        }
        // Initial fetch — gated by a 5-second cooldown per series ID.
        // Prevents redundant calls when navigating between series quickly.
        let cooldown_key = format!("fetch_media_info_scan_count_for_series_{}", sid);
        if crate::utils::check_cooldown(&cooldown_key, 5_000.0) {
            let sid = sid.clone();
            spawn_local(async move {
                if let Ok(c) = crate::api::fetch_media_info_scan_count_for_series(&sid).await {
                    set_scan_queue_count.set(c.get(&sid).copied().filter(|&c| c > 0));
                }
            });
        }
        // Poll every 5 seconds, cleaned up on component unmount
        if let Ok(handle) = set_interval_with_handle(
            move || {
                let cooldown_key = format!("fetch_media_info_scan_count_for_series_{}", sid);
                if crate::utils::check_cooldown(&cooldown_key, 5_000.0) {
                    let sid = sid.clone();
                    spawn_local(async move {
                        if let Ok(c) =
                            crate::api::fetch_media_info_scan_count_for_series(&sid).await
                        {
                            set_scan_queue_count.set(c.get(&sid).copied().filter(|&c| c > 0));
                        }
                    });
                }
            },
            std::time::Duration::from_secs(5),
        ) {
            on_cleanup(move || handle.clear());
        }
    });

    let scan_queue_progress = Signal::derive(move || scan_queue_count.with(|c| *c));

    let nav_back_btn = navigate.clone();
    let nav_err_btn = navigate.clone();
    let nav_del_confirm = navigate.clone();

    view! {
        <div class="view active" id="editSeries">
            <div class="edit-header">
                <div class="back-btn" on:click=move |_| nav_back_btn(&format!("/{}", path::SERIES), Default::default())>
                    <span class="icon"><ArrowLeftIcon /></span>
                    <span>"Back to Library"</span>
                </div>
                <div id="edit-header-message">
                    <div id="edit-header-progress">
                        {move || {
                            scan_queue_progress.get().map(|count| {
                                view! {
                                    <span class="progress-text" title={format!("{} file(s) remaining to be scanned", count)}>
                                        {format!("{} file(s) scanning...", count)}
                                    </span>
                                }
                            })
                        }}
                    </div>
                    <div id="edit-header-not-found">
                        {move || {
                            let series_id = series_id.get();
                            if series_id.is_empty() {
                                return view! {}.into_any();
                            }
                            let missing_count = series_details_view.with(|d| {
                                d.as_ref().map_or(0, |details| {
                                    details.episodes.iter().filter(|ep| {
                                        crate::components::edit_series::season_accordion::episode_is_not_found(ep.assigned, ep.disk_present)
                                    }).count()
                                })
                            });
                            if missing_count == 0 {
                                return view! {}.into_any();
                            }
                            view! {
                                <span class="not-found-text">
                                    {move || format!("{} ep(s) not found", missing_count)}
                                    <button
                                        class="btn btn-sm btn-danger"
                                        title="Clear the file path from episodes whose files could not be found on disk. Episodes will revert to missing status."
                                        on:click={
                                            let sid = series_id.clone();
                                            move |_| {
                                                let sid = sid.clone();
                                                spawn_local(async move {
                                                    match crate::api::clear_not_found_episode_files(&sid).await {
                                                        Ok(count) => {
                                                            crate::components::common::toast::show_success(
                                                                format!("Cleared {} missing episode file reference(s)", count),
                                                            );
                                                            if let Some(ctx) = use_context::<EditSeriesCtx>() {
                                                                ctx.series_details.refetch();
                                                            }
                                                        }
                                                        Err(e) => {
                                                            crate::components::common::toast::show_error(
                                                                format!("Failed to clear missing files: {}", e),
                                                            );
                                                        }
                                                    }
                                                });
                                            }
                                        }
                                    >
                                        "Clear"
                                    </button>
                                </span>
                            }.into_any()
                        }}
                    </div>
                </div>
            </div>
            {
                let _nav_err = nav_err_btn.clone();
                let has_details = Memo::new(move |_| series_details_view.with(|d| d.is_some()));
                move || {
                    if has_details.get() {
                        view! {
                            <div class="edit-tabs">
                                {tab_item("general", "General")}
                                {tab_item("episodes", "Episodes")}
                                {tab_item("advanced", "Advanced")}
                            </div>

                            // All tabs rendered unconditionally — visibility
                            // toggled via CSS display so the DOM stays stable.
                            // No Show / into_any() removal means no layout
                            // shift when switching tabs, which eliminates
                            // the flash on the tab indicator.
                            <div style:display=move || if active_tab.get() == "general" { "contents" } else { "none" }>
                                <GeneralTab/>
                            </div>

                            <div style:display=move || if active_tab.get() == "advanced" { "contents" } else { "none" }>
                                <AdvancedTab/>
                            </div>

                            // EpisodesTab – lazy-loaded once, then persists
                            // in the DOM for instant back-and-forth switching.
                            {move || {
                                if episodes_loaded.get() {
                                    view! {
                                        <div style:display=move || if active_tab.get() == "episodes" { "contents" } else { "none" }>
                                            <EpisodesTab/>
                                        </div>
                                    }.into_any()
                                } else {
                                    ().into_any()
                                }
                            }}

                        }.into_any()
                    } else if series_details.get().is_none() {
                        view! { <EditSeriesSkeleton/> }.into_any()
                    } else {
                        view! {
                            <ErrorPage
                                title="Series Not Found"
                                message="Unable to load series details. It may have been deleted or the server is unreachable."
                                icon="⚠️"
                            />
                        }.into_any()
                    }
                }
            }

            <SearchModal
                show=show_search
                set_show=set_show_search
                id="edit-series-search"
                initial_query=search_query_initial.into()
                series_id=Signal::derive(move || Some(series_id.with(|s| s.clone())))
                episode_id=Signal::derive(move || None::<String>)
            />

            <EpisodeDetailsModal
                show=show_modal
                set_show=set_show_modal
                episode=selected_episode
                all_episodes=episode_details_all_episodes
                series_id=episode_details_series_id
                series_title=series_title
                season_overrides=episode_details_season_overrides
                series_search_format=Signal::derive(move || {
                    let absolute = form_state.with(|s| s.absolute_numbering).unwrap_or(global_config.get().flatten().map(|c| c.general.absolute_numbering).unwrap_or(false));
                    let global = global_config.get().flatten();
                    form_state.with(|s| if absolute { s.search_format_absolute.clone() } else { s.search_format.clone() })
                        .or_else(|| global.map(|c| if absolute { c.organization.search_format_absolute } else { c.organization.search_format }))
                        .unwrap_or_default()
                })
                absolute_numbering=Signal::derive(move || form_state.with(|s| s.absolute_numbering).unwrap_or(global_config.get().flatten().map(|c| c.general.absolute_numbering).unwrap_or(false)))
                metadata_plugins=active_metadata_plugins
                hide_jump_to_series=true
                on_override_saved=Callback::new({
                    let episode_ctx = crate::utils::episode_state::use_episode_state();
                    move |(episode_id, series_id): (String, String)| {
                        crate::utils::episode_state::notify_episode_saved(
                            &episode_ctx,
                            &episode_id,
                            &series_id,
                        );
                        series_details.refetch();
                    }
                })
            />

            <ManageEpisodesModal
                show=show_manage
                set_show=set_show_manage
                series_id=series_id
                series_details=series_details_view
                absolute_numbering=absolute_numbering
                on_refresh=Callback::new(move |_| {
                    series_details.refetch();
                })
            />

            <SeasonManageModal
                season=season
                season_absolute=season_absolute
                managing_season=managing_season
                series_id=Signal::derive(move || series_id.with(|s| s.clone()))
                absolute_numbering=absolute_numbering
                series_search_format=Signal::derive(move || form_state.with(|s| s.search_format.clone()))
                series_search_format_absolute=Signal::derive(move || form_state.with(|s| s.search_format_absolute.clone()))
                global_search_format=Signal::derive(move || global_config.get().flatten().map(|c| c.organization.search_format.clone()).unwrap_or_else(|| "S${season:02}E${episode:02}".to_string()))
                global_search_format_absolute=Signal::derive(move || global_config.get().flatten().map(|c| c.organization.search_format_absolute.clone()).unwrap_or_else(|| "E${episode:02}".to_string()))
                season_monitor_info=Signal::derive(move || season_monitor_info.get())
                has_cached_metadata=Signal::derive(move || {
                    let s = managing_season.get().unwrap_or_default();
                    let n = s.trim().parse::<i32>().unwrap_or(-1);
                    if n < 0 {
                        return false;
                    }
                    series_details_view
                        .get()
                        .map(|d| d.metadata_seasons.iter().any(|m| m.season_number == n))
                        .unwrap_or(false)
                })
                on_save=Callback::new(move |_| {
                    // Trigger a full API save but suppress the "Series saved successfully"
                    // notification, since the modal already shows its own toast
                    // ("Season configuration updated.").
                    set_save_trigger.update(|v| *v += 1);
                })
                on_refresh=Callback::new(move |_| {
                    series_details.refetch();
                })
            />

            <EditSeriesPathModal
                show=show_path_modal
                set_show=set_show_path_modal
                current_path=series_path
                fallback_path=root_path_sig
                set_current_path=set_series_path
                set_path_operation=set_path_operation
                save_series=save_series
                series_title=series_title
                series_id=series_id
            />

            <RemoveSeriesModal
                show=Signal::from(show_delete_modal)
                set_show=set_show_delete_modal
                series_ids=Signal::derive(move || vec![series_id.with(|s| s.clone())])
                delete_configurations=delete_configurations
                set_delete_configurations=set_delete_configurations
                delete_episodes=delete_episodes
                set_delete_episodes=set_delete_episodes
                delete_episode_data=delete_episode_data
                set_delete_episode_data=set_delete_episode_data
                on_success={
                    let nav_back = nav_del_confirm.clone();
                    Callback::new(move |_| {
                        nav_back(&format!("/{}", path::SERIES), Default::default())
                    })
                }
            />

            <super::rename_modal::RenameModal
                show=Signal::from(show_rename_modal)
                set_show=set_show_rename_modal
                message=Signal::derive(move || {
                    let form = form_state.get();
                    let global = global_config.get().flatten();
                    let season_folder_format = form
                        .season_folder_format
                        .clone()
                        .or_else(|| global.as_ref().map(|c| c.organization.season_folder_format.clone()))
                        .unwrap_or_default();
                    let episode_file_format = form
                        .episode_file_format
                        .clone()
                        .or_else(|| global.as_ref().map(|c| c.organization.episode_file_format.clone()))
                        .unwrap_or_default();
                    let mut changes = Vec::new();
                    if form.absolute_numbering != form.original_absolute_numbering { changes.push("Absolute Numbering Mapping"); }
                    if season_folder_format != form.original_season_folder_format { changes.push("Season Folder Format"); }
                    if episode_file_format != form.original_episode_file_format { changes.push("Episode File Format"); }

                    let fields_str = if changes.len() > 1 {
                        let last = changes.pop().unwrap();
                        format!("{} and {}", changes.join(", "), last)
                    } else if changes.len() == 1 {
                        changes[0].to_string()
                    } else {
                        "File formatting".to_string()
                    };

                    Some(format!("The {} setting has been changed. Would you like to rename and reorganize the existing files for this series now, or apply it later?", fields_str))
                })
                on_now=Callback::new(move |_| {
                    set_show_rename_modal.set(false);
                    let id = series_id.get();
                    let id_for_cache = id.clone();
                    let target_absolute = form_state
                        .with_untracked(|s| s.absolute_numbering)
                        .unwrap_or(
                            global_config
                                .get()
                                .flatten()
                                .map(|c| c.general.absolute_numbering)
                                .unwrap_or(false),
                        );
                    spawn_local(async move {
                        crate::components::common::toast::show_toast("Reorganization started...", NotificationType::Info);
                        match crate::api::reorganize_series(id, target_absolute).await {
                            Ok(_) => {
                                crate::components::common::toast::show_toast("Reorganization completed successfully", NotificationType::Success);
                                form_state.update(|s| {
                                    s.original_absolute_numbering = s.absolute_numbering;
                                    let global = global_config.get_untracked().flatten();
                                    s.original_season_folder_format = s
                                        .season_folder_format
                                        .clone()
                                        .or_else(|| global.as_ref().map(|c| c.organization.season_folder_format.clone()))
                                        .unwrap_or_default();
                                    s.original_episode_file_format = s
                                        .episode_file_format
                                        .clone()
                                        .or_else(|| global.as_ref().map(|c| c.organization.episode_file_format.clone()))
                                        .unwrap_or_default();
                                });
                                series_details.refetch();
                                // Mutate the cached series list entry in-place (SSoT helper)
                                if let Some(form) = form_state.try_get_untracked() {
                                    crate::utils::update_cached_series_profiles(
                                        &id_for_cache,
                                        &form.quality_profile,
                                        &form.release_profile,
                                    );
                                }
                                if let Some(r) = refresh_rename_queue {
                                    r.1.run(());
                                }
                            },
                            Err(e) => {
                                crate::components::common::toast::show_toast(format!("Failed to reorganize: {}", e.user_message()), NotificationType::Error);
                            }
                        }
                    });
                })
                on_later=Callback::new(move |_| {
                    set_show_rename_modal.set(false);
                })
            />
        </div>
    }
}
