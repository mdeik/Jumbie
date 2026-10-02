use crate::api::fetch_available_plugins;
use crate::components::common::icons::TrashIcon;
use crate::components::common::modal_wrapper::ModalWrapper;
use jumbie_shared::plugin::Capability;
use leptos::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub struct PluginDisplayItem {
    pub id: String,
    pub name: String,
    pub plugin_id: String, // e.g., "qbittorrent"
    pub enabled: bool,
}

#[component]
pub fn PluginGrid(
    items: Signal<Vec<PluginDisplayItem>>,
    capability_filter: Capability,
    on_add: Callback<String>,  // plugin_type
    on_edit: Callback<String>, // instance_id
) -> impl IntoView {
    let (show_add_modal, set_show_add_modal) = signal(false);

    let handle_add = move |plugin_type: String| {
        set_show_add_modal.set(false);
        on_add.run(plugin_type);
    };

    view! {
        <div class="plugin-grid">
            <For
                each=move || items.get()
                key=|item| format!("{}_{}_{}", item.id, item.enabled, item.name)
                children=move |item| {
                    let id = item.id.clone();
                    view! {
                        <PluginTile
                            item=item.clone()
                            on_click=Callback::new(move |_| on_edit.run(id.clone()))
                        />
                    }
                }
            />

            <div
                class="plugin-card add-new clickable unselectable"
                on:click=move |_| set_show_add_modal.set(true)
            >
                <div class="icon-add-large">"+"</div>
                <div class="text-add-new">"Add Client"</div>
            </div>

            <AddPluginModal
                show=show_add_modal
                set_show=set_show_add_modal
                capability=capability_filter.clone()
                on_select=Callback::new(handle_add)
            />
        </div>
    }
}

#[component]
pub fn PluginTile(item: PluginDisplayItem, on_click: Callback<()>) -> impl IntoView {
    view! {
        <div
            class="plugin-card clickable unselectable"
            on:click=move |_| on_click.run(())
        >
            <div class="card-header">
                <div class="plugin-type">
                    {item.plugin_id}
                </div>
                <h3 class="my-sm text-lg">{item.name}</h3>
            </div>

            <div class="card-footer flex items-center justify-between">
                <span class={if item.enabled { "status-badge pd-0 enabled text-success text-sm" } else { "status-badge pd-0 disabled text-danger text-sm" }}>
                    {if item.enabled { "Enabled" } else { "Disabled" }}
                </span>
            </div>
        </div>
    }
}

#[component]
pub fn AddPluginModal(
    show: ReadSignal<bool>,
    set_show: WriteSignal<bool>,
    capability: Capability,
    on_select: Callback<String>, // plugin_type
) -> impl IntoView {
    let available_plugins =
        LocalResource::new(
            move || async move { fetch_available_plugins().await.unwrap_or_default() },
        );

    let capability_stored = StoredValue::new_local(capability);
    let on_close = Callback::new(move |_| set_show.set(false));

    view! {
        <ModalWrapper
            show=show
            on_close=on_close
            title=Signal::derive(move || "Add New Plugin".to_string())
            size="modal-md"
        >
            <Suspense fallback=move || view! { <div class="p-md text-center"><crate::components::common::loading_spinner::LoadingSpinner /></div> }>
                {
                    move || {
                        let filter_cap = capability_stored.read_value();
                        available_plugins.get().map(move |plugins| {
                            let filtered: Vec<_> = plugins.into_iter()
                                .filter(|p| p.capabilities.contains(&filter_cap))
                                .collect();

                            if filtered.is_empty() {
                                view! { <div class="text-muted-color text-center p-xl">"No plugins available for this category."</div> }.into_any()
                            } else {
                                view! {
                                    <div class="plugin-list flex flex-col gap-sm">
                                        <For
                                            each=move || filtered.clone()
                                            key=|p| p.plugin_id.clone().unwrap_or_default()
                                            children=move |plugin| {
                                                // Use plugin_id (unique registry key like "downloader.qbittorrent")
                                                // instead of display_name — multiple plugins may share a display name.
                                                let p_id = plugin.plugin_id.clone().unwrap_or_default();
                                                let on_select = on_select;
                                                view! {
                                                    <div
                                                        class="status-card clickable"
                                                        on:click={
                                                            let p_id = p_id.clone();
                                                            move |_| on_select.run(p_id.clone())
                                                        }
                                                    >
                                                        <div class="font-bold mb-xs">{plugin.display_name}</div>
                                                        <div class="text-sm text-muted-color">{plugin.description}</div>
                                                        <div class="text-xs text-muted-color mt-xs">
                                                            {
                                                                // Built-in plugins ship with Jumbie
                                                                // and version with it, so their version
                                                                // is not meaningful to users — hide it.
                                                                let is_builtin = plugin.plugin_id.as_deref()
                                                                    .is_some_and(jumbie_shared::plugin::is_jumbie_plugin_id);
                                                                if plugin.author.is_empty() {
                                                                    if is_builtin { String::new() } else { format!("v{}", plugin.version) }
                                                                } else if is_builtin {
                                                                    format!("by {}", plugin.author)
                                                                } else {
                                                                    format!("by {}  ·  v{}", plugin.author, plugin.version)
                                                                }
                                                            }
                                                        </div>
                                                    </div>
                                                }
                                            }
                                        />
                                    </div>
                                }.into_any()
                            }
                        })
                }}
            </Suspense>
        </ModalWrapper>
    }
}

#[component]
pub fn PluginConfigWrapper(
    title: String,
    #[prop(into, optional)] subtitle: Signal<String>,
    on_save: Callback<()>,
    on_delete: Option<Callback<()>>,
    on_cancel: Callback<()>,
    on_test: Option<Callback<()>>,
    children: Children,
) -> impl IntoView {
    let on_close = on_cancel.clone();
    let footer = view! {
        <div class="modal-footer flex items-center justify-between bg-primary-block plugin-config-footer">
            <div class="footer-left">
                {move || {
                    if let Some(on_delete) = &on_delete {
                        let on_delete = on_delete.clone();
                        view! {
                            <button
                                class="btn btn-danger footer-delete-btn"
                                on:click=move |_| on_delete.run(())
                            ><span class="delete-label">"Delete"</span><span class="delete-icon"><TrashIcon /></span></button>
                        }.into_any()
                    } else {
                        view! { <></> }.into_any()
                    }
                }}
            </div>
            <div class="footer-right">
                {move || {
                    if let Some(on_test) = &on_test {
                        let on_test = on_test.clone();
                        view! {
                            <button
                                class="btn bg-info-white"
                                on:click=move |_| on_test.run(())
                            >"Test"</button>
                        }.into_any()
                    } else {
                        view! { <></> }.into_any()
                    }
                }}
                <button
                    class="btn btn-primary"
                    on:click=move |_| on_save.run(())
                >"Save"</button>
            </div>
        </div>
    }
    .into_any();
    let (show, _) = signal(true);

    view! {
        <ModalWrapper
            show=show
            on_close=on_close
            title=Signal::derive(move || title.clone())
            subtitle=subtitle
            size="modal-xl"
            footer=footer
        >
            {children()}
        </ModalWrapper>
    }
}
