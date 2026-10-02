use serde::{Deserialize, Serialize};

// SSoT for application error codes — the host's `PolicyPlugin` classifier and the
// backend reference these same constants. Documented in the plugin specification
// §3 (error codes) and §8 (failure policy).

/// Rate limited (429) — includes `retry_after` in `data`. Pauses all calls.
pub const JSONRPC_CODE_RATE_LIMITED: i32 = -32029;
/// Authentication failed — credentials rejected. Never retried; auth cooldown.
pub const JSONRPC_CODE_AUTH_FAILED: i32 = -32030;
/// Transient failure (network / 5xx) — safe to retry with backoff.
pub const JSONRPC_CODE_TRANSIENT: i32 = -32031;
/// Permanent failure (deterministic — config, not-found). Never retried.
pub const JSONRPC_CODE_PERMANENT: i32 = -32032;
/// The plugin does not implement the requested method. Deterministic and
/// specific to the method (not a health problem), so it must never be retried
/// or trigger a cooldown — the caller simply treats the method as unsupported.
pub const JSONRPC_CODE_METHOD_NOT_FOUND: i32 = -32601;

/// Current external plugin protocol version.
///
/// Exchanged during the startup `hello` handshake: the host refuses to run a
/// process that reports a different major version (the plugin was built against
/// an incompatible protocol). Bump on any breaking protocol change.
///
/// v3: source `search` / `auto_search` responses are the `{ entries, queries }`
/// envelope (the plugin reports the exact query string(s) it sent); the bare
/// `Vec<MediaEntry>` array is no longer accepted.
pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value, // Can be Object, Array, or Null
    pub id: Option<serde_json::Value>,
    /// Which instance (config instance key) this request targets.
    ///
    /// One process serves all instances of a type; `None` marks a type-level
    /// method (get_info / get_config_schema / validate_config).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// Optional authentication token for IPC channel security.
    ///
    /// The host generates a random secret at spawn time, passes it via
    /// `JUMBIE_PLUGIN_SECRET`, and the server validates it on every request,
    /// preventing another local process from injecting requests into stdin.
    /// Skipped when absent so old plugins and non-SDK clients aren't forced
    /// to include it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
}

impl JsonRpcRequest {
    /// SSoT constructor — every request the host sends is built through here.
    pub fn new(
        method: impl Into<String>,
        params: serde_json::Value,
        id: Option<serde_json::Value>,
        instance_id: Option<String>,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
            id,
            instance_id,
            auth: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResultResponse {
    pub jsonrpc: String,
    pub result: serde_json::Value,
    pub id: Option<serde_json::Value>,
    /// Echo of the request's `auth` token, proving the plugin received it.
    /// The host validates this to prevent a rogue process from injecting
    /// fake responses into the plugin's stdout pipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub jsonrpc: String,
    pub error: RpcError,
    pub id: Option<serde_json::Value>,
    /// Echo of the request's `auth` token, proving the plugin received it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// A JSON-RPC 2.0 response discriminated by shape rather than by a tag field.
///
/// The spec has no discriminator: a response is a success if it contains
/// `"result"` and an error if it contains `"error"` (tried in declaration order).
/// An internally-tagged enum would invent a field that doesn't exist in the spec.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    Result(ResultResponse),
    Error(ErrorResponse),
}

impl JsonRpcResponse {
    /// Constructs a successful JSON-RPC 2.0 response.
    ///
    /// Serialization failure degrades to `null` rather than panicking, keeping the
    /// RPC channel alive.
    pub fn ok(id: Option<serde_json::Value>, result: impl Serialize) -> Self {
        JsonRpcResponse::Result(ResultResponse {
            jsonrpc: "2.0".to_string(),
            result: serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
            id,
            auth: None,
        })
    }

    /// Constructs a generic JSON-RPC error response for arbitrary error conditions.
    ///
    /// `code` is left generic so callers can supply application-specific codes.
    pub fn error(
        id: Option<serde_json::Value>,
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    ) -> Self {
        JsonRpcResponse::Error(ErrorResponse {
            jsonrpc: "2.0".to_string(),
            error: RpcError {
                code,
                message,
                data,
            },
            id,
            auth: None,
        })
    }

    /// Attach an auth token to this response (echoing the request's auth).
    ///
    /// The host validates the echoed auth to verify the plugin process
    /// received the original auth token. This prevents a rogue process
    /// from injecting fake responses into the host's stdin pipe.
    pub fn with_auth(mut self, auth: Option<String>) -> Self {
        match &mut self {
            JsonRpcResponse::Result(r) => r.auth = auth,
            JsonRpcResponse::Error(e) => e.auth = auth,
        }
        self
    }

    /// Pre-built "-32601" error for an unrecognized method.
    pub fn method_not_found(id: Option<serde_json::Value>) -> Self {
        Self::error(
            id,
            crate::rpc::JSONRPC_CODE_METHOD_NOT_FOUND,
            "Method not found".to_string(),
            None,
        )
    }

    /// Pre-built "-32602" error for invalid method parameters.
    ///
    /// `msg` should indicate which parameter was invalid and why.
    pub fn invalid_params(id: Option<serde_json::Value>, msg: String) -> Self {
        Self::error(id, -32602, format!("Invalid params: {}", msg), None)
    }

    /// Pre-built "-32029" error — application-level rate limit with optional retry_after.
    ///
    /// `retry_after` goes in the structured `data` field, never in the message
    /// string, so the host's `PolicyPlugin` can parse it without string grepping.
    pub fn rate_limited(id: Option<serde_json::Value>, retry_after_secs: u64) -> Self {
        Self::error(
            id,
            JSONRPC_CODE_RATE_LIMITED,
            "Rate limited".to_string(),
            Some(serde_json::json!({ "retry_after": retry_after_secs })),
        )
    }

    /// Pre-built "-32030" error — the remote service rejected credentials
    /// (e.g. HTTP 401/403 on a login endpoint).
    ///
    /// Non-retryable: `PolicyPlugin` short-circuits these into an escalating
    /// cooldown that clears on success, `test`, `reconfigure`, or restart — see
    /// the plugin specification, §8 "Rate Limiting & Failure Policy".
    pub fn auth_failed(id: Option<serde_json::Value>, message: String) -> Self {
        Self::error(id, JSONRPC_CODE_AUTH_FAILED, message, None)
    }

    /// Pre-built "-32031" error — a transient failure (connect / timeout / DNS /
    /// TLS, or a 5xx server error).
    ///
    /// The host retries these with exponential backoff before entering a transient
    /// cooldown. Return this instead of a generic error for non-deterministic failures.
    pub fn transient(id: Option<serde_json::Value>, message: String) -> Self {
        Self::error(id, JSONRPC_CODE_TRANSIENT, message, None)
    }

    /// Pre-built "-32032" error — a deterministic failure retrying will never fix
    /// (bad config, missing resource, unsupported operation).
    ///
    /// Surfaced immediately without retrying or entering a cooldown.
    pub fn permanent(id: Option<serde_json::Value>, message: String) -> Self {
        Self::error(id, JSONRPC_CODE_PERMANENT, message, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_failed_serializes_with_code_32030() {
        let resp = JsonRpcResponse::auth_failed(Some(serde_json::json!(1)), "bad key".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], serde_json::json!(-32030));
        assert_eq!(json["error"]["message"], serde_json::json!("bad key"));
    }

    #[test]
    fn auth_failed_round_trips_through_error_variant() {
        let resp = JsonRpcResponse::auth_failed(None, "denied".to_string());
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: JsonRpcResponse = serde_json::from_str(&json).unwrap();
        match parsed {
            JsonRpcResponse::Error(e) => {
                assert_eq!(e.error.code, -32030);
                assert_eq!(e.error.message, "denied");
            }
            JsonRpcResponse::Result(_) => panic!("expected an error response"),
        }
    }

    #[test]
    fn transient_and_permanent_use_their_documented_codes() {
        let transient =
            JsonRpcResponse::transient(Some(serde_json::json!(2)), "net down".to_string());
        let json = serde_json::to_value(&transient).unwrap();
        assert_eq!(
            json["error"]["code"],
            serde_json::json!(JSONRPC_CODE_TRANSIENT)
        );
        assert_eq!(json["error"]["message"], serde_json::json!("net down"));

        let permanent = JsonRpcResponse::permanent(None, "bad config".to_string());
        let json = serde_json::to_value(&permanent).unwrap();
        assert_eq!(
            json["error"]["code"],
            serde_json::json!(JSONRPC_CODE_PERMANENT)
        );
        assert_eq!(json["error"]["message"], serde_json::json!("bad config"));
    }
}
