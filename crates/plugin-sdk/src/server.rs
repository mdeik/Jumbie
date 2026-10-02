use crate::rpc::{JsonRpcRequest, JsonRpcResponse};
use crate::traits::{CallContext, PluginHandler, PluginTypeInfo};

use std::collections::HashMap;
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Per-instance data inside the shared process: the config (input) plus a
/// derived-state cache that is cleared whenever `set_config` replaces the
/// config. Instances are DATA, not plugin objects — there is exactly one
/// handler per process.
struct InstanceData {
    config: serde_json::Value,
    cache: HashMap<String, serde_json::Value>,
}

/// The multi-instance plugin server.
///
/// One process runs one `PluginServer` (one [`PluginHandler`]) and serves all
/// instances of its plugin type, routed by `JsonRpcRequest::instance_id`:
/// `None` for type-level methods (hello / get_info / get_config_schema /
/// validate_config / health_check), `Some` for that instance's config + cache
/// passed via [`CallContext`].
///
/// `set_config` idempotently creates-or-updates a config row (config is input,
/// there is no `reconfigure`); `shutdown_instance` drops the row + cache.
/// Requests targeting an unknown instance fail with -32001.
pub struct PluginServer {
    /// Type-level metadata (same for every instance).
    type_info: PluginTypeInfo,
    /// Exactly one handler per process.
    handler: Box<dyn PluginHandler>,
    /// Per-instance config rows + derived-state caches.
    instances: HashMap<String, InstanceData>,
    expected_auth: Option<String>,
}

impl PluginServer {
    /// `type_info` answers the type-level `get_info`; `handler` processes every
    /// instance's calls (config + params via [`CallContext`]).
    pub fn new(type_info: PluginTypeInfo, handler: Box<dyn PluginHandler>) -> Self {
        // AUTH IS FAIL-CLOSED: without the secret the server still runs (so
        // errors surface cleanly) but rejects every request.
        let expected_auth = std::env::var("JUMBIE_PLUGIN_SECRET").ok();
        if expected_auth.is_none() {
            tracing::error!(
                "JUMBIE_PLUGIN_SECRET is not set — this plugin server is running unauthenticated and will reject every request. The host always sets this variable."
            );
        }

        Self {
            type_info,
            handler,
            instances: HashMap::new(),
            expected_auth,
        }
    }

    /// Runs the plugin server, reading from stdin and writing to stdout.
    /// This will block until stdin is closed.
    pub async fn run(mut self) {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();

        while let Ok(Some(line)) = reader.next_line().await {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<JsonRpcRequest>(line) {
                Ok(req) => {
                    // Auth check (fail-closed)
                    if !self.auth_ok(req.auth.as_deref()) {
                        let resp = JsonRpcResponse::error(
                            req.id,
                            -32001,
                            "Authentication failed: invalid or missing auth token".to_string(),
                            None,
                        );
                        if let Ok(json) = serde_json::to_string(&resp) {
                            let mut stdout = std::io::stdout();
                            let _ = writeln!(stdout, "{}", json);
                        }
                        continue;
                    }

                    let id = req.id.clone();
                    let response = self.handle_request(req).await;
                    if let Some(resp) = response {
                        if id.is_none() {
                            continue;
                        }

                        if let Ok(json) = serde_json::to_string(&resp) {
                            let mut stdout = std::io::stdout();
                            let _ = writeln!(stdout, "{}", json);
                        }
                    }
                }
                Err(e) => {
                    let resp =
                        JsonRpcResponse::error(None, -32700, format!("Parse error: {}", e), None);
                    if let Ok(json) = serde_json::to_string(&resp) {
                        let mut stdout = std::io::stdout();
                        let _ = writeln!(stdout, "{}", json);
                    }
                }
            }
        }
    }

    async fn handle_request(&mut self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let is_notification = req.id.is_none();
        // Captured before the match so every response echo goes through one place.
        let req_auth = req.auth.clone();

        let response = match req.method.as_str() {
            // Startup handshake: the host requires this response before replaying
            // instances or draining the queue, and fails the type fast on a
            // `PROTOCOL_VERSION` mismatch.
            "hello" => JsonRpcResponse::ok(
                req.id,
                serde_json::json!({ "protocol_version": crate::rpc::PROTOCOL_VERSION }),
            ),
            // Type-level methods (no instance)
            "get_info" => JsonRpcResponse::ok(req.id, self.type_info.clone()),
            "get_config_schema" => {
                JsonRpcResponse::ok(req.id, self.handler.get_config_schema().await)
            }
            "validate_config" => match self.handler.validate_config(req.params).await {
                Ok(_) => JsonRpcResponse::ok(req.id, true),
                Err(errors) => JsonRpcResponse::ok(req.id, serde_json::json!({ "errors": errors })),
            },
            // Instance lifecycle (config is input)
            "set_config" => match req.instance_id {
                Some(instance_id) => {
                    // Replace the config row and clear the cache; the handler
                    // re-derives lazily on the next call.
                    self.instances.insert(
                        instance_id.clone(),
                        InstanceData {
                            config: req.params,
                            cache: HashMap::new(),
                        },
                    );
                    tracing::info!("Applied config to plugin instance '{instance_id}'");
                    JsonRpcResponse::ok(req.id, true)
                }
                None => JsonRpcResponse::error(
                    req.id,
                    -32602,
                    "set_config requires instance_id".to_string(),
                    None,
                ),
            },
            "shutdown_instance" => match req.instance_id {
                Some(instance_id) => {
                    self.instances.remove(&instance_id);
                    tracing::info!("Shut down plugin instance '{instance_id}'");
                    JsonRpcResponse::ok(req.id, true)
                }
                None => JsonRpcResponse::error(
                    req.id,
                    -32602,
                    "shutdown_instance requires instance_id".to_string(),
                    None,
                ),
            },
            // Health is type-level: one probe per process
            "health_check" => match self.handler.health_check().await {
                Ok(()) => JsonRpcResponse::ok(req.id, "ok"),
                Err(e) => JsonRpcResponse::error(
                    req.id,
                    -32001,
                    format!("Health check failed: {e}"),
                    None,
                ),
            },
            // Instance-level custom methods
            _ => {
                let Some(instance_id) = req.instance_id.as_deref() else {
                    return Some(
                        JsonRpcResponse::error(
                            req.id,
                            -32601,
                            "Method not found".to_string(),
                            None,
                        )
                        .with_auth(req_auth),
                    );
                };
                let Some(data) = self.instances.get_mut(instance_id) else {
                    return Some(
                        JsonRpcResponse::error(
                            req.id,
                            -32001,
                            "Unknown instance: not initialized".to_string(),
                            None,
                        )
                        .with_auth(req_auth),
                    );
                };
                // Disjoint borrows: `instances` is mutable while `handler` is shared.
                let InstanceData { config, cache } = data;
                let ctx = CallContext {
                    instance_id,
                    config,
                    method: &req.method,
                    params: req.params,
                    cache,
                };
                match self.handler.handle(ctx).await {
                    Ok(val) => JsonRpcResponse::ok(req.id, val),
                    // A handler can raise a structured `PluginError` to signal a
                    // specific failure kind (auth / rate-limit / transient /
                    // permanent / method-not-supported); map it to the matching
                    // JSON-RPC code so the host classifies external plugins like
                    // internal ones. Any other error stays the generic `-32000`
                    // (the host treats it as transient). `{e:#}` keeps the full
                    // cause chain visible.
                    Err(e) => match e.downcast_ref::<crate::error::PluginError>() {
                        Some(pe) => {
                            JsonRpcResponse::error(req.id, pe.code(), pe.to_string(), pe.data())
                        }
                        None => JsonRpcResponse::error(
                            req.id,
                            -32000,
                            format!("Plugin error: {e:#}"),
                            None,
                        ),
                    },
                }
            }
        };

        // Echo the auth token once, rather than on every match arm.
        let response = response.with_auth(req_auth);

        if is_notification {
            None
        } else {
            Some(response)
        }
    }

    /// AUTH FAIL-CLOSED: a request must carry the secret; if the secret is
    /// missing from the environment, every request is rejected.
    fn auth_ok(&self, req_auth: Option<&str>) -> bool {
        match self.expected_auth.as_deref() {
            Some(expected) => req_auth == Some(expected),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PluginError;
    use crate::rpc::PROTOCOL_VERSION;
    use crate::traits::PluginTypeInfo;
    use async_trait::async_trait;
    use serde_json::Value;

    /// A record of every `handle` invocation (for asserting routing/context).
    #[derive(Debug, Clone, Default)]
    struct CallRecord {
        instance_id: String,
        config: serde_json::Value,
        method: String,
        params: serde_json::Value,
    }

    /// Shared state between the mock handler and the test (no downcasting).
    struct MockState {
        calls: std::sync::Mutex<Vec<CallRecord>>,
        health_ok: std::sync::Mutex<bool>,
        /// When set, `handle` returns this structured error (testing code mapping).
        plugin_error: std::sync::Mutex<Option<PluginError>>,
        /// When set, `handle` returns this unstructured error (testing the -32000
        /// fallback); `take`n so it need not be `Clone`.
        plain_error: std::sync::Mutex<Option<anyhow::Error>>,
    }

    struct MockHandler {
        state: std::sync::Arc<MockState>,
    }

    fn new_mock() -> (MockHandler, std::sync::Arc<MockState>) {
        let state = std::sync::Arc::new(MockState {
            calls: std::sync::Mutex::new(Vec::new()),
            health_ok: std::sync::Mutex::new(true),
            plugin_error: std::sync::Mutex::new(None),
            plain_error: std::sync::Mutex::new(None),
        });
        (
            MockHandler {
                state: state.clone(),
            },
            state,
        )
    }

    fn type_info() -> PluginTypeInfo {
        PluginTypeInfo {
            display_name: "Mock".to_string(),
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            description: "mock handler".to_string(),
            capabilities: vec![crate::traits::Capability::FeedProvider],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        }
    }

    #[async_trait]
    impl PluginHandler for MockHandler {
        async fn get_info(&self) -> PluginTypeInfo {
            type_info()
        }
        async fn get_config_schema(&self) -> Value {
            serde_json::json!({ "type": "object" })
        }
        async fn validate_config(&self, _config: Value) -> Result<(), Vec<String>> {
            Ok(())
        }
        async fn health_check(&self) -> Result<(), anyhow::Error> {
            if *self.state.health_ok.lock().unwrap() {
                Ok(())
            } else {
                anyhow::bail!("mock unhealthy")
            }
        }
        async fn handle(&self, ctx: CallContext<'_>) -> Result<Value, anyhow::Error> {
            self.state.calls.lock().unwrap().push(CallRecord {
                instance_id: ctx.instance_id.to_string(),
                config: ctx.config.clone(),
                method: ctx.method.to_string(),
                params: ctx.params.clone(),
            });
            // Prove the cache is usable: memoize the method result.
            ctx.cache
                .insert("last".to_string(), serde_json::json!(ctx.method));
            if let Some(error) = self.state.plugin_error.lock().unwrap().clone() {
                return Err(error.into());
            }
            if let Some(error) = self.state.plain_error.lock().unwrap().take() {
                return Err(error);
            }
            Ok(ctx.config.clone())
        }
    }

    fn server() -> (PluginServer, std::sync::Arc<MockState>) {
        let (handler, state) = new_mock();
        (
            PluginServer {
                type_info: type_info(),
                handler: Box::new(handler),
                instances: HashMap::new(),
                expected_auth: Some("secret".to_string()),
            },
            state,
        )
    }

    fn req(method: &str, params: Value, instance_id: Option<&str>) -> JsonRpcRequest {
        JsonRpcRequest::new(
            method,
            params,
            Some(serde_json::json!(1)),
            instance_id.map(String::from),
        )
    }

    fn result_of(resp: &JsonRpcResponse) -> &Value {
        match resp {
            JsonRpcResponse::Result(r) => &r.result,
            JsonRpcResponse::Error(e) => panic!("expected result, got error: {:?}", e),
        }
    }

    fn error_code(resp: &JsonRpcResponse) -> i32 {
        match resp {
            JsonRpcResponse::Result(r) => panic!("expected error, got result: {:?}", r),
            JsonRpcResponse::Error(e) => e.error.code,
        }
    }

    #[tokio::test]
    async fn hello_reports_protocol_version() {
        let (mut s, _state) = server();
        let resp = s
            .handle_request(req("hello", Value::Null, None))
            .await
            .unwrap();
        assert_eq!(
            result_of(&resp)["protocol_version"],
            serde_json::json!(PROTOCOL_VERSION)
        );
    }

    #[tokio::test]
    async fn get_info_is_type_level() {
        let (mut s, _state) = server();
        let resp = s
            .handle_request(req("get_info", Value::Null, None))
            .await
            .unwrap();
        assert_eq!(result_of(&resp)["display_name"], serde_json::json!("Mock"));
    }

    #[tokio::test]
    async fn validate_config_delegates_to_handler() {
        let (mut s, _state) = server();
        let resp = s
            .handle_request(req("validate_config", serde_json::json!({}), None))
            .await
            .unwrap();
        assert_eq!(result_of(&resp), &serde_json::json!(true));
    }

    #[tokio::test]
    async fn set_config_is_idempotent_create_and_update() {
        let (mut s, _state) = server();
        // Create.
        let resp = s
            .handle_request(req(
                "set_config",
                serde_json::json!({ "api_key": "a" }),
                Some("inst-1"),
            ))
            .await
            .unwrap();
        assert_eq!(result_of(&resp), &serde_json::json!(true));
        assert_eq!(s.instances.len(), 1);
        assert_eq!(
            s.instances["inst-1"].config["api_key"],
            serde_json::json!("a")
        );

        // Update (same RPC, no plugin logic involved).
        let resp = s
            .handle_request(req(
                "set_config",
                serde_json::json!({ "api_key": "b" }),
                Some("inst-1"),
            ))
            .await
            .unwrap();
        assert_eq!(result_of(&resp), &serde_json::json!(true));
        assert_eq!(
            s.instances["inst-1"].config["api_key"],
            serde_json::json!("b")
        );
    }

    #[tokio::test]
    async fn set_config_clears_derived_cache() {
        let (mut s, _state) = server();
        s.handle_request(req(
            "set_config",
            serde_json::json!({ "k": 1 }),
            Some("inst-1"),
        ))
        .await
        .unwrap();
        // Handler memoizes into the per-instance cache.
        s.handle_request(req("ping", Value::Null, Some("inst-1")))
            .await
            .unwrap();
        assert!(s.instances["inst-1"].cache.contains_key("last"));

        // New config ⇒ cache wiped (derived state re-derives; no invalidation logic).
        s.handle_request(req(
            "set_config",
            serde_json::json!({ "k": 2 }),
            Some("inst-1"),
        ))
        .await
        .unwrap();
        assert!(
            s.instances["inst-1"].cache.is_empty(),
            "set_config must clear the cache"
        );
        assert_eq!(s.instances["inst-1"].config["k"], serde_json::json!(2));
    }

    #[tokio::test]
    async fn shutdown_instance_removes_config_row() {
        let (mut s, _state) = server();
        s.handle_request(req("set_config", Value::Null, Some("inst-1")))
            .await
            .unwrap();
        let resp = s
            .handle_request(req("shutdown_instance", Value::Null, Some("inst-1")))
            .await
            .unwrap();
        assert_eq!(result_of(&resp), &serde_json::json!(true));
        assert!(s.instances.is_empty());
    }

    #[tokio::test]
    async fn custom_method_routes_with_config_and_params() {
        let (mut s, state) = server();
        s.handle_request(req(
            "set_config",
            serde_json::json!({ "url": "x" }),
            Some("inst-1"),
        ))
        .await
        .unwrap();
        let resp = s
            .handle_request(req(
                "search",
                serde_json::json!({ "q": "tv" }),
                Some("inst-1"),
            ))
            .await
            .unwrap();
        assert_eq!(result_of(&resp)["url"], serde_json::json!("x"));

        let record = state.calls.lock().unwrap().pop().unwrap();
        assert_eq!(record.instance_id, "inst-1");
        assert_eq!(record.method, "search");
        assert_eq!(record.config["url"], serde_json::json!("x"));
        assert_eq!(record.params["q"], serde_json::json!("tv"));
    }

    #[tokio::test]
    async fn unknown_instance_returns_32001() {
        let (mut s, _state) = server();
        let resp = s
            .handle_request(req("search", Value::Null, Some("ghost")))
            .await
            .unwrap();
        assert_eq!(error_code(&resp), -32001);
    }

    #[tokio::test]
    async fn unknown_type_level_method_returns_32601() {
        let (mut s, _state) = server();
        let resp = s
            .handle_request(req("bogus", Value::Null, None))
            .await
            .unwrap();
        assert_eq!(error_code(&resp), -32601);
    }

    #[tokio::test]
    async fn structured_plugin_error_maps_to_its_jsonrpc_code() {
        let cases = [
            (PluginError::AuthFailed("bad key".into()), -32030),
            (
                PluginError::RateLimited {
                    retry_after_secs: 7,
                    message: "slow down".into(),
                },
                -32029,
            ),
            (PluginError::Transient("net down".into()), -32031),
            (PluginError::Permanent("bad config".into()), -32032),
            (PluginError::MethodNotSupported("search".into()), -32601),
        ];

        for (error, expected_code) in cases {
            let (mut s, state) = server();
            s.handle_request(req("set_config", serde_json::json!({}), Some("inst-1")))
                .await
                .unwrap();
            *state.plugin_error.lock().unwrap() = Some(error.clone());

            let resp = s
                .handle_request(req("search", Value::Null, Some("inst-1")))
                .await
                .unwrap();

            assert_eq!(error_code(&resp), expected_code, "wrong code for {error:?}");
        }
    }

    #[tokio::test]
    async fn rate_limited_error_carries_retry_after_data() {
        let (mut s, state) = server();
        s.handle_request(req("set_config", serde_json::json!({}), Some("inst-1")))
            .await
            .unwrap();
        *state.plugin_error.lock().unwrap() = Some(PluginError::RateLimited {
            retry_after_secs: 9,
            message: "slow down".into(),
        });

        let resp = s
            .handle_request(req("search", Value::Null, Some("inst-1")))
            .await
            .unwrap();

        match resp {
            JsonRpcResponse::Error(e) => {
                assert_eq!(e.error.code, -32029);
                assert_eq!(e.error.data, Some(serde_json::json!({ "retry_after": 9 })));
                assert_eq!(e.error.message, "slow down");
            }
            JsonRpcResponse::Result(r) => panic!("expected error, got result: {:?}", r),
        }
    }

    #[tokio::test]
    async fn unstructured_handler_error_stays_32000_with_full_chain() {
        let (mut s, state) = server();
        s.handle_request(req("set_config", serde_json::json!({}), Some("inst-1")))
            .await
            .unwrap();
        *state.plain_error.lock().unwrap() =
            Some(anyhow::anyhow!("root cause").context("outer failure"));

        let resp = s
            .handle_request(req("search", Value::Null, Some("inst-1")))
            .await
            .unwrap();

        match resp {
            JsonRpcResponse::Error(e) => {
                assert_eq!(e.error.code, -32000);
                assert_eq!(e.error.message, "Plugin error: outer failure: root cause");
            }
            JsonRpcResponse::Result(r) => panic!("expected error, got result: {:?}", r),
        }
    }

    #[tokio::test]
    async fn health_check_is_type_level_and_delegates() {
        let (mut s, state) = server();
        let resp = s
            .handle_request(req("health_check", Value::Null, None))
            .await
            .unwrap();
        assert_eq!(result_of(&resp), &serde_json::json!("ok"));

        *state.health_ok.lock().unwrap() = false;
        let resp = s
            .handle_request(req("health_check", Value::Null, None))
            .await
            .unwrap();
        assert_eq!(error_code(&resp), -32001);
    }

    #[test]
    fn auth_is_fail_closed() {
        // Secret set: only the exact secret passes.
        let (s, _state) = server();
        assert!(s.auth_ok(Some("secret")));
        assert!(!s.auth_ok(Some("wrong")));
        assert!(!s.auth_ok(None));

        // Secret MISSING from the environment ⇒ every request rejected.
        let mut s = server().0;
        s.expected_auth = None;
        assert!(!s.auth_ok(Some("secret")));
        assert!(!s.auth_ok(None));
    }
}
