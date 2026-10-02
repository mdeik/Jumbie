// Rate-Limited Plugin Decorator
//
// Decorator (GoF) wrapping an inner `PluginInstance` with the backend's shared
// failure policy. Transparent — callers interact through the same
// `PluginInstance` trait. Four layers of protection:
//
//   1. Failure cooldowns (auth + transient): a plugin that fails in a way that
//      retrying won't fix (rejected credentials) — or that keeps failing
//      transiently (network down, provider erroring) — is put on an escalating
//      backoff window instead of being hammered. `health_check` reports the
//      cached failure during the window so the status page shows why the plugin
//      is blocked.
//   2. Explicit pause (429 handling): if the plugin itself returns a 429, pause
//      ALL calls to it for `retry_after` seconds, shared across concurrent
//      callers.
//   3. Token bucket (governor): for plugins declaring a `rate_limit`, enforce a
//      per-minute quota with burst allowance.
//   4. Retry with exponential backoff: transient errors retry up to 3 times with
//      jitter to avoid a thundering herd upstream.
//
// Failure classification (SSoT): plugins never decide policy — they only
// classify, using one shared vocabulary:
//   • Auth failure — the remote answered but rejected the credentials (e.g. HTTP
//     401/403 on login). Internal plugins return `PluginCallError::AuthFailed`;
//     external plugins return JSON-RPC code `-32030` (see
//     `plugin-sdk::rpc::JsonRpcResponse::auth_failed`). NEVER retried — the same
//     credentials will fail again. → layer 1.
//   • Rate limited — the remote asked us to slow down (429). Internal plugins
//     return `PluginCallError::RetryAfter`; external plugins return JSON-RPC code
//     `-32029` with `retry_after` in `data`. → layer 2.
//   • Transient — no handshake (connect/timeout/DNS/TLS), 5xx, or unknown. Safe
//     to retry with backoff. → layers 3-4.
//
// The `governor` crate uses GCRA, a token bucket needing no background timer —
// it tracks the last request time and decides whether the next is allowed.
// `DefaultDirectRateLimiter` is a type alias for `RateLimiter<NotKeyed,
// InMemoryState, DefaultClock>`: a single shared in-memory bucket on the wall
// clock.

use anyhow::Result;
use async_trait::async_trait;
use governor::{
    Quota, RateLimiter, clock::DefaultClock, state::InMemoryState, state::direct::NotKeyed,
};
use nonzero_ext::nonzero;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use super::{PluginCallError, PluginInstance};
use jumbie_shared::plugin::RateLimit;

pub type DefaultDirectRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// The classified outcome of a failed plugin call.
///
/// SSoT: plugins classify (via `PluginCallError` variants or JSON-RPC error
/// codes); this wrapper applies policy based on the classification.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FailureKind {
    /// Credentials were rejected (handshake OK, 4xx on login). Never retry.
    Auth(String),
    /// The upstream asked us to slow down (429). Respect the pause.
    RateLimit(u64),
    /// Explicit transient failure (no handshake / 5xx) — safe to retry.
    Transient,
    /// Deterministic failure (bad config, not-found) — never retry.
    Permanent(String),
    /// Unidentified / custom error with no matching variant or code. Treated
    /// as transient (safe default) but logged distinctly.
    Unknown,
}

/// Which failure mode an active cooldown is guarding against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CooldownKind {
    /// Login/credentials rejected — retrying won't help until config changes.
    Auth,
    /// Repeated transient failures (network down, provider erroring).
    Transient,
}

/// Shared cooldown state — one slot per plugin, shared by all concurrent
/// callers via the same `Arc<Mutex>` pattern as `paused_until`.
#[derive(Debug, Clone)]
struct CooldownState {
    kind: CooldownKind,
    /// Do not attempt another call before this instant.
    next_attempt: Instant,
    /// Escalation level (1-based) driving the exponential backoff.
    attempt: u32,
    /// The last error message, surfaced via `health_check` during the cooldown.
    last_error: String,
}

/// Backoff base for failure cooldowns: 1 minute.
const COOLDOWN_BASE_SECS: u64 = 60;
/// Escalation factor per consecutive failure of the same kind.
const COOLDOWN_ESCALATION_FACTOR: u32 = 5;
/// Cap for the failure cooldown: 1 hour.
const COOLDOWN_CAP_SECS: u64 = 3600;

/// Backoff schedule for failure cooldowns: 1 min → 5 min → 15 min → capped at
/// 1 hour. Exponential so a persistently broken plugin degrades gracefully
/// instead of hammering the upstream service.
fn cooldown_duration(attempt: u32) -> Duration {
    let secs = COOLDOWN_BASE_SECS
        .saturating_mul(
            (COOLDOWN_ESCALATION_FACTOR as u64).saturating_pow(attempt.saturating_sub(1)),
        )
        .min(COOLDOWN_CAP_SECS);
    Duration::from_secs(secs)
}

/// Classify a plugin error into the shared failure kinds.
///
/// SSOT for the vocabulary → policy mapping. Plugins never decide policy — they
/// classify, using one of:
///   • `PluginCallError` variants (internal plugins), possibly wrapped in
///     context (the whole chain is walked).
///   • JSON-RPC error codes (external plugins), which the host serializes as
///     structured JSON strings: `-32030` auth, `-32029` rate limit,
///     `-32031` transient, `-32032` permanent.
///   • Anything else (custom/unidentified) → the catch-all, treated as
///     transient and logged distinctly by the wrapper.
fn classify_failure(err: &anyhow::Error) -> FailureKind {
    // 1. Internal plugins: explicit PluginCallError variants (chain-walked in
    //    case the error was wrapped with context).
    for cause in err.chain() {
        if let Some(pce) = cause.downcast_ref::<PluginCallError>() {
            return match pce {
                PluginCallError::AuthFailed(msg) => FailureKind::Auth(msg.clone()),
                PluginCallError::RetryAfter(secs) => FailureKind::RateLimit(*secs),
                // A timed-out call may succeed on retry — treat as transient.
                PluginCallError::Timeout => FailureKind::Transient,
                PluginCallError::Transient(_) => FailureKind::Transient,
                PluginCallError::Permanent(msg) => FailureKind::Permanent(msg.clone()),
                // Deterministic / structural — retrying repeats the same failure.
                _ => FailureKind::Permanent(pce.to_string()),
            };
        }
    }

    // 2. External plugins: the host serializes JSON-RPC errors as JSON strings
    //    with a structured `code` field.
    let err_str = err.to_string();
    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&err_str) {
        let code = payload.get("code").and_then(|c| c.as_i64());
        let msg = payload
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        match code {
            Some(c) if c == plugin_sdk::rpc::JSONRPC_CODE_AUTH_FAILED as i64 => {
                return FailureKind::Auth(msg);
            }
            Some(c) if c == plugin_sdk::rpc::JSONRPC_CODE_RATE_LIMITED as i64 => {
                return FailureKind::RateLimit(retry_after_from_json(&payload));
            }
            Some(c) if c == plugin_sdk::rpc::JSONRPC_CODE_TRANSIENT as i64 => {
                return FailureKind::Transient;
            }
            Some(c) if c == plugin_sdk::rpc::JSONRPC_CODE_PERMANENT as i64 => {
                return FailureKind::Permanent(msg);
            }
            // Unimplemented method — deterministic and method-specific, not a
            // plugin health problem. Never retry or cooldown.
            Some(c) if c == plugin_sdk::rpc::JSONRPC_CODE_METHOD_NOT_FOUND as i64 => {
                return FailureKind::Permanent(msg);
            }
            _ => return FailureKind::Unknown,
        }
    }

    FailureKind::Unknown
}

/// True when a plugin error means the requested method is not implemented —
/// JSON-RPC `-32601` for external plugins, or `PluginCallError::MethodNotSupported`
/// for internal ones. Mirrors the classification in [`classify_failure`].
///
/// Callers use this to skip an *optional* method quietly: a missing method is a
/// deliberate plugin choice, not a health problem, and must not be logged as one.
pub fn is_method_not_supported(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(PluginCallError::MethodNotSupported(_)) =
            cause.downcast_ref::<PluginCallError>()
        {
            return true;
        }
    }

    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&err.to_string()) {
        return payload.get("code").and_then(|c| c.as_i64())
            == Some(plugin_sdk::rpc::JSONRPC_CODE_METHOD_NOT_FOUND as i64);
    }
    false
}

/// Extract `retry_after` from a parsed JSON-RPC error payload's structured
/// `data` field (`{"code": -32029, ..., "data": {"retry_after": 30}}`).
/// Defaults to 30 seconds when absent or unparseable.
fn retry_after_from_json(payload: &serde_json::Value) -> u64 {
    payload
        .get("data")
        .and_then(|d| d.get("retry_after"))
        .and_then(|v| v.as_u64())
        .unwrap_or(30)
}

pub struct PolicyPlugin {
    inner: Arc<dyn PluginInstance>,
    /// Canonical TYPE id (e.g. "metadata.tvdb" / "test.restart_plugin") — used
    /// as the structured `plugin` field on every policy log so cooldown/panic/
    /// classification events are filterable by identity, matching the host's
    /// `plugin_call` spans (P4).
    plugin_id: String,
    /// Shared per-TYPE token bucket (all instances of a type draw from the
    /// same bucket — the declared rate_limit is about the upstream endpoint,
    /// which the type's instances share). `None` = no declared limit.
    limiter: Option<Arc<DefaultDirectRateLimiter>>,
    // Shared pause state: None = not paused, Some(Instant) = paused until this time.
    // All concurrent callers check and respect this.
    paused_until: Arc<Mutex<Option<Instant>>>,
    // Shared failure cooldown (auth + transient). All concurrent callers check
    // and respect this; cleared on success, `test`, `set_config`, or restart.
    cooldown: Arc<Mutex<Option<CooldownState>>>,
    // Backend-managed host fields (priority / enabled / refresh_interval).
    // Stored ON the wrapper — mirroring `PluginTypeHost` — so instances kept in
    // place can have these updated without depending on the inner plugin's
    // setter support (internal plugins' setters are no-ops). Snapshot from the
    // inner at construction; setters also delegate to keep inner state in sync.
    priority: StdMutex<i32>,
    enabled: AtomicBool,
    refresh_interval: StdMutex<Option<u64>>,
}

impl PolicyPlugin {
    /// Wrap a plugin instance with the failure policy.
    ///
    /// `limiter` is the type's SHARED token bucket: the manager builds ONE per
    /// type (from the type's declared `rate_limit`) and hands the same Arc to
    /// every instance of that type, so combined throughput never exceeds the
    /// declared quota. Pass `None` for types without a rate limit.
    pub fn new(
        inner: Arc<dyn PluginInstance>,
        limiter: Option<Arc<DefaultDirectRateLimiter>>,
        plugin_id: impl Into<String>,
    ) -> Self {
        // Snapshot host fields from the inner before it is moved into the wrapper.
        let priority = inner.priority();
        let enabled = inner.is_enabled();
        let refresh_interval = inner.refresh_interval();

        Self {
            inner,
            plugin_id: plugin_id.into(),
            limiter,
            paused_until: Arc::new(Mutex::new(None)),
            cooldown: Arc::new(Mutex::new(None)),
            priority: StdMutex::new(priority),
            enabled: AtomicBool::new(enabled),
            refresh_interval: StdMutex::new(refresh_interval),
        }
    }

    /// Build a token bucket from a type's declared rate limit.
    /// SSoT: the manager calls this once per type and shares the bucket across
    /// all instances of that type.
    pub fn build_limiter(rate_limit: &RateLimit) -> Arc<DefaultDirectRateLimiter> {
        let quota = Quota::per_minute(
            std::num::NonZeroU32::new(rate_limit.requests_per_minute).unwrap_or(nonzero!(1u32)),
        )
        .allow_burst(std::num::NonZeroU32::new(rate_limit.burst).unwrap_or(nonzero!(1u32)));
        Arc::new(RateLimiter::direct(quota))
    }
}

impl PolicyPlugin {
    /// Call the inner plugin with panic isolation.
    ///
    /// Spawns the inner call on a new tokio task so that any panic
    /// (e.g. unwrap, index-out-of-bounds) is caught by the runtime
    /// and surfaced as a `PluginCallError::PluginPanicked` instead
    /// of crashing the worker thread.
    async fn call_inner_safe(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let inner = self.inner.clone();
        let method = method.to_string();
        let params = params.clone();
        let handle = tokio::spawn(async move { inner.call(&method, params).await });
        match handle.await {
            Ok(result) => result,
            Err(join_err) if join_err.is_panic() => {
                let panic_payload = join_err.into_panic();
                let msg = panic_payload
                    .downcast::<String>()
                    .map(|s| s.to_string())
                    .or_else(|payload| payload.downcast::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|_| "unknown".to_string());
                tracing::error!(
                    plugin = %self.plugin_id,
                    instance = %self.inner.instance_id(),
                    "plugin panicked: {msg}"
                );
                Err(anyhow::Error::new(PluginCallError::PluginPanicked(msg)))
            }
            Err(join_err) => Err(join_err.into()),
        }
    }

    /// Return the active cooldown state if the plugin is currently throttled.
    ///
    /// Expired cooldowns report `None` — the next failing call re-arms them at
    /// the escalated level (attempt counters persist across expiry).
    async fn active_cooldown(&self) -> Option<CooldownState> {
        let guard = self.cooldown.lock().await;
        let state = guard.as_ref()?;
        if Instant::now() < state.next_attempt {
            Some(state.clone())
        } else {
            None
        }
    }

    /// Arm (or escalate) the failure cooldown.
    ///
    /// Consecutive failures of the SAME kind escalate the backoff; a kind
    /// switch or a prior success restarts the escalation at level 1.
    async fn set_cooldown(&self, kind: CooldownKind, last_error: String) {
        let mut guard = self.cooldown.lock().await;
        let attempt = match &*guard {
            Some(prev) if prev.kind == kind => prev.attempt + 1,
            _ => 1,
        };
        let duration = cooldown_duration(attempt);
        *guard = Some(CooldownState {
            kind,
            next_attempt: Instant::now() + duration,
            attempt,
            last_error,
        });
        tracing::warn!(
            plugin = %self.plugin_id,
            instance = %self.inner.instance_id(),
            "entered {} cooldown for ~{}s",
            match kind {
                CooldownKind::Auth => "auth-failure",
                CooldownKind::Transient => "transient-failure",
            },
            duration.as_secs()
        );
    }

    /// Reset the failure cooldown (successful call, `test`, or `set_config`).
    async fn clear_cooldown(&self) {
        *self.cooldown.lock().await = None;
    }

    /// Build the error returned while a cooldown is active.
    ///
    /// Mirrors the original failure kind so callers and the status page see a
    /// consistent, actionable message (auth failures stay classified as such).
    fn cooldown_error(state: &CooldownState) -> anyhow::Error {
        let remaining = state
            .next_attempt
            .saturating_duration_since(Instant::now())
            .as_secs();
        match state.kind {
            CooldownKind::Auth => anyhow::Error::new(PluginCallError::AuthFailed(format!(
                "{} (retrying in ~{}s — fix the credentials or run a test)",
                state.last_error, remaining
            ))),
            CooldownKind::Transient => {
                anyhow::anyhow!("{} (retrying in ~{}s)", state.last_error, remaining)
            }
        }
    }
}

#[async_trait]
impl PluginInstance for PolicyPlugin {
    fn instance_id(&self) -> &str {
        self.inner.instance_id()
    }

    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        self.inner.plugin_info()
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        self.inner.supported_protocols()
    }

    fn priority(&self) -> i32 {
        *self.priority.lock().unwrap()
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn refresh_interval(&self) -> Option<u64> {
        *self.refresh_interval.lock().unwrap()
    }

    fn set_priority(&self, new_priority: i32) {
        *self.priority.lock().unwrap() = new_priority;
        self.inner.set_priority(new_priority);
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.inner.set_enabled(enabled);
    }

    fn set_refresh_interval(&self, minutes: Option<u64>) {
        *self.refresh_interval.lock().unwrap() = minutes;
        self.inner.set_refresh_interval(minutes);
    }

    async fn set_config(&self, config: Value) -> Result<Value> {
        // A new config (e.g. fixed credentials) invalidates the old failure
        // state — clear the cooldown before handing the config down.
        self.clear_cooldown().await;
        self.inner.set_config(config).await
    }

    /// In-process health for the status page: an active failure cooldown or
    /// 429-pause reports the cached reason (NO RPC to the plugin process);
    /// otherwise the type/process health decides.
    async fn health_status(&self) -> (bool, Option<String>) {
        if let Some(state) = self.active_cooldown().await {
            return (false, Some(Self::cooldown_error(&state).to_string()));
        }
        {
            let paused = self.paused_until.lock().await;
            if let Some(until) = *paused
                && Instant::now() < until
            {
                let remaining = until.duration_since(Instant::now()).as_secs();
                return (
                    false,
                    Some(format!("Rate limited — retrying in ~{}s", remaining)),
                );
            }
        }
        (self.inner.is_healthy(), None)
    }

    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        // Bypass for metadata calls: get_info must never be gated — the UI needs
        // plugin metadata even while the plugin is in a failure cooldown.
        if method == "get_info" {
            return self.call_inner_safe(method, params).await;
        }

        // health_check: report an active failure cooldown. The status page polls
        // health_check; during a cooldown we fail with the cached error so the UI
        // shows WHY the plugin is blocked. We never probe the network here.
        if method == "health_check" {
            if let Some(state) = self.active_cooldown().await {
                return Err(Self::cooldown_error(&state));
            }
            return self.call_inner_safe(method, params).await;
        }

        // test: the user explicitly asked for a live check, so it bypasses the
        // cooldown. Success clears the cooldown; failure re-arms it. The 429
        // pause below still applies — we don't probe a provider that told us
        // to stop.
        let bypass_cooldown = method == "test";

        // Check 0: Failure cooldown. If the plugin recently failed with auth
        // errors or a run of transient errors, short-circuit without touching
        // the inner plugin. All concurrent callers see the same window because
        // the state is shared.
        if !bypass_cooldown && let Some(state) = self.active_cooldown().await {
            return Err(Self::cooldown_error(&state));
        }

        // Check 1: Explicit pause. If the plugin returned a 429 in a previous
        // call, respect the advertised retry_after. All concurrent callers see
        // the same pause because `paused_until` is behind an Arc<Mutex>.
        {
            let paused = self.paused_until.lock().await;
            if let Some(until) = *paused {
                let now = Instant::now();
                if now < until {
                    let remaining = until.duration_since(now).as_secs();
                    return Err(anyhow::Error::new(PluginCallError::RetryAfter(remaining)));
                }
            }
        }

        // Check 2: Token bucket. Negative decisions happen in microseconds (no
        // I/O), so this is safe to check on every call.
        if let Some(limiter) = &self.limiter
            && limiter.check().is_err()
        {
            return Err(anyhow::Error::new(PluginCallError::Overload));
        }

        // Make the call with retries. Retry loop with exponential backoff +
        // jitter for TRANSIENT failures only:
        //   Attempt 1: immediate
        //   Attempt 2: 500-700ms delay (base 500 + random 0-200)
        //   Attempt 3: 1000-1200ms delay (base 1000 + random 0-200)
        //
        // Auth failures are NEVER retried (the credentials are the problem —
        // retrying repeats the same failure and hammers the upstream). 429s
        // are NEVER retried (the remote said "stop asking"). Both arm their
        // respective backoff state instead.
        let mut last_error = None;
        let mut delay_ms = 500;

        for attempt in 1..=3 {
            let result = self.call_inner_safe(method, params.clone()).await;

            match result {
                Ok(v) => {
                    // Success proves the failure is over — reset any cooldown.
                    self.clear_cooldown().await;
                    return Ok(v);
                }
                Err(e) => match classify_failure(&e) {
                    FailureKind::Auth(msg) => {
                        self.set_cooldown(CooldownKind::Auth, msg).await;
                        return Err(e);
                    }
                    // 429 — respect the advertised retry_after via the shared pause.
                    FailureKind::RateLimit(retry_after) => {
                        let mut paused = self.paused_until.lock().await;
                        *paused = Some(Instant::now() + Duration::from_secs(retry_after));
                        return Err(anyhow::Error::new(PluginCallError::RetryAfter(retry_after)));
                    }
                    // Deterministic failure — retrying repeats it. Surface now.
                    FailureKind::Permanent(msg) => {
                        tracing::debug!(
                            plugin = %self.plugin_id,
                            instance = %self.inner.instance_id(),
                            method = %method,
                            "plugin failed permanently: {msg}"
                        );
                        return Err(e);
                    }
                    // No handshake / 5xx — safe to retry with backoff.
                    FailureKind::Transient => {
                        last_error = Some(e);
                    }
                    // Catch-all: an error we have no variant or code for. Safe
                    // default is to treat it as transient — but log it so
                    // unclassified errors stay visible to maintainers.
                    FailureKind::Unknown => {
                        tracing::debug!(
                            plugin = %self.plugin_id,
                            instance = %self.inner.instance_id(),
                            method = %method,
                            "unclassified plugin error — treating as transient: {e}"
                        );
                        last_error = Some(e);
                    }
                },
            }

            // Exponential backoff with jitter before the next attempt.
            if attempt < 3 {
                let jitter: u64 = rand::random::<u64>() % 200;
                tokio::time::sleep(Duration::from_millis(delay_ms + jitter)).await;
                delay_ms *= 2;
            }
        }

        // All 3 attempts failed with transient errors. Enter a transient
        // cooldown so the NEXT caller doesn't immediately re-hammer a provider
        // that is unreachable or erroring. Escalates on repeated episodes.
        if let Some(err) = last_error {
            self.set_cooldown(CooldownKind::Transient, err.to_string())
                .await;
            return Err(err);
        }

        // Unreachable in practice: the loop above always returns or sets
        // `last_error`. Kept as a defensive fallback.
        Err(anyhow::Error::new(PluginCallError::Internal(format!(
            "Call to {} failed after 3 attempts",
            method
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Mock Plugin
    // A controlled test double tracking call counts and configurable to fail with
    // specific error types; `call_count` asserts exactly how many times the inner
    // plugin was invoked (to verify retry/pause logic).

    struct MockPlugin {
        call_count: Arc<AtomicU32>,
        fail_with_429: Arc<Mutex<bool>>,
        fail_with_auth: Arc<Mutex<bool>>,
        fail_with_permanent: Arc<Mutex<bool>>,
    }

    #[async_trait]
    impl PluginInstance for MockPlugin {
        fn instance_id(&self) -> &str {
            "mock"
        }

        fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
            plugin_sdk::traits::PluginTypeInfo {
                display_name: "MockPlugin".to_string(),
                version: "1.0.0".to_string(),
                author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
                description: "Mock plugin for rate-limit tests".to_string(),
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
        async fn call(&self, method: &str, _params: Option<Value>) -> Result<Value> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if method == "fail" {
                if *self.fail_with_auth.lock().await {
                    // Internal plugins signal rejected credentials structurally.
                    return Err(anyhow::Error::new(PluginCallError::AuthFailed(
                        "invalid api key".to_string(),
                    )));
                }
                if *self.fail_with_permanent.lock().await {
                    return Err(anyhow::Error::new(PluginCallError::Permanent(
                        "bad request".to_string(),
                    )));
                }
                if *self.fail_with_429.lock().await {
                    // Structured JSON format that the host's reader task
                    // produces for JSON-RPC error responses (external path).
                    let error_json = serde_json::json!({
                        "code": -32029,
                        "message": "Rate limited",
                        "data": {"retry_after": 30},
                    });
                    return Err(anyhow::anyhow!("{}", error_json));
                }
                anyhow::bail!("generic error");
            }
            if method == "panic" {
                panic!("simulated plugin panic");
            }
            Ok(json!("ok"))
        }
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let call_count = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(false)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });

        let rl = PolicyPlugin::new(
            mock,
            Some(PolicyPlugin::build_limiter(&RateLimit {
                requests_per_minute: 2,
                burst: 1,
            })),
            "mock",
        );

        // First call should succeed (burst allows 1 immediate request)
        rl.call("test", None).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second call immediately should fail with Overload
        let res = rl.call("test", None).await;
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.to_string().contains("overloaded"));
    }

    #[tokio::test]
    async fn test_429_pause() {
        let call_count = Arc::new(AtomicU32::new(0));
        let fail_with_429 = Arc::new(Mutex::new(true));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: fail_with_429.clone(),
            fail_with_auth: Arc::new(Mutex::new(false)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });

        let rl = PolicyPlugin::new(mock, None, "mock");

        // Call should fail and trigger pause (no retries for 429)
        let res = rl.call("fail", None).await;
        assert!(res.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Next call should be skipped because of active pause
        let res2 = rl.call("test", None).await;
        assert!(res2.is_err());
        assert!(res2.unwrap_err().to_string().contains("retry after"));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retries() {
        let call_count = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(false)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });

        let rl = PolicyPlugin::new(mock, None, "mock");

        // Generic error should trigger all 3 retries
        let res = rl.call("fail", None).await;
        assert!(res.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_set_config_not_supported() {
        // The MockPlugin doesn't implement set_config, so the default
        // trait impl returns MethodNotSupported.
        let mock = Arc::new(MockPlugin {
            call_count: Arc::new(AtomicU32::new(0)),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(false)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });

        let rl = PolicyPlugin::new(mock, None, "mock");

        let res = rl.set_config(serde_json::json!({"key": "value"})).await;
        assert!(
            res.is_err(),
            "Expected MethodNotSupported for default set_config"
        );
        let err = res.unwrap_err().to_string();
        assert!(
            err.contains("not supported") || err.contains("Method not found"),
            "Error should indicate method not supported, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_panic_isolation() {
        // A panic inside the plugin should be caught and returned as
        // PluginPanicked, not crash the test runner.
        let mock = Arc::new(MockPlugin {
            call_count: Arc::new(AtomicU32::new(0)),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(false)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });

        let rl = PolicyPlugin::new(mock, None, "mock");

        let res = rl.call("panic", None).await;
        assert!(res.is_err(), "Expected an error from panicking plugin");

        let err = res.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("panicked"),
            "Error should mention panicked, got: {}",
            err_str
        );
        assert!(
            err_str.contains("simulated plugin panic"),
            "Error should contain the panic message, got: {}",
            err_str
        );
    }

    // Auth/transient classification and cooldown behavior (plugin spec §8).

    #[tokio::test]
    async fn test_auth_failure_no_retry_and_cooldown() {
        let call_count = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(true)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });
        let rl = PolicyPlugin::new(mock, None, "mock");

        // Auth failure must NOT be retried — exactly one inner call.
        let res = rl.call("fail", None).await;
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("Authentication failed"),
            "should surface the auth classification"
        );
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Subsequent calls short-circuit during the cooldown — no inner calls.
        let res2 = rl.call("fail", None).await;
        assert!(res2.is_err());
        assert!(
            res2.unwrap_err().to_string().contains("retrying in"),
            "cooldown error should say when it will retry"
        );
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_transient_failures_retry_then_cooldown() {
        let call_count = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(false)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });
        let rl = PolicyPlugin::new(mock, None, "mock");

        // First episode: generic (transient) error → 3 retries, then cooldown.
        let res = rl.call("fail", None).await;
        assert!(res.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);

        // Second call during the transient cooldown → short-circuits.
        let res2 = rl.call("fail", None).await;
        assert!(res2.is_err());
        assert!(
            res2.unwrap_err().to_string().contains("retrying in"),
            "cooldown error should say when it will retry"
        );
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_success_clears_cooldown() {
        let call_count = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(true)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });
        let rl = PolicyPlugin::new(mock, None, "mock");

        // Arm the auth cooldown.
        rl.call("fail", None).await.unwrap_err();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // "test" bypasses the cooldown and succeeds → clears it.
        rl.call("test", None).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 2);

        // The next "fail" call reaches the inner plugin again (cooldown reset).
        rl.call("fail", None).await.unwrap_err();
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_set_config_clears_cooldown() {
        let call_count = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(true)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });
        let rl = PolicyPlugin::new(mock, None, "mock");

        // Arm the auth cooldown.
        rl.call("fail", None).await.unwrap_err();

        // Reconfigure clears the cooldown even though the mock rejects it.
        let reconf = rl.set_config(serde_json::json!({"key": "value"})).await;
        assert!(reconf.is_err(), "mock doesn't supportset_config");

        // Next call reaches the inner plugin again.
        rl.call("fail", None).await.unwrap_err();
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_health_check_reports_cooldown_without_calling_inner() {
        let call_count = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(true)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });
        let rl = PolicyPlugin::new(mock.clone(), None, "mock");

        // Arm the auth cooldown.
        rl.call("fail", None).await.unwrap_err();

        // health_check reports the cached auth failure WITHOUT touching inner.
        let res = rl.call("health_check", None).await;
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("Authentication failed"),
            "status should surface the auth failure"
        );
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // A fresh wrapper (no cooldown) delegates health_check to the inner.
        let fresh = PolicyPlugin::new(mock, None, "mock");
        fresh.call("health_check", None).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_cooldown_backoff_schedule() {
        // Exponential: 1m → 5m → 15m → capped at 1h.
        assert_eq!(cooldown_duration(1), Duration::from_secs(60));
        assert_eq!(cooldown_duration(2), Duration::from_secs(300));
        assert_eq!(cooldown_duration(3), Duration::from_secs(1500));
        assert_eq!(cooldown_duration(4), Duration::from_secs(3600));
        assert_eq!(cooldown_duration(10), Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn test_cooldown_escalates_on_repeated_auth_failures() {
        let call_count = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(true)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });
        let rl = PolicyPlugin::new(mock, None, "mock");

        // First auth failure arms the cooldown at escalation level 1.
        rl.call("fail", None).await.unwrap_err();
        let state = rl
            .active_cooldown()
            .await
            .expect("cooldown should be armed");
        assert_eq!(state.attempt, 1);
        assert_eq!(state.kind, CooldownKind::Auth);

        // Simulate the cooldown expiring, then fail again → escalates to 2.
        rl.cooldown.lock().await.as_mut().unwrap().next_attempt =
            Instant::now() - Duration::from_secs(1);
        rl.call("fail", None).await.unwrap_err();
        let state = rl.active_cooldown().await.expect("cooldown should re-arm");
        assert_eq!(state.attempt, 2);
        assert_eq!(state.kind, CooldownKind::Auth);
    }

    #[tokio::test]
    async fn test_wrapper_stores_host_fields() {
        let mock = Arc::new(MockPlugin {
            call_count: Arc::new(AtomicU32::new(0)),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(false)),
            fail_with_permanent: Arc::new(Mutex::new(false)),
        });
        let rl = PolicyPlugin::new(mock, None, "mock");

        // Snapshot from the inner at construction.
        assert_eq!(rl.priority(), 0);
        assert!(rl.is_enabled());
        assert_eq!(rl.refresh_interval(), None);

        // Updates stick on the wrapper even when the inner plugin's setters are
        // no-ops — this is what lets kept-in-place instances apply host fields.
        rl.set_priority(7);
        rl.set_enabled(false);
        rl.set_refresh_interval(Some(30));
        assert_eq!(rl.priority(), 7);
        assert!(!rl.is_enabled());
        assert_eq!(rl.refresh_interval(), Some(30));
    }

    #[tokio::test]
    async fn test_permanent_failure_no_retry_no_cooldown() {
        let call_count = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockPlugin {
            call_count: call_count.clone(),
            fail_with_429: Arc::new(Mutex::new(false)),
            fail_with_auth: Arc::new(Mutex::new(false)),
            fail_with_permanent: Arc::new(Mutex::new(true)),
        });
        let rl = PolicyPlugin::new(mock, None, "mock");

        // Permanent error: exactly one inner call, no retries.
        let res = rl.call("fail", None).await;
        assert!(res.is_err());
        assert!(
            res.unwrap_err().to_string().contains("Permanent error"),
            "should surface the permanent classification"
        );
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // No cooldown was armed — the next call reaches the inner plugin again.
        rl.call("fail", None).await.unwrap_err();
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    // `classify_failure` is the SSoT mapping for the failure vocabulary — these
    // pin the internal-variant and external-JSON-RPC-code mappings.

    #[test]
    fn test_classify_failure_internal_variants() {
        assert_eq!(
            classify_failure(&anyhow::Error::new(PluginCallError::AuthFailed("x".into()))),
            FailureKind::Auth("x".into())
        );
        assert_eq!(
            classify_failure(&anyhow::Error::new(PluginCallError::RetryAfter(12))),
            FailureKind::RateLimit(12)
        );
        assert_eq!(
            classify_failure(&anyhow::Error::new(PluginCallError::Transient("t".into()))),
            FailureKind::Transient
        );
        assert_eq!(
            classify_failure(&anyhow::Error::new(PluginCallError::Permanent("p".into()))),
            FailureKind::Permanent("p".into())
        );
        // A timed-out call may succeed on retry → transient.
        assert_eq!(
            classify_failure(&anyhow::Error::new(PluginCallError::Timeout)),
            FailureKind::Transient
        );
        // Structural / deterministic variants → permanent (no pointless retries).
        assert_eq!(
            classify_failure(&anyhow::Error::new(PluginCallError::MethodNotSupported(
                "m".into()
            ))),
            FailureKind::Permanent(PluginCallError::MethodNotSupported("m".into()).to_string())
        );
        assert_eq!(
            classify_failure(&anyhow::Error::new(PluginCallError::PluginPanicked(
                "boom".into()
            ))),
            FailureKind::Permanent(PluginCallError::PluginPanicked("boom".into()).to_string())
        );
        // Plain (unclassified) errors hit the catch-all.
        assert_eq!(
            classify_failure(&anyhow::anyhow!("generic error")),
            FailureKind::Unknown
        );
    }

    #[test]
    fn test_classify_failure_external_jsonrpc_codes() {
        // External plugins signal via serialized JSON-RPC errors.
        let auth = anyhow::anyhow!(
            serde_json::json!({ "code": -32030, "message": "bad key" }).to_string()
        );
        assert_eq!(classify_failure(&auth), FailureKind::Auth("bad key".into()));

        let rl = anyhow::anyhow!(
            serde_json::json!({
                "code": -32029,
                "message": "Rate limited",
                "data": { "retry_after": 45 }
            })
            .to_string()
        );
        assert_eq!(classify_failure(&rl), FailureKind::RateLimit(45));

        let transient = anyhow::anyhow!(
            serde_json::json!({ "code": -32031, "message": "net down" }).to_string()
        );
        assert_eq!(classify_failure(&transient), FailureKind::Transient);

        let permanent = anyhow::anyhow!(
            serde_json::json!({ "code": -32032, "message": "bad config" }).to_string()
        );
        assert_eq!(
            classify_failure(&permanent),
            FailureKind::Permanent("bad config".into())
        );

        // Unknown codes fall to the catch-all.
        let unknown =
            anyhow::anyhow!(serde_json::json!({ "code": -32099, "message": "custom" }).to_string());
        assert_eq!(classify_failure(&unknown), FailureKind::Unknown);
    }

    #[test]
    fn test_classify_failure_method_not_found_is_permanent() {
        // An unimplemented optional method must not be retried or cooled down.
        let not_found = anyhow::anyhow!(
            serde_json::json!({ "code": -32601, "message": "Method not found" }).to_string()
        );
        assert_eq!(
            classify_failure(&not_found),
            FailureKind::Permanent("Method not found".into())
        );
    }

    #[test]
    fn test_is_method_not_supported() {
        // External plugin: JSON-RPC -32601.
        let external = anyhow::anyhow!(
            serde_json::json!({ "code": -32601, "message": "Method not found" }).to_string()
        );
        assert!(is_method_not_supported(&external));

        // Internal plugin: PluginCallError::MethodNotSupported, even when wrapped.
        let internal = anyhow::Error::new(PluginCallError::MethodNotSupported(
            "get_download_failure".to_string(),
        ))
        .context("calling plugin");
        assert!(is_method_not_supported(&internal));

        // Real failures are not mistaken for a missing method.
        let network = anyhow::anyhow!("connection refused");
        assert!(!is_method_not_supported(&network));
        let permanent = anyhow::anyhow!(
            serde_json::json!({ "code": -32032, "message": "bad config" }).to_string()
        );
        assert!(!is_method_not_supported(&permanent));
    }
}
