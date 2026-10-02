use crate::api::{add_ban, fetch_bans, fetch_config, remove_ban, save_config};
use crate::components::common::form_fields::{
    CheckboxField, FormGroup, NumberInput, NumberInputMode, TextInput,
};
use crate::components::common::table_builder::{ManagedColumn, TableBuilder};
use crate::components::common::{SettingsBuilder, SettingsCheckbox, Stack};
use crate::components::settings::time_input::{TimeInputAdapter, TimeUnit};

use crate::hooks::use_config::{ConfigContext, use_config};
use crate::hooks::use_persistent_table_state;
use crate::hooks::use_ui_config::use_time_format;
use crate::hooks::{use_config_binding, use_number_binding};
use crate::utils::format_datetime_local;

use jumbie_shared::config::{AuthConfig, Config};
use jumbie_shared::types::BanEntry;
use leptos::prelude::*;

const TEXTAREA_ROWS: &str = "4";

#[component]
pub fn SecuritySettings() -> impl IntoView {
    let ConfigContext { config, set_config } = use_config();

    // User's configured 12h/24h preference for ban timestamps.
    let time_format = use_time_format();
    crate::utils::create_mount_resource(move || async move {
        match fetch_config().await {
            Ok(c) => set_config.set(Some(c)),
            Err(e) => crate::debug_error!("Failed to load config: {}", e),
        }
    });

    // Numeric access-control fields: blank resolves to the shared default. Saving
    // is manual (the section's Save button), so the bindings take a no-op trigger.
    let no_autosave = |_: bool| {};
    let bind_max_auth_fail_count = use_number_binding(
        |c: &Config| c.auth.max_auth_fail_count,
        |c: &mut Config, v| c.auth.max_auth_fail_count = v,
        AuthConfig::MAX_AUTH_FAIL_COUNT_DEFAULT,
        no_autosave,
    );
    let bind_ban_duration = use_number_binding(
        |c: &Config| c.auth.ban_duration_seconds,
        |c: &mut Config, v| c.auth.ban_duration_seconds = v,
        AuthConfig::BAN_DURATION_SECONDS_DEFAULT,
        no_autosave,
    );
    let bind_ban_increment_factor = use_number_binding(
        |c: &Config| c.auth.ban_increment_factor,
        |c: &mut Config, v| c.auth.ban_increment_factor = v,
        AuthConfig::BAN_INCREMENT_FACTOR_DEFAULT,
        no_autosave,
    );
    let bind_ban_increment_max_seconds = use_number_binding(
        |c: &Config| c.auth.ban_increment_max_seconds,
        |c: &mut Config, v| c.auth.ban_increment_max_seconds = v,
        AuthConfig::BAN_INCREMENT_MAX_SECONDS_DEFAULT,
        no_autosave,
    );
    let bind_ban_count_reset_days = use_number_binding(
        |c: &Config| c.auth.ban_count_reset_days,
        |c: &mut Config, v| c.auth.ban_count_reset_days = v,
        AuthConfig::BAN_COUNT_RESET_DAYS_DEFAULT,
        no_autosave,
    );

    let ban_duration_adapter = TimeInputAdapter::new(
        bind_ban_duration,
        TimeUnit::Seconds,
        1,
        u64::MAX,
        AuthConfig::BAN_DURATION_SECONDS_DEFAULT,
    );
    let ban_increment_max_adapter = TimeInputAdapter::new(
        bind_ban_increment_max_seconds,
        TimeUnit::Seconds,
        1,
        u64::MAX,
        AuthConfig::BAN_INCREMENT_MAX_SECONDS_DEFAULT,
    );
    let ban_count_reset_adapter = TimeInputAdapter::new(
        bind_ban_count_reset_days,
        TimeUnit::Days,
        1,
        u64::MAX,
        AuthConfig::BAN_COUNT_RESET_DAYS_DEFAULT as u64,
    );

    SettingsBuilder::new("security-access")
        .section("Access Control", |section| {
            section.field(view! {
                <div class="flex flex-col gap-sm">
                    <CheckboxField
                        label="Bypass authentication for clients on localhost"
                        checked=Signal::derive(move || config.with(|c| c.as_ref().map(|c| c.auth.bypass_local_auth).unwrap_or(false)))
                        set_checked=Callback::new(move |checked| {
                            set_config.update(|c| { if let Some(c) = c { c.auth.bypass_local_auth = checked; } });
                        })
                    />
                    <CheckboxField
                        label="Bypass authentication for clients in whitelisted IP subnets"
                        checked=Signal::derive(move || config.with(|c| c.as_ref().map(|c| c.auth.bypass_subnet_whitelist).unwrap_or(false)))
                        set_checked=Callback::new(move |checked| {
                            set_config.update(|c| { if let Some(c) = c { c.auth.bypass_subnet_whitelist = checked; } });
                        })
                    />
                </div>
            })
            .field(view! {
                <FormGroup
                    label="Whitelisted subnets".to_string()
                    help_text="One CIDR per line, e.g. 192.168.1.0/24 or fdff::/40".to_string()
                >
                    <textarea
                        id="whitelistedSubnets"
                        class="form-input auth-textarea-mono"
                        rows={TEXTAREA_ROWS}
                        placeholder="192.168.0.0/24\n10.0.0.0/8"
                        prop:value=move || config.with(|c|
                            c.as_ref().map(|c| c.auth.subnet_whitelist.join("\n")).unwrap_or_default()
                        )
                        on:change=move |ev| {
                            let raw = event_target_value(&ev);
                            let list: Vec<String> = raw.lines()
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            set_config.update(|c| { if let Some(c) = c { c.auth.subnet_whitelist = list; } });
                        }
                    />
                </FormGroup>
            })
            .field(view! {
                    <NumberInput
                        label="Ban after consecutive failures".to_string()
                        help_text="Set to 0 to disable".to_string()
                        id="banAfterConsecutiveFailures".to_string()
                        placeholder="0"
                        min="0".to_string()
                        mode=NumberInputMode::Integer
                        value=bind_max_auth_fail_count.value
                        set_value=bind_max_auth_fail_count.set_value
                    />
                    <NumberInput
                        label="Ban duration".to_string()
                        help_text="How long a banned client is blocked (e.g. 5min, 2h)".to_string()
                        id="banDurationSeconds".to_string()
                        value=ban_duration_adapter.display
                        set_value=ban_duration_adapter.set_value
                        error=ban_duration_adapter.error
                        placeholder=ban_duration_adapter.placeholder
                    />
            })
            .field(view! {
                <div class="flex flex-col gap-sm">
                    <CheckboxField
                        label="Enable incrementing ban durations for reoccurring offenders"
                        checked=Signal::derive(move || config.with(|c| c.as_ref().map(|c| c.auth.ban_increment_enabled).unwrap_or(false)))
                        set_checked=Callback::new(move |checked| {
                            set_config.update(|c| { if let Some(c) = c { c.auth.ban_increment_enabled = checked; } });
                        })
                    />
                    <Show when=move || config.with(|c| c.as_ref().map(|c| c.auth.ban_increment_enabled).unwrap_or(false))>
                            <NumberInput
                                label="Multiplier factor".to_string()
                                help_text="Duration = base * (factor ^ (count - 1))".to_string()
                                id="banIncrementFactor".to_string()
                                min="1.0".to_string()
                                mode=NumberInputMode::Decimal
                                value=bind_ban_increment_factor.value
                                set_value=bind_ban_increment_factor.set_value
                            />
                            <NumberInput
                                label="Maximum ban duration".to_string()
                                help_text="Cap for incremented durations (e.g. 24h, 7d, 1y)".to_string()
                                id="banIncrementMaxSeconds".to_string()
                                value=ban_increment_max_adapter.display
                                set_value=ban_increment_max_adapter.set_value
                                error=ban_increment_max_adapter.error
                                placeholder=ban_increment_max_adapter.placeholder
                            />
                            <NumberInput
                                label="Reset counter after".to_string()
                                help_text="IP gets a clean slate after this long without offenses (e.g. 30d, 1mon, 2mon)".to_string()
                                id="banCountResetDays".to_string()
                                value=ban_count_reset_adapter.display
                                set_value=ban_count_reset_adapter.set_value
                                error=ban_count_reset_adapter.error
                                placeholder=ban_count_reset_adapter.placeholder
                            />
                    </Show>
                </div>
            })

            .custom_view(view! {
                <div class="flex justify-end mt-md pt-md">
                    <button class="btn btn-primary"
                        on:click=move |_| {
                            if let Some(c) = config.get_untracked() {
                                crate::utils::spawn_api_toast(
                                    save_config(c),
                                    None,
                                    move |_| {
                                        crate::components::common::toast::show_success("Access control settings saved");
                                    }
                                );
                            }
                        }
                    >"Save Access Control"</button>
                </div>
            })
        })
        .raw_section({
            let (bans, set_bans) = signal(Vec::<BanEntry>::new());
            let (new_ban_ip, set_new_ban_ip) = signal(String::new());
            let (new_ban_duration, set_new_ban_duration) = signal(String::new());

            let (sort_col, sort_asc, on_sort) = use_persistent_table_state("ip_bans".to_string(), "ip".to_string(), false);

            crate::utils::spawn_cached_with(
                "fetch_bans".to_string(),
                || fetch_bans(),
                move |list| {
                    let _ = set_bans.try_update(|s| *s = list);
                },
            );

            let refresh_bans = move || {
                crate::utils::invalidate_cache_prefix("fetch_bans");
                crate::utils::spawn_cached_with(
                    "fetch_bans".to_string(),
                    || fetch_bans(),
                    move |list| {
                        let _ = set_bans.try_update(|s| *s = list);
                    },
                );
            };

            let columns: Vec<ManagedColumn<BanEntry>> = vec![
                ManagedColumn {
                    id: "ip".into(), label: "IP Address".into(), sortable: true, class: "text-xs font-mono".into(),
                    cell_render: Callback::new(|ban: BanEntry| view! { <span>{ban.ip}</span> }.into_any())
                },
                ManagedColumn {
                    id: "attempts".into(), label: "Attempts".into(), sortable: true, class: "text-xs".into(),
                    cell_render: Callback::new(|ban: BanEntry| view! { <span>{ban.fail_count}</span> }.into_any())
                },
                ManagedColumn {
                    id: "ban_count".into(), label: "Ban Count".into(), sortable: true, class: "text-xs".into(),
                    cell_render: Callback::new(|ban: BanEntry| view! { <span>{ban.ban_count}</span> }.into_any())
                },
                ManagedColumn {
                    id: "banned_at".into(), label: "Banned At".into(), sortable: true, class: "text-xs-muted".into(),
                    cell_render: Callback::new(move |ban: BanEntry| {
                        let banned_at = format_datetime_local(&ban.banned_at, &time_format.get());
                        view! { <span>{banned_at}</span> }.into_any()
                    })
                },
                ManagedColumn {
                    id: "expires".into(), label: "Expires".into(), sortable: true, class: "text-xs".into(),
                    cell_render: Callback::new(move |ban: BanEntry| {
                        let expires_label = if ban.is_permanent {
                            "Permanent".to_string()
                        } else {
                            ban.banned_until.as_deref()
                                .map(|ts| format_datetime_local(ts, &time_format.get()))
                                .unwrap_or_else(|| "Unknown".to_string())
                        };
                        view! {
                            <span style=move || if ban.is_permanent { "color: var(--danger); font-weight: 600;" } else { "color: var(--text-muted);" }>
                                {expires_label}
                            </span>
                        }.into_any()
                    })
                },
                ManagedColumn {
                    id: "actions".into(), label: "Actions".into(), sortable: false, class: "col-actions".into(),
                    cell_render: Callback::new(move |ban: BanEntry| {
                        let ip = ban.ip.clone();
                        view! {
                            <div class="action-cell">
                                <button class="btn btn-sm text-danger"
                                    on:click=move |_| {
                                        let ip = ip.clone();
                                        crate::utils::spawn_api_toast(
                                            remove_ban(ip),
                                            None,
                                            move |_| {
                                                crate::components::common::toast::show_success("IP unbanned");
                                                refresh_bans();
                                            }
                                        );
                                    }
                                >"Unban"</button>
                            </div>
                        }.into_any()
                    })
                },
            ];

            view! {
                <div class="settings-section">
                    <Stack>
                        <h3>"Banned IPs"</h3>

                        {TableBuilder::new(sort_col, sort_asc)
                            .loading(Signal::derive(move || false))
                            .empty_message("No banned IPs found.")
                            .on_sort(on_sort)
                            .build_managed(
                                Signal::from(bans),
                                columns,
                                Callback::new(|(a, b, col): (BanEntry, BanEntry, String)| {
                                    match col.as_str() {
                                        "ip" => jumbie_shared::formatting::natural_cmp(&a.ip, &b.ip),
                                        "attempts" => a.fail_count.cmp(&b.fail_count),
                                        "ban_count" => a.ban_count.cmp(&b.ban_count),
                                        "banned_at" => jumbie_shared::formatting::natural_cmp(&a.banned_at, &b.banned_at),
                                        "expires" => jumbie_shared::formatting::natural_cmp(
                                            a.banned_until.as_deref().unwrap_or(""),
                                            b.banned_until.as_deref().unwrap_or(""),
                                        ),
                                        _ => std::cmp::Ordering::Equal,
                                    }
                                }),
                                None::<Callback<jumbie_shared::types::BanEntry>>,
                                None::<Callback<jumbie_shared::types::BanEntry, bool>>,
                                Some(Callback::new(move |ban: BanEntry| {
                                    let ip = ban.ip.clone();
                                    let banned_at = format_datetime_local(&ban.banned_at, &time_format.get());
                                    let expires_label = if ban.is_permanent {
                                        "Permanent".to_string()
                                    } else {
                                        ban.banned_until.as_deref()
                                            .map(|ts| format_datetime_local(ts, &time_format.get()))
                                            .unwrap_or_else(|| "Unknown".to_string())
                                    };

                                    view! {
                                        <div class="card p-md bg-secondary border border-radius flex flex-col gap-sm">
                                            <div class="flex justify-between items-center min-w-0">
                                                <strong class="font-mono truncate">{ban.ip.clone()}</strong>
                                                <span class="text-xs text-muted">{banned_at}</span>
                                            </div>
                                            <div class="flex justify-between items-end">
                                                <div class="flex flex-col gap-xs">
                                                    <span class="text-xs text-muted">"Attempts: " {ban.fail_count}</span>
                                                    <span class="text-xs text-muted">"Ban Count: " {ban.ban_count}</span>
                                                    <span class="text-xs" style=move || if ban.is_permanent { "color: var(--danger); font-weight: 600;" } else { "color: var(--text-muted);" }>
                                                        {expires_label.clone()}
                                                    </span>
                                                </div>
                                                <button class="btn btn-sm text-danger"
                                                    on:click=move |_| {
                                                        let ip = ip.clone();
                                                        crate::utils::spawn_api_toast(
                                                            remove_ban(ip),
                                                            None,
                                                            move |_| {
                                                                crate::components::common::toast::show_success("IP unbanned");
                                                                refresh_bans();
                                                            }
                                                        );
                                                    }
                                                >"Unban"</button>
                                            </div>
                                        </div>
                                    }.into_any()
                                }))
                            )}

                        <div id="manual-ban-settings">
                            <NumberInput
                                id="banIp"
                                label="IP Address"
                                mode=NumberInputMode::IpAddress
                                placeholder="192.168.1.50"
                                class="font-mono"
                                value=Signal::derive(move || new_ban_ip.get())
                                set_value=Callback::new(move |v| set_new_ban_ip.set(v))
                            />
                            <NumberInput
                                id="banDuration"
                                label="Duration (seconds)"
                                mode=NumberInputMode::Integer
                                min="1"
                                placeholder="blank = permanent"
                                value=Signal::derive(move || new_ban_duration.get())
                                set_value=Callback::new(move |v| set_new_ban_duration.set(v))
                            />
                        </div>
                        <div class="flex justify-end mt-md pt-md">
                        <button class="btn btn-primary"
                            on:click=move |_| {
                                let ip = new_ban_ip.get_untracked();
                                if ip.is_empty() {
                                    crate::components::common::toast::show_error("Enter an IP address to ban");
                                    return;
                                }
                                let duration = new_ban_duration.get_untracked()
                                    .parse::<u64>().ok()
                                    .filter(|&d| d > 0);
                                crate::utils::spawn_api_toast(
                                    add_ban(ip, duration),
                                    None,
                                    move |_| {
                                        crate::components::common::toast::show_success("IP banned");
                                        set_new_ban_ip.set(String::new());
                                        set_new_ban_duration.set(String::new());
                                        refresh_bans();
                                    }
                                );
                            }
                        >"Add Ban"</button>
                        </div>
                    </Stack>
                </div>
            }
        })
        .section("Security Protections", |section| {
            section.field(view! {
                <div class="flex flex-col gap-sm">
                    <CheckboxField
                        label="Enable clickjacking protection (X-Frame-Options: SAMEORIGIN)"
                        checked=Signal::derive(move || config.with(|c| c.as_ref().map(|c| c.security.clickjacking_protection).unwrap_or(false)))
                        set_checked=Callback::new(move |v| {
                            set_config.update(|c| if let Some(c) = c { c.security.clickjacking_protection = v });
                        })
                    />
                    <CheckboxField
                        label="Enable CSRF protection (X-Content-Type-Options, Referrer-Policy)"
                        checked=Signal::derive(move || config.with(|c| c.as_ref().map(|c| c.security.csrf_protection).unwrap_or(false)))
                        set_checked=Callback::new(move |v| {
                            set_config.update(|c| if let Some(c) = c { c.security.csrf_protection = v });
                        })
                    />
                </div>
            })
            .field(view! {
                <FormGroup
                    label="Host header validation".to_string()
                    help_text="Reject requests with a non-matching Host. One entry per line, * as wildcard.".to_string()
                >
                    <CheckboxField
                        label="Enable Host header validation"
                        class="form-input-below"
                        checked=Signal::derive(move || config.with(|c| c.as_ref().map(|c| c.security.host_header_validation).unwrap_or(false)))
                        set_checked=Callback::new(move |v| {
                            set_config.update(|c| if let Some(c) = c { c.security.host_header_validation = v });
                        })
                    />
                    <textarea id="allowedDomains" class="form-input auth-textarea-mono" rows={TEXTAREA_ROWS}
                        placeholder="localhost\n192.168.1.100\n*.example.com"
                        prop:value=move || config.with(|c| c.as_ref().map(|c| c.security.allowed_domains.join("\n")).unwrap_or_default())
                        on:change=move |ev| {
                            let list: Vec<String> = event_target_value(&ev).lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                            set_config.update(|c| { if let Some(c) = c { c.security.allowed_domains = list; } });
                        }
                    />
                </FormGroup>
            })
            .field(view! {
                <FormGroup
                    label="Cross-Origin Resource Sharing (CORS)".to_string()
                    help_text="Restrict API access from web browsers on other domains. One origin per line.".to_string()
                >
                    <CheckboxField
                        label="Restrict CORS to specific origins (uncheck for permissive wildcard CORS)"
                        class="form-input-below"
                        checked=Signal::derive(move || config.with(|c| c.as_ref().map(|c| c.security.restrict_cors).unwrap_or(false)))
                        set_checked=Callback::new(move |v| {
                            set_config.update(|c| if let Some(c) = c { c.security.restrict_cors = v });
                        })
                    />
                    <textarea id="allowedOrigins" class="form-input auth-textarea-mono" rows={TEXTAREA_ROWS}
                        placeholder="http://localhost:8080\nhttps://myapp.example.com"
                        prop:value=move || config.with(|c| c.as_ref().map(|c| c.security.allowed_origins.join("\n")).unwrap_or_default())
                        on:change=move |ev| {
                            let list: Vec<String> = event_target_value(&ev).lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                            set_config.update(|c| { if let Some(c) = c { c.security.allowed_origins = list; } });
                        }
                    />
                </FormGroup>
            })
            .field(view! {
                <FormGroup
                    label="Custom HTTP headers".to_string()
                    help_text="One \"Header: value\" pair per line, added to every response.".to_string()
                >
                    <CheckboxField
                        label="Enable custom HTTP headers"
                        checked=Signal::derive(move || config.with(|c| c.as_ref().map(|c| c.security.use_custom_headers).unwrap_or(false)))
                        set_checked=Callback::new(move |v| {
                            set_config.update(|c| if let Some(c) = c { c.security.use_custom_headers = v });
                        })
                    />
                    <textarea id="customHeaders" class="form-input auth-textarea-mono" rows={TEXTAREA_ROWS}
                        placeholder="Strict-Transport-Security: max-age=31536000\nContent-Security-Policy: default-src 'self'"
                        prop:value=move || config.with(|c| c.as_ref().map(|c| c.security.custom_headers.join("\n")).unwrap_or_default())
                        on:change=move |ev| {
                            let list: Vec<String> = event_target_value(&ev).lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                            set_config.update(|c| { if let Some(c) = c { c.security.custom_headers = list; } });
                        }
                    />
                </FormGroup>
            })
            .custom_view(view! {
                <div class="flex justify-end mt-md pt-md">
                    <button class="btn btn-primary"
                        on:click=move |_| {
                            if let Some(c) = config.get_untracked() {
                                crate::utils::spawn_api_toast(
                                    save_config(c),
                                    None,
                                    move |_| {
                                        crate::components::common::toast::show_success("Security settings saved");
                                    }
                                );
                            }
                        }
                    >"Save Security"</button>
                </div>
            })
        })
        .section("Proxy Settings", |section| {
            let bind_proxy = use_config_binding(
                move |c| c.proxy.enabled,
                move |c, v| c.proxy.enabled = v,
                |_| {}
            );
            section.field(view! {
                <SettingsCheckbox
                    label="Enable Proxy"
                    binding=bind_proxy
                />
            })
            .field(view! {

                    <TextInput
                        label="HTTP Proxy".to_string()
                        id="httpProxy".to_string()
                        value=Signal::derive(move || config.get().map(|c| c.proxy.http).unwrap_or_default())
                        set_value=move |v| set_config.update(|c| if let Some(c) = c { c.proxy.http = v })

                    />

            })
            .field(view! {

                    <TextInput
                        label="HTTPS Proxy"
                        id="httpsProxy".to_string()
                        value=Signal::derive(move || config.get().map(|c| c.proxy.https).unwrap_or_default())
                        set_value=move |v| set_config.update(|c| if let Some(c) = c { c.proxy.https = v })

                    />

            })
            .custom_view(view! {
                <div class="flex justify-end mt-md pt-md">
                    <button class="btn btn-primary"
                        on:click=move |_| {
                            if let Some(c) = config.get_untracked() {
                                let mut to_save = c.clone();
                                if to_save.proxy.enabled {
                                    if to_save.proxy.http.is_empty() {
                                        to_save.proxy.http = String::new();
                                    }
                                    if to_save.proxy.https.is_empty() {
                                        to_save.proxy.https = String::new();
                                    }
                                }
                                crate::utils::spawn_api_toast(
                                    save_config(to_save),
                                    None,
                                    move |_| {
                                        crate::components::common::toast::show_success("Proxy settings saved");
                                    }
                                );
                            }
                        }
                    >"Save Proxy"</button>
                </div>
            })
        })
        .build()
}
