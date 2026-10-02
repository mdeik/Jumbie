use super::{InstanceHandle, PluginInstance, PluginTypeHost};
use anyhow::Result;
use jumbie_shared::plugin::Capability;
use jumbie_shared::types::media::MediaEntry;
use jumbie_shared::validation::plugin_data::Validate;
use serde_json::json;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;
use uuid::Uuid;

/// Initializes a tracing subscriber ONCE so the plugin lifecycle task's
/// `error!`/`warn!` diagnostics (e.g. "failed startup handshake: <reason>",
/// which would otherwise be silently dropped under `cargo test`) reach the
/// test output. Env-gated so it defaults to off unless `RUST_LOG` is set;
/// set `RUST_LOG=jumbie=debug` to see the handshake/respawn trace, plus the
/// child's own `plugin_stderr` output (the reason a Python plugin exits is
/// forwarded there and would otherwise be filtered out).
static TRACING_INIT: LazyLock<()> = LazyLock::new(|| {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("jumbie=debug,plugin_stderr=warn")
            .try_init();
    }
});

/// Serializes Python subprocess tests to avoid resource contention.
/// Python processes compete for the GIL and system process slots, so
/// running them concurrently (as cargo test does by default) causes
/// spurious failures. This mutex ensures only one Python-based plugin
/// test runs at a time.
static PYTHON_TEST_MUTEX: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Runs a Python plugin test with retry on transient failures.
/// Under concurrent test load, subprocess startup and response races
/// can cause spurious assertion failures. This gives each test up to
/// 3 attempts with backoff, catching both `Err` returns and `assert!`
/// panics (via `catch_unwind`).
async fn run_python_test<F, Fut>(f: F) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let mut last_err: Option<anyhow::Error> = None;
    // Surface the lifecycle task's diagnostics (handshake failures, respawn
    // reasons) when RUST_LOG is set — otherwise they're silently dropped.
    LazyLock::force(&TRACING_INIT);
    for attempt in 1..=5 {
        // catch_unwind handles assert! panics.
        // AssertUnwindSafe is safe here — the panic boundary is solely
        // within the test body; no shared state escapes.
        let outcome = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&f)) {
            Ok(fut) => match fut.await {
                Ok(()) => return Ok(()),
                Err(e) => Err(e),
            },
            Err(panic) => {
                let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic.downcast_ref::<String>() {
                    s.clone()
                } else {
                    format!("{:?}", panic)
                };
                Err(anyhow::anyhow!("Test panicked: {}", msg))
            }
        };

        match outcome {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!("Python plugin test attempt {}/5 failed: {}", attempt, e);
                last_err = Some(e);
                if attempt < 5 {
                    tokio::time::sleep(Duration::from_millis(2000 * attempt)).await;
                }
            }
        }
    }
    Err(last_err.unwrap())
}

/// Helper to create a temporary executable script for testing IPC.
/// The script loops, reads JSON-RPC requests from stdin, and writes responses to stdout.
struct TestScript {
    path: PathBuf,
}

impl TestScript {
    fn new(script_content: &str, ext: &str) -> Self {
        let dir = env::temp_dir();
        let filename = format!("series_org_test_{}.{}", Uuid::new_v4(), ext);
        let path = dir.join(filename);

        fs::write(&path, script_content).expect("Failed to write test script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }

        std::thread::sleep(std::time::Duration::from_millis(50));

        Self { path }
    }
}

impl Drop for TestScript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// Ensure Python is available, otherwise skip the test (SSoT: the interpreter
// pick lives in `jumbie_shared::plugin::python_command`).
fn init_python_script(logic: &str) -> Option<TestScript> {
    jumbie_shared::plugin::python_command()?;

    let script = jumbie_shared::plugin::PYTHON_PLUGIN_SCRIPT.replace("__LOGIC__", logic);

    Some(TestScript::new(&script, "py"))
}

/// Guard test: every other Python plugin test SKIPS (not fails) when no
/// interpreter is available — `init_python_script` returns `None` and the test
/// returns early. That makes a Python-less environment SILENT. This test exists
/// to make it LOUD: if the suite cannot run its Python plugin tests, it fails
/// here with an actionable message instead of quietly reporting them all as
/// skipped.
#[test]
fn test_python_plugin_interpreter_available() {
    assert!(
        jumbie_shared::plugin::python_command().is_some(),
        "No working Python interpreter found (probed `python3 -c pass` and \
         `python -c pass`). Every Python plugin test in host_tests, manager_tests, \
         and the integration tests is being SKIPPED — install Python 3.8+ on PATH \
         (on Windows, ensure the 3.14 `pymanager` launcher has a runtime installed) \
         to actually run them."
    );
}

/// Spawn a type host + initialize one test instance, returning an
/// `InstanceHandle` that routes calls by instance_id.
async fn spawn_test_instance(
    script: &TestScript,
    instance_id: &str,
) -> Result<Arc<InstanceHandle>> {
    let host = PluginTypeHost::spawn(
        "test.test_plugin",
        instance_id,
        &script.path,
        tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    host.set_config(instance_id, json!({})).await?;
    Ok(Arc::new(InstanceHandle::new(instance_id.to_string(), host)))
}

/// Acquires the serialisation mutex and creates the Python script.
/// Expands to two `let` bindings (`_guard` and `$script`) and returns
/// `Ok(())` early if Python is unavailable.
///
/// Usage: `python_setup!(logic, script);`  then use `script.path`.
macro_rules! python_setup {
    ($logic:expr, $script:ident) => {
        let _guard = PYTHON_TEST_MUTEX.lock().await;
        let $script = match init_python_script($logic) {
            Some(s) => s,
            None => return Ok(()),
        };
    };
}

#[tokio::test]
async fn test_plugin_host_get_info() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "get_info":
                print(json.dumps({"jsonrpc": "2.0", "result": {"id": "test_plugin", "display_name": "Test", "version": "1.0.0", "category": "metadata", "author": "me", "description": "test"}, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);


        let host = spawn_test_instance(&script, "test_id").await?;

        // Type-level get_info routes without an instance id.
        let res = host
            .type_host()
            .get_info()
            .await?;
        anyhow::ensure!(res["id"] == json!("test_plugin"), "Expected id 'test_plugin'");
        anyhow::ensure!(res["display_name"] == json!("Test"), "Expected display_name 'Test'");

        // The instance handle surfaces the type's cached info and its own id.
        host.type_host()
            .set_cached_info(jumbie_shared::plugin::PluginTypeInfo {
                display_name: "Test".to_string(),
                version: "1.0.0".to_string(),
                author: "me".to_string(),
                description: "test".to_string(),
                capabilities: vec![jumbie_shared::plugin::Capability::MetadataProvider],
                supported_protocols: None,
                series_identifier_label: None,
                series_identifier_placeholder: None,
                rate_limit: None,
                supports_test: true,
            });
        anyhow::ensure!(
            host.instance_id() == "test_id",
            "instance_id() should return the spawn-time instance id, not the canonical type id"
        );
        anyhow::ensure!(
            host.plugin_info()
                .capabilities
                .contains(&jumbie_shared::plugin::Capability::MetadataProvider),
            "plugin_info().capabilities should come from the type's cached info"
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_plugin_host_error_response() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "fail_me":
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -12345, "message": "Custom test error"}, "id": req_id, "auth": req_auth}), flush=True)
            else:
                pass
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_id").await?;

        let res = host.call("fail_me", Some(json!({}))).await;
        anyhow::ensure!(res.is_err(), "Expected call to return an error, got: {:?}", res);

        let err_str = res.unwrap_err().to_string();
        anyhow::ensure!(
            err_str.contains("Custom test error"),
            "Error message should contain string from plugin, got: {}",
            err_str
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_plugin_host_timeout() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "sleep":
                time.sleep(3) # Will sleep long enough to trigger a shorter timeout
                print(json.dumps({"jsonrpc": "2.0", "result": "done", "id": req_id, "auth": req_auth}), flush=True)
            else:
                pass
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_id").await?;

        // We don't have a built-in way to change the timeout on PluginTypeHost easily without modifying the struct,
        // so let's wrap our own tokio::time::timeout around the call to simulate it
        let res = tokio::time::timeout(
            Duration::from_millis(200), // Python sleeps 3s — 200ms is more than enough
            host.call("sleep", Some(json!({}))),
        )
        .await;

        // Under parallel test load the plugin process may crash before the tokio
        // timeout fires, producing Ok(Err(Plugin process crashed)). Accept both
        // failure modes — the important thing is the plugin never returned "done".
        match &res {
            Err(_) => {} // tokio timed out — expected path
            Ok(Err(e)) => {
                let msg = format!("{:#}", e);
                anyhow::ensure!(
                    msg.contains("Plugin process crashed"),
                    "Expected plugin crash or tokio timeout, got Ok(Err({}))",
                    msg
                );
            }
            Ok(Ok(v)) => {
                anyhow::bail!(
                    "Expected plugin crash or tokio timeout, but call returned Ok: {:?}",
                    v
                );
            }
        }

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_plugin_host_multiple_concurrent_calls() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "echo":
                print(json.dumps({"jsonrpc": "2.0", "result": params.get("msg"), "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_id").await?;

        // Fire off 5 concurrent requests
        let mut handles: Vec<tokio::task::JoinHandle<_>> = Vec::new();
        for i in 0..5 {
            let h = host.clone();
            handles.push(tokio::spawn(async move {
                h.call("echo", Some(json!({"msg": format!("hello {}", i)})))
                    .await
            }));
        }

        let results = futures::future::join_all(handles).await;

        let mut success_count = 0;
        for (i, res) in results.into_iter().enumerate() {
            let r = res
                .map_err(|e| anyhow::anyhow!("Task join failed: {}", e))?
                .map_err(|e| anyhow::anyhow!("RPC call failed: {}", e))?;
            anyhow::ensure!(r == format!("hello {}", i), "Expected 'hello {}', got '{}'", i, r);
            success_count += 1;
        }

        anyhow::ensure!(success_count == 5, "Expected 5 successes, got {}", success_count);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_plugin_host_set_config() -> Result<()> {
    run_python_test(|| async {
        // The template handles `set_config` GENERICALLY (stores the config,
        // returns true) — there is no plugin "reconfigure" logic.
        let logic = r#"
            if method == "get_config":
                print(json.dumps({"jsonrpc": "2.0", "result": instances.get(instance_id, {}), "id": req_id, "auth": req_auth}), flush=True)
            elif method == "get_info":
                print(json.dumps({"jsonrpc": "2.0", "result": {"id": "reconf_test", "display_name": "Reconf", "version": "1.0.0", "category": "metadata", "author": "test", "description": "test"}, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "reconf_test").await?;

        // set_config replaces the instance's config input in the process.
        let new_api_key = serde_json::json!({ "api_key": "new-key-123" });
        let result = host.set_config(new_api_key.clone()).await?;
        anyhow::ensure!(result == json!(true), "set_config must return true, got {result}");
        // The process now holds the new config as its input.
        let stored = host.call("get_config", None).await?;
        anyhow::ensure!(
            stored == new_api_key,
            "set_config must replace the process-side config, got {stored}"
        );

        Ok(())
    })
    .await
}

/// Verifies that the PolicyPlugin wrapper stores and returns all host-managed
/// fields (priority / enabled / refresh_interval) — the same contract internal
/// plugins get from the wrapper.
#[tokio::test]
async fn test_policy_plugin_host_field_parity() -> Result<()> {
    use crate::plugins::policy::PolicyPlugin;

    struct Neutral {
        enabled: std::sync::atomic::AtomicBool,
        priority: std::sync::Mutex<i32>,
        interval: std::sync::Mutex<Option<u64>>,
    }

    #[async_trait::async_trait]
    impl super::PluginInstance for Neutral {
        fn instance_id(&self) -> &str {
            "neutral"
        }
        fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
            plugin_sdk::traits::PluginTypeInfo {
                display_name: "Neutral".to_string(),
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
            *self.priority.lock().unwrap()
        }
        fn is_enabled(&self) -> bool {
            self.enabled.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn refresh_interval(&self) -> Option<u64> {
            *self.interval.lock().unwrap()
        }
        fn set_priority(&self, p: i32) {
            *self.priority.lock().unwrap() = p;
        }
        fn set_enabled(&self, e: bool) {
            self.enabled.store(e, std::sync::atomic::Ordering::SeqCst);
        }
        fn set_refresh_interval(&self, m: Option<u64>) {
            *self.interval.lock().unwrap() = m;
        }
        async fn call(&self, _: &str, _: Option<serde_json::Value>) -> Result<serde_json::Value> {
            Ok(serde_json::json!(true))
        }
    }

    let inner: Arc<dyn PluginInstance> = Arc::new(Neutral {
        enabled: std::sync::atomic::AtomicBool::new(true),
        priority: std::sync::Mutex::new(0),
        interval: std::sync::Mutex::new(None),
    });
    let host = Arc::new(PolicyPlugin::new(inner, None, "test.test_plugin"));

    // Snapshot from the inner at construction.
    anyhow::ensure!(host.is_enabled());
    anyhow::ensure!(host.priority() == 0);
    anyhow::ensure!(host.refresh_interval().is_none());

    // Host-field setters stick on the wrapper even when the inner supports them.
    host.set_enabled(false);
    anyhow::ensure!(!host.is_enabled());
    host.set_enabled(true);
    anyhow::ensure!(host.is_enabled());

    host.set_priority(42);
    anyhow::ensure!(host.priority() == 42);

    host.set_refresh_interval(Some(30));
    anyhow::ensure!(host.refresh_interval() == Some(30));
    host.set_refresh_interval(None);
    anyhow::ensure!(host.refresh_interval().is_none());

    Ok(())
}

/// Tests that the manager's host-field sync applies enabled / priority /
/// refresh_interval from a config onto a plugin (the mechanism `apply_config`
/// uses after a successful external reconfigure and at internal build time).
#[tokio::test]
async fn test_sync_host_fields_applies_config() -> Result<()> {
    use crate::plugins::manager::PluginManager;

    struct Mock {
        enabled: std::sync::atomic::AtomicBool,
        priority: std::sync::Mutex<i32>,
        interval: std::sync::Mutex<Option<u64>>,
    }

    #[async_trait::async_trait]
    impl super::PluginInstance for Mock {
        fn instance_id(&self) -> &str {
            "mock_id"
        }
        fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
            plugin_sdk::traits::PluginTypeInfo {
                display_name: "Mock".to_string(),
                version: "1.0.0".to_string(),
                author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
                description: String::new(),
                capabilities: vec![Capability::Polling, Capability::AutomaticSearch],
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
            *self.priority.lock().unwrap()
        }
        fn is_enabled(&self) -> bool {
            self.enabled.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn refresh_interval(&self) -> Option<u64> {
            *self.interval.lock().unwrap()
        }
        fn set_priority(&self, p: i32) {
            *self.priority.lock().unwrap() = p;
        }
        fn set_enabled(&self, e: bool) {
            self.enabled.store(e, std::sync::atomic::Ordering::SeqCst);
        }
        fn set_refresh_interval(&self, m: Option<u64>) {
            *self.interval.lock().unwrap() = m;
        }
        async fn call(&self, _: &str, _: Option<serde_json::Value>) -> Result<serde_json::Value> {
            Ok(serde_json::json!(true))
        }
    }

    let plugin: Arc<dyn PluginInstance> = Arc::new(Mock {
        enabled: std::sync::atomic::AtomicBool::new(true),
        priority: std::sync::Mutex::new(0),
        interval: std::sync::Mutex::new(None),
    });

    let config = serde_json::json!({
        "enabled": false, "priority": 99,
        "enable_polling": true, "enable_manual_search": false,
        "enable_automatic_search": true, "refresh_interval": 60,
    });
    PluginManager::sync_host_fields(&plugin, "mock_id", &config);

    anyhow::ensure!(!plugin.is_enabled());
    anyhow::ensure!(plugin.priority() == 99);
    anyhow::ensure!(plugin.refresh_interval() == Some(60));
    // Capability toggles are NOT mutated by host-field sync (static info).
    anyhow::ensure!(
        plugin
            .plugin_info()
            .capabilities
            .contains(&Capability::Polling)
    );
    anyhow::ensure!(
        plugin
            .plugin_info()
            .capabilities
            .contains(&Capability::AutomaticSearch)
    );

    Ok(())
}

// External Plugin RPC Round-trip Tests
// Each test uses a Python subprocess running inline logic via `init_python_script`
// to verify that PluginTypeHost correctly serializes requests, dispatches them to the
// subprocess, and deserializes the JSON-RPC responses back into Rust types.

#[tokio::test]
async fn test_python_plugin_search() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "search":
                result = {"entries": [{"title": "Test S01E01", "source": "pytest", "size": 500000000, "seeders": 100, "leechers": 5, "link": "https://example.com/t", "magnet": "magnet:?xt=urn:btih:abc", "guid": "g1", "published": "2025-01-15T14:30:00Z", "info_hash": "abc123"}], "queries": ["Test"]}
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_search").await?;

        let res = host.call("search", Some(json!({"query": "Test"}))).await?;

        let response: plugin_sdk::query::SearchResponse<Vec<MediaEntry>> =
            serde_json::from_value(res)?;
        anyhow::ensure!(
            response.queries == vec!["Test".to_string()],
            "Expected the plugin's query to be reported, got {:?}",
            response.queries
        );
        let entries = response.entries;
        anyhow::ensure!(entries.len() == 1, "Expected 1 entry, got {}", entries.len());

        let entry = &entries[0];
        anyhow::ensure!(entry.title == "Test S01E01", "Expected 'Test S01E01', got '{}'", entry.title);
        anyhow::ensure!(entry.source == "pytest", "Expected source 'pytest', got '{}'", entry.source);
        anyhow::ensure!(entry.size == Some(500000000), "Expected size 500000000, got {:?}", entry.size);
        anyhow::ensure!(entry.seeders == Some(100), "Expected seeders 100, got {:?}", entry.seeders);
        anyhow::ensure!(entry.leechers == Some(5), "Expected leechers 5, got {:?}", entry.leechers);
        anyhow::ensure!(entry.link == Some("https://example.com/t".into()), "Unexpected link");
        anyhow::ensure!(entry.guid == Some("g1".into()), "Unexpected guid");
        anyhow::ensure!(entry.download_id == Some("abc123".into()), "Unexpected download_id");
        anyhow::ensure!(entry.published.is_some(), "Expected published date");

        // Verify that validation passes for valid entries
        entry.validate().map_err(|errors| {
            anyhow::anyhow!("Validation failed: {:?}", errors)
        })?;

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_fetch_entries() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "fetch_entries":
                result = [{"title": "Fetched Entry", "source": "pytest", "size": 123456, "seeders": 42, "leechers": 7, "link": "https://example.com/f", "magnet": "magnet:?xt=urn:btih:def", "guid": "g2", "published": "2025-02-10T08:15:00Z", "info_hash": "def456"}]
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": [], "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_fetch_entries").await?;

        // Test non-empty response
        let res = host.call("fetch_entries", Some(json!({"feed_url": "https://example.com/feed"}))).await?;
        let entries: Vec<MediaEntry> = serde_json::from_value(res)?;
        anyhow::ensure!(entries.len() == 1, "Expected 1 entry, got {}", entries.len());
        anyhow::ensure!(entries[0].title == "Fetched Entry", "Unexpected title");
        anyhow::ensure!(entries[0].source == "pytest", "Unexpected source");

        // Test empty vec response via unknown method
        let res_empty = host.call("unknown_method", Some(json!({}))).await?;
        let empty_entries: Vec<MediaEntry> = serde_json::from_value(res_empty)?;
        anyhow::ensure!(empty_entries.is_empty(), "Expected empty vec, got {} entries", empty_entries.len());

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_auto_search() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "auto_search":
                # Echo back the params so we can verify they arrived correctly
                result = {"entries": [{"title": "Auto Found S01E01", "source": "pytest", "size": 100000000, "seeders": 200, "leechers": 10, "link": "https://example.com/auto", "magnet": "magnet:?xt=urn:btih:auto123", "guid": "g3", "published": "2025-03-01T12:00:00Z", "info_hash": "auto123"}], "queries": ["Test Series S01E01"]}
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": [], "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_auto_search").await?;

        let res = host.call(
            "auto_search",
            Some(json!({
                "series_title": "Test Series",
                "season": 1,
                "episode": 1,
                "query": "Test Series S01E01"
            })),
        )
        .await?;

        let response: plugin_sdk::query::SearchResponse<Vec<MediaEntry>> =
            serde_json::from_value(res)?;
        anyhow::ensure!(
            response.queries == vec!["Test Series S01E01".to_string()],
            "Expected the plugin's query to be reported, got {:?}",
            response.queries
        );
        let entries = response.entries;
        anyhow::ensure!(entries.len() == 1, "Expected 1 entry, got {}", entries.len());
        anyhow::ensure!(entries[0].title == "Auto Found S01E01", "Unexpected title: {}", entries[0].title);
        anyhow::ensure!(entries[0].source == "pytest", "Unexpected source");
        anyhow::ensure!(entries[0].seeders == Some(200), "Unexpected seeders");

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_fetch_series_metadata() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "fetch_series_metadata":
                result = {
                    "episodes_and_seasons": {
                        "episodes": [
                            {
                                "unique_id": "ep1",
                                "season": 1,
                                "episode": 1,
                                "title": "Ep1",
                                "description": None,
                                "runtime": 45,
                                "image_url": None,
                                "meta_date": None
                            }
                        ],
                        "seasons": [
                            {"season": 1, "episode_count": 24}
                        ]
                    },
                    "series_info": {
                        "name": "Test",
                        "overview": "A test",
                        "original_country": None,
                        "aliases": {}
                    }
                }
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_fetch_metadata").await?;

        use crate::plugins::metadata::{SeriesMetadata, SeriesMetadataInfo};

        let res = host.call("fetch_series_metadata", Some(json!({"id": "test_id"}))).await?;

        // Deserialize episodes_and_seasons (SeriesMetadata)
        let ep_and_seasons: SeriesMetadata = serde_json::from_value(res["episodes_and_seasons"].clone())?;
        anyhow::ensure!(ep_and_seasons.episodes.len() == 1, "Expected 1 episode, got {}", ep_and_seasons.episodes.len());
        anyhow::ensure!(ep_and_seasons.episodes[0].unique_id == "ep1", "Unexpected unique_id");
        anyhow::ensure!(ep_and_seasons.episodes[0].season == 1, "Unexpected season");
        anyhow::ensure!(ep_and_seasons.episodes[0].episode == 1, "Unexpected episode");
        anyhow::ensure!(ep_and_seasons.episodes[0].title == "Ep1", "Unexpected title");
        anyhow::ensure!(ep_and_seasons.episodes[0].runtime == Some(45), "Unexpected runtime");
        anyhow::ensure!(ep_and_seasons.episodes[0].description.is_none(), "Expected None description");
        anyhow::ensure!(ep_and_seasons.episodes[0].meta_date.is_none(), "Expected None meta_date");

        anyhow::ensure!(ep_and_seasons.seasons.len() == 1, "Expected 1 season, got {}", ep_and_seasons.seasons.len());
        anyhow::ensure!(ep_and_seasons.seasons[0].season == 1, "Unexpected season number");
        anyhow::ensure!(ep_and_seasons.seasons[0].episode_count == 24, "Unexpected episode_count");

        // Deserialize series_info
        let series_info: SeriesMetadataInfo = serde_json::from_value(res["series_info"].clone())?;
        anyhow::ensure!(series_info.name == "Test", "Unexpected name");
        anyhow::ensure!(series_info.overview == Some("A test".into()), "Unexpected overview");
        anyhow::ensure!(series_info.original_country.is_none(), "Expected None original_country");
        anyhow::ensure!(series_info.aliases.is_empty(), "Expected empty aliases");

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_fetch_series_info() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "fetch_series_info":
                result = {"name": "Test Series", "overview": "An overview", "original_country": "usa", "aliases": {"eng": ["TS"]}}
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_fetch_series_info").await?;

        use crate::plugins::metadata::SeriesMetadataInfo;

        let res = host.call("fetch_series_info", Some(json!({"id": "test"}))).await?;
        let info: SeriesMetadataInfo = serde_json::from_value(res)?;
        anyhow::ensure!(info.name == "Test Series", "Unexpected name");
        anyhow::ensure!(info.overview == Some("An overview".into()), "Unexpected overview");
        anyhow::ensure!(info.original_country == Some("usa".into()), "Unexpected country");
        anyhow::ensure!(info.aliases.contains_key("eng"), "Expected eng aliases");
        anyhow::ensure!(info.aliases["eng"] == vec!["TS"], "Unexpected aliases");

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_fetch_series_aliases() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "fetch_series_aliases":
                result = ["Alias1", "Alias2"]
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": [], "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_fetch_aliases").await?;

        let res = host
            .call("fetch_series_aliases", Some(json!({"id": "test"})))
            .await?;
        let aliases: Vec<String> = serde_json::from_value(res)?;
        anyhow::ensure!(
            aliases == vec!["Alias1", "Alias2"],
            "Unexpected aliases: {:?}",
            aliases
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_get_updated_series() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "get_updated_series":
                result = ["id1", "id2"]
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": [], "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_get_updated").await?;

        let res = host
            .call("get_updated_series", Some(json!({"since": "2025-01-01"})))
            .await?;
        let ids: Vec<String> = serde_json::from_value(res)?;
        anyhow::ensure!(ids == vec!["id1", "id2"], "Unexpected ids: {:?}", ids);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_add_download() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "add_download":
                result = None
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_add_dl").await?;

        let res = host.call(
            "add_download",
            Some(json!({
                "url": "magnet:?xt=urn:btih:abc",
                "category": "tv",
                "tag": "test",
                "title": "Test"
            })),
        )
        .await?;

        anyhow::ensure!(res.is_null(), "Expected null result, got: {:?}", res);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_get_download_progress() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "get_download_progress":
                result = 0.75
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": 0.0, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_dl_progress").await?;

        let res = host
            .call("get_download_progress", Some(json!({"hash": "abc"})))
            .await?;
        let progress: f64 = serde_json::from_value(res)?;
        // Use approximate comparison for floating point
        anyhow::ensure!(
            (progress - 0.75).abs() < 0.001,
            "Expected progress ~0.75, got {}",
            progress
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_get_download_status() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "get_download_status":
                result = "downloading"
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": "unknown", "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_dl_status").await?;

        let res = host
            .call("get_download_status", Some(json!({"hash": "abc"})))
            .await?;
        let status: String = serde_json::from_value(res)?;
        anyhow::ensure!(
            status == "downloading",
            "Expected 'downloading', got '{}'",
            status
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_get_completed_downloads() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "get_completed_downloads":
                result = ["hash1", "hash2"]
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": [], "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_completed_dl").await?;

        let res = host
            .call("get_completed_downloads", Some(json!({})))
            .await?;
        let hashes: Vec<String> = serde_json::from_value(res)?;
        anyhow::ensure!(
            hashes == vec!["hash1", "hash2"],
            "Unexpected hashes: {:?}",
            hashes
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_notify() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "notify":
                result = None
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_notify").await?;

        let res = host
            .call(
                "notify",
                Some(json!({
                    "event": "test_event",
                    "context": {"key": "value"}
                })),
            )
            .await?;

        anyhow::ensure!(res.is_null(), "Expected null result, got: {:?}", res);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_health_check() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "health_check":
                result = "ok"
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": "unknown", "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_health").await?;

        let res = host.call("health_check", None).await?;
        let status: String = serde_json::from_value(res)?;
        anyhow::ensure!(status == "ok", "Expected 'ok', got '{}'", status);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_method_not_found() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            # Always return error for any method
            print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_unknown_method").await?;

        let res = host.call("nonexistent_method", Some(json!({}))).await;
        anyhow::ensure!(res.is_err(), "Expected error for unknown method, got Ok: {:?}", res);

        let err_str = format!("{:#}", res.unwrap_err());
        anyhow::ensure!(
            err_str.contains("-32601") || err_str.contains("Method not found"),
            "Error should contain method-not-found code/message, got: {}",
            err_str
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_malformed_json_response() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "broken_method":
                # Send non-JSON garbage — the reader loop will skip it
                print("this is not valid json", flush=True)
                # Then send a valid response so the next RPC can succeed
                import time
                time.sleep(0.1)
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": "ok", "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_malformed_json").await?;

        // Call the broken method — should return an error (not panic/crash the host)
        let res = host.call("broken_method", Some(json!({}))).await;
        // The garbage line is skipped by the reader; the error response for the
        // same req_id arrives after. It should be an error.
        anyhow::ensure!(res.is_err(), "Expected error from garbage-response method");

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_missing_required_field() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "search":
                # Return an entry without the required 'title' field inside the envelope
                result = {"entries": [{"source": "pytest", "size": 100}], "queries": ["Test"]}
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "result": [], "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_missing_field").await?;

        let res = host.call("search", Some(json!({"query": "Test"}))).await?;
        // MediaEntry.title is not #[serde(default)], so missing "title" in the
        // JSON causes serde_json::from_value to fail. This is desirable — it means
        // malformed entries are caught at the deserialization boundary rather than
        // silently entering the pipeline with empty titles.
        let entries_result: Result<plugin_sdk::query::SearchResponse<Vec<MediaEntry>>, _> =
            serde_json::from_value(res);
        anyhow::ensure!(
            entries_result.is_err(),
            "Expected deserialization to fail for missing 'title' field"
        );
        let err_msg = format!("{}", entries_result.unwrap_err());
        anyhow::ensure!(
            err_msg.contains("title"),
            "Error should mention missing 'title' field, got: {}",
            err_msg
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_plugin_info_roundtrip() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "get_info":
                result = {"display_name": "External Plugin", "version": "2.0.0", "author": "ext_dev", "description": "An external plugin", "capabilities": ["metadata"], "supported_protocols": None, "series_identifier_label": "ID", "series_identifier_placeholder": "e.g. 123", "rate_limit": None, "supports_test": True, "plugin_id": None}
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "ext_plugin").await?;

        // 1. RPC path: call get_info over JSON-RPC
        let res = host.call("get_info", Some(json!({}))).await?;
        anyhow::ensure!(
            res["display_name"] == json!("External Plugin"),
            "Unexpected display_name via RPC"
        );
        anyhow::ensure!(
            res["version"] == json!("2.0.0"),
            "Unexpected version via RPC"
        );

        // 2. Sync path: plugin_info() before set_cached_info returns defaults
        let info = host.plugin_info();
        // Before set_cached_info, plugin_info() returns fallback values from the host fields
        anyhow::ensure!(
            info.display_name == "ext_plugin" || info.display_name == "External Plugin",
            "Unexpected display_name: {}",
            info.display_name
        );

        // 3. set_cached_info with new data
        let new_info = plugin_sdk::traits::PluginTypeInfo {
            display_name: "Updated Plugin".to_string(),
            version: "3.0.0".to_string(),
            author: "ext_dev".to_string(),
            description: "Updated description".to_string(),
            capabilities: vec![jumbie_shared::plugin::Capability::MetadataProvider],
            supported_protocols: None,
            series_identifier_label: Some("NewID".to_string()),
            series_identifier_placeholder: Some("e.g. 456".to_string()),
            rate_limit: None,
            supports_test: true,
        };
        host.type_host().set_cached_info(new_info.clone());

        // 4. Verify plugin_info() returns the updated data
        let updated_info = host.plugin_info();
        anyhow::ensure!(
            updated_info.display_name == "Updated Plugin",
            "Expected 'Updated Plugin', got '{}'",
            updated_info.display_name
        );
        anyhow::ensure!(
            updated_info.version == "3.0.0",
            "Expected version 3.0.0, got '{}'",
            updated_info.version
        );
        anyhow::ensure!(
            updated_info.author == "ext_dev",
            "Unexpected author"
        );
        anyhow::ensure!(
            updated_info.description == "Updated description",
            "Unexpected description"
        );
        anyhow::ensure!(
            updated_info
                .capabilities
                .contains(&jumbie_shared::plugin::Capability::MetadataProvider),
            "Should have MetadataProvider capability"
        );
        anyhow::ensure!(
            updated_info.series_identifier_label == Some("NewID".to_string()),
            "Unexpected label"
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_python_plugin_concurrent_mixed_calls() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "search":
                result = {"entries": [{"title": "Concurrent S01E01", "source": "pytest", "size": 100, "seeders": 10, "leechers": 1, "link": "https://example.com/c", "magnet": "magnet:?xt=urn:btih:con", "guid": "gc1", "published": "2025-04-01T00:00:00Z", "info_hash": "con123"}], "queries": ["test"]}
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "fetch_entries":
                result = []
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "get_info":
                result = {"display_name": "Concurrent", "version": "1.0.0", "author": "test", "description": "", "capabilities": ["feed"], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "supports_test": False}
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "health_check":
                result = "ok"
                print(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "test_concurrent_mixed").await?;

        // Fire off concurrent calls to different methods using tokio::join!
        // which runs all futures concurrently on the same task.
        let h_search = host.clone();
        let h_fetch = host.clone();
        let h_info = host.clone();
        let h_health = host.clone();

        let search_fut = h_search.call("search", Some(json!({"query": "test"})));
        let fetch_fut = h_fetch.call("fetch_entries", Some(json!({})));
        let info_fut = h_info.call("get_info", Some(json!({})));
        let health_fut = h_health.call("health_check", None);

        let (search_res, fetch_res, info_res, health_res) =
            tokio::join!(search_fut, fetch_fut, info_fut, health_fut);

        // Verify search returned valid entries
        let search_val = search_res?;
        let search_response: plugin_sdk::query::SearchResponse<Vec<MediaEntry>> =
            serde_json::from_value(search_val)?;
        let entries = search_response.entries;
        anyhow::ensure!(entries.len() == 1, "Search expected 1 entry, got {}", entries.len());
        anyhow::ensure!(
            entries[0].title == "Concurrent S01E01",
            "Unexpected search title: {}",
            entries[0].title
        );

        // Verify fetch_entries returned empty
        let fetch_val = fetch_res?;
        let fetched: Vec<MediaEntry> = serde_json::from_value(fetch_val)?;
        anyhow::ensure!(fetched.is_empty(), "Expected empty fetch entries");

        // Verify get_info returned display_name
        let info_val = info_res?;
        anyhow::ensure!(
            info_val["display_name"] == json!("Concurrent"),
            "Unexpected display_name"
        );

        // Verify health_check returned ok
        let health_val = health_res?;
        let health_status: String = serde_json::from_value(health_val)?;
        anyhow::ensure!(health_status == "ok", "Expected 'ok', got '{}'", health_status);

        Ok(())
    })
    .await
}

/// ONE process serves ALL instances of a plugin type: two `InstanceHandle`s
/// must share the same `PluginTypeHost` (Arc identity), and requests must
/// route to the right instance by `instance_id` (each sees its own config).
#[tokio::test]
async fn test_one_process_serves_multiple_instances() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "get_info":
                print(json.dumps({"jsonrpc": "2.0", "result": {"display_name": "Multi", "version": "1.0.0", "author": "test", "description": "d", "capabilities": [], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "plugin_id": None, "instance_id": None, "supports_test": False}, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "get_name":
                print(json.dumps({"jsonrpc": "2.0", "result": instances.get(instance_id, {}).get("name", ""), "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = PluginTypeHost::spawn(
            "test.multi",
            "Multi",
            &script.path,
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;
        host.set_config("inst-a", json!({ "name": "Alpha" }))
            .await?;
        host.set_config("inst-b", json!({ "name": "Beta" }))
            .await?;

        let a = Arc::new(InstanceHandle::new("inst-a".to_string(), host.clone()));
        let b = Arc::new(InstanceHandle::new("inst-b".to_string(), host));

        // One process per TYPE — both handles point at the SAME type host.
        anyhow::ensure!(Arc::ptr_eq(a.type_host(), b.type_host()));

        // Per-instance routing: each instance sees its own config.
        let res_a = a.call("get_name", None).await?;
        let res_b = b.call("get_name", None).await?;
        anyhow::ensure!(res_a == json!("Alpha"), "got {res_a}");
        anyhow::ensure!(res_b == json!("Beta"), "got {res_b}");

        Ok(())
    })
    .await
}

/// The unbounded request queue must never drop calls to a slow instance: a
/// handler that sleeps per request still completes every concurrent call (each
/// waits its turn with the 30s per-call timeout).
#[tokio::test]
async fn test_unbounded_queue_never_drops_requests() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "echo":
                time.sleep(0.4)
                print(json.dumps({"jsonrpc": "2.0", "result": params.get("msg"), "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "slow_inst").await?;

        // 10 concurrent calls to a 0.4s-per-request handler: a queue that
        // dropped requests (or a bounded queue that refused them) would leave
        // some calls unanswered. Every one must come back.
        let mut handles = Vec::new();
        for i in 0..10 {
            let h = host.clone();
            handles.push(tokio::spawn(async move {
                h.call("echo", Some(json!({ "msg": i }))).await
            }));
        }
        let results = futures::future::join_all(handles).await;
        for (i, res) in results.into_iter().enumerate() {
            let r = res
                .map_err(|e| anyhow::anyhow!("Task join failed: {}", e))?
                .map_err(|e| anyhow::anyhow!("RPC call failed: {}", e))?;
            anyhow::ensure!(
                r == json!(i),
                "call {} expected {}, got {}",
                i,
                i,
                r
            );
        }

        Ok(())
    })
    .await
}

/// After a crash, the lifecycle task respawns the process and REPLAYS
/// `initialize` for every live instance. A request queued during the respawn
/// window must drain into the fresh process and observe the replayed config.
#[tokio::test]
async fn test_crash_replays_instance_configs() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "get_info":
                print(json.dumps({"jsonrpc": "2.0", "result": {"display_name": "Crashy", "version": "1.0.0", "author": "test", "description": "d", "capabilities": [], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "plugin_id": None, "instance_id": None, "supports_test": False}, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "crash":
                sys.exit(1)
            elif method == "get_name":
                print(json.dumps({"jsonrpc": "2.0", "result": instances.get(instance_id, {}).get("name", ""), "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = PluginTypeHost::spawn(
            "test.crashy",
            "Crashy",
            &script.path,
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;
        host.set_config("inst-1", json!({ "name": "Replay" }))
            .await?;
        let handle = Arc::new(InstanceHandle::new("inst-1".to_string(), host));
        anyhow::ensure!(handle.call("get_name", None).await? == json!("Replay"));

        // Crash it — the call itself may error (process exits mid-response).
        let _ = handle.call("crash", None).await;

        // This call is sent during the respawn window: it must queue (never be
        // dropped), then drain into the fresh process AFTER the replay
        // re-initializes the instance with its cached config.
        let queued = handle.call("get_name", None).await?;
        anyhow::ensure!(
            queued == json!("Replay"),
            "instance config must be replayed after a crash; got {queued}"
        );

        Ok(())
    })
    .await
}

/// The `hello` handshake must reject a process speaking an OLD protocol
/// version — fast, before any request is served.
#[tokio::test]
async fn test_hello_version_mismatch_fails_fast() -> Result<()> {
    run_python_test(|| async {
        let _guard = PYTHON_TEST_MUTEX.lock().await;
        // Standalone script (not the template): hello reports version 1.
        let script_content = r#"#!/usr/bin/env python3
import sys, json

def main():
    for line in sys.stdin:
        req = json.loads(line)
        req_id = req.get("id")
        method = req.get("method")
        auth = req.get("auth")
        if method == "hello":
            print(json.dumps({"jsonrpc": "2.0", "result": {"protocol_version": 1}, "id": req_id, "auth": auth}), flush=True)
        elif method == "get_info":
            print(json.dumps({"jsonrpc": "2.0", "result": {"display_name": "Old", "version": "1.0.0", "author": "test", "description": "d", "capabilities": [], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "plugin_id": None, "instance_id": None, "supports_test": False}, "id": req_id, "auth": auth}), flush=True)

if __name__ == "__main__":
    main()
"#;
        let script = TestScript::new(script_content, "py");
        let host = PluginTypeHost::spawn(
            "test.old_version",
            "Old",
            &script.path,
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;

        // The version-mismatched process must fail fast (the handshake rejects
        // it and drains queued requests) — NOT hang for the full 30s timeout.
        let res = tokio::time::timeout(
            Duration::from_secs(10),
            host.set_config("inst-1", json!({})),
        )
        .await;
        anyhow::ensure!(
            res.is_err() || matches!(res, Ok(Err(_))),
            "initialize must fail for a version-mismatched plugin, got {res:?}"
        );

        Ok(())
    })
    .await
}

/// AUTH FAIL-CLOSED, response direction: a process that does not echo the
/// request's auth token is untrusted — the host discards its responses, the
/// handshake times out, and every call fails fast.
#[tokio::test]
async fn test_response_without_auth_echo_is_rejected() -> Result<()> {
    run_python_test(|| async {
        let _guard = PYTHON_TEST_MUTEX.lock().await;
        // Responds correctly EXCEPT it never echoes `auth`.
        let script_content = r#"#!/usr/bin/env python3
import sys, json

def main():
    for line in sys.stdin:
        req = json.loads(line)
        req_id = req.get("id")
        method = req.get("method")
        if method == "hello":
            print(json.dumps({"jsonrpc": "2.0", "result": {"protocol_version": 3}, "id": req_id}), flush=True)
        elif method == "get_info":
            print(json.dumps({"jsonrpc": "2.0", "result": {"display_name": "NoEcho", "version": "1.0.0", "author": "test", "description": "d", "capabilities": [], "supported_protocols": None, "series_identifier_label": None, "series_identifier_placeholder": None, "rate_limit": None, "plugin_id": None, "instance_id": None, "supports_test": False}, "id": req_id}), flush=True)

if __name__ == "__main__":
    main()
"#;
        let script = TestScript::new(script_content, "py");
        let host = PluginTypeHost::spawn(
            "test.no_echo",
            "NoEcho",
            &script.path,
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;

        let res = tokio::time::timeout(
            Duration::from_secs(15),
            host.set_config("inst-1", json!({})),
        )
        .await;
        anyhow::ensure!(
            res.is_err() || matches!(res, Ok(Err(_))),
            "a non-echoing process must be rejected, got {res:?}"
        );

        Ok(())
    })
    .await
}

/// Sandbox — environment isolation: the plugin process inherits ONLY the
/// allowlist (plus the IPC secret). Backend env vars must never leak through.
#[tokio::test]
async fn test_child_env_is_isolated() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "dump_env":
                print(json.dumps({"jsonrpc": "2.0", "result": dict(os.environ), "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "env_inst").await?;
        let env: std::collections::HashMap<String, String> =
            serde_json::from_value(host.call("dump_env", None).await?)?;

        // The IPC secret MUST be present (the plugin needs it to authenticate).
        anyhow::ensure!(
            env.contains_key("JUMBIE_PLUGIN_SECRET"),
            "IPC secret must be in the child env"
        );
        // And EVERYTHING else must be on the explicit allowlist — no backend
        // secrets, tokens, or arbitrary variables leak into the child.
        let allowlist = [
            "JUMBIE_PLUGIN_SECRET",
            "PATH",
            "HOME",
            "TMPDIR",
            "TEMP",
            "TMP",
            "LANG",
            "LC_ALL",
            "__CF_USER_TEXT_ENCODING",
            "SystemRoot",
            "SystemDrive",
            "WINDIR",
            "PATHEXT",
            // Must mirror the spawn-side allowlist in host.rs: these Windows
            // location vars are required for Python 3.14's `pymanager` launcher
            // to find runtimes; without them the child crashes at startup.
            "LOCALAPPDATA",
            "APPDATA",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "COMSPEC",
            "OS",
            "PROGRAMFILES",
            "PROGRAMFILES(X86)",
            "PROGRAMDATA",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ];
        for key in env.keys() {
            // Windows environment variables are case-insensitive and Python's
            // `os.environ` normalises keys to UPPERCASE there (e.g. the
            // allowlist's "SystemRoot" arrives as "SYSTEMROOT"). Compare
            // case-insensitively so the allowlist matches on every platform.
            let allowed = allowlist
                .iter()
                .any(|a| a.eq_ignore_ascii_case(key.as_str()));
            anyhow::ensure!(
                allowed,
                "env var '{key}' leaked into the plugin process — isolation violated"
            );
        }

        Ok(())
    })
    .await
}

/// Sandbox — group kill: a plugin that spawns grandchildren (shell-outs,
/// helpers) must not leave orphans behind when it crashes and respawns.
///
/// Unix-only: process groups don't exist on Windows and `child.kill()` there
/// is TerminateProcess (child only — grandchildren may survive; documented
/// limitation).
///
/// The heartbeat file is used instead of `kill(pid, 0)`: after the group kill
/// SIGKILLs the grandchild it lingers as a ZOMBIE until PID 1 reaps it, and
/// `kill(pid, 0)` reports zombies as "exists". In containerized CI the init
/// may not reap promptly, so a correctly-killed grandchild looks like a
/// survivor. A zombie cannot write files, so "the heartbeat file stopped
/// growing" is a reaping-proof liveness signal.
#[cfg(unix)]
#[tokio::test]
async fn test_plugin_grandchildren_killed_on_restart() -> Result<()> {
    run_python_test(|| async {
        let tmp = std::env::temp_dir().join(format!("grandchild_pid_{}.txt", uuid::Uuid::new_v4()));
        let pid_file = tmp.to_string_lossy().to_string();
        let logic = r#"
            if method == "spawn_child":
                import subprocess
                # A grandchild that appends a heartbeat line every 50ms for
                # 60s. Its liveness is observable through the FILE, so the test
                # can distinguish "killed by the group kill" from "still
                # running" even when the dead process lingers as a zombie.
                p = subprocess.Popen([sys.executable, "-c",
                    "import time,sys; f=open(sys.argv[1],'a'); [ (f.write('x\\n'), f.flush(), time.sleep(0.05)) for _ in range(1200) ]",
                    params.get("pid_file", "")])
                with open(params.get("pid_file", ""), "a") as f:
                    f.write("pid=%s\n" % p.pid)
                print(json.dumps({"jsonrpc": "2.0", "result": True, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "crash":
                sys.exit(1)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "parent_inst").await?;
        if let Err(e) = host
            .call("spawn_child", Some(json!({ "pid_file": pid_file })))
            .await
        {
            let msg = format!("{:#}", e);
            if msg.contains("Resource temporarily unavailable")
                || msg.contains("EAGAIN")
                || msg.contains("errno 11")
            {
                // The environment cannot fork a grandchild (a pids cgroup
                // limit or a low inherited ulimit) — group-kill can't be
                // exercised here. Skip rather than fail; the test still runs
                // on environments that can fork (normal dev machines, CI).
                eprintln!("skipping group-kill test: cannot fork grandchild ({msg})");
                return Ok(());
            }
            return Err(e);
        }

        // The grandchild is writing heartbeats — confirm it is alive.
        let size_after_spawn = std::fs::metadata(&tmp).unwrap().len();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let size_alive = std::fs::metadata(&tmp).unwrap().len();
        anyhow::ensure!(
            size_alive > size_after_spawn,
            "grandchild must be writing heartbeats (file did not grow)"
        );

        // Crash the plugin; the group kill must stop the grandchild's
        // heartbeats (a respawned plugin must not fight a stale orphan).
        let _ = host.call("crash", None).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let size_after_crash = std::fs::metadata(&tmp).unwrap().len();
        let _ = std::fs::remove_file(&tmp);
        anyhow::ensure!(
            size_after_crash == size_alive,
            "grandchild kept writing after the plugin restart ({size_alive} -> {size_after_crash} bytes) — group kill failed"
        );

        Ok(())
    })
    .await
}

/// Sandbox — bounded stderr: a log-flooding plugin must not take the host
/// down; calls keep working and the flood is capped by the stderr reader.
#[tokio::test]
async fn test_stderr_flood_is_bounded() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "flood":
                for i in range(20000):
                    print("flood line %d" % i, file=sys.stderr)
                print(json.dumps({"jsonrpc": "2.0", "result": True, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "echo":
                print(json.dumps({"jsonrpc": "2.0", "result": params.get("msg"), "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "flood_inst").await?;
        host.call("flood", None).await?;
        // The host survived the flood and still serves calls.
        let res = host.call("echo", Some(json!({ "msg": "alive" }))).await?;
        anyhow::ensure!(res == json!("alive"), "host must survive stderr flood");

        Ok(())
    })
    .await
}

/// Sandbox — per-plugin rlimits (Linux): the child's RLIMIT_FSIZE must be the
/// 1 GiB cap, proving `pre_exec` limits apply to the PLUGIN process.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_rlimit_fsize_applied_to_child() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "fsize":
                import re
                text = open("/proc/self/limits").read()
                m = re.search(r"Max file size\s+([0-9]+)", text)
                print(json.dumps({"jsonrpc": "2.0", "result": int(m.group(1)) if m else -1, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "rlimit_inst").await?;
        let limit: i64 = serde_json::from_value(host.call("fsize", None).await?)?;
        anyhow::ensure!(
            limit == 1024 * 1024 * 1024,
            "RLIMIT_FSIZE must be 1 GiB in the child, got {limit}"
        );

        Ok(())
    })
    .await
}

/// Host robustness — a scripted fake that emits GARBAGE on stdout (malformed
/// JSON, responses with unknown ids, wrong auth echoes) before the correct
/// response. The host's reader must discard each and still route the real
/// response — a well-behaved process's noise never breaks calls.
#[tokio::test]
async fn test_host_tolerates_garbage_stdout() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "ping":
                print("this is not json", flush=True)
                print(json.dumps({"jsonrpc": "2.0", "result": "unknown-id", "id": 999999, "auth": req_auth}), flush=True)
                print(json.dumps({"jsonrpc": "2.0", "result": "wrong-echo", "id": req_id, "auth": "forged"}), flush=True)
                print(json.dumps({"jsonrpc": "2.0", "result": "ok", "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "garbage_inst").await?;
        let res = host.call("ping", None).await?;
        anyhow::ensure!(
            res == json!("ok"),
            "host must route the real response past garbage; got {res}"
        );

        // And it keeps working afterwards.
        let res = host.call("ping", None).await?;
        anyhow::ensure!(res == json!("ok"), "second call: got {res}");

        Ok(())
    })
    .await
}

/// Metrics — per-type stats account for real round-trips: successful calls
/// bump `calls`, RPC error responses bump both `calls` and `errors`, and
/// queue depth is a live read (never a guess). This is the in-process data
/// behind `/api/system/plugins-metrics`.
#[tokio::test]
async fn test_type_stats_account_calls_and_errors() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "ok":
                print(json.dumps({"jsonrpc": "2.0", "result": True, "id": req_id, "auth": req_auth}), flush=True)
            elif method == "fail":
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32032, "message": "boom"}, "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "stats_inst").await?;
        let stats = host.type_host().stats();
        // set_config during spawn already routed one call — snapshot the
        // baseline so this test asserts DELTAS, not absolute counters.
        let (calls0, errors0, _restarts0, _depth0) = stats.snapshot();

        anyhow::ensure!(host.call("ok", None).await?.as_bool() == Some(true));
        let (calls1, errors1, ..) = stats.snapshot();
        anyhow::ensure!(
            calls1 == calls0 + 1 && errors1 == errors0,
            "successful call: calls {calls0}->{calls1}, errors {errors0}->{errors1}"
        );

        let res = host.call("fail", None).await;
        anyhow::ensure!(res.is_err(), "plugin error response must surface as Err");
        let (calls2, errors2, ..) = stats.snapshot();
        anyhow::ensure!(
            calls2 == calls1 + 1 && errors2 == errors1 + 1,
            "erroring call: calls {calls1}->{calls2}, errors {errors1}->{errors2}"
        );

        Ok(())
    })
    .await
}

/// P2 — hung-process detection: a plugin that stops reading stdin must be
/// killed and respawned (bounded stdin writes), not wedge the lifecycle task
/// forever (which would also starve health checks).
#[tokio::test]
async fn test_hung_plugin_is_killed_and_respawned() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "hang":
                print(json.dumps({"jsonrpc": "2.0", "result": True, "id": req_id, "auth": req_auth}), flush=True)
                time.sleep(3600)  # never reads stdin again
            elif method == "echo":
                print(json.dumps({"jsonrpc": "2.0", "result": params.get("msg"), "id": req_id, "auth": req_auth}), flush=True)
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "hang_inst").await?;

        // Plugin goes to sleep after this call — it is alive but stops reading
        // stdin, so the OS pipe buffer will fill.
        anyhow::ensure!(host.call("hang", None).await?.as_bool() == Some(true));

        // ~16 KiB per line; >64 KiB total fills the pipe buffer, so the next
        // write blocks until the 5s bound trips and the lifecycle task kills
        // the hung process (in-flight requests fail fast; queued ones survive
        // the respawn and are served by the fresh process).
        let big = json!({ "pad": "x".repeat(16 * 1024) });
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let host = host.clone();
                let big = big.clone();
                tokio::spawn(async move { host.call("echo", Some(big)).await })
            })
            .collect();
        let mut saw_error = false;
        for task in tasks {
            if task.await.unwrap().is_err() {
                saw_error = true;
            }
        }
        anyhow::ensure!(
            saw_error,
            "a hung plugin must surface errors (write timeout), not hang silently"
        );

        // The lifecycle respawns with exponential backoff + replays instances.
        // Poll until the fresh process serves calls again (bounded window).
        let mut recovered = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            match host.call("echo", Some(json!({ "msg": "alive" }))).await {
                Ok(v) if v == json!("alive") => {
                    recovered = true;
                    break;
                }
                _ => {}
            }
        }
        anyhow::ensure!(
            recovered,
            "plugin must recover after hung-process respawn (replay + queue drain)"
        );

        Ok(())
    })
    .await
}

/// Template robustness — a raised exception inside a plugin method must
/// surface as an IMMEDIATE error carrying the exception message, not a 30s
/// timeout. Regression: the template's except block used to respond with
/// `id: None` and no auth echo, which the host's reader drops, so any
/// exception in a scripted plugin looked like a hung process.
#[tokio::test]
async fn test_plugin_exception_surfaces_immediately() -> Result<()> {
    run_python_test(|| async {
        let logic = r#"
            if method == "boom":
                raise ValueError("kaboom")
            else:
                print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": req_id, "auth": req_auth}), flush=True)
        "#;

        python_setup!(logic, script);

        let host = spawn_test_instance(&script, "boom_inst").await?;
        let started = std::time::Instant::now();
        let res = host.call("boom", None).await;
        let elapsed = started.elapsed();

        anyhow::ensure!(res.is_err(), "a raised exception must surface as an error");
        anyhow::ensure!(
            elapsed < Duration::from_secs(10),
            "must fail fast, took {}ms — a dropped response would time out at 30s",
            elapsed.as_millis()
        );
        let msg = format!("{:#}", res.unwrap_err());
        anyhow::ensure!(
            msg.contains("kaboom"),
            "error must carry the exception message, got: {msg}"
        );

        Ok(())
    })
    .await
}
