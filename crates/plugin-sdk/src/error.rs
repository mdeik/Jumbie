//! Structured plugin failures.
//!
//! `PluginHandler::handle` returns `anyhow::Error`, which `PluginServer`
//! reports to the host as the generic `-32000` (unknown → treated as transient).
//! To signal a *specific* failure kind — one the host's failure policy acts on
//! (retry, cooldown, pause, skip) — return a [`PluginError`]:
//!
//! ```ignore
//! return Err(PluginError::Transient(format!("nyaa.si unreachable: {e}")).into());
//! ```
//!
//! The server downcasts the handler error, emits the matching JSON-RPC code, and
//! the host's `PolicyPlugin` classifies it exactly like an internal plugin's
//! `PluginCallError`. The codes are the SSoT in [`crate::rpc`] and are documented
//! in the plugin specification §3 (error codes) and §8 (failure policy).

use crate::rpc;
use serde_json::Value;

/// A structured failure a plugin handler can raise so the server reports the
/// matching JSON-RPC error code instead of the generic `-32000`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PluginError {
    /// The remote service answered but rejected the credentials (e.g. HTTP
    /// 401/403 on a login endpoint). Never retried; enters the host's auth cooldown.
    #[error("{0}")]
    AuthFailed(String),
    /// The remote asked us to slow down (HTTP 429). Pauses all calls for
    /// `retry_after_secs`.
    #[error("{message}")]
    RateLimited {
        retry_after_secs: u64,
        message: String,
    },
    /// A transient failure — no handshake (connect/timeout/DNS/TLS) or a 5xx.
    /// Safe to retry with backoff.
    #[error("{0}")]
    Transient(String),
    /// A deterministic failure retrying will never fix (bad config, not-found,
    /// unsupported operation). Surfaced immediately.
    #[error("{0}")]
    Permanent(String),
    /// The requested method is not implemented. The caller treats it as
    /// "unsupported" and never retries or cools down.
    #[error("{0}")]
    MethodNotSupported(String),
}

impl PluginError {
    /// The JSON-RPC error code the server reports for this failure.
    pub fn code(&self) -> i32 {
        match self {
            Self::AuthFailed(_) => rpc::JSONRPC_CODE_AUTH_FAILED,
            Self::RateLimited { .. } => rpc::JSONRPC_CODE_RATE_LIMITED,
            Self::Transient(_) => rpc::JSONRPC_CODE_TRANSIENT,
            Self::Permanent(_) => rpc::JSONRPC_CODE_PERMANENT,
            Self::MethodNotSupported(_) => rpc::JSONRPC_CODE_METHOD_NOT_FOUND,
        }
    }

    /// Structured `error.data` payload, if the failure carries one. Only
    /// rate-limiting does — `retry_after`, matching the spec's wire format.
    pub fn data(&self) -> Option<Value> {
        match self {
            Self::RateLimited {
                retry_after_secs, ..
            } => Some(serde_json::json!({ "retry_after": retry_after_secs })),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_the_rpc_ssot() {
        assert_eq!(
            PluginError::AuthFailed("x".into()).code(),
            rpc::JSONRPC_CODE_AUTH_FAILED
        );
        assert_eq!(
            PluginError::RateLimited {
                retry_after_secs: 5,
                message: "slow".into()
            }
            .code(),
            rpc::JSONRPC_CODE_RATE_LIMITED
        );
        assert_eq!(
            PluginError::Transient("x".into()).code(),
            rpc::JSONRPC_CODE_TRANSIENT
        );
        assert_eq!(
            PluginError::Permanent("x".into()).code(),
            rpc::JSONRPC_CODE_PERMANENT
        );
        assert_eq!(
            PluginError::MethodNotSupported("m".into()).code(),
            rpc::JSONRPC_CODE_METHOD_NOT_FOUND
        );
    }

    #[test]
    fn only_rate_limited_carries_data() {
        let rate_limited = PluginError::RateLimited {
            retry_after_secs: 42,
            message: "slow".into(),
        };
        assert_eq!(
            rate_limited.data(),
            Some(serde_json::json!({ "retry_after": 42 }))
        );
        assert_eq!(PluginError::Transient("x".into()).data(), None);
    }

    #[test]
    fn downcasts_out_of_anyhow() {
        let err: anyhow::Error = PluginError::Transient("net down".into()).into();
        let plugin_error = err
            .downcast_ref::<PluginError>()
            .expect("PluginError must survive the anyhow boundary");
        assert_eq!(plugin_error.code(), rpc::JSONRPC_CODE_TRANSIENT);
    }
}
