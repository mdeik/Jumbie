//! End-to-end: the real Rust SDK plugin binary (`plugin-test-fixture`) driven
//! through `PluginTypeHost` — the full protocol round trip that the python
//! fixtures only approximate.

use anyhow::Result;
use jumbie::plugins::host::{InstanceHandle, PluginTypeHost};
use jumbie::plugins::policy::is_method_not_supported;
use jumbie::plugins::{PluginCallError, PluginInstance, PolicyPlugin};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Cargo builds this fixture binary for integration tests of the same crate.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_plugin-test-fixture"))
}

async fn spawn_fixture(
    instance_id: &str,
    config: serde_json::Value,
) -> Result<(Arc<PluginTypeHost>, Arc<InstanceHandle>)> {
    let host = PluginTypeHost::spawn(
        "test.rust_fixture",
        "Test Rust Fixture",
        &fixture_path(),
        CancellationToken::new(),
    )
    .await?;
    host.set_config(instance_id, config).await?;
    Ok((
        host.clone(),
        Arc::new(InstanceHandle::new(instance_id.to_string(), host)),
    ))
}

#[tokio::test]
async fn rust_plugin_round_trip() -> Result<()> {
    let (_host, inst) = spawn_fixture("inst-a", json!({ "api_key": "k1" })).await?;

    // The handler echoes the instance's config, proving set_config reached the
    // real Rust process and routed by instance_id.
    let res = inst.call("ping", None).await?;
    assert_eq!(res, json!({ "api_key": "k1" }));

    // set_config as input: a new config replaces the old one in the same process.
    _host
        .set_config("inst-a", json!({ "api_key": "k2" }))
        .await?;
    let res = inst.call("ping", None).await?;
    assert_eq!(res, json!({ "api_key": "k2" }));

    let res = inst.call("echo", Some(json!({ "msg": "hi" }))).await?;
    assert_eq!(res, json!({ "msg": "hi" }));

    Ok(())
}

#[tokio::test]
async fn rust_plugin_multi_instance_routing() -> Result<()> {
    let host = PluginTypeHost::spawn(
        "test.rust_fixture",
        "Test Rust Fixture",
        &fixture_path(),
        CancellationToken::new(),
    )
    .await?;
    host.set_config("inst-a", json!({ "name": "A" })).await?;
    host.set_config("inst-b", json!({ "name": "B" })).await?;

    let a = Arc::new(InstanceHandle::new("inst-a".to_string(), host.clone()));
    let b = Arc::new(InstanceHandle::new("inst-b".to_string(), host));

    // ONE process, per-instance config routing.
    assert_eq!(a.call("ping", None).await?, json!({ "name": "A" }));
    assert_eq!(b.call("ping", None).await?, json!({ "name": "B" }));

    a.type_host()
        .set_config("inst-a", json!({ "name": "A2" }))
        .await?;
    assert_eq!(a.call("ping", None).await?, json!({ "name": "A2" }));
    assert_eq!(b.call("ping", None).await?, json!({ "name": "B" }));

    Ok(())
}

#[tokio::test]
async fn rust_plugin_unknown_instance_fails() -> Result<()> {
    let (_host, _inst) = spawn_fixture("inst-a", json!({})).await?;
    let ghost = Arc::new(InstanceHandle::new("ghost".to_string(), _host));
    let res = ghost.call("ping", None).await;
    assert!(res.is_err(), "unknown instance must fail, got {res:?}");
    Ok(())
}

#[tokio::test]
async fn rust_plugin_health_is_type_level() -> Result<()> {
    let (host, _inst) = spawn_fixture("inst-a", json!({})).await?;
    // After the handshake, the host's in-process health is true (no RPC).
    assert!(host.is_healthy(), "host must be healthy after handshake");
    Ok(())
}

/// Wrap a spawned fixture in the production failure policy so classification is
/// exercised end to end: SDK emits a code → the host serializes it → the policy
/// acts on it.
fn policy_over(inst: &Arc<InstanceHandle>) -> Arc<PolicyPlugin> {
    Arc::new(PolicyPlugin::new(inst.clone(), None, "test.rust_fixture"))
}

#[tokio::test]
async fn rust_plugin_structured_errors_surface_their_codes() -> Result<()> {
    let (_host, inst) = spawn_fixture("inst-a", json!({})).await?;

    for (method, expected_code) in [
        ("fail_auth", -32030),
        ("fail_rate_limited", -32029),
        ("fail_transient", -32031),
        ("fail_permanent", -32032),
        ("fail_unsupported", -32601),
    ] {
        let err = inst.call(method, None).await.expect_err("call must fail");
        let payload: serde_json::Value = serde_json::from_str(&err.to_string())
            .unwrap_or_else(|_| panic!("error is not the JSON-RPC payload: {err}"));
        assert_eq!(
            payload["code"],
            json!(expected_code),
            "wrong JSON-RPC code for {method}"
        );
    }

    Ok(())
}

#[tokio::test]
async fn failure_policy_retries_transient_then_cools_down() -> Result<()> {
    let (_host, inst) = spawn_fixture("inst-a", json!({})).await?;
    let policy = policy_over(&inst);

    assert!(policy.call("fail_transient", None).await.is_err());
    assert_eq!(
        inst.call("fail_calls", None).await?,
        json!(3),
        "transient must be retried 3x"
    );

    // Cooldown armed: the next call is short-circuited (no 4th attempt).
    let err = policy.call("fail_transient", None).await.unwrap_err();
    assert!(
        err.to_string().contains("retrying in"),
        "expected cooldown error, got: {err}"
    );
    assert_eq!(
        inst.call("fail_calls", None).await?,
        json!(3),
        "cooldown must not re-call the plugin"
    );

    Ok(())
}

#[tokio::test]
async fn failure_policy_auth_fails_fast_then_cools_down() -> Result<()> {
    let (_host, inst) = spawn_fixture("inst-a", json!({})).await?;
    let policy = policy_over(&inst);

    assert!(policy.call("fail_auth", None).await.is_err());
    assert_eq!(
        inst.call("fail_calls", None).await?,
        json!(1),
        "auth failure must not be retried"
    );

    let err = policy.call("fail_auth", None).await.unwrap_err();
    assert!(
        err.to_string().contains("retrying in"),
        "expected auth cooldown, got: {err}"
    );
    assert_eq!(inst.call("fail_calls", None).await?, json!(1));

    Ok(())
}

#[tokio::test]
async fn failure_policy_permanent_and_unsupported_fail_fast() -> Result<()> {
    let (_host, inst) = spawn_fixture("inst-a", json!({})).await?;
    let policy = policy_over(&inst);

    // Permanent: one attempt, no cooldown — the next call reaches the plugin again.
    assert!(policy.call("fail_permanent", None).await.is_err());
    assert_eq!(
        inst.call("fail_calls", None).await?,
        json!(1),
        "permanent must not be retried"
    );
    assert!(policy.call("fail_permanent", None).await.is_err());
    assert_eq!(
        inst.call("fail_calls", None).await?,
        json!(2),
        "permanent must not cool down"
    );

    // Method-not-supported is recognized so callers can skip it quietly.
    let err = policy.call("fail_unsupported", None).await.unwrap_err();
    assert!(
        is_method_not_supported(&err),
        "expected method-not-supported, got: {err}"
    );

    Ok(())
}

#[tokio::test]
async fn failure_policy_rate_limited_pauses() -> Result<()> {
    let (_host, inst) = spawn_fixture("inst-a", json!({})).await?;
    let policy = policy_over(&inst);

    let err = policy.call("fail_rate_limited", None).await.unwrap_err();
    match err.downcast_ref::<PluginCallError>() {
        Some(PluginCallError::RetryAfter(secs)) => assert_eq!(*secs, 7),
        other => panic!("expected RetryAfter(7), got {other:?}"),
    }
    assert_eq!(
        inst.call("fail_calls", None).await?,
        json!(1),
        "429 must not be retried"
    );

    // Paused: the next call is short-circuited with another RetryAfter.
    let again = policy.call("fail_rate_limited", None).await.unwrap_err();
    assert!(matches!(
        again.downcast_ref::<PluginCallError>(),
        Some(PluginCallError::RetryAfter(_))
    ));
    assert_eq!(
        inst.call("fail_calls", None).await?,
        json!(1),
        "pause must not re-call the plugin"
    );

    Ok(())
}
