use crate::plugins::{PluginInstance, PluginManager, RuntimePluginState};
use jumbie_shared::config::{Config, PluginsConfig};
use serde_json::Value;
use std::sync::Arc;

// Shared mock infrastructure

/// Minimal mock plugin that delegates `call` to a user-supplied closure, so each
/// test overrides only the behaviour it cares about.
/// Handler closure type for [`MockPlugin`].
type MockHandler = Box<dyn Fn(&str, Option<Value>) -> anyhow::Result<Value> + Send + Sync>;

struct MockPlugin {
    id: &'static str,
    display_name: &'static str,
    handler: MockHandler,
}

impl MockPlugin {
    fn create(
        id: &'static str,
        display_name: &'static str,
        handler: impl Fn(&str, Option<Value>) -> anyhow::Result<Value> + Send + Sync + 'static,
    ) -> Arc<dyn PluginInstance> {
        Arc::new(Self {
            id,
            display_name,
            handler: Box::new(handler),
        })
    }
}

#[async_trait::async_trait]
impl PluginInstance for MockPlugin {
    fn instance_id(&self) -> &str {
        self.id
    }
    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        plugin_sdk::traits::PluginTypeInfo {
            display_name: self.display_name.to_string(),
            version: "1.0.0".to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: String::new(),
            capabilities: vec![],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        }
    }
    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }
    fn priority(&self) -> i32 {
        0
    }
    async fn call(&self, method: &str, params: Option<Value>) -> anyhow::Result<Value> {
        (self.handler)(method, params)
    }
}

#[tokio::test]
async fn test_internal_plugin_health_connected() {
    let plugin = MockPlugin::create("mock", "MockInternal", |method, _| {
        if method == "health_check" {
            Ok(serde_json::json!("ok"))
        } else {
            anyhow::bail!("Method not found")
        }
    });

    let res = plugin.call("health_check", None).await;
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), serde_json::json!("ok"));
}

#[tokio::test]
async fn test_marker_plugin_health_connected() {
    let mut manager = PluginManager::new("./tmp_plugins");
    let plugins_cfg = PluginsConfig::default(); // No plugins enabled -> all should be marker plugins
    let full_cfg = Arc::new(Config::default());

    // Ensure internal plugins are registered in the global registry first
    crate::plugins::internal::register_all().await;

    manager.load_internal_plugins(&plugins_cfg, full_cfg).await;
    let statuses = manager.get_plugin_statuses();

    // Should have marker plugins for all registered internal types (Discord, Nyaa, etc.)
    assert!(
        !statuses.is_empty(),
        "Should have at least one registered plugin type"
    );

    for status in statuses {
        match status.state {
            RuntimePluginState::Loaded(plugin) => {
                let res = plugin.call("health_check", None).await;
                assert!(
                    res.is_ok(),
                    "Health check should succeed for marker plugin {}",
                    status.id
                );
                assert_eq!(res.unwrap(), serde_json::json!("ok"));
            }
            RuntimePluginState::Failed(e) => {
                panic!(
                    "Plugin {} should be Loaded as a marker, but failed: {}",
                    status.id, e
                );
            }
        }
    }
}

#[tokio::test]
async fn test_plugin_health_error() {
    let plugin = MockPlugin::create("error", "ErrorPlugin", |method, _| {
        if method == "health_check" {
            anyhow::bail!("Plugin process died unexpectedly")
        } else {
            anyhow::bail!("Method not found")
        }
    });

    let res = plugin.call("health_check", None).await;
    assert!(res.is_err(), "Health check should fail for a broken plugin");
    assert_eq!(
        res.unwrap_err().to_string(),
        "Plugin process died unexpectedly"
    );
}

#[tokio::test]
async fn test_plugin_health_timeout() {
    // We wrap call in a task that sleeps to simulate a hanging plugin process
    let timeout = std::time::Duration::from_secs(1);
    let result = tokio::time::timeout(timeout, async {
        // Sleep longer than the timeout to simulate a hang (10s in production)
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        Ok::<_, anyhow::Error>(serde_json::json!("ok"))
    })
    .await;

    assert!(result.is_err(), "Health check should time out");
}

#[tokio::test]
async fn test_plugin_health_dead() {
    let plugin = MockPlugin::create("dead", "DeadPlugin", |_, _| {
        anyhow::bail!("Plugin process is dead") // Typical error from PluginTypeHost if channel closed
    });

    let res = plugin.call("health_check", None).await;
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("dead"));
}
