use super::{
    PluginBuild, PluginInstance, PluginManager, RuntimePluginState, RuntimePluginTypeInfo,
};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

struct TempPluginDir {
    path: PathBuf,
}

impl TempPluginDir {
    fn new() -> Self {
        let dir = env::temp_dir();
        let path = dir.join(format!("series_org_plugins_{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn add_manifest(&self, dirname: &str, content: &str) {
        let plugin_dir = self.path.join(dirname);
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("manifest.json"), content).unwrap();
    }

    fn add_file(&self, dirname: &str, filename: &str, content: &str) {
        let plugin_dir = self.path.join(dirname);
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join(filename), content).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = plugin_dir.join(filename);
            let mut perms = fs::metadata(&p).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&p, perms).unwrap();
        }
    }
}

impl Drop for TempPluginDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Whether a working Python interpreter is available.
///
/// External-plugin fixtures here are `.py` scripts (via `run.py`), so they
/// cannot run without Python. Mirrors the `init_python_script` gate in the
/// host tests: a missing interpreter SKIPS the test rather than failing with a
/// confusing "0 plugins discovered" assertion, which does not say that Python
/// is the problem.
fn python_available() -> bool {
    jumbie_shared::plugin::python_command().is_some()
}

/// Minimal in-process plugin for swap tests (no subprocess involved).
struct MockInternalPlugin {
    id: String,
}

#[async_trait::async_trait]
impl PluginInstance for MockInternalPlugin {
    fn instance_id(&self) -> &str {
        &self.id
    }
    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }
    fn priority(&self) -> i32 {
        0
    }
    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        plugin_sdk::traits::PluginTypeInfo {
            display_name: self.id.clone(),
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
    async fn call(
        &self,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!("ok"))
    }
}

#[tokio::test]
async fn test_instance_types_populated_for_internal_plugins() {
    crate::plugins::internal::register_all().await;
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());

    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    plugins_cfg.downloader.insert(
        "jumbie.qbittorrent".to_string(),
        HashMap::from([(
            "my-instance".to_string(),
            serde_json::json!({ "enabled": true }),
        )]),
    );

    let full_cfg = std::sync::Arc::new(jumbie_shared::config::Config::default());
    manager.load_internal_plugins(&plugins_cfg, full_cfg).await;

    // The backend-owned identity map knows the type id for the loaded instance...
    assert_eq!(
        manager.get_instance_type_id("my-instance"),
        Some("jumbie.qbittorrent")
    );
    // ...and the instance itself reports the same instance id.
    let plugin = manager.get_plugin("my-instance").expect("instance loaded");
    assert_eq!(plugin.instance_id(), "my-instance");
}

#[tokio::test]
async fn test_manager_empty_dir() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());

    assert!(
        manager
            .discover_and_start(&jumbie_shared::config::PluginsConfig::default())
            .await
            .is_ok()
    );
    assert!(manager.get_all_plugins().is_empty());
}

#[tokio::test]
async fn test_manager_missing_dir() {
    let tmp = TempPluginDir::new();
    let non_existent = tmp.path.join("non_existent");
    let mut manager = PluginManager::new(non_existent.clone());

    // Should create the dir and report ok
    assert!(
        manager
            .discover_and_start(&jumbie_shared::config::PluginsConfig::default())
            .await
            .is_ok()
    );
    assert!(
        non_existent.exists(),
        "discover_and_start should create missing plugins dir"
    );
    assert!(manager.get_all_plugins().is_empty());
}

#[tokio::test]
async fn test_manager_manifest_no_executable() {
    let tmp = TempPluginDir::new();
    // Manifest references 'run.sh' but it doesn't exist
    let manifest = r#"{
        "display_name": "Test",
        "executable": "run.sh"
    }"#;
    tmp.add_manifest("testplug", manifest);

    let mut manager = PluginManager::new(tmp.path.clone());
    assert!(
        manager
            .discover_and_start(&jumbie_shared::config::PluginsConfig::default())
            .await
            .is_ok()
    ); // Should not fail overall discovery
    assert!(
        manager.get_all_plugins().is_empty(),
        "Plugin with missing executable should fail to load and be ignored"
    );
}

#[tokio::test]
async fn test_manager_id_collision() {
    let tmp = TempPluginDir::new();

    // Both plugins have the same display_name & author → same derived ID → collision
    // The 'id' field is intentionally absent; if present it is ignored by the manager.
    let manifest = r#"{
        "display_name": "Collider",
        "executable": "run.sh"
    }"#;

    // Two plugins with identical metadata but different directories
    tmp.add_manifest("plug1", manifest);
    tmp.add_file("plug1", "run.sh", "#!/bin/sh\nexit 0");

    tmp.add_manifest("plug2", manifest);
    tmp.add_file("plug2", "run.sh", "#!/bin/sh\nexit 0");

    let mut manager = PluginManager::new(tmp.path.clone());
    assert!(
        manager
            .discover_and_start(&jumbie_shared::config::PluginsConfig::default())
            .await
            .is_ok()
    );

    let hosts = manager.get_all_plugins();
    assert_eq!(
        hosts.len(),
        0,
        "a process that exits immediately cannot be Loaded — it must surface as Failed, not pretend to work"
    );
    // The collision resolves to ONE canonical type via the fallback slug
    // (to_plugin_slug(display_name) = "collider" when get_info fails on the
    // immediately-exiting script). Every status entry is keyed by that slug —
    // never the directory names — and every one is Failed (the broken process
    // must not disappear silently, and must not be reported as Loaded).
    let statuses = manager.get_plugin_statuses();
    assert!(
        !statuses.is_empty(),
        "broken plugin type must still have a status entry"
    );
    for entry in &statuses {
        assert_eq!(
            entry.id, "collider",
            "ID collision must resolve to the canonical slug, got {}",
            entry.id
        );
        assert!(
            matches!(entry.state, RuntimePluginState::Failed(_)),
            "broken process must be Failed"
        );
    }
}

/// Signature policy: with `require_signatures` on, an UNSIGNED external plugin
/// is rejected at discovery (identity control — the plugin must be signed to
/// run). With the flag off (default), unsigned plugins load normally.
#[tokio::test]
async fn test_require_signatures_rejects_unsigned_plugins() {
    if !python_available() {
        return;
    }
    let tmp = TempPluginDir::new();
    // A perfectly valid plugin — except its manifest carries no signature.
    let logic = r#"
            if method == "get_info":
                print(json.dumps({"jsonrpc": "2.0", "result": {"display_name": "Unsigned Plugin", "version": "1.0.0", "author": "test", "description": "d", "capabilities": [], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "plugin_id": None, "instance_id": None, "supports_test": False}, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
"#;
    let py = jumbie_shared::plugin::PYTHON_PLUGIN_SCRIPT.replace("__LOGIC__", logic);
    tmp.add_manifest(
        "unsigned_plugin",
        r#"{"display_name": "Unsigned Plugin", "executable": "run.py"}"#,
    );
    tmp.add_file("unsigned_plugin", "run.py", &py);

    // Required ⇒ rejected at discovery.
    let mut manager = PluginManager::new(tmp.path.clone());
    manager.set_require_signatures(true);
    manager
        .discover_and_start(&jumbie_shared::config::PluginsConfig::default())
        .await
        .unwrap();
    assert!(
        manager.get_all_plugins().is_empty(),
        "unsigned plugin must not load when signatures are required"
    );
    let statuses = manager.get_plugin_statuses();
    assert!(
        statuses.iter().any(|s| {
            s.id == "unsigned_plugin" && matches!(s.state, RuntimePluginState::Failed(_))
        }),
        "rejected plugin must surface as Failed"
    );

    // Not required (default) ⇒ loads fine.
    let mut manager = PluginManager::new(tmp.path.clone());
    manager
        .discover_and_start(&jumbie_shared::config::PluginsConfig::default())
        .await
        .unwrap();
    assert_eq!(
        manager.get_all_plugins().len(),
        1,
        "unsigned plugin loads when signatures are optional"
    );
}

#[tokio::test]
async fn test_external_plugin_multi_instance_from_config() {
    if !python_available() {
        return;
    }
    let tmp = TempPluginDir::new();
    // Minimal JSON-RPC plugin: answers get_info (id + metadata) and initialize.
    // The logic is inserted inside the template's `try:` block, so it must be
    // indented to match (12 spaces), same as the host tests.
    let logic = r#"
            if method == "get_info":
                print(json.dumps({"jsonrpc": "2.0", "result": {"display_name": "Test Plugin", "version": "1.0.0", "author": "test", "description": "d", "capabilities": [], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "plugin_id": None, "instance_id": None, "supports_test": False}, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "initialize":
                print(json.dumps({"jsonrpc": "2.0", "result": None, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
"#;
    let py = jumbie_shared::plugin::PYTHON_PLUGIN_SCRIPT.replace("__LOGIC__", logic);
    tmp.add_manifest(
        "test_plugin",
        r#"{"display_name": "Test Plugin", "executable": "run.py"}"#,
    );
    tmp.add_file("test_plugin", "run.py", &py);

    let mut manager = PluginManager::new(tmp.path.clone());

    // Two ENABLED config instances for the same external type → one subprocess
    // per instance, each keyed by its config instance id (consistent with
    // internal plugins). Canonical type id: derived_id("test", "Test Plugin").
    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    plugins_cfg.source.insert(
        "test.test_plugin".to_string(),
        HashMap::from([
            (
                "inst-a".to_string(),
                serde_json::json!({ "enabled": true, "name": "Alpha" }),
            ),
            (
                "inst-b".to_string(),
                serde_json::json!({ "enabled": true, "name": "Beta" }),
            ),
        ]),
    );

    manager.discover_and_start(&plugins_cfg).await.unwrap();

    let plugins = manager.get_all_plugins();
    let ids_debug: Vec<String> = plugins
        .iter()
        .map(|p| p.instance_id().to_string())
        .collect();
    assert_eq!(
        plugins.len(),
        2,
        "one subprocess per configured external instance; ids={:?}",
        ids_debug
    );
    let mut ids: Vec<String> = plugins
        .iter()
        .map(|p| p.instance_id().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["inst-a".to_string(), "inst-b".to_string()]);

    // Backend-owned identity + display-name maps are populated like internal.
    assert_eq!(
        manager.get_instance_type_id("inst-a"),
        Some("test.test_plugin")
    );
    assert_eq!(
        manager.get_instance_type_id("inst-b"),
        Some("test.test_plugin")
    );
    assert_eq!(manager.get_instance_name("inst-a"), "Alpha");

    // Removing an instance from config and re-syncing drops its process.
    plugins_cfg
        .source
        .get_mut("test.test_plugin")
        .unwrap()
        .remove("inst-b");
    manager.sync_external_instances(&plugins_cfg).await;
    let plugins = manager.get_all_plugins();
    assert_eq!(plugins.len(), 1, "removed instance should be dropped");
    assert_eq!(plugins[0].instance_id(), "inst-a");
    assert_eq!(manager.get_instance_type_id("inst-b"), None);
}

#[tokio::test]
async fn test_external_plugin_restart_applies_new_config() {
    if !python_available() {
        return;
    }
    let tmp = TempPluginDir::new();

    // Multi-instance fixture: `initialize` is GENERIC (handled by the template,
    // storing the config in the module-level `instances` dict). There is NO
    // `reconfigure` handler, so the reconfigure RPC fails with "Method not
    // found" and apply_config falls back to re-initializing the instance in
    // the shared process. `get_name` reads the process-side config, so the test
    // observes exactly what config the process currently holds.
    let logic = r#"
            if method == "get_info":
                print(json.dumps({"jsonrpc": "2.0", "result": {"display_name": "Restart Plugin", "version": "1.0.0", "author": "test", "description": "d", "capabilities": [], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "plugin_id": None, "instance_id": None, "supports_test": False}, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "get_name":
                print(json.dumps({"jsonrpc": "2.0", "result": instances.get(instance_id, {}).get("name", ""), "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
"#;
    let py = jumbie_shared::plugin::PYTHON_PLUGIN_SCRIPT.replace("__LOGIC__", logic);
    tmp.add_manifest(
        "restart_plugin",
        r#"{"display_name": "Restart Plugin", "executable": "run.py"}"#,
    );
    tmp.add_file("restart_plugin", "run.py", &py);

    let mut manager = PluginManager::new(tmp.path.clone());
    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    plugins_cfg.source.insert(
        "test.restart_plugin".to_string(),
        HashMap::from([(
            "inst-1".to_string(),
            serde_json::json!({ "enabled": true, "name": "A" }),
        )]),
    );

    manager.discover_and_start(&plugins_cfg).await.unwrap();
    assert_eq!(manager.get_all_plugins().len(), 1);
    assert_eq!(manager.get_instance_name("inst-1"), "A");
    let plugin = manager.get_plugin("inst-1").unwrap();
    assert_eq!(
        plugin.call("get_name", None).await.unwrap(),
        serde_json::json!("A"),
        "process must hold config A after initial load"
    );

    // Change the config: the fixture can't reconfigure in place (no `reconfigure`
    // handler → RPC fails with "Method not found"), so apply_config
    // re-initializes the instance INSIDE the same shared process with the NEW
    // config ("B") — an in-process RPC, not a process restart.
    plugins_cfg
        .source
        .get_mut("test.restart_plugin")
        .unwrap()
        .insert(
            "inst-1".to_string(),
            serde_json::json!({ "enabled": true, "name": "B" }),
        );

    let full_cfg = Arc::new(jumbie_shared::config::Config::default());
    manager.apply_config(&plugins_cfg, full_cfg).await;

    // Same instance id, re-initialized with the new config.
    assert_eq!(manager.get_all_plugins().len(), 1);
    assert_eq!(
        manager.get_plugin("inst-1").unwrap().instance_id(),
        "inst-1"
    );
    assert_eq!(
        manager.get_instance_type_id("inst-1"),
        Some("test.restart_plugin")
    );
    assert_eq!(manager.get_instance_name("inst-1"), "B");
    assert_eq!(
        plugin.call("get_name", None).await.unwrap(),
        serde_json::json!("B"),
        "re-init must re-initialize the instance with the new config in the same process"
    );
}

#[tokio::test]
async fn test_external_plugin_reconfigure_failure_triggers_reinit() {
    if !python_available() {
        return;
    }
    let tmp = TempPluginDir::new();

    // Fixture that HANDLES reconfigure but returns a NON-method-not-found error.
    // ANY failure routes to re-initialization in the same shared process (never a
    // half-reconfigured instance).
    let logic = r#"
            if method == "get_info":
                print(json.dumps({"jsonrpc": "2.0", "result": {"display_name": "Fragile Plugin", "version": "1.0.0", "author": "test", "description": "d", "capabilities": [], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "plugin_id": None, "instance_id": None, "supports_test": False}, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "initialize":
                print(json.dumps({"jsonrpc": "2.0", "result": None, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "reconfigure":
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32000, "message": "boom"}, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
"#;
    let py = jumbie_shared::plugin::PYTHON_PLUGIN_SCRIPT.replace("__LOGIC__", logic);
    tmp.add_manifest(
        "fragile_plugin",
        r#"{"display_name": "Fragile Plugin", "executable": "run.py"}"#,
    );
    tmp.add_file("fragile_plugin", "run.py", &py);

    let mut manager = PluginManager::new(tmp.path.clone());
    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    plugins_cfg.source.insert(
        "test.fragile_plugin".to_string(),
        HashMap::from([(
            "inst-1".to_string(),
            serde_json::json!({ "enabled": true, "name": "A" }),
        )]),
    );

    manager.discover_and_start(&plugins_cfg).await.unwrap();
    assert_eq!(manager.get_all_plugins().len(), 1);

    // Change the config: reconfigure returns a generic error (code -32000), NOT
    // "Method not found". The unified fallback must still re-initialize the
    // instance inside the shared process (in-process RPC, not a restart).
    plugins_cfg
        .source
        .get_mut("test.fragile_plugin")
        .unwrap()
        .insert(
            "inst-1".to_string(),
            serde_json::json!({ "enabled": true, "name": "B" }),
        );

    let full_cfg = Arc::new(jumbie_shared::config::Config::default());
    manager.apply_config(&plugins_cfg, full_cfg).await;
    assert_eq!(manager.get_all_plugins().len(), 1);
    assert_eq!(manager.get_instance_name("inst-1"), "B");
}

#[tokio::test]
async fn test_internal_plugin_reconfigures_in_place() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());
    crate::plugins::internal::register_all().await;

    let full_cfg = Arc::new(jumbie_shared::config::Config::default());
    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    plugins_cfg.metadata.insert(
        "jumbie.tvdb".to_string(),
        HashMap::from([(
            "primary".to_string(),
            serde_json::json!({
                "enabled": true,
                "name": "TVDB A",
                "api_key": "k",
                "lang": "en",
                "priority": 1,
            }),
        )]),
    );

    manager
        .load_internal_plugins(&plugins_cfg, full_cfg.clone())
        .await;

    let original = manager
        .get_plugin("primary")
        .expect("tvdb should be loaded");
    assert_eq!(original.priority(), 1);

    // Change config: name/lang/priority change, credentials unchanged. The
    // instance must reconfigure IN PLACE (Arc identity preserved — there is
    // exactly one copy of an instance at any time) with the new config applied
    // and host fields synced onto the wrapper.
    plugins_cfg.metadata.get_mut("jumbie.tvdb").unwrap().insert(
        "primary".to_string(),
        serde_json::json!({
            "enabled": true,
            "name": "TVDB B",
            "api_key": "k",
            "lang": "de",
            "priority": 5,
        }),
    );

    manager.apply_config(&plugins_cfg, full_cfg).await;

    let after = manager
        .get_plugin("primary")
        .expect("tvdb should still be loaded");
    assert!(
        Arc::ptr_eq(&original, &after),
        "a changed internal instance must reconfigure in place (same Arc), not be rebuilt"
    );
    assert_eq!(
        after.priority(),
        5,
        "host fields must sync onto the kept instance"
    );

    // The renamed instance must be reflected in the backend-owned name map.
    assert_eq!(manager.get_instance_name("primary"), "TVDB B");

    // The status entry for the kept instance must point at the SAME surviving
    // Arc — there is exactly one copy of an instance at any time.
    let status = manager.get_plugin_statuses();
    let entry = status
        .iter()
        .find(|e| e.id == "primary")
        .expect("kept instance should have a status entry");
    match &entry.state {
        RuntimePluginState::Loaded(arc) => {
            assert!(
                Arc::ptr_eq(&original, arc),
                "status must reference the kept instance"
            );
        }
        _ => panic!("expected Loaded state for kept instance"),
    }
}

#[tokio::test]
async fn test_disabled_internal_plugin_is_dropped() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());
    crate::plugins::internal::register_all().await;

    let full_cfg = Arc::new(jumbie_shared::config::Config::default());
    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    plugins_cfg.metadata.insert(
        "jumbie.tvdb".to_string(),
        HashMap::from([(
            "primary".to_string(),
            serde_json::json!({ "enabled": true, "name": "TVDB", "api_key": "k" }),
        )]),
    );
    manager
        .load_internal_plugins(&plugins_cfg, full_cfg.clone())
        .await;
    assert!(manager.get_plugin("primary").is_some());

    // Disable the instance — it must NOT be kept in place; apply_config drops it
    // (a disabled instance is not reconfigured, matching build_internal_plugins).
    plugins_cfg
        .metadata
        .get_mut("jumbie.tvdb")
        .unwrap()
        .get_mut("primary")
        .unwrap()["enabled"] = serde_json::json!(false);

    manager.apply_config(&plugins_cfg, full_cfg).await;

    assert!(
        manager.get_plugin("primary").is_none(),
        "disabled instance must be dropped"
    );
}

#[tokio::test]
async fn test_fresh_internal_plugin_respects_host_fields_from_cold_start() {
    // Cold-start gap: factories ignore the refresh_interval parameter (and the
    // wrapper snapshots host fields from the inner plugin), so host fields must
    // be applied at build time. This test pins ALL of them — it would have
    // caught the original bug where refresh_interval was silently ignored.
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());
    crate::plugins::internal::register_all().await;

    let full_cfg = Arc::new(jumbie_shared::config::Config::default());
    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    plugins_cfg.source.insert(
        "jumbie.basic_rss".to_string(),
        HashMap::from([(
            "feed".to_string(),
            serde_json::json!({
                "enabled": true,
                "name": "My Feed",
                "url": "https://example.com/feed.xml",
                "priority": 3,
                "refresh_interval": 60,
            }),
        )]),
    );

    manager.load_internal_plugins(&plugins_cfg, full_cfg).await;

    let plugin = manager
        .get_plugin("feed")
        .expect("rss source should be loaded");
    assert_eq!(
        plugin.refresh_interval(),
        Some(60),
        "configured refresh_interval must be respected from cold start"
    );
    assert_eq!(
        plugin.priority(),
        3,
        "configured priority must be respected"
    );
    assert!(plugin.is_enabled(), "enabled:true must be respected");
    // Backend-owned name map must be populated from config at cold start.
    assert_eq!(manager.get_instance_name("feed"), "My Feed");
    assert_eq!(
        manager.get_instance_type_id("feed"),
        Some("jumbie.basic_rss")
    );
}

/// A save with an UNCHANGED config must not touch the instance at all: the
/// exact Arc survives (no factory rebuild, no reconfigure) — this is the
/// targeted-rebuild diff at work.
#[tokio::test]
async fn test_apply_config_unchanged_config_preserves_arc() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());
    crate::plugins::internal::register_all().await;

    let full_cfg = Arc::new(jumbie_shared::config::Config::default());
    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    plugins_cfg.metadata.insert(
        "jumbie.tvdb".to_string(),
        HashMap::from([(
            "primary".to_string(),
            serde_json::json!({
                "enabled": true,
                "name": "TVDB",
                "api_key": "k",
                "lang": "en",
            }),
        )]),
    );

    manager
        .load_internal_plugins(&plugins_cfg, full_cfg.clone())
        .await;
    let original = manager
        .get_plugin("primary")
        .expect("tvdb should be loaded");

    // Identical config, applied again — the instance must keep its exact Arc
    // (identity and runtime state survive; nothing is rebuilt or reconfigured).
    manager.apply_config(&plugins_cfg, full_cfg).await;

    let after = manager
        .get_plugin("primary")
        .expect("tvdb should still be loaded");
    assert!(
        Arc::ptr_eq(&original, &after),
        "an unchanged config must never rebuild or reconfigure the instance"
    );
    assert_eq!(manager.get_instance_name("primary"), "TVDB");
}

/// A save with an UNCHANGED external config must not send a `set_config` RPC:
/// the diff against `applied_configs` skips untouched instances entirely.
///
/// The fixture is a STANDALONE script (not the template) so it owns its
/// `set_config` handler and can count invocations.
#[tokio::test]
async fn test_external_unchanged_config_is_untouched() {
    if !python_available() {
        return;
    }
    let tmp = TempPluginDir::new();

    // Standalone JSON-RPC plugin: counts `set_config` calls per instance.
    let py = r##"#!/usr/bin/env python3
import os, sys, json
JUMBIE_SECRET = os.environ.get("JUMBIE_PLUGIN_SECRET")
counts = {}
instances = {}

def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        req = json.loads(line)
        req_id = req.get("id")
        method = req.get("method")
        instance_id = req.get("instance_id")
        params = req.get("params", {})
        req_auth = req.get("auth")
        if JUMBIE_SECRET and req_auth != JUMBIE_SECRET:
            print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32001, "message": "Authentication failed"}, "id": req_id, "auth": req_auth}), flush=True)
            continue
        if method == "hello":
            print(json.dumps({"jsonrpc": "2.0", "result": {"protocol_version": 3}, "id": req_id, "auth": req_auth}), flush=True)
        elif method == "get_info":
            print(json.dumps({"jsonrpc": "2.0", "result": {"display_name": "Counter Plugin", "version": "1.0.0", "author": "test", "description": "d", "capabilities": [], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "plugin_id": None, "instance_id": None, "supports_test": False}, "id": req_id, "auth": req_auth}), flush=True)
        elif method == "set_config":
            counts[instance_id] = counts.get(instance_id, 0) + 1
            instances[instance_id] = params
            print(json.dumps({"jsonrpc": "2.0", "result": True, "id": req_id, "auth": req_auth}), flush=True)
        elif method == "get_set_config_count":
            print(json.dumps({"jsonrpc": "2.0", "result": counts.get(instance_id, 0), "id": req_id, "auth": req_auth}), flush=True)
        else:
            print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)

if __name__ == "__main__":
    main()
"##;
    tmp.add_manifest(
        "counter_plugin",
        r#"{"display_name": "Counter Plugin", "executable": "run.py"}"#,
    );
    tmp.add_file("counter_plugin", "run.py", py);

    let mut manager = PluginManager::new(tmp.path.clone());
    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    let config = serde_json::json!({ "enabled": true, "name": "A" });
    plugins_cfg.source.insert(
        "test.counter_plugin".to_string(),
        HashMap::from([("inst-1".to_string(), config)]),
    );

    manager.discover_and_start(&plugins_cfg).await.unwrap();
    assert_eq!(manager.get_all_plugins().len(), 1);
    let plugin = manager.get_plugin("inst-1").unwrap();
    async fn count(plugin: &Arc<dyn PluginInstance>) -> u64 {
        serde_json::from_value(plugin.call("get_set_config_count", None).await.unwrap()).unwrap()
    }
    // Initial load = exactly one set_config (registration).
    assert_eq!(count(&plugin).await, 1, "initial load sends one set_config");

    // Identical config, applied again → the diff skips the instance.
    let full_cfg = Arc::new(jumbie_shared::config::Config::default());
    manager.apply_config(&plugins_cfg, full_cfg.clone()).await;
    assert_eq!(
        count(&plugin).await,
        1,
        "an unchanged external config must not send a set_config RPC"
    );

    // A real change DOES push set_config (single RPC, no process restart).
    plugins_cfg
        .source
        .get_mut("test.counter_plugin")
        .unwrap()
        .insert(
            "inst-1".to_string(),
            serde_json::json!({ "enabled": true, "name": "B" }),
        );
    manager.apply_config(&plugins_cfg, full_cfg).await;
    assert_eq!(
        count(&plugin).await,
        2,
        "a changed external config must send exactly one set_config RPC"
    );
    assert_eq!(manager.get_instance_name("inst-1"), "B");
}

#[tokio::test]
async fn test_manager_swap_internal_plugins() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());

    // Setup initial state with an "external" plugin.
    let external_id = "external_bin".to_string();
    manager.registered_plugins.push(RuntimePluginTypeInfo {
        id: external_id.clone(),
        name: "External".to_string(),
        category: "plugin".to_string(), // The key marker for external plugins
        state: RuntimePluginState::Failed("Simulated".to_string()),
    });
    // No real host needed here — swap only checks IDs and categories.

    // Perform a swap with some "internal" plugins.
    let new_internal_id = "internal_1".to_string();
    let mock: Arc<dyn PluginInstance> = Arc::new(MockInternalPlugin {
        id: new_internal_id.clone(),
    });

    let new_order = vec![new_internal_id.clone()];
    let new_registered = vec![RuntimePluginTypeInfo {
        id: "metadata.test".to_string(),
        name: "Test Metadata".to_string(),
        category: "metadata".to_string(),
        state: RuntimePluginState::Failed("Init".to_string()),
    }];

    let keep = std::collections::HashSet::new();
    manager.swap_internal_plugins(
        &keep,
        PluginBuild {
            plugins: HashMap::from([(new_internal_id.clone(), mock)]),
            plugin_order: new_order,
            registered_plugins: new_registered,
            instance_names: HashMap::new(),
            instance_types: HashMap::new(),
        },
    );

    // External must survive the swap.
    assert!(
        manager
            .registered_plugins
            .iter()
            .any(|p| p.id == external_id),
        "External plugin should survive swap"
    );
    // Internal must be present.
    assert!(
        manager
            .registered_plugins
            .iter()
            .any(|p| p.id == "metadata.test"),
        "New internal plugin should be present"
    );
    assert!(manager.plugin_order.contains(&new_internal_id));
}

#[tokio::test]
async fn test_manager_hot_reload_clears_old_internal() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());

    // Add an old internal plugin.
    manager.plugin_order.push("old_internal".to_string());
    manager.registered_plugins.push(RuntimePluginTypeInfo {
        id: "metadata.old".to_string(),
        name: "Old".to_string(),
        category: "metadata".to_string(),
        state: RuntimePluginState::Failed("Old".to_string()),
    });

    // Swap with nothing.
    let keep = std::collections::HashSet::new();
    manager.swap_internal_plugins(&keep, PluginBuild::default());

    // Verify it's gone.
    assert!(
        manager.registered_plugins.is_empty(),
        "Old internal plugin should be gone"
    );
    assert!(manager.plugin_order.is_empty());
}

#[tokio::test]
async fn test_manager_shutdown_token_contract() {
    let tmp = TempPluginDir::new();
    let manager = PluginManager::new(tmp.path.clone());

    let token = manager.shutdown_token();
    assert!(
        !token.is_cancelled(),
        "Fresh PluginManager should have an active shutdown token"
    );

    manager.shutdown();

    assert!(
        token.is_cancelled(),
        "After shutdown(), the token returned by shutdown_token() must be cancelled"
    );
}

#[test]
fn test_get_instance_name_returns_configured_name() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());
    let instance_id = "test-instance-uuid";
    let configured_name = "My Custom Nyaa";

    manager
        .instance_names
        .insert(instance_id.to_string(), configured_name.to_string());

    let result = manager.get_instance_name(instance_id);
    assert_eq!(result, configured_name);
}

#[test]
fn test_get_instance_name_unknown_returns_empty() {
    let tmp = TempPluginDir::new();
    let manager = PluginManager::new(tmp.path.clone());
    let unknown_id = "unknown-instance-uuid";

    let result = manager.get_instance_name(unknown_id);
    assert_eq!(result, "");
}

#[test]
fn test_get_instance_name_prefers_configured_over_empty() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());
    let instance_id = "test-instance-uuid";

    // Empty string name should not be stored (build_internal_plugins skips empty)
    manager
        .instance_names
        .insert(instance_id.to_string(), String::new());

    let result = manager.get_instance_name(instance_id);
    // It's stored as empty string, so it would return empty
    assert_eq!(result, "");
}

#[test]
fn test_swap_preserves_external_instance_names() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());

    let ext_id = "external-plugin".to_string();
    manager
        .instance_names
        .insert(ext_id.clone(), "My External".to_string());
    manager.registered_plugins.push(RuntimePluginTypeInfo {
        id: ext_id.clone(),
        name: "Ext".to_string(),
        category: "plugin".to_string(),
        state: RuntimePluginState::Failed("test".to_string()),
    });

    // Internal plugin that should be swept away
    let int_id = "source.old".to_string();
    manager
        .instance_names
        .insert(int_id.clone(), "Old Name".to_string());

    // Swap with empty new data
    let keep = std::collections::HashSet::new();
    manager.swap_internal_plugins(&keep, PluginBuild::default());

    // External instance name survived
    assert_eq!(manager.get_instance_name(&ext_id), "My External");
    // Internal instance name was replaced (not present → empty)
    assert_eq!(manager.get_instance_name(&int_id), "");
}

#[test]
fn test_swap_replaces_internal_instance_names() {
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());

    let int_id = "source.old".to_string();
    manager
        .instance_names
        .insert(int_id.clone(), "Old Name".to_string());

    // New instance names from hot-reload
    let mut new_names = HashMap::new();
    new_names.insert("source.new".to_string(), "New Name".to_string());

    let keep = std::collections::HashSet::new();
    manager.swap_internal_plugins(
        &keep,
        PluginBuild {
            plugins: HashMap::new(),
            plugin_order: vec![],
            registered_plugins: vec![],
            instance_names: new_names,
            instance_types: HashMap::new(),
        },
    );

    // Old internal name is gone
    assert_eq!(manager.get_instance_name(&int_id), "");
    // New internal name is present
    assert_eq!(manager.get_instance_name("source.new"), "New Name");
}

/// A capability must be BOTH declared by the plugin type AND enabled on the
/// instance before dispatch selects it (see `plugins::capabilities`).
///
/// Uses the real built-in TVDB plugin: it declares `Polling` statically, so the
/// `enable_polling` toggle is what decides whether the metadata refresh loop
/// (`[MetadataProviderNormal, Polling]`) can see it.
#[tokio::test]
async fn test_capability_toggle_gates_metadata_polling() {
    use jumbie_shared::plugin::Capability;

    crate::plugins::internal::register_all().await;
    let tmp = TempPluginDir::new();
    let mut manager = PluginManager::new(tmp.path.clone());
    let full_cfg = std::sync::Arc::new(jumbie_shared::config::Config::default());

    let mut plugins_cfg = jumbie_shared::config::PluginsConfig::default();
    plugins_cfg.metadata.insert(
        "jumbie.tvdb".to_string(),
        HashMap::from([(
            "tvdb-inst".to_string(),
            serde_json::json!({
                "enabled": true,
                "enable_polling": true,
                "api_key": "test-key",
            }),
        )]),
    );
    manager
        .load_internal_plugins(&plugins_cfg, full_cfg.clone())
        .await;

    // Declared + enabled → dispatchable for both capabilities.
    let matched = manager.get_plugins_by_all_capabilities(&[
        Capability::MetadataProviderNormal,
        Capability::Polling,
    ]);
    assert_eq!(
        matched.len(),
        1,
        "polling-enabled metadata plugin should match"
    );
    assert_eq!(matched[0].instance_id(), "tvdb-inst");

    // User disables polling on the instance.
    plugins_cfg
        .metadata
        .get_mut("jumbie.tvdb")
        .unwrap()
        .get_mut("tvdb-inst")
        .unwrap()["enable_polling"] = serde_json::json!(false);
    manager.apply_config(&plugins_cfg, full_cfg.clone()).await;

    // `Polling` is now ineffective, so the refresh-loop query excludes it...
    let matched = manager.get_plugins_by_all_capabilities(&[
        Capability::MetadataProviderNormal,
        Capability::Polling,
    ]);
    assert!(
        matched.is_empty(),
        "disabled polling must exclude the instance from the refresh loop"
    );
    // ...but the non-toggle metadata capability is unaffected.
    let base = manager.get_plugins_by_capability(Capability::MetadataProviderNormal);
    assert_eq!(
        base.len(),
        1,
        "base capability must not be gated by the toggle"
    );
}
