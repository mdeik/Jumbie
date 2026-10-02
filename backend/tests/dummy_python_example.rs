//! Drives the real `examples/plugins/dummy-python/run.py` reference example
//! through `PluginTypeHost`, exercising the full protocol round trip that
//! hand-written Python plugins must implement. Keeps the reference example
//! honest against the spec (a drifted example fails here, not at a user's desk).

use anyhow::Result;
use jumbie::plugins::PluginInstance;
use jumbie::plugins::host::{InstanceHandle, PluginTypeHost};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Path to the reference Python example; `None` when python is unavailable
/// (the tests then skip rather than fail).
fn example_script() -> Option<PathBuf> {
    jumbie_shared::plugin::python_command()?;
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/dummy-python/run.py");
    // The host spawns the script directly on Unix — needs +x (the manager's
    // discovery chmods, but a fresh checkout may not have it).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
    path.exists().then_some(path)
}

#[tokio::test]
async fn dummy_python_example_speaks_the_protocol() -> Result<()> {
    let Some(script) = example_script() else {
        eprintln!("python or example not available — skipping");
        return Ok(());
    };

    let host = PluginTypeHost::spawn(
        "test.dummy_python",
        "Dummy Python",
        &script,
        CancellationToken::new(),
    )
    .await?;

    // Reaching here means the `hello` handshake + auth round-trip passed —
    // a process that fails them is killed at spawn.
    let info = host.get_info().await?;
    assert_eq!(info["display_name"], json!("Dummy Python"));

    // Instance lifecycle: set_config is idempotent create-or-update.
    host.set_config("inst-1", json!({ "api_key": "abcde" }))
        .await?;
    let inst = Arc::new(InstanceHandle::new("inst-1".to_string(), host.clone()));

    // Instance-level custom method routes through the shared process.
    let meta = inst
        .call("fetch_series_metadata", Some(json!({ "id": "1" })))
        .await?;
    assert_eq!(meta["episodes"][0]["title"], json!("Episode 1"));

    // The Test button method works (supports_test: true).
    let res = inst
        .call("test", Some(json!({ "api_key": "abcde" })))
        .await?;
    assert!(
        res.is_string(),
        "test must return a success message, got {res}"
    );

    // Health is type-level and reported in-process.
    assert!(host.is_healthy(), "host must be healthy after handshake");

    Ok(())
}
