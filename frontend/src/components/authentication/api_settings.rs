use crate::api::{fetch_config, generate_api_key, save_config};
use crate::components::common::calendar_link_modal::CreateCalendarLinkModal;
use crate::components::common::form_fields::{FormGroup, Select, SelectOption, TextInput};
use crate::components::common::icons::{CopyIcon, PlusIcon, TrashIcon};
use crate::components::common::modal_wrapper::ModalWrapper;
use crate::components::common::table_builder::{ManagedColumn, TableBuilder, TableVariant};
use crate::components::common::toast::*;
use crate::components::common::{
    RevealButton, RevealState, RevealableCode, SettingsBuilder, ViewHeader,
};
use crate::hooks::use_config::{ConfigContext, use_config};
use crate::hooks::use_persistent_table_state;
use crate::hooks::use_ui_config::use_time_format;
use jumbie_shared::auth::ApiScope;
use jumbie_shared::config::{ApiKey, Config, TimeFormat};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashSet;
use std::str::FromStr;

#[derive(Clone)]
struct ScopeDef {
    id: String,
    label: String,
    description: String,
    children: Vec<ScopeDef>,
}

/// Derives a human-readable label from a scope ID string.
/// e.g. "series:write" → "Series (Write)", "search" → "Search"
fn scope_label(id: &str) -> String {
    match id.split_once(':') {
        Some((resource, action)) => {
            let mut chars = resource.chars();
            let capitalized: String =
                chars.next().unwrap().to_uppercase().collect::<String>() + chars.as_str();
            format!(
                "{} ({})",
                capitalized,
                match action {
                    "write" => "Write",
                    "read" => "Read",
                    _ => action,
                }
            )
        }
        None => {
            // Standalone scopes like "search"
            let mut chars = id.chars();
            chars.next().unwrap().to_uppercase().collect::<String>() + chars.as_str()
        }
    }
}

/// Builds the scope tree from `ApiScope::all()`, deriving the write→read
/// hierarchy from the naming convention: write scopes parent their read scope,
/// while read-only scopes (e.g. `activity:read`) and `search` are roots.
///
/// The scope UI is generated from `ApiScope::all()` and `ApiScope::description()`,
/// so adding a scope there surfaces it here automatically.
fn build_scope_tree() -> Vec<ScopeDef> {
    // Collect resource names that have a write scope (and thus a write→read hierarchy)
    let write_resources: HashSet<&str> = ApiScope::all()
        .iter()
        .copied()
        .filter(|s| s.read_scope().is_some())
        .map(|s| s.as_str().split_once(':').unwrap().0)
        .collect();

    let mut seen = HashSet::new();
    let mut result = Vec::new();

    for scope in ApiScope::all() {
        let id = scope.as_str();
        let resource_key = id.split_once(':').map(|(r, _)| r).unwrap_or(id);

        // Skip read scopes that have a write counterpart — they'll be added as children
        if id.ends_with(":read") && write_resources.contains(resource_key) {
            continue;
        }

        if !seen.insert(resource_key) {
            continue;
        }

        let children = if let Some(read_scope) = scope.read_scope() {
            seen.insert(read_scope.as_str().split_once(':').unwrap().0);
            vec![ScopeDef {
                id: read_scope.as_str().to_string(),
                label: scope_label(read_scope.as_str()),
                description: read_scope.description().to_string(),
                children: vec![],
            }]
        } else {
            vec![]
        };

        result.push(ScopeDef {
            id: id.to_string(),
            label: scope_label(id),
            description: scope.description().to_string(),
            children,
        });
    }

    result
}

/// Formats a slice of scopes as a comma-separated string, or "None" when empty.
fn display_scopes(scopes: &[ApiScope]) -> String {
    if scopes.is_empty() {
        "None".to_string()
    } else {
        scopes
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Formats an expiry date string for display. Returns "Expired", a formatted date, "Invalid", or "Never".
fn format_expiry(expires_at: &Option<String>, time_format: &TimeFormat) -> String {
    match expires_at {
        Some(expires_at) => {
            if let Some(dt) = crate::utils::parse_timestamp_utc(expires_at) {
                if chrono::Utc::now() > dt {
                    "Expired".into()
                } else {
                    crate::utils::format_datetime_local(expires_at, time_format)
                }
            } else {
                "Invalid".into()
            }
        }
        None => "Never".into(),
    }
}

/// Renders an expiry value as a styled [`AnyView`] span.
fn expiry_cell(expires_at: &Option<String>, time_format: &TimeFormat) -> AnyView {
    match expires_at {
        Some(expires_at) => {
            if let Some(dt) = crate::utils::parse_timestamp_utc(expires_at) {
                if chrono::Utc::now() > dt {
                    view! { <span class="text-error">"Expired"</span> }.into_any()
                } else {
                    view! { <span>{crate::utils::format_datetime_local(expires_at, time_format)}</span> }.into_any()
                }
            } else {
                view! { <span>"Invalid"</span> }.into_any()
            }
        }
        None => view! { <span class="text-muted">"Never"</span> }.into_any(),
    }
}

/// Returns a click handler that removes an item from config, writes cache, and triggers save.
fn on_delete<Ts, Remove>(
    set_config: WriteSignal<Option<Config>>,
    ts: Ts,
    id: String,
    remove: Remove,
) -> impl Fn(ev::MouseEvent)
where
    Ts: Fn(bool) + Clone + 'static,
    Remove: Fn(&mut Config, &str) + Clone + 'static,
{
    move |e: ev::MouseEvent| {
        e.stop_propagation();
        let delete_id = id.clone();
        set_config.update(|c| {
            if let Some(c) = c {
                remove(c, &delete_id);
                crate::utils::write_cache("fetch_config", c);
            }
        });
        ts(true);
    }
}

struct DurationOption {
    value: &'static str,
    label: &'static str,
    days: Option<i64>,
}

const DURATION_OPTIONS: &[DurationOption] = &[
    DurationOption {
        value: "7",
        label: "7 Days",
        days: Some(7),
    },
    DurationOption {
        value: "30",
        label: "30 Days",
        days: Some(30),
    },
    DurationOption {
        value: "90",
        label: "90 Days",
        days: Some(90),
    },
    DurationOption {
        value: "365",
        label: "1 Year",
        days: Some(365),
    },
    DurationOption {
        value: "0",
        label: "No Expiration",
        days: None,
    },
];

#[component]
fn ScopeTree(
    scopes: Vec<ScopeDef>,
    selected_scopes: Signal<Vec<ApiScope>>,
    toggle_scope: Callback<(String, bool)>,
    #[prop(optional)] is_root: bool,
) -> impl IntoView {
    view! {
        <ul
            class=format!("scope-list list-style-none p-0 m-0 {} {}", if is_root {"root-scope-list auth-table-container"} else {"nested-scope-list"}, "")
        >
            {scopes.into_iter().map(move |scope| {
                let scope_id = scope.id.clone();
                let children = scope.children.clone();
                let has_children = !children.is_empty();

                let item_class = if is_root { "auth-table-item auth-table-item-root" } else { "auth-table-item auth-table-item-child" };

                // A non-root scope is forced when its parent write-scope is selected.
                // e.g. "series:read" is forced when "series:write" is in selected_scopes.
                let is_forced = {
                    let scope_id = scope_id.clone();
                    move || {
                        if !is_root
                            && let Ok(s) = ApiScope::from_str(&scope_id)
                                && let Some(write_scope) = s.write_scope() {
                                    return selected_scopes.get().contains(&write_scope);
                                }
                        false
                    }
                };

                let toggle_id = scope_id.clone();
                let is_forced_signal = Signal::derive(move || is_forced());

                view! {
                    <li class=item_class.to_string()>
                        <div class="flex gap-sm items-start">
                            <crate::components::common::form_fields::CheckboxInput
                                id=format!("scope-{}", scope_id)
                                checked=Signal::derive(move || selected_scopes.get().iter().any(|s| s.as_str() == scope_id.as_str()))
                                set_checked=Callback::new(move |checked| toggle_scope.run((toggle_id.clone(), checked)))
                                forced=is_forced_signal
                                disabled=is_forced_signal
                            >
                                <div class="flex flex-col cursor-pointer">
                                    <span class="font-bold">{scope.label}</span>
                                    <span class="text-muted text-sm">{scope.description}</span>
                                </div>
                            </crate::components::common::form_fields::CheckboxInput>
                        </div>
                        {if has_children {
                            view! {
                                <ScopeTree
                                    scopes=children.clone()
                                    selected_scopes=selected_scopes
                                    toggle_scope=toggle_scope
                                    is_root=false
                                />
                            }.into_any()
                        } else {
                            view! {}.into_any()
                        }}
                    </li>
                }
            }).collect_view()}
        </ul>
        {if is_root {
            view!{ <style>".root-scope-list > li:last-child { border-bottom: none !important; }"</style> }.into_any()
        } else {
            view!{}.into_any()
        }}
    }
}

#[component]
pub fn ApiSettings() -> impl IntoView {
    let ConfigContext { config, set_config } = use_config();
    let time_format = use_time_format();
    let show_modal = signal(false);
    let new_key_name = signal(String::new());
    let new_key_scopes = signal(Vec::<ApiScope>::new());
    let new_key_duration = signal("7".to_string());
    let generated_key = signal(Option::<String>::None);

    let trigger_save = crate::utils::use_autosave(config, |c| save_config(c));

    // Built once from ApiScope::all() — the single source of truth for scope definitions.
    let available_scopes = build_scope_tree();

    let handle_generate = Callback::new(move |_: ()| {
        let name = new_key_name.0.get().trim().to_string();
        if name.is_empty() {
            show_error("Key name cannot be empty");
            return;
        }

        let scopes = new_key_scopes.0.get();
        if scopes.is_empty() {
            show_error("At least one scope must be selected");
            return;
        }

        let duration_days = DURATION_OPTIONS
            .iter()
            .find(|o| o.value == new_key_duration.0.get())
            .and_then(|o| o.days)
            .or(Some(7));

        spawn_local(async move {
            match generate_api_key(name, scopes, duration_days).await {
                Ok(resp) => {
                    // Refresh config to get the new key with its prefix
                    if let Ok(new_cfg) = fetch_config().await {
                        set_config.set(Some(new_cfg));
                    }
                    generated_key.1.set(Some(resp.key));
                    new_key_name.1.set(String::new());
                    new_key_scopes.1.set(Vec::new());
                    new_key_duration.1.set("7".to_string());
                    show_success("API key generated");
                }
                Err(e) => {
                    show_error(format!("Failed to generate API key: {}", e));
                }
            }
        });
    });

    let show_cal_modal = signal(false);

    // Compute origin once — it doesn't change during the page's lifetime.
    let origin = web_sys::window()
        .expect("window is always available in CSR")
        .location()
        .origin()
        .expect("window.location.origin should never fail in a browser");

    let copy_key = move |_: ev::MouseEvent| {
        if let Some(key) = generated_key.0.get()
            && let Some(window) = web_sys::window()
        {
            let navigator = window.navigator();
            let _ = navigator.clipboard().write_text(&key);
            show_toast("API key copied to clipboard", NotificationType::Success);
        }
    };

    let toggle_scope = move |(scope_id, checked): (String, bool)| {
        if let Ok(scope) = ApiScope::from_str(&scope_id) {
            new_key_scopes.1.update(|scopes| {
                if checked {
                    if !scopes.contains(&scope) {
                        scopes.push(scope);
                    }
                    // Write implies Read: when a write scope is checked, also check its read counterpart
                    if let Some(read_scope) = scope.read_scope()
                        && !scopes.contains(&read_scope)
                    {
                        scopes.push(read_scope);
                    }
                } else {
                    scopes.retain(|s| s != &scope);
                    // Unchecking a read scope also unchecks its write counterpart
                    if let Some(write_scope) = scope.write_scope() {
                        scopes.retain(|s| s != &write_scope);
                    }
                }
            });
        }
    };

    let copy_calendar_link = {
        let origin = origin.clone();
        Callback::new(move |token: String| {
            let link = crate::utils::build_calendar_link_url(&origin, &token);

            if crate::utils::write_clipboard(&link) {
                show_toast(
                    "Calendar link copied to clipboard",
                    NotificationType::Success,
                );
            } else {
                show_toast(
                    "Clipboard access unavailable. Copy the URL manually.",
                    NotificationType::Info,
                );
            }
        })
    };

    SettingsBuilder::new("api")
    .raw_section({
        let trigger_save_keys = trigger_save.clone();
        let trigger_save_keys_mobile = trigger_save_keys.clone();
        let api_keys_data = Signal::derive(move || config.get().map(|c| c.auth.api_keys).unwrap_or_default());
        let (sort_col, sort_asc, on_sort) = use_persistent_table_state("api_keys".to_string(), "name".to_string(), true);

        let columns: Vec<ManagedColumn<ApiKey>> = vec![
            ManagedColumn {
                id: "name".into(), label: "Name".into(), sortable: true, class: "".into(),
                cell_render: Callback::new(|key: ApiKey| view! { <span>{key.name}</span> }.into_any()),
            },
            ManagedColumn {
                id: "scopes".into(), label: "Scopes".into(), sortable: false, class: "".into(),
                cell_render: Callback::new(|key: ApiKey| {
                    let scopes_display = display_scopes(&key.scopes);
                    view! { <span class="badge">{scopes_display}</span> }.into_any()
                }),
            },
            ManagedColumn {
                id: "prefix".into(), label: "Key Prefix".into(), sortable: true, class: "".into(),
                cell_render: Callback::new(|key: ApiKey| {
                    let prefix = if key.prefix.is_empty() { "***".to_string() } else { key.prefix.clone() };
                    view! { <code class="auth-code-bg">{prefix}</code> }.into_any()
                }),
            },
            ManagedColumn {
                id: "expires".into(), label: "Expires".into(), sortable: true, class: "".into(),
                cell_render: Callback::new({
                    let tf = time_format.clone();
                    move |key: ApiKey| expiry_cell(&key.expires_at, &tf.get())
                }),
            },
            ManagedColumn {
                id: "actions".into(), label: "Actions".into(), sortable: false, class: "col-actions".into(),
                cell_render: Callback::new(move |key: ApiKey| {
                    let ts = trigger_save_keys.clone();
                    view! {
                        <div class="action-cell">
                            <button class="btn btn-danger" title="Delete API Key"
                                on:click=on_delete(set_config, ts, key.id.clone(), |c: &mut Config, id: &str| c.auth.api_keys.retain(|k| k.id != id))>
                                <span class="icon"><TrashIcon /></span>
                            </button>
                        </div>
                    }.into_any()
                }),
            },
        ];

        view! {
            <div class="card-section">
            <div class="flex justify-between items-center mb-md">
                <div>
                    <p class="section-description text-muted-color">"Manage API keys used by external applications to integrate with Jumbie."</p>
                </div>
            </div>
            {TableBuilder::new(sort_col, sort_asc)
                .container_class(Signal::derive(move || "table-container".to_string()))
                .table_variant(TableVariant::Hover)
                .loading(Signal::derive(move || false))
                .empty_message("No API keys have been generated yet.")
                .on_sort(on_sort)
                .header_view(
                    view! {
                        <ViewHeader title="API Keys".to_string()
                        actions={view! {
                            <button class="btn btn-primary" on:click=move |_| {
                                generated_key.1.set(None);
                                show_modal.1.set(true);
                            }>
                                <span class="icon"><PlusIcon /></span> "Create API Key"
                            </button>
                        }.into_any()}/>
                    }.into_any()
                )
                .build_managed(
                    api_keys_data,
                    columns,
                    Callback::new(|(a, b, col): (ApiKey, ApiKey, String)| {
                        match col.as_str() {
                            "name" => jumbie_shared::formatting::natural_cmp(&a.name, &b.name),
                            _ => std::cmp::Ordering::Equal,
                        }
                    }),
                    None::<Callback<ApiKey>>,
                    None::<Callback<ApiKey, bool>>,
                    Some(Callback::new(move |key: ApiKey| {
                        let ts = trigger_save_keys_mobile.clone();
                        let prefix = if key.prefix.is_empty() { "***".to_string() } else { key.prefix.clone() };
                        let scopes_display = display_scopes(&key.scopes);

                        view! {
                            <div class="card p-md flex flex-col gap-sm">
                                <div class="flex justify-between items-start">
                                    <div class="flex flex-col gap-xs min-w-0">
                                        <span class="font-bold truncate">{key.name.clone()}</span>
                                        <code class="text-xs auth-code-bg w-fit">{prefix}</code>
                                    </div>
                                    <button class="btn-danger" on:click=on_delete(set_config, ts, key.id.clone(), |c: &mut Config, id: &str| c.auth.api_keys.retain(|k| k.id != id))>
                                        <span class="icon"><TrashIcon /></span>
                                    </button>
                                </div>
                                <div class="flex justify-between items-center text-xs text-muted">
                                    <span>"Scopes: " {scopes_display}</span>
                                    <span>"Expires: " {format_expiry(&key.expires_at, &time_format.get())}</span>
                                </div>
                            </div>
                        }.into_any()
                    })),
                )
            }

            </div>}
    })
    .raw_section({
        let trigger_save_cals = trigger_save.clone();
        let trigger_save_cals_mobile = trigger_save_cals.clone();
        let copy_calendar_link_mobile = copy_calendar_link.clone();
        let cal_tokens_data = Signal::derive(move || config.get().map(|c| c.auth.calendar_tokens).unwrap_or_default());
        let (sort_col, sort_asc, on_sort) = use_persistent_table_state("cal_tokens".to_string(), "name".to_string(), true);

        let columns: Vec<ManagedColumn<jumbie_shared::config::CalendarToken>> = vec![
            ManagedColumn {
                id: "name".into(), label: "Name".into(), sortable: true, class: "calendar-url-name".into(),
                cell_render: Callback::new(|t: jumbie_shared::config::CalendarToken| view! { <span>{t.name}</span> }.into_any()),
            },
            ManagedColumn {
                id: "url".into(), label: "Calendar URL".into(), sortable: false, class: "".into(),
                cell_render: Callback::new({
                    let origin = origin.clone();
                    move |t: jumbie_shared::config::CalendarToken| {
                        let url = crate::utils::build_calendar_link_url(&origin, &t.token);
                        let state = RevealState::new();
                        view! {
                            <div class="flex items-center gap-xs overflow-hidden">
                                <RevealableCode content={url.clone()} state={state} />
                                <RevealButton state={state} />
                            </div>
                        }.into_any()
                    }
                }),
            },
            ManagedColumn {
                id: "actions".into(), label: "Actions".into(), sortable: false, class: "col-actions".into(),
                cell_render: Callback::new(move |t: jumbie_shared::config::CalendarToken| {
                    let id_clone = t.id.clone();
                    let token_clone = t.token.clone();
                    let ts = trigger_save_cals.clone();
                    view! {
                        <div class="action-cell">
                            <button class="btn btn-secondary btn-calendar-link" title="Copy Calendar Link"
                                on:click=move |_| {
                                    copy_calendar_link.run(token_clone.clone());
                                }>
                                <span class="icon"><CopyIcon /></span> "Copy"
                            </button>
                            <button class="btn btn-danger" title="Delete Calendar Link"
                                on:click=on_delete(set_config, ts, id_clone.clone(), |c: &mut Config, id: &str| c.auth.calendar_tokens.retain(|k| k.id != id))>
                                <span class="icon"><TrashIcon /></span>
                            </button>
                        </div>
                    }.into_any()
                }),
            },
        ];

        view! {
            <div class="card-section">
                <div class="flex justify-between items-center mb-md">
                    <div>
                        <p class="section-description text-muted-color">"Generate personal iCal feed URLs to export release schedules to a calendar app."</p>
                        <p class="section-description text-sm text-muted-color mt-xs">"Note: Calendar links are automatically generated using the URL of the site you are currently accessing."</p>
                    </div>
                </div>
                {TableBuilder::new(sort_col, sort_asc)
                    .container_class(Signal::derive(move || "table-container".to_string()))
                    .table_variant(TableVariant::Hover)
                    .loading(Signal::derive(move || false))
                    .empty_message("No calendar links have been generated yet.")
                    .on_sort(on_sort)
                    .header_view(
                        view! {
                            <ViewHeader title="Calendar Feeds".to_string()
                            actions={view! {
                                <button class="btn btn-primary" on:click=move |_| show_cal_modal.1.set(true)>
                                    <span class="icon"><PlusIcon /></span> "Create Link"
                                </button>
                                                        }.into_any()}/>
                        }.into_any()
                    )
                    .build_managed(
                        cal_tokens_data,
                        columns,
                        Callback::new(|(a, b, col): (jumbie_shared::config::CalendarToken, jumbie_shared::config::CalendarToken, String)| {
                            match col.as_str() {
                                "name" => jumbie_shared::formatting::natural_cmp(&a.name, &b.name),
                                _ => std::cmp::Ordering::Equal,
                            }
                        }),
                        None::<Callback<jumbie_shared::config::CalendarToken>>,
                        None::<Callback<jumbie_shared::config::CalendarToken, bool>>,
                        Some(Callback::new({
                            let origin = origin.clone();
                            move |t: jumbie_shared::config::CalendarToken| {
                                let id = t.id.clone();
                                let token = t.token.clone();
                                let url = crate::utils::build_calendar_link_url(&origin, &t.token);
                                let state = RevealState::new();
                                let ts = trigger_save_cals_mobile.clone();
                                let copy_cb = copy_calendar_link_mobile.clone();
                                view! {
                                    <div class="card p-md flex flex-col gap-md">
                                        <div class="flex justify-between items-center">
                                            <div class="flex flex-col gap-xs min-w-0 overflow-hidden">
                                                <span class="font-bold">{t.name.clone()}</span>
                                                <RevealableCode content={url.clone()} state={state} class="overflow-wrap-anywhere" />
                                            </div>
                                            <button class="btn btn-danger self-end" on:click=on_delete(set_config, ts, id.clone(), |c: &mut Config, id: &str| c.auth.calendar_tokens.retain(|k| k.id != id))>
                                                <span class="icon"><TrashIcon /></span>
                                            </button>
                                        </div>
                                        <div class="flex gap-xs">
                                            <RevealButton state={state} />
                                            <button class="btn btn-secondary btn-calendar-link" title="Copy Calendar Link" on:click=move |_| {
                                                copy_cb.run(token.clone());
                                            }>
                                                <span class="icon"><CopyIcon /></span> "Copy"
                                            </button>
                                        </div>
                                    </div>
                                }.into_any()
                            }
                        })),
                    )
                }
            </div>
        }
    })
    .raw_section(view! {
        <ModalWrapper
            show=show_modal.0
            on_close=move |_| show_modal.1.set(false)
            title="Create API Key"
            size="modal-md"
            footer={
                (move || if generated_key.0.get().is_some() {
                     view! { <></> }.into_any()
                } else {
                    view! {
                        <div class="modal-footer flex items-center justify-end gap-md">
                            <button class="btn btn-primary"
                                disabled=move || new_key_scopes.0.get().is_empty()
                                on:click=move |_| handle_generate.run(())
                                type="button"
                            >
                                "Create"
                            </button>
                        </div>
                    }.into_any()
                }).into_any()
            }
        >
            {move || {
                let available_scopes_clone = available_scopes.clone();
                if let Some(key) = generated_key.0.get() {
                    view! {
                        <div class="alert alert-success mb-md">
                            <p><strong>"API Key Generated!"</strong></p>
                            <p>"Please copy this key now. You will not be able to see it again."</p>
                        </div>
                        <div class="flex gap-sm items-center mt-sm">
                            <label for="generatedApiKey" class="sr-only">"Generated API Key"</label>
                            <input type="text" id="generatedApiKey" class="form-input flex-1" value=key readonly=true />
                            <button class="btn btn-secondary btn-calendar-link" title="Copy Calendar Link" on:click=copy_key>
                                <span class="icon"><CopyIcon /></span> "Copy"
                            </button>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <FormGroup label="Key Name".to_string() label_for="newApiKeyName">
                            <TextInput
                                id="newApiKeyName".to_string()
                                value=new_key_name.0
                                set_value=move |v| new_key_name.1.set(v)
                                placeholder="Integration Name"
                                help_text="A descriptive name for this key, e.g., 'Home Assistant'".to_string()
                                required=true
                            />
                        </FormGroup>

                        <FormGroup label="Duration".to_string() label_for="newApiKeyDuration">
                            <Select
                                id="newApiKeyDuration".to_string()
                                help_text="How long before this key expires.".to_string()
                                value=new_key_duration.0
                                set_value=move |v| new_key_duration.1.set(v)
                                options=Signal::derive(move || {
                                    DURATION_OPTIONS.iter().map(|o| {
                                        SelectOption::from((o.value.to_string(), o.label.to_string()))
                                    }).collect()
                                })
                            />
                        </FormGroup>

                        <FormGroup label="Scopes".to_string() help_text="Select one or more permissions for this key.".to_string()>
                            <div class="mt-xs mb-md">
                                <ScopeTree
                                    scopes=available_scopes_clone
                                    selected_scopes=new_key_scopes.0.into()
                                    toggle_scope=Callback::new(toggle_scope)
                                    is_root=true
                                />
                            </div>
                        </FormGroup>
                    }.into_any()
                }
            }}
        </ModalWrapper>

        <CreateCalendarLinkModal
            show=show_cal_modal.0
            set_show=show_cal_modal.1
            on_success=Callback::new(move |_token: String| {
                spawn_local(async move {
                    if let Ok(new_cfg) = fetch_config().await {
                        set_config.set(Some(new_cfg));
                    }
                });
            })
        />
    })
    .build()
}
