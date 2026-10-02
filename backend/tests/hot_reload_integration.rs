mod common;

use async_trait::async_trait;
use common::TestApp;
use jumbie::plugins::{PluginInstance, RuntimePluginState};
use std::collections::HashMap;
use std::sync::Arc;

struct MockExternalPlugin {
    id: String,
}

#[async_trait]
impl PluginInstance for MockExternalPlugin {
    fn priority(&self) -> i32 {
        0
    }
    fn instance_id(&self) -> &str {
        &self.id
    }

    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        plugin_sdk::traits::PluginTypeInfo {
            display_name: self.id.clone(),
            version: "1.0.0".to_string(),
            author: "external".to_string(),
            description: "Mock external plugin".to_string(),
            capabilities: vec![],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: true,
        }
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }
    async fn call(
        &self,
        _method: &str,
        _params: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!("ok"))
    }
}

#[tokio::test]
async fn test_plugin_hot_reload_integration() {
    let (router, state, _tmp) = common::setup_test_app().await;

    let mut tvdb_cfg = serde_json::Map::new();
    tvdb_cfg.insert("enabled".to_string(), serde_json::json!(true));
    tvdb_cfg.insert("api_key".to_string(), serde_json::json!("test_key_1"));
    tvdb_cfg.insert("name".to_string(), serde_json::json!("My TVDB"));

    let mut instances = HashMap::new();
    instances.insert(
        "tvdb_instance".to_string(),
        serde_json::Value::Object(tvdb_cfg),
    );

    let mut section = HashMap::<String, HashMap<String, serde_json::Value>>::new();
    section.insert("jumbie.tvdb".to_string(), instances);

    // PUT through the API must trigger a config hot-reload.
    let _: jumbie_shared::types::SavePluginsSectionResponse = router
        .put_json("/api/config/plugins_cfg/metadata", &section)
        .await;

    {
        let pm = state.plugin_manager.read().await;
        assert!(
            pm.get_plugin("tvdb_instance").is_some(),
            "Internal plugin should be loaded after API update"
        );

        let statuses = pm.get_plugin_statuses();
        // Status entries are PER-INSTANCE (id = config instance key) — the
        // backend-owned type id lives in `instance_types`, not the entry id.
        let tvdb_status = statuses
            .iter()
            .find(|s| s.id == "tvdb_instance")
            .expect("TVDB registry entry should exist");
        assert!(matches!(tvdb_status.state, RuntimePluginState::Loaded(_)));
    }

    // Manually inject a mock external plugin to verify it survives reload.
    let external_id = "external_binary_plugin".to_string();
    let mock_external = Arc::new(MockExternalPlugin {
        id: external_id.clone(),
    });
    {
        let mut pm = state.plugin_manager.write().await;
        pm.add_internal_plugin(mock_external.clone());
    }

    let mut tvdb_cfg_2 = serde_json::Map::new();
    tvdb_cfg_2.insert("enabled".to_string(), serde_json::json!(true));
    tvdb_cfg_2.insert("api_key".to_string(), serde_json::json!("test_key_2"));
    tvdb_cfg_2.insert("name".to_string(), serde_json::json!("My TVDB Updated"));

    let mut instances_2 = HashMap::new();
    instances_2.insert(
        "tvdb_instance".to_string(),
        serde_json::Value::Object(tvdb_cfg_2),
    );

    let mut section_2 = HashMap::<String, HashMap<String, serde_json::Value>>::new();
    section_2.insert("jumbie.tvdb".to_string(), instances_2);

    let _: jumbie_shared::types::SavePluginsSectionResponse = router
        .put_json("/api/config/plugins_cfg/metadata", &section_2)
        .await;

    {
        let pm = state.plugin_manager.read().await;
        assert!(
            pm.get_plugin("tvdb_instance").is_some(),
            "Plugin should still exist after swap"
        );
    }

    // An empty metadata section disables the plugin.
    let empty_section = HashMap::<String, HashMap<String, serde_json::Value>>::new();
    let _: jumbie_shared::types::SavePluginsSectionResponse = router
        .put_json("/api/config/plugins_cfg/metadata", &empty_section)
        .await;

    {
        let pm = state.plugin_manager.read().await;
        assert!(
            pm.get_plugin("tvdb_instance").is_none(),
            "Plugin should be removed after being disabled in config"
        );
    }
}
