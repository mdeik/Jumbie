//! Test-only plugin fixture: a REAL Rust SDK plugin process, used by the host
//! e2e tests (`backend/tests/rust_plugin_e2e.rs`) to prove the full host ↔
//! SDK-plugin round trip (handshake, auth, set_config, method routing).
//!
//! The fixture is a plain `PluginHandler`: config is INPUT, the handler echoes
//! it back, and any derived state would live in the per-instance cache.

use anyhow::Result;
use async_trait::async_trait;
use plugin_sdk::{CallContext, PluginError, PluginHandler, PluginServer, traits::PluginTypeInfo};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

/// Counts invocations of the `fail_*` methods so the host e2e tests can prove
/// the failure policy retries exactly as classified (3× transient, 1× the rest).
/// Fresh per spawned process, so tests never share it.
static FAIL_CALLS: AtomicU64 = AtomicU64::new(0);

struct FixtureHandler;

#[async_trait]
impl PluginHandler for FixtureHandler {
    async fn get_info(&self) -> PluginTypeInfo {
        PluginTypeInfo {
            display_name: "Test Rust Fixture".to_string(),
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            description: "in-process Rust SDK plugin for host e2e tests".to_string(),
            capabilities: vec![plugin_sdk::traits::Capability::FeedProvider],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        }
    }

    async fn get_config_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn validate_config(&self, _config: Value) -> Result<(), Vec<String>> {
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }

    async fn handle(&self, ctx: CallContext<'_>) -> Result<Value, anyhow::Error> {
        match ctx.method {
            // Echo the instance's config back — proves the host pushed the
            // right config for THIS instance through the real Rust handler.
            "ping" => Ok(ctx.config.clone()),
            "echo" => Ok(ctx.params),
            // How many `fail_*` calls happened this process — lets a test observe
            // how many attempts the host's failure policy made. Not counted itself.
            "fail_calls" => Ok(json!(FAIL_CALLS.load(Ordering::SeqCst))),
            // Structured failures — each maps to a JSON-RPC code via `PluginError`.
            "fail_auth" => {
                FAIL_CALLS.fetch_add(1, Ordering::SeqCst);
                Err(PluginError::AuthFailed("fixture: credentials rejected".into()).into())
            }
            "fail_rate_limited" => {
                FAIL_CALLS.fetch_add(1, Ordering::SeqCst);
                Err(PluginError::RateLimited {
                    retry_after_secs: 7,
                    message: "fixture: slow down".into(),
                }
                .into())
            }
            "fail_transient" => {
                FAIL_CALLS.fetch_add(1, Ordering::SeqCst);
                Err(PluginError::Transient("fixture: network down".into()).into())
            }
            "fail_permanent" => {
                FAIL_CALLS.fetch_add(1, Ordering::SeqCst);
                Err(PluginError::Permanent("fixture: bad config".into()).into())
            }
            "fail_unsupported" => {
                FAIL_CALLS.fetch_add(1, Ordering::SeqCst);
                Err(PluginError::MethodNotSupported("fixture: not implemented".into()).into())
            }
            _ => Err(PluginError::MethodNotSupported(format!(
                "method '{}' not implemented by fixture",
                ctx.method
            ))
            .into()),
        }
    }
}

#[tokio::main]
async fn main() {
    let handler = FixtureHandler;
    let info = handler.get_info().await;
    let server = PluginServer::new(info, Box::new(handler));
    server.run().await;
}
