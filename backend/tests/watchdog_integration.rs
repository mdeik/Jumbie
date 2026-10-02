mod common;

use anyhow::Result;
use jumbie::plugins::PluginInstance;
use jumbie::plugins::host::{InstanceHandle, PluginTypeHost};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use common::{TestScript, init_python_script};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("jumbie=debug")
        .try_init();
}

/// Spawn a type host + one test instance, mirroring the manager's per-instance
/// registration so the crash/restart path matches production.
async fn spawn_test_instance(
    canonical_id: &str,
    script: &TestScript,
    instance_id: &str,
) -> Result<(Arc<PluginTypeHost>, Arc<InstanceHandle>)> {
    let host = PluginTypeHost::spawn(
        canonical_id,
        "watchdog_test",
        &script.path,
        CancellationToken::new(),
    )
    .await?;
    host.set_config(instance_id, json!({})).await?;
    Ok((
        host.clone(),
        Arc::new(InstanceHandle::new(instance_id.to_string(), host)),
    ))
}

#[tokio::test]
async fn test_plugin_auto_restart_on_crash() -> Result<()> {
    init_tracing();
    let script = match init_python_script() {
        Some(s) => s,
        None => return Ok(()),
    };

    let (_host, instance) = spawn_test_instance("watchdog_test", &script, "inst-1").await?;

    let res = instance.call("echo", Some(json!({"msg": "hello"}))).await?;
    assert_eq!(res, json!("hello"));

    // The call may return an error because the process exits before/during response.
    let _ = instance.call("crash", None).await;

    tokio::time::sleep(Duration::from_secs(3)).await;

    // The lifecycle task respawns the process and replays `initialize` for the
    // live instance, so the same handle keeps working without re-registration.
    let res = instance
        .call("echo", Some(json!({"msg": "back again"})))
        .await?;
    assert_eq!(res, json!("back again"));

    Ok(())
}

#[tokio::test]
async fn test_plugin_auto_restart_on_stdout_close() -> Result<()> {
    init_tracing();
    // A script that closes its stdout but stays alive (or just exits).
    let script_content = r#"#!/usr/bin/env python3
import sys, json, time

instances = {}
JUMBIE_SECRET = __import__("os").environ.get("JUMBIE_PLUGIN_SECRET")

def main():
    while True:
        line = sys.stdin.readline()
        if not line: break
        req = json.loads(line)
        method = req.get("method")
        req_id = req.get("id")
        req_auth = req.get("auth")
        if JUMBIE_SECRET and req_auth != JUMBIE_SECRET:
            print(json.dumps({"jsonrpc": "2.0", "error": {"code": -32001, "message": "Authentication failed"}, "id": req_id, "auth": req_auth}), flush=True)
            continue
        if method == "hello":
            print(json.dumps({"jsonrpc": "2.0", "result": {"protocol_version": 3}, "id": req_id, "auth": req_auth}), flush=True)
        elif method == "set_config":
            instances[req.get("instance_id")] = req.get("params", {})
            print(json.dumps({"jsonrpc": "2.0", "result": True, "id": req_id, "auth": req_auth}), flush=True)
        elif method == "health_check":
            print(json.dumps({"jsonrpc": "2.0", "result": "ok", "id": req_id, "auth": req_auth}), flush=True)
        elif method == "close_stdout":
            sys.stdout.close()
            time.sleep(10) # Stay alive for a bit
            break
        elif method == "echo":
            print(json.dumps({"jsonrpc": "2.0", "result": "ok", "id": req_id, "auth": req_auth}), flush=True)

if __name__ == "__main__":
    main()
"#;
    let script = TestScript::new(script_content);

    let (_host, instance) = spawn_test_instance("stdout_test", &script, "inst-1").await?;

    instance.call("echo", None).await?;

    let _ = instance.call("close_stdout", None).await;

    tokio::time::sleep(Duration::from_secs(3)).await;

    let res = instance.call("echo", None).await?;
    assert_eq!(res, json!("ok"));

    Ok(())
}
