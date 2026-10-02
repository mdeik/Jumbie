use crate::api::{fetch_health, fetch_plugin_status};
use crate::components::common::SettingsPage;
use jumbie_shared::types::PluginStatusEntry;
use leptos::prelude::*;

#[component]
pub fn SystemStatus() -> impl IntoView {
    let (status, set_status) = signal("—".to_string());
    let (db_integrity, set_db_integrity) = signal("—".to_string());
    let (dir_permissions, set_dir_permissions) =
        signal(Vec::<crate::components::common::structs::PermissionCheck>::new());
    let (dirs_loaded, set_dirs_loaded) = signal(false);
    let (ffmpeg_installed, set_ffmpeg_installed) = signal(None::<bool>);
    let (ffmpeg_version, set_ffmpeg_version) = signal(None::<Option<String>>);
    let (plugin_statuses, set_plugin_statuses) = signal(None::<Vec<PluginStatusEntry>>);

    crate::utils::use_api_cache(
        "fetch_health".to_string(),
        || fetch_health(),
        move |h| {
            let _ = set_status.try_update(|s| *s = h.status);
            let _ = set_db_integrity.try_update(|s| *s = h.db_integrity_status);
            let _ = set_dir_permissions.try_update(|s| *s = h.dir_permissions);
            let _ = set_ffmpeg_installed.try_update(|s| *s = Some(h.ffmpeg_installed));
            let _ = set_ffmpeg_version.try_update(|s| *s = Some(h.ffmpeg_version));
            let _ = set_dirs_loaded.try_update(|s| *s = true);
        },
    );

    crate::utils::use_api_cache(
        "fetch_plugin_status".to_string(),
        || fetch_plugin_status(),
        move |entries| {
            let _ = set_plugin_statuses.try_update(|s| *s = Some(entries));
        },
    );

    view! {
        <SettingsPage id="status">
            <div class="sys-flex-col-lg">
                <div>
                    <h3 class="weight-medium text-lg m-0 mb-sm">"Core Metrics"</h3>
                    <div class="status-grid">
                        <div class="status-card">
                            <div class="label">"Status"</div>
                            <div class="value">{move || status.get()}</div>
                        </div>
                        <div class="status-card">
                            <div class="label">"Database Integrity"</div>
                            <div class="value" style=move || {
                                match db_integrity.get().as_str() {
                                    "Healthy" => "",
                                    "—"       => "",
                                    _         => "color: var(--accent-danger)",
                                }
                            }>
                                {move || db_integrity.get()}
                            </div>
                        </div>
                        <div class="status-card" title=move || {
                            match ffmpeg_version.get() {
                                Some(Some(v)) => v,
                                _ => String::new(),
                            }
                        }>
                            <div class="label">"FFmpeg Installed"</div>
                            <div class="value" style=move || {
                                match ffmpeg_installed.get() {
                                    Some(true) => "",
                                    Some(false) => "color: var(--accent-danger)",
                                    None => "",
                                }
                            }>
                                {move || match ffmpeg_installed.get() {
                                    Some(true) => "Ready",
                                    Some(false) => "Not Installed",
                                    None => "—",
                                }}
                            </div>
                        </div>
                    </div>
                </div>

                <div>
                    <h3 class="weight-medium text-lg m-0 mb-sm">"Directory Permissions"</h3>
                    <div class="status-grid">
                        {move || {
                            if !dirs_loaded.get() {
                                view! {
                                    <div class="status-card">
                                        <div class="label">"—"</div>
                                        <div class="value">
                                            <span>"Read —"</span>
                                            <span>" | "</span>
                                            <span>"Write —"</span>
                                        </div>
                                    </div>
                                }.into_any()
                            } else {
                                dir_permissions.get().into_iter().map(|p| {
                                    let read_color = if p.can_read { "" } else { "var(--accent-danger)" };
                                    let write_color = if p.can_write { "" } else { "var(--accent-danger)" };
                                    view! {
                                        <div class="status-card">
                                            <div class="label truncate" title=p.path.clone()>
                                                {p.path.clone()}
                                            </div>
                                            <div class="value">
                                                <span style=format!("color: {};", read_color)>
                                                    {if p.can_read { "Read OK" } else { "Read FAIL" }}
                                                </span>
                                                <span>" | "</span>
                                                <span style=format!("color: {};", write_color)>
                                                    {if p.can_write { "Write OK" } else { "Write FAIL" }}
                                                </span>
                                            </div>
                                        </div>
                                    }
                                }).collect_view().into_any()
                            }
                        }}
                    </div>
                </div>

                <div>
                    <h3 class="weight-medium text-lg m-0 mb-sm">"Plugins"</h3>
                    // Entries arrive pre-sorted by the backend (category, then
                    // name) so this view renders them as received.
                    <div class="status-grid">
                        {move || {
                            match plugin_statuses.get() {
                                None => {
                                    view! {
                                        <div class="status-card">
                                            <div class="label">"—"</div>
                                            <div class="value">"—"</div>
                                        </div>
                                    }.into_any()
                                }
                                Some(entries) if entries.is_empty() => {
                                    view! {
                                        <div class="status-card">
                                            <div class="label">"No Plugins"</div>
                                            <div class="value text-muted-color">
                                                "No plugins configured"
                                            </div>
                                        </div>
                                    }.into_any()
                                }
                                Some(entries) => {
                                    entries.into_iter().map(|entry| {
                                        let msg_str = entry.message.clone().unwrap_or_default();

                                        let (value_color, value_text) = if entry.ok {
                                            ("", "Connected".to_string())
                                        } else {
                                            ("color: var(--accent-danger)", "Error".to_string())
                                        };

                                        // Raw category id; `.plugin-category-badge` applies
                                        // `text-transform: uppercase` for display.
                                        view! {
                                            <div class="status-card" title=msg_str>
                                                <div class="label">
                                                    {entry.name.clone()}
                                                    <span class="plugin-category-badge">
                                                        {entry.category}
                                                    </span>
                                                </div>
                                                <div class="value" style=value_color>
                                                    {value_text}
                                                </div>
                                            </div>
                                        }
                                    }).collect_view().into_any()
                                }
                            }
                        }}
                    </div>
                </div>
            </div>
        </SettingsPage>
    }
}
