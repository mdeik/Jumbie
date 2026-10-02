use crate::components::common::SettingsBuilder;
use crate::components::common::plugin_settings::PluginSettings;
use jumbie_shared::plugin::Capability;
use leptos::prelude::*;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Test handler for plugin configs; the backend's returned message is shown in a success toast.
pub fn create_plugin_test_handler(
    category: &'static str,
) -> Arc<dyn Fn(String, Value) + Send + Sync> {
    Arc::new(move |p_type: String, current_val: Value| {
        crate::utils::spawn_api_toast(
            crate::api::test_plugin_config(category.to_string(), p_type.clone(), current_val),
            None,
            {
                move |msg: String| {
                    crate::components::common::toast::show_success(&msg);
                }
            },
        );
    })
}

/// Returns `plugin_id`'s serialized default when the type matches, else an empty object.
pub fn create_default_config_loader<T>(
    plugin_id: &'static str,
) -> Arc<dyn Fn(String) -> Value + Send + Sync>
where
    T: Default + Serialize,
{
    Arc::new(move |plugin_type: String| -> Value {
        if plugin_type == plugin_id {
            serde_json::to_value(T::default()).unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        }
    })
}

/// Builds a plugin settings section view for any capability type; shared by
/// `MetadataSettings`, `SourcesSettings`, `DownloadClientSettings`, and
/// `NotifierSettings`.
pub fn plugin_section(
    settings_id: &'static str,
    capability: Capability,
    config_section: &'static str,
    header: &'static str,
    description: &'static str,
    test_handler_category: &'static str,
) -> impl IntoView {
    let default_config: std::sync::Arc<dyn Fn(String) -> Value + Send + Sync> =
        std::sync::Arc::new(|_plugin_type: String| serde_json::json!({}));
    let editor = PluginEditorBuilder::new().build();
    let test_handler = create_plugin_test_handler(test_handler_category);

    SettingsBuilder::new(settings_id)
        .raw_section(view! {
            <PluginSettings
                capability=capability
                config_section=config_section
                header=header.to_string()
                description=description.to_string()
                default_config=default_config
                test_handler=test_handler
                editor=editor
            />
        })
        .build()
}

type PluginRenderer = Arc<dyn Fn(Value, Callback<Value>) -> AnyView + Send + Sync>;

/// A builder for creating a standardized plugin editor closure.
pub struct PluginEditorBuilder {
    renderers: HashMap<String, PluginRenderer>,
}

impl Default for PluginEditorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginEditorBuilder {
    pub fn new() -> Self {
        Self {
            renderers: HashMap::new(),
        }
    }

    /// Registers a specialized form renderer for a specific plugin type.
    pub fn add<T, F, V>(mut self, plugin_type: &str, form_render: F) -> Self
    where
        T: serde::de::DeserializeOwned
            + jumbie_shared::plugin::PluginConfig
            + Clone
            + Send
            + Sync
            + 'static,
        F: Fn(ReadSignal<T>, WriteSignal<T>) -> V + Send + Sync + 'static,
        V: IntoView + 'static,
    {
        self.renderers.insert(
            plugin_type.to_string(),
            Arc::new(move |current_val, update_draft| {
                let initial: T = serde_json::from_value(current_val)
                    .unwrap_or_else(|_| <T as jumbie_shared::plugin::PluginConfig>::default());
                let (settings, set_settings) = signal(initial);
                crate::hooks::settings::use_serialization_effect(settings, update_draft);
                form_render(settings, set_settings).into_any()
            }),
        );
        self
    }

    /// Builds the final editor closure used by PluginSettings. Receives the
    /// short plugin type name ("qbittorrent"); PluginSettings normalises the
    /// full plugin_id before passing it here.
    pub fn build(self) -> Arc<dyn Fn(String, Value, Callback<Value>) -> AnyView + Send + Sync> {
        let renderers = self.renderers;
        Arc::new(move |p_type, current_val, update_draft| {
            if let Some(renderer) = renderers.get(&p_type) {
                renderer(current_val, update_draft)
            } else {
                view! {
                    <crate::components::common::plugin_form::DynamicPluginFormWrapper
                        plugin_type=p_type
                        initial_value=current_val
                        update_draft=update_draft
                    />
                }
                .into_any()
            }
        })
    }
}
