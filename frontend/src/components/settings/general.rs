use crate::api::{fetch_public_theme, save_config};
use crate::components::common::form_fields::{NumberInput, NumberInputMode, Select, SelectOption};
use crate::components::common::{Group, SettingsBuilder, SettingsCheckbox, SettingsSectionBuilder};
use crate::components::settings::time_input::{TimeInputAdapter, TimeUnit};
use crate::hooks::use_config::{ConfigContext, use_config};
use crate::hooks::use_ui_config::{persist_ui_config, use_ui_config};
use crate::hooks::{ConfigBinding, use_config_binding, use_number_binding};
use jumbie_shared::config::{Config, GeneralConfig, SeasonPackStrategy, Theme, TimeFormat};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn GeneralSettings() -> impl IntoView {
    let ConfigContext { config, set_config } = use_config();
    let trigger_save = crate::utils::use_autosave(config, |c| save_config(c));
    let ts = trigger_save.clone();

    let bind_scan_enabled = use_config_binding(
        |c| c.general.media_info_scan_enabled,
        |c, v| c.general.media_info_scan_enabled = v,
        ts.clone(),
    );
    let bind_scan_interval = use_number_binding(
        |c: &Config| c.general.media_info_scan_interval,
        |c: &mut Config, v| c.general.media_info_scan_interval = v,
        GeneralConfig::MEDIA_INFO_SCAN_INTERVAL_DEFAULT,
        ts.clone(),
    );
    // Blank or "0" both mean no cap (the UNLIMITED sentinel).
    let bind_scan_concurrency = use_number_binding(
        |c: &Config| c.general.media_info_scan_concurrency,
        |c: &mut Config, v| c.general.media_info_scan_concurrency = v,
        GeneralConfig::MEDIA_INFO_SCAN_CONCURRENCY_UNLIMITED,
        ts.clone(),
    );
    let bind_auto_search_wanted = use_config_binding(
        |c| c.general.auto_search_wanted_enabled,
        |c, v| c.general.auto_search_wanted_enabled = v,
        ts.clone(),
    );
    let bind_auto_search_wanted_interval = use_number_binding(
        |c: &Config| c.general.auto_search_wanted_interval,
        |c: &mut Config, v| c.general.auto_search_wanted_interval = v,
        GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_DEFAULT,
        ts.clone(),
    );
    let bind_auto_search_wanted_min_wait = use_number_binding(
        |c: &Config| c.general.auto_search_wanted_min_wait,
        |c: &mut Config, v| c.general.auto_search_wanted_min_wait = v,
        GeneralConfig::AUTO_SEARCH_WANTED_MIN_WAIT_DEFAULT,
        ts.clone(),
    );
    let bind_auto_search_wanted_max_age_days = use_number_binding(
        |c: &Config| c.general.auto_search_wanted_max_age_days,
        |c: &mut Config, v| c.general.auto_search_wanted_max_age_days = v,
        GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_DEFAULT,
        ts.clone(),
    );

    let bind_season_thresh = use_number_binding(
        |c: &Config| c.general.season_pack_replace_threshold,
        |c: &mut Config, v| c.general.season_pack_replace_threshold = v,
        GeneralConfig::SEASON_PACK_REPLACE_THRESHOLD_DEFAULT,
        ts.clone(),
    );
    // Optional numeric fields resolve blank to their default in the frontend too,
    // so every settings field follows the same "blank → default" rule. `None` is
    // still accepted from legacy/API payloads and reads as 0.
    let bind_manual_score = use_number_binding(
        |c: &Config| c.general.default_score_for_manual_files.unwrap_or(0),
        |c: &mut Config, v| c.general.default_score_for_manual_files = Some(v),
        0,
        ts.clone(),
    );

    let bind_pack_score_modifier = use_number_binding(
        |c: &Config| c.general.season_pack_score_modifier.unwrap_or(0),
        |c: &mut Config, v| c.general.season_pack_score_modifier = Some(v),
        0,
        ts.clone(),
    );

    // Wraps raw config bindings with smart display parsing ("2h", "90min", "3days", ...).
    // Raw value signals are saved before consuming bindings into adapters, so the
    // warning logic in auto_search_section can still read them.

    let min_wait_raw = bind_auto_search_wanted_min_wait.value;
    let give_up_raw = bind_auto_search_wanted_max_age_days.value;

    let interval_adapter = TimeInputAdapter::new(
        bind_auto_search_wanted_interval,
        TimeUnit::Minutes,
        GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MIN,
        GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_MAX,
        GeneralConfig::AUTO_SEARCH_WANTED_INTERVAL_DEFAULT,
    );
    let min_wait_adapter = TimeInputAdapter::new(
        bind_auto_search_wanted_min_wait,
        TimeUnit::Minutes,
        GeneralConfig::AUTO_SEARCH_WANTED_MIN_WAIT_MIN,
        u64::MAX,
        GeneralConfig::AUTO_SEARCH_WANTED_MIN_WAIT_DEFAULT,
    );
    let give_up_adapter = TimeInputAdapter::new(
        bind_auto_search_wanted_max_age_days,
        TimeUnit::Days,
        0,
        GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_MAX,
        GeneralConfig::AUTO_SEARCH_WANTED_MAX_AGE_DAYS_DEFAULT,
    );
    let scan_interval_adapter = TimeInputAdapter::new(
        bind_scan_interval,
        TimeUnit::Minutes,
        1,
        u64::MAX,
        GeneralConfig::MEDIA_INFO_SCAN_INTERVAL_DEFAULT,
    );

    let (ffmpeg_installed, set_ffmpeg_installed) = signal(false);
    crate::utils::use_api_cache(
        "fetch_health".to_string(),
        || crate::api::fetch_health(),
        move |h| {
            let _ = set_ffmpeg_installed.try_update(|s| *s = h.ffmpeg_installed);
        },
    );

    let (theme, set_theme) = signal(Theme::Auto);
    let (time_format, set_time_format) = signal(TimeFormat::Hour12);
    let (release_order, set_release_order) = signal(vec![
        "metadata".to_string(),
        "source".to_string(),
        "estimated".to_string(),
    ]);
    let (metadata_enabled, set_metadata_enabled) = signal(true);
    let (source_enabled, set_source_enabled) = signal(true);
    let (estimated_enabled, set_estimated_enabled) = signal(true);

    {
        let ui_config_signal = use_ui_config().ui_config;
        Effect::new(move |_| {
            if let Some(ui) = ui_config_signal.get() {
                let _ = set_theme.try_update(|s| *s = ui.theme.clone());
                let _ = set_time_format.try_update(|s| *s = ui.time_format.clone());
                let _ =
                    set_release_order.try_update(|s| *s = ui.release_date_display.order.clone());
                let _ = set_metadata_enabled
                    .try_update(|s| *s = ui.release_date_display.metadata_enabled);
                let _ = set_estimated_enabled
                    .try_update(|s| *s = ui.release_date_display.estimated_enabled);
                let _ =
                    set_source_enabled.try_update(|s| *s = ui.release_date_display.source_enabled);
            }
        });
    }

    let save_release_display = move || {
        let ctx = use_ui_config();
        ctx.set_ui_config.update(|ui| {
            if let Some(ui) = ui {
                ui.release_date_display.order = release_order.get_untracked();
                ui.release_date_display.metadata_enabled = metadata_enabled.get_untracked();
                ui.release_date_display.estimated_enabled = estimated_enabled.get_untracked();
                ui.release_date_display.source_enabled = source_enabled.get_untracked();
            }
        });
        if let Some(ui) = ctx.ui_config.get_untracked() {
            spawn_local(async move {
                persist_ui_config(ui).await;
            });
        }

        // Calendar positioning resolves client-side via `pick_release_date`, so no
        // cache invalidation is needed — changes take effect reactively.
    };

    let move_up = move |item_id: String| {
        set_release_order.update(|order| {
            if let Some(pos) = order.iter().position(|r| r == &item_id)
                && pos > 0
            {
                order.swap(pos, pos - 1);
            }
        });
        save_release_display();
    };

    let move_down = move |item_id: String| {
        set_release_order.update(|order| {
            if let Some(pos) = order.iter().position(|r| r == &item_id)
                && pos < order.len() - 1
            {
                order.swap(pos, pos + 1);
            }
        });
        save_release_display();
    };

    SettingsBuilder::new("settings-general")
        .section("Appearance", |s| {
            appearance_section(s, theme, set_theme, time_format, set_time_format)
        })
        .section("Series Features", |s| {
            series_features_section(
                s,
                ReleaseDateDisplaySignals {
                    order: release_order,
                    set_order: set_release_order,
                    metadata_enabled,
                    set_metadata_enabled,
                    source_enabled,
                    set_source_enabled,
                    estimated_enabled,
                    set_estimated_enabled,
                },
                save_release_display,
                move_up,
                move_down,
            )
        })
        .section("Auto-Search Wanted Episodes", |s| {
            auto_search_section(
                s,
                bind_auto_search_wanted,
                &interval_adapter,
                &min_wait_adapter,
                &give_up_adapter,
                min_wait_raw,
                give_up_raw,
            )
        })
        .section("Media Info Scanner", |s| {
            media_scanner_section(
                s,
                bind_scan_enabled,
                &scan_interval_adapter,
                bind_scan_concurrency,
                ffmpeg_installed,
            )
        })
        .section("Season Packs", |s| {
            season_packs_section(
                s,
                config,
                set_config,
                bind_season_thresh,
                bind_pack_score_modifier,
            )
        })
        .section("Manually Added Files", |s| {
            manual_files_section(s, bind_manual_score)
        })
        .build()
}

fn appearance_section(
    section: SettingsSectionBuilder,
    theme: ReadSignal<Theme>,
    set_theme: WriteSignal<Theme>,
    time_format: ReadSignal<TimeFormat>,
    set_time_format: WriteSignal<TimeFormat>,
) -> SettingsSectionBuilder {
    section
        .field(view! {
            <Select
                label="Theme".to_string()
                id="theme".to_string()
                value=Signal::derive(move || match theme.get() {
                    Theme::Auto => "auto".to_string(),
                    Theme::Light => "light".to_string(),
                    Theme::Dark => "dark".to_string(),
                })
                set_value=move |v: String| {
                    let t = match v.as_str() {
                        "light" => Theme::Light,
                        "dark" => Theme::Dark,
                        _ => Theme::Auto,
                    };
                    set_theme.set(t.clone());
                    let ctx = use_ui_config();
                    ctx.set_ui_config.update(|ui| {
                        if let Some(ui) = ui {
                            ui.theme = t.clone();
                        }
                    });
                    if let Some(ui) = ctx.ui_config.get_untracked() {
                        spawn_local(async move {
                            persist_ui_config(ui).await;
                            if let Ok(t_res) = fetch_public_theme().await
                                && let Some(window) = web_sys::window() {
                                    if let Some(doc) = window.document() {
                                        doc.document_element().map(|r| r.set_attribute("data-theme", &t_res.theme));
                                    }
                                    window.local_storage().ok().flatten().map(|s| s.set_item("jb_active_theme", &t_res.theme));
                                }
                        });
                    }
                }
                options=Signal::derive(move || vec![
                    SelectOption::from(("auto".to_string(), "Auto".to_string())),
                    SelectOption::from(("light".to_string(), "Light".to_string())),
                    SelectOption::from(("dark".to_string(), "Dark".to_string())),
                ])
            />
        })
        .field(view! {
            <Select
                label="Time Format".to_string()
                id="timeFormat".to_string()
                value=Signal::derive(move || match time_format.get() {
                    TimeFormat::Hour12 => "12h".to_string(),
                    TimeFormat::Hour24 => "24h".to_string(),
                })
                set_value=move |v: String| {
                    let tf = if v == "24h" { TimeFormat::Hour24 } else { TimeFormat::Hour12 };
                    set_time_format.set(tf.clone());
                    let ctx = use_ui_config();
                    ctx.set_ui_config.update(|ui| {
                        if let Some(ui) = ui {
                            ui.time_format = tf.clone();
                        }
                    });
                    if let Some(ui) = ctx.ui_config.get_untracked() {
                        spawn_local(async move {
                            persist_ui_config(ui).await;
                        });
                    }
                }
                options=Signal::derive(move || vec![
                    SelectOption::from(("12h".to_string(), "12-hour (1:00 PM)".to_string())),
                    SelectOption::from(("24h".to_string(), "24-hour (13:00)".to_string())),
                ])
            />
        })
}

#[derive(Clone, Copy)]
struct ReleaseDateDisplaySignals {
    order: ReadSignal<Vec<String>>,
    set_order: WriteSignal<Vec<String>>,
    metadata_enabled: ReadSignal<bool>,
    set_metadata_enabled: WriteSignal<bool>,
    source_enabled: ReadSignal<bool>,
    set_source_enabled: WriteSignal<bool>,
    estimated_enabled: ReadSignal<bool>,
    set_estimated_enabled: WriteSignal<bool>,
}

fn series_features_section(
    section: SettingsSectionBuilder,
    signals: ReleaseDateDisplaySignals,
    save_release_display: impl Fn() + Copy + 'static + Send + Sync,
    move_up: impl Fn(String) + Copy + 'static + Send + Sync,
    move_down: impl Fn(String) + Copy + 'static + Send + Sync,
) -> SettingsSectionBuilder {
    section
        .custom_view(view! {
            <div class="flex flex-col">
                <span class="form-label mb-0">"Release Date Display"</span>
                <p class="text-muted settings-description">
                    "Drag to reorder prioritize. Checked items will be fallback."
                </p>
                <div class="draggable-list" id="release-date-display-list">
                    <For
                        each=move || signals.order.get()
                        key=|item| item.clone()
                        children=move |item| {
                            let (label, is_enabled, set_enabled) = if item == "metadata" {
                                                            ("Online Release Date", signals.metadata_enabled, signals.set_metadata_enabled)
                                                        } else if item == "source" {
                                                            ("Source Feed Date", signals.source_enabled, signals.set_source_enabled)
                                                        } else {
                                                            ("Estimated Release Date", signals.estimated_enabled, signals.set_estimated_enabled)
                                                        };

                            let item_drag = item.clone();
                            let item_drop = item.clone();

                            view! {
                                <div
                                    class="draggable-item draggable-list-item"
                                    draggable="true"
                                    on:dragstart=move |e: ev::DragEvent| {
                                        if let Some(dt) = e.data_transfer() {
                                            let _ = dt.set_data("text/plain", &item_drag);
                                        }
                                    }
                                    on:dragover=move |e: ev::DragEvent| {
                                        e.prevent_default();
                                        if let Some(dt) = e.data_transfer() {
                                            dt.set_drop_effect("move");
                                        }
                                    }
                                    on:drop=move |e: ev::DragEvent| {
                                        e.prevent_default();
                                        if let Some(dt) = e.data_transfer() {
                                            let dragged_item = dt.get_data("text/plain").unwrap_or_default();
                                            if !dragged_item.is_empty() && dragged_item != item_drop {
                                                signals.set_order.update(|order| {
                                                    if let (Some(idx1), Some(idx2)) = (order.iter().position(|r| r == &dragged_item), order.iter().position(|r| r == &item_drop)) {
                                                        order.swap(idx1, idx2);
                                                    }
                                                });
                                                save_release_display();
                                            }
                                        }
                                    }
                                >
                                    <Group justify="between" class="w-full">
                                        <Group gap="md">
                                            <div class="flex flex-col gap-sm hide-desktop">
                                                <button class="btn-arrow" on:click={
                                                    let it = item.clone();
                                                    move |_| move_up(it.clone())
                                                }>"▲"</button>
                                                <button class="btn-arrow" on:click={
                                                    let it = item.clone();
                                                    move |_| move_down(it.clone())
                                                }>"▼"</button>
                                            </div>
                                            <span class="drag-handle-icon drag-handle hide-mobile">"☰"</span>
                                            <span class="drag-item-label">{label}</span>
                                        </Group>
                                        <SettingsCheckbox
                                            label="Enabled"
                                            binding=ConfigBinding {
                                                value: Signal::derive(move || is_enabled.get()),
                                                set_value: Callback::new(move |v: bool| {
                                                    set_enabled.set(v);
                                                    save_release_display();
                                                })
                                            }
                                            class="mb-0"
                                        />
                                    </Group>
                                </div>
                            }
                        }
                    />
                </div>
            </div>
        })
}

fn auto_search_section(
    section: SettingsSectionBuilder,
    bind_auto_search_wanted: ConfigBinding<bool>,
    interval_adapter: &TimeInputAdapter,
    min_wait_adapter: &TimeInputAdapter,
    give_up_adapter: &TimeInputAdapter,
    min_wait_raw: Signal<String>,
    give_up_raw: Signal<String>,
) -> SettingsSectionBuilder {
    let auto_search_wanted_value = bind_auto_search_wanted.clone();
    let interval_adapter = interval_adapter.clone();
    let min_wait_adapter = min_wait_adapter.clone();
    let give_up_adapter = give_up_adapter.clone();
    section
        .field(view! {
            <SettingsCheckbox
                label="Enable Auto-Search for Wanted Episodes"
                binding=bind_auto_search_wanted
                help_text=Signal::derive(move || "When enabled, the system will periodically auto-search for wanted episodes that haven't been downloaded yet. Episodes are only searched after they've been in the 'wanted' state for the configured minimum wait time.".to_string())
            />
        })
        .field(view! {
            <Show when=move || auto_search_wanted_value.value.get()>
                <div class="flex flex-col gap-sm">
                    <NumberInput
                        id="auto-search-interval"
                        label="Search Interval"
                        value=interval_adapter.display
                        set_value=interval_adapter.set_value
                        error=interval_adapter.error
                        placeholder=interval_adapter.placeholder
                        help_text=Signal::derive(move || "How often to check for wanted episodes. Enter as 90m, 2h, or 1d. Blank uses the default.".to_string())
                    />
                    <NumberInput
                        id="auto-search-min-wait"
                        label="Minimum Wait Time"
                        value=min_wait_adapter.display
                        set_value=min_wait_adapter.set_value
                        error=min_wait_adapter.error
                        placeholder=min_wait_adapter.placeholder
                        help_text=Signal::derive(move || "How long an episode must remain in 'wanted' state before auto-search will attempt to find it. This gives scene groups time to release encodes after an episode airs. Blank uses the default.".to_string())
                    />
                    <NumberInput
                        id="auto-search-give-up-days"
                        label="Give Up After"
                        value=give_up_adapter.display
                        set_value=give_up_adapter.set_value
                        error=give_up_adapter.error
                        placeholder=give_up_adapter.placeholder
                        help_text=Signal::derive(move || "Stop auto-searching episodes that have been wanted longer than this. Set to 0 to never give up. Enter as 2d, 1mon, or 1y. Blank uses the default.".to_string())
                    />

                    // Warn when Minimum Wait Time exceeds Give Up After: no episode
                    // would ever be searched, since episodes give up before the wait
                    // elapses.
                    {move || {
                        let min_wait = min_wait_raw.get().parse::<i64>().unwrap_or(0);
                        let give_up_days = give_up_raw.get().parse::<i64>().unwrap_or(0);
                        let give_up_minutes = give_up_days * 24 * 60;
                        if give_up_days > 0 && min_wait > give_up_minutes {
                            Some(view! {
                                <div class="text-warning text-sm mt-xs">
                                    {format!("Minimum Wait Time ({min_wait} min) exceeds Give Up After ({give_up_days} days = {give_up_minutes} min). Auto-search will never run because episodes will give up before the wait time elapses. Either reduce the wait time or increase the give-up period.")}
                                </div>
                            }.into_any())
                        } else {
                            None
                        }
                    }}
                </div>
            </Show>
        })
}

fn media_scanner_section(
    section: SettingsSectionBuilder,
    bind_scan_enabled: ConfigBinding<bool>,
    scan_interval_adapter: &TimeInputAdapter,
    bind_scan_concurrency: ConfigBinding<String>,
    ffmpeg_installed: ReadSignal<bool>,
) -> SettingsSectionBuilder {
    let scan_enabled_value = bind_scan_enabled.clone();
    let scan_interval_adapter = scan_interval_adapter.clone();
    section
        .field(view! {
            <SettingsCheckbox
                label="Enable Background Media Scan"
                binding=bind_scan_enabled
                disabled=Signal::derive(move || !ffmpeg_installed.get())
                help_text=Signal::derive(move || {
                    if ffmpeg_installed.get() {
                        "Scans media files via ffprobe to extract metadata (duration, resolution, codec, etc.) and populate episode information in the database.".to_string()
                    } else {
                        "FFmpeg/FFprobe is not installed. The toggle is disabled until FFmpeg is available.".to_string()
                    }
                })
            />
        })
        .field(view! {
            <Show when=move || scan_enabled_value.value.get()>
                <div class="flex flex-col gap-sm">
                    <NumberInput
                        id="media-scan-interval"
                        label="Media Scan Interval"
                        value=scan_interval_adapter.display
                        set_value=scan_interval_adapter.set_value
                        error=scan_interval_adapter.error
                        placeholder=scan_interval_adapter.placeholder
                        help_text=Signal::derive(move || "How frequently the background scanner checks for files missing media info. Enter as 15m, 1h, or 30min. Blank uses the default.".to_string())
                    />
                    <NumberInput
                        id="max-concurrent-scans"
                        label="Maximum Concurrent Media Scans"
                        mode=NumberInputMode::Integer
                        value=bind_scan_concurrency.value
                        set_value=bind_scan_concurrency.set_value
                        placeholder="no limit"
                        help_text=Signal::derive(move || "Maximum concurrent ffprobe + fingerprint operations globally. Higher values use more CPU/I/O; safe to increase on multi-core machines with SSDs. Set to 0 for no limit.".to_string())
                        min="0".to_string()
                    />
                </div>
            </Show>
        })
}

fn season_packs_section(
    section: SettingsSectionBuilder,
    config: ReadSignal<Option<Config>>,
    set_config: WriteSignal<Option<Config>>,
    bind_season_thresh: ConfigBinding<String>,
    bind_pack_score_modifier: ConfigBinding<String>,
) -> SettingsSectionBuilder {
    section
        .description("How season packs are scored and prioritized against individual episodes.")
        .field(view! {
            <Select
                label="Season Pack Strategy".to_string()
                id="seasonPackStrategy".to_string()
                value=Signal::derive(move || config.get().map(|c| match c.general.season_pack_strategy {
                    SeasonPackStrategy::FavorEpisodes => "favorepisodes".to_string(),
                    SeasonPackStrategy::FavorSeasonPacks => "favorpacks".to_string(),
                }).unwrap_or("favorepisodes".to_string()))
                set_value=move |v| {
                    crate::utils::write_config_field(&set_config, |c| {
                        c.general.season_pack_strategy = if v == "favorpacks" {
                            SeasonPackStrategy::FavorSeasonPacks
                        } else {
                            SeasonPackStrategy::FavorEpisodes
                        };
                    });
                }
                help_signal=Signal::derive(move || "Used as a tie breaker when scores of releases are the same.".to_string())
                options=Signal::derive(move || vec![
                    SelectOption::from(("favorepisodes".to_string(), "Favor Individual Episodes".to_string())),
                    SelectOption::from(("favorpacks".to_string(), "Favor Season Packs".to_string())),
                ])
            />
        })
        .field(view! {
            <NumberInput
                id="season-replacement-threshold"
                label="Consistency Replacement Threshold (%)"
                mode=NumberInputMode::Integer
                min="0"
                max="100"
                value=bind_season_thresh.value
                set_value=bind_season_thresh.set_value
                placeholder="50"
                help_text=Signal::derive(move || "A pack or episode range replaces existing episodes when it is the only source of a missing episode, or when its combined score beats the individual episodes (ties follow the season-pack preference). Below this share of missing episodes it only fills the gaps. Blank uses the default of 50.".to_string())
            />
        })
        .field(view! {
            <NumberInput
                id="season-pack-score-modifier"
                label="Season Pack Score Modifier"
                mode=NumberInputMode::SignedInteger
                value=bind_pack_score_modifier.value
                set_value=bind_pack_score_modifier.set_value
                placeholder="0"
                help_text=Signal::derive(move || "Flat score adjustment added to every season/complete pack. Positive values favor packs, negative values favor individual episodes. Blank is treated as 0.".to_string())
            />
        })
}

fn manual_files_section(
    section: SettingsSectionBuilder,
    bind_manual_score: ConfigBinding<String>,
) -> SettingsSectionBuilder {
    section
        .description("Scoring applied to files added outside of automatic search, such as manual imports.")
        .field(view! {
            <NumberInput
                id="manual-score"
                label="Default score for manually added files"
                mode=NumberInputMode::SignedInteger
                value=bind_manual_score.value
                set_value=bind_manual_score.set_value
                placeholder="0"
                help_text=Signal::derive(move || "Score assumed for files you add manually, used when comparing them against releases found by search. Blank is treated as 0.".to_string())
            />
        })
}
