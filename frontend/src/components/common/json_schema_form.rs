use crate::components::common::form_fields::format_field::FormatFieldBuilder;
use crate::components::common::form_fields::tooltip::TooltipBuilder;
use crate::components::common::form_fields::{
    CheckboxInput, FormGroup, NumberInput, NumberInputMode, Select, SelectOption, SelectOptionItem,
    TextInput, TimeInput, TimeUnit,
};
use jumbie_shared::variables::TemplateContext;
use leptos::prelude::*;

use serde_json::{Map, Value};

/// Recursively fill missing fields in `value` from schema `"default"` values
/// (the JSON schema is the SSoT for defaults). Ensures the form always produces
/// a complete config regardless of how sparse the stored config was.
fn fill_schema_defaults(value: &Value, schema: &Value) -> Value {
    let mut merged: Map<String, Value> = value.as_object().cloned().unwrap_or_default();
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        for (prop_name, prop_schema) in props {
            if let Some(existing) = merged.get(prop_name) {
                // Key exists — recurse if it's an object (may have missing children).
                if existing.is_object() {
                    let filled = fill_schema_defaults(existing, prop_schema);
                    if filled != *existing {
                        merged.insert(prop_name.clone(), filled);
                    }
                }
            } else {
                // Key missing — use schema default, or recurse for child objects.
                if let Some(default) = prop_schema.get("default") {
                    merged.insert(prop_name.clone(), default.clone());
                } else if prop_schema.get("type").and_then(|t| t.as_str()) == Some("object") {
                    let filled = fill_schema_defaults(&Value::Null, prop_schema);
                    merged.insert(prop_name.clone(), filled);
                }
            }
        }
    }
    Value::Object(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn fill_top_level_defaults() {
        let schema = json!({
            "properties": {
                "name": {"type": "string", "default": "Discord"},
                "enabled": {"type": "boolean", "default": true}
            }
        });
        let value = json!({});
        let result = fill_schema_defaults(&value, &schema);
        assert_eq!(result, json!({"name": "Discord", "enabled": true}));
    }

    #[wasm_bindgen_test]
    fn fill_preserves_existing_values() {
        let schema = json!({
            "properties": {
                "name": {"type": "string", "default": "Discord"},
                "enabled": {"type": "boolean", "default": true},
                "webhook_url": {"type": "string"}
            }
        });
        let value = json!({"name": "My Bot"});
        let result = fill_schema_defaults(&value, &schema);
        assert_eq!(result["name"], "My Bot");
        assert_eq!(result["enabled"], true);
        // webhook_url has no default and isn't present — should not appear
        assert!(result.get("webhook_url").is_none());
    }

    #[wasm_bindgen_test]
    fn fill_nested_object_defaults() {
        let schema = json!({
            "properties": {
                "events": {
                    "type": "object",
                    "properties": {
                        "Download Started": {
                            "type": "object",
                            "properties": {
                                "enabled": {"type": "boolean", "default": true},
                                "template": {"type": "string", "default": "started: ${series}"}
                            }
                        },
                        "Error": {
                            "type": "object",
                            "properties": {
                                "enabled": {"type": "boolean", "default": false},
                                "template": {"type": "string", "default": "error: ${error_message}"}
                            }
                        }
                    }
                }
            }
        });
        // Config has no events at all
        let value = json!({"name": "Discord"});
        let result = fill_schema_defaults(&value, &schema);
        assert_eq!(result["name"], "Discord");
        let events = result.get("events").unwrap().as_object().unwrap();
        assert_eq!(
            events["Download Started"]["enabled"], true,
            "nested default should be filled"
        );
        assert_eq!(events["Download Started"]["template"], "started: ${series}");
        assert_eq!(events["Error"]["enabled"], false);
    }

    #[wasm_bindgen_test]
    fn fill_partial_nested_object() {
        let schema = json!({
            "properties": {
                "events": {
                    "type": "object",
                    "properties": {
                        "Download Started": {
                            "type": "object",
                            "properties": {
                                "enabled": {"type": "boolean", "default": true},
                                "template": {"type": "string", "default": "started: ${series}"}
                            }
                        },
                        "Error": {
                            "type": "object",
                            "properties": {
                                "enabled": {"type": "boolean", "default": false},
                                "template": {"type": "string", "default": "error: ${error_message}"}
                            }
                        }
                    }
                }
            }
        });
        // Config has events but only Error is present
        let value = json!({
            "name": "Discord",
            "events": {
                "Error": {"enabled": true}
            }
        });
        let result = fill_schema_defaults(&value, &schema);
        let events = result.get("events").unwrap().as_object().unwrap();
        // Error's explicit value preserved
        assert_eq!(events["Error"]["enabled"], true);
        // Missing "Download Started" filled with full defaults
        assert_eq!(
            events["Download Started"]["enabled"], true,
            "missing nested object should be filled from defaults"
        );
        assert_eq!(events["Download Started"]["template"], "started: ${series}");
    }

    #[wasm_bindgen_test]
    fn fill_null_input_creates_full_object() {
        let schema = json!({
            "properties": {
                "name": {"type": "string", "default": "Discord"},
                "enabled": {"type": "boolean", "default": true}
            }
        });
        let result = fill_schema_defaults(&Value::Null, &schema);
        assert_eq!(result, json!({"name": "Discord", "enabled": true}));
    }

    #[wasm_bindgen_test]
    fn fill_skips_fields_without_defaults() {
        let schema = json!({
            "properties": {
                "webhook_url": {"type": "string"},
                "api_key": {"type": "string"}
            }
        });
        let result = fill_schema_defaults(&json!({}), &schema);
        assert!(
            result.as_object().unwrap().is_empty(),
            "fields without defaults and without object children should not appear"
        );
    }
}

/// A Leptos component that dynamically renders form fields from a JSON Schema.
/// Fetches the schema via `GET /api/plugins/:name/schema` on first use, then
/// caches it so that consecutive edits of the same plugin type are instant.
#[component]
pub fn DynamicPluginForm(
    /// The plugin id (e.g. "qbitturrent") — must be the short name, not the
    /// full plugin_id with category prefix (caller normalises before passing).
    #[prop(into)]
    plugin_id: String,
    #[prop(into)] value: Signal<Value>,
    #[prop(into)] set_value: Callback<Value>,
) -> impl IntoView {
    let cache_key = format!("fetch_plugin_schema_{}", plugin_id);
    let cache_hit = crate::utils::read_cache::<Value>(&cache_key);

    // Seed from cache synchronously — the preload system fetches schemas
    // for all available plugins in Wave 1, so in practice this is instant.
    let (schema, set_schema) = signal(cache_hit.clone());

    // Track loading state separately: show spinner while first fetch is in flight.
    let (is_loading, set_is_loading) = signal(cache_hit.is_none());

    // Fetch from network if cache is stale or missing, with full dedup against
    // the preload system (which fetches schemas for all plugins in Wave 1).
    crate::utils::spawn_cached_with(
        cache_key.clone(),
        move || {
            let pid = plugin_id.clone();
            async move { crate::api::fetch_plugin_schema(&pid).await }
        },
        move |s| {
            set_schema.set(Some(s));
            set_is_loading.set(false);
        },
    );

    // Clear loading state when the pending request resolves (even on error),
    // since spawn_cached_with always calls resolve_pending in its finally block.
    if crate::utils::is_pending(&cache_key) {
        crate::utils::on_pending_complete(
            &cache_key,
            Box::new(move || {
                set_is_loading.set(false);
            }),
        );
    }

    // When the schema loads, recursively fill missing fields from schema
    // defaults.  This ensures that a new plugin created with an empty config
    // `{}` still gets `enabled: true`, a default name, and all nested event
    // objects populated — without the user touching every field first.
    Effect::new(move |_| {
        if let Some(ref schema) = schema.get() {
            let current = value.get_untracked();
            let merged = fill_schema_defaults(&current, schema);
            if merged != current {
                set_value.run(merged);
            }
        }
    });

    view! {
        {move || {
            if is_loading.get() {
                view! {
                    <div class="loading-spinner-sm">
                        <crate::components::common::loading_spinner::LoadingSpinner />
                    </div>
                }.into_any()
            } else if let Some(s) = schema.get() {
                view! {
                    <JsonSchemaForm schema=Signal::derive(move || s.clone()) value=value set_value=set_value />
                }.into_any()
            } else {
                view! {
                    <div class="text-muted text-sm p-md">
                        "No schema available for this plugin. It may not be installed or reachable."
                    </div>
                }.into_any()
            }
        }}
    }
}

#[component]
pub fn JsonSchemaForm(
    #[prop(into)] schema: Signal<Value>,
    #[prop(into)] value: Signal<Value>,
    #[prop(into)] set_value: Callback<Value>,
) -> impl IntoView {
    view! {
        <div class="json-schema-form form-container">
            {move || {
                let s = schema.get();

                let mut all_props: Vec<(String, Value, i64)> = s.get("properties")
                    .and_then(|p| p.as_object())
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, v)| {
                        let default_order = match k.as_str() {
                            "name" => -20,
                            "enabled" => -10,
                            _ => 1000,
                        };
                        let order = v.get("order").and_then(|o| o.as_i64()).unwrap_or(default_order);
                        (k, v, order)
                    })
                    .collect();

                all_props.sort_by_key(|(_, _, order)| *order);

                // Separate child fields (those with a "parent" property) from standalone fields.
                // Child fields will be rendered nested under their parent checkbox.
                let mut children_map: std::collections::HashMap<String, Vec<(String, Value)>> =
                    std::collections::HashMap::new();
                let mut standalone: Vec<(String, Value, i64)> = Vec::new();
                for (name, schema, order) in all_props {
                    let parent = schema.get("parent").and_then(|p| p.as_str()).map(|s| s.to_string());
                    if let Some(parent_name) = parent {
                        children_map.entry(parent_name).or_default().push((name, schema));
                    } else {
                        standalone.push((name, schema, order));
                    }
                }

                let value_ref = value.clone();
                let set_value_ref = set_value.clone();
                let children_map_ref = children_map.clone();
                let derived_children = move || -> std::collections::HashMap<String, Vec<(String, Value)>> {
                    children_map_ref.clone()
                };

                standalone.into_iter().map(move |(prop_name, prop_schema, _)| {
                    let prop_name_clone = prop_name.clone();
                    let schema_default = prop_schema.get("default").cloned();
                    let field_val_sig = Signal::derive(move || {
                        value.get()
                            .get(&prop_name_clone)
                            .cloned()
                            .or_else(|| schema_default.clone())
                            .unwrap_or(Value::Null)
                    });

                    let title = prop_schema.get("title").and_then(|t| t.as_str()).unwrap_or(&prop_name).to_string();
                    let field_type = prop_schema.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                    let description = prop_schema.get("description").and_then(|t| t.as_str()).unwrap_or("").to_string();
                    let placeholder = prop_schema.get("placeholder").and_then(|t| t.as_str()).unwrap_or("").to_string();
                    let examples_placeholder = prop_schema.get("examples").and_then(|e| e.as_array()).and_then(|a| a.first()).and_then(|f| f.as_str()).unwrap_or("").to_string();
                    let final_placeholder = if !placeholder.is_empty() { placeholder } else { examples_placeholder };
                    let is_password = prop_name.to_lowercase().contains("password")
                        || prop_name.to_lowercase().contains("token")
                        || prop_name.to_lowercase().contains("secret")
                        || (prop_name.to_lowercase().contains("key") && !prop_name.to_lowercase().contains("api_key_label"));

                    let prop_name_for_cb = prop_name.clone();
                    let set_v = set_value_ref.clone();

                    // Check dependsOn: if this field has a dependency that isn't met, skip it.
                    if let Some(depends) = prop_schema.get("dependsOn") {
                        let dep_field = depends.get("field").and_then(|f| f.as_str()).unwrap_or("");
                        let dep_expected = depends.get("value");
                        let show = match (dep_expected, value.get().as_object()) {
                            (Some(expected), Some(obj)) => obj.get(dep_field) == Some(expected),
                            _ => true,
                        };
                        if !show {
                            return view! {}.into_any();
                        }
                    }

                    let on_change = move |new_val: Value| {
                        let mut updated = value.get_untracked();
                        if !updated.is_object() {
                            updated = Value::Object(serde_json::Map::new());
                        }
                        if let Some(obj) = updated.as_object_mut() {
                            obj.insert(prop_name_for_cb.clone(), new_val);
                        }
                        set_v.run(updated);
                    };

                    match field_type {
                        "boolean" => {
                            let children = derived_children()
                                .get(&prop_name)
                                .cloned()
                                .unwrap_or_default();

                            if children.is_empty() {
                                view! {
                                    <div class="form-group mb-md checkbox-row">
                                        <CheckboxInput
                                            label=title.clone()
                                            checked=Signal::derive(move || field_val_sig.get().as_bool().unwrap_or(false))
                                            set_checked=Callback::new({
                                                let on_change = on_change.clone();
                                                move |c| on_change(Value::Bool(c))
                                            })
                                            class="checkbox-group".to_string()
                                            help_text=description.clone()
                                        />
                                    </div>
                                }.into_any()
                            } else {
                                // Parent checkbox with nested child checkboxes.
                                // When parent is unchecked, children are disabled.
                                let parent_checked = Signal::derive(move || {
                                    value.get().get(&prop_name).and_then(|v| v.as_bool()).unwrap_or(false)
                                });
                                let children_signal = Signal::derive(move || children.clone());

                                let value_for_kids = value_ref.clone();
                                let set_value_for_kids = set_value_ref.clone();

                                view! {
                                    <div class="form-group mb-md checkbox-row">
                                        <CheckboxInput
                                            label=title.clone()
                                            checked=Signal::derive(move || field_val_sig.get().as_bool().unwrap_or(false))
                                            set_checked=Callback::new({
                                                let on_change = on_change.clone();
                                                move |c| on_change(Value::Bool(c))
                                            })
                                            class="checkbox-group mb-0".to_string()
                                        >
                                        </CheckboxInput>
                                            <div class="nested-checkboxes">
                                                {move || {
                                                    let parent_val = parent_checked.get();
                                                    let kids = children_signal.get();
                                                    kids.into_iter().map(move |(child_name, child_schema)| {
                                                        let child_name2 = child_name.clone();
                                                        let child_title = child_schema.get("title")
                                                            .and_then(|t| t.as_str())
                                                            .unwrap_or(&child_name)
                                                            .to_string();
                                                        let child_desc = child_schema.get("description")
                                                            .and_then(|t| t.as_str())
                                                            .unwrap_or("")
                                                            .to_string();
                                                        let child_default = child_schema.get("default")
                                                            .and_then(|d| d.as_bool())
                                                            .unwrap_or(true);

                                                        let child_name_for_val = child_name2.clone();
                                                        let child_val = Signal::derive(move || {
                                                            value_for_kids.get()
                                                                .get(&child_name_for_val)
                                                                .and_then(|v| v.as_bool())
                                                                .unwrap_or(child_default)
                                                        });

                                                        let child_set = {
                                                            let val = value_for_kids.clone();
                                                            let set_v = set_value_for_kids.clone();
                                                            let child_name3 = child_name2.clone();
                                                            move |checked: bool| {
                                                                let mut updated = val.get_untracked();
                                                                if !updated.is_object() {
                                                                    updated = Value::Object(serde_json::Map::new());
                                                                }
                                                                if let Some(obj) = updated.as_object_mut() {
                                                                    obj.insert(child_name3.clone(), Value::Bool(checked));
                                                                }
                                                                set_v.run(updated);
                                                            }
                                                        };

                                                        view! {
                                                            <div class="nested-checkbox-row">
                                                                <CheckboxInput
                                                                    label=child_title.clone()
                                                                    checked=child_val
                                                                    set_checked=Callback::new(child_set)
                                                                    class="checkbox-group-sub".to_string()
                                                                    disabled=Signal::derive(move || !parent_val)
                                                                    help_text=child_desc.clone()
                                                                />
                                                            </div>
                                                        }
                                                    }).collect_view()
                                                }}
                                            </div>

                                    </div>
                                }.into_any()
                            }
                        },
                        "integer" | "decimal" | "number" => {
                            // Check for duration format — render a TimeInput instead of plain NumberInput.
                            if prop_schema.get("format").and_then(|f| f.as_str()) == Some("duration") {
                                let unit = match prop_schema
                                    .get("x-time-unit")
                                    .and_then(|u| u.as_str())
                                {
                                    Some("seconds") => TimeUnit::Seconds,
                                    Some("minutes") => TimeUnit::Minutes,
                                    Some("hours") => TimeUnit::Hours,
                                    Some("days") => TimeUnit::Days,
                                    _ => TimeUnit::Minutes,
                                };
                                let min_val = prop_schema.get("minimum").and_then(|m| m.as_u64()).unwrap_or(0);
                                let max_val = prop_schema.get("maximum").and_then(|m| m.as_u64()).unwrap_or(u64::MAX);

                                let display_val = Signal::derive(move || {
                                    let v = field_val_sig.get();
                                    if v.is_null() {
                                        String::new()
                                    } else {
                                        v.as_f64().map(|n| n.to_string()).unwrap_or_default()
                                    }
                                });

                                let on_change = on_change.clone();
                                return view! {
                                    <TimeInput
                                        id=prop_name.clone()
                                        label=title.clone()
                                        value=display_val
                                        unit=unit
                                        min_value=min_val
                                        max_value=max_val
                                        set_value=Callback::new(move |val: String| {
                                            if val.is_empty() {
                                                on_change(Value::Null);
                                            } else if let Ok(num) = val.parse::<f64>() {
                                                on_change(serde_json::json!(num));
                                            }
                                        })
                                        placeholder=final_placeholder.clone()
                                        help_text=description.clone()
                                    />
                                }.into_any();
                            }

                            let mode = if field_type == "integer" {
                                NumberInputMode::SignedInteger
                            } else {
                                NumberInputMode::SignedDecimal
                            };
                            let display_val = Signal::derive(move || {
                                let v = field_val_sig.get();
                                if v.is_null() {
                                    String::new()
                                } else {
                                    v.as_f64().map(|n| n.to_string()).unwrap_or_default()
                                }
                            });
                            view! {
                                <NumberInput
                                    id=prop_name.clone()
                                    label=title.clone()
                                    value=display_val
                                    mode=mode
                                    set_value={
                                        let on_change = on_change.clone();
                                        move |val: String| {
                                            if val.is_empty() {
                                                on_change(Value::Null);
                                            } else if let Ok(num) = val.parse::<f64>() {
                                                on_change(serde_json::json!(num));
                                            }
                                        }
                                    }
                                    placeholder=final_placeholder.clone()
                                    help_text=description.clone()
                                />
                            }.into_any()
                        },
                        "object" => {
                            view! {
                                <div class="nested-object mb-md">
                                    <legend class="font-bold text-base mb-sm">{title.clone()}</legend>
                                    {if !description.is_empty() {
                                        view! { <p class="text-sm text-muted-color mb-sm">{description.clone()}</p> }.into_any()
                                    } else {
                                        view! {}.into_any()
                                    }}
                                    <NestedObjectForm
                                        schema=prop_schema.clone()
                                        current_value=field_val_sig
                                        on_change=Callback::new(on_change.clone())
                                    />
                                </div>
                            }.into_any()
                        },
                        _ => {
                            let has_enum = prop_schema.get("enum").and_then(|e| e.as_array()).is_some();
                            let has_x_enum = prop_schema.get("x-enum-options").and_then(|o| o.as_array()).is_some();
                            let template_ctx = prop_schema.get("x-template-context").and_then(|t| t.as_str());

                            if has_enum || has_x_enum {
                                let enum_names = prop_schema.get("enumNames").and_then(|e| e.as_array());
                                let options: Vec<SelectOption> = if let Some(x_options) = prop_schema.get("x-enum-options").and_then(|o| o.as_array()) {
                                    x_options.iter().filter_map(|opt| {
                                        if let Some(obj) = opt.as_object() {
                                            if let (Some(val), Some(label)) = (obj.get("value").and_then(|v| v.as_str()), obj.get("label").and_then(|l| l.as_str())) {
                                                Some(SelectOption::Single(SelectOptionItem { value: val.to_string(), label: label.to_string() }))
                                            } else if let (Some(label), Some(items)) = (obj.get("group").and_then(|g| g.as_str()), obj.get("items").and_then(|i| i.as_array())) {
                                                let group_items = items.iter().filter_map(|i| {
                                                    let i_obj = i.as_object()?;
                                                    let v = i_obj.get("value")?.as_str()?;
                                                    let l = i_obj.get("label")?.as_str()?;
                                                    Some(SelectOptionItem { value: v.to_string(), label: l.to_string() })
                                                }).collect();
                                                Some(SelectOption::Group { label: label.to_string(), items: group_items })
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        }
                                    }).collect()
                                } else if let Some(enum_vals) = prop_schema.get("enum").and_then(|e| e.as_array()) {
                                    enum_vals.iter()
                                        .enumerate()
                                        .filter_map(|(i, e)| e.as_str().map(|code| {
                                            let label = enum_names
                                                .and_then(|names| names.get(i))
                                                .and_then(|n| n.as_str())
                                                .unwrap_or(code)
                                                .to_string();
                                            SelectOption::Single(SelectOptionItem { value: code.to_string(), label })
                                        }))
                                        .collect()
                                } else {
                                    vec![]
                                };
                                let opt_sig = Signal::derive(move || options.clone());

                                view! {
                                    <FormGroup label=title.clone()>
                                        <Select
                                            id=prop_name.clone()
                                            value=Signal::derive(move || field_val_sig.get().as_str().unwrap_or("").to_string())
                                            set_value={
                                                let on_change = on_change.clone();
                                                move |val: String| on_change(Value::String(val))
                                            }
                                            options=opt_sig
                                            help_text=description.clone()
                                        />
                                    </FormGroup>
                                }.into_any()
                            } else if let Some(ctx_str) = template_ctx {
                                let ctx = match ctx_str {
                                    "Notifier" => TemplateContext::Notifier,
                                    "NotifierDownload" => TemplateContext::NotifierDownload,
                                    "NotifierError" => TemplateContext::NotifierError,
                                    "NotifierRenameQueue" => TemplateContext::NotifierRenameQueue,
                                    "SeasonFolder" => TemplateContext::SeasonFolder,
                                    "EpisodeFile" => TemplateContext::EpisodeFile,
                                    "SeriesPath" => TemplateContext::SeriesPath,
                                    _ => TemplateContext::Notifier,
                                };
                                view! {
                                    <FormatFieldBuilder
                                        id=prop_name.clone()
                                        label=title.clone()
                                        value=Signal::derive(move || field_val_sig.get().as_str().unwrap_or("").to_string())
                                        on_change=Callback::new(move |val: String| on_change(Value::String(val)))
                                        tooltip=TooltipBuilder::new().with_context(ctx).with_formatting()
                                        placeholder=Signal::derive(move || final_placeholder.clone())
                                        error=MaybeProp::default()
                                    />
                                }.into_any()
                            } else {
                                let input_type = if is_password { "password" } else { "text" };
                                view! {
                                    <FormGroup label=title.clone()>
                                        <TextInput
                                            value=Signal::derive(move || field_val_sig.get().as_str().unwrap_or("").to_string())
                                            set_value={
                                                let on_change = on_change.clone();
                                                move |val: String| on_change(Value::String(val))
                                            }
                                            type_=input_type.to_string()
                                            placeholder=final_placeholder.clone()
                                            show_toggle=is_password
                                            help_text=description.clone()
                                        />
                                    </FormGroup>
                                }.into_any()
                            }
                        }
                    }
                }).collect_view()
            }}
        </div>
    }
}

/// Helper component that renders a nested sub-object's properties using JsonSchemaForm.
/// Kept separate so its `view!` macro doesn't clash with closure captures from the
/// parent `JsonSchemaForm`'s property iteration.
#[component]
fn NestedObjectForm(
    /// The JSON Schema for this sub-object (must have "properties")
    schema: Value,
    current_value: Signal<Value>,
    on_change: Callback<Value>,
) -> impl IntoView {
    view! {
        <div class="nested-fields">
            <JsonSchemaForm
                schema=Signal::derive(move || schema.clone())
                value=current_value
                set_value=on_change
            />
        </div>
    }
}
