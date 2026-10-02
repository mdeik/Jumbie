use leptos::prelude::Callable;
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fmt;
use std::sync::LazyLock;
use std::time::Duration;

// ApiError has three variants because they map to distinct recovery strategies:
// Network errors (connection refused, DNS) are retry candidates; Api errors carry
// the server's status and body, which the UI shows directly in toasts;
// Deserialization errors are a frontend/backend contract mismatch (a dev bug).
//
// The body is captured eagerly as a String because reqwest response bodies are
// single-use streams — reading it later would yield an empty body.

#[derive(Debug, Clone)]
pub enum ApiError {
    Network(String),
    Api {
        status: reqwest::StatusCode,
        body: String,
    },
    Deserialization(String),
}

impl ApiError {
    /// Parse the backend's error envelope out of the body. SSoT for the
    /// envelope shape: the type lives in `jumbie_shared` (ApiErrorEnvelope),
    /// so all consumers (`user_message`, `series_id`) share one parser and
    /// can't drift on field names.
    fn envelope(&self) -> Option<jumbie_shared::types::ApiErrorEnvelope> {
        match self {
            ApiError::Api { body, .. } => {
                serde_json::from_str::<jumbie_shared::types::ApiErrorEnvelope>(body).ok()
            }
            _ => None,
        }
    }

    /// Extract a user-facing message from the API error, stripping technical
    /// wrappers (HTTP status prefix, JSON envelope).
    pub fn user_message(&self) -> String {
        match self {
            ApiError::Api { body, .. } => self
                .envelope()
                .map(|env| env.error)
                .unwrap_or_else(|| body.clone()),
            ApiError::Network(msg) => msg.clone(),
            ApiError::Deserialization(msg) => msg.clone(),
        }
    }

    /// If the backend flagged this error as a path collision with an existing
    /// series (the envelope's `series_id` field), return the claiming series
    /// id so the UI can link the user to it. `None` for every other error
    /// (including generic 400s).
    pub fn series_id(&self) -> Option<String> {
        self.envelope().and_then(|env| env.series_id)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Network(msg) => write!(f, "Network error: {msg}"),
            ApiError::Api { status, .. } => {
                write!(
                    f,
                    "Request failed with status {status}: {}",
                    self.user_message()
                )
            }
            ApiError::Deserialization(msg) => write!(f, "Deserialization error: {msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(err: reqwest::Error) -> Self {
        // is_decode() is reqwest's own classification of transport vs decode failure.
        if err.is_decode() {
            ApiError::Deserialization(err.to_string())
        } else {
            ApiError::Network(err.to_string())
        }
    }
}

// Configuration

#[derive(Clone, Debug)]
pub struct ApiConfig {
    pub base_url: String,
    /// Request timeout. 120s because `POST /api/series` performs the metadata
    /// sync (and optional search) inside the request, which can exceed 30s.
    pub timeout: Duration,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: Self::detect_base_url(),
            // Other requests are unaffected — the timeout only bounds slow ones.
            timeout: Duration::from_secs(120),
            max_retries: 3,
            retry_delay_ms: 1000,
        }
    }
}

impl ApiConfig {
    /// Auto-detect the base URL from the browser location, falling back to /api.
    fn detect_base_url() -> String {
        // Priority 1: compile-time override (dev mode: trunk serve). When frontend
        // and backend are on different ports, override via:
        //   API_BASE_URL=http://localhost:3000 trunk serve
        // Checked before the origin because `window.location.origin` always
        // succeeds in a browser, so it would otherwise mask this override.
        // The backend build.rs removes API_BASE_URL before release builds, so this
        // is only active during local `trunk serve` development.
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(url) = option_env!("API_BASE_URL") {
                return format!("{}/api", url.trim_end_matches('/'));
            }
        }

        // Priority 2: page origin (embedded frontend / same-origin). Deriving the
        // base URL from the origin works regardless of port, Docker remapping, or
        // reverse-proxy config, and is correct when the backend serves both the
        // assets and the API on the same host.
        #[cfg(target_arch = "wasm32")]
        {
            // reqwest's WASM backend needs an absolute URL: it constructs a browser
            // `new Request(url)` before calling fetch(), and that constructor does
            // not resolve relative URLs (a relative "/api" causes a builder error).
            if let Some(window) = web_sys::window() {
                let origin = window.location().origin().unwrap_or_default();
                if !origin.is_empty() {
                    return format!("{}/api", origin);
                }
            }
        }

        #[cfg(all(not(target_arch = "wasm32"), debug_assertions))]
        crate::debug_log!("Using fallback API base URL: /api (non-WASM)");

        "/api".to_string()
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_retry_delay_ms(mut self, delay_ms: u64) -> Self {
        self.retry_delay_ms = delay_ms;
        self
    }
}

// Request Builder — fluent API for HTTP requests.
//
// Most requests share setup (URL, auth header) but differ in small ways (custom
// error message, JSON body, error handler). The builder lets each call site
// configure only what differs, avoiding method explosion on ApiClient.
// The builder holds a short-lived `&ApiClient` reference rather than an owned clone.

pub struct RequestBuilder<'a> {
    client: &'a ApiClient,
    method: reqwest::Method,
    path: String,
    body: Option<serde_json::Value>,
    toast_error_msg: Option<String>,
    toast_success_msg: Option<String>,
    error_handler: Option<(crate::hooks::error_handler::ErrorHandler, String)>,
    /// Per-request retry override. `None` = use `ApiConfig::max_retries`.
    retries: Option<u32>,
}

impl<'a> RequestBuilder<'a> {
    pub fn new(client: &'a ApiClient, method: reqwest::Method, path: impl Into<String>) -> Self {
        Self {
            client,
            method,
            path: path.into(),
            body: None,
            toast_error_msg: None,
            toast_success_msg: None,
            error_handler: None,
            retries: None,
        }
    }

    pub fn json<B: Serialize>(mut self, body: &B) -> Self {
        self.body = Some(serde_json::to_value(body).unwrap_or(serde_json::Value::Null));
        self
    }

    pub fn toast_error(mut self, msg: impl Into<String>) -> Self {
        self.toast_error_msg = Some(msg.into());
        self
    }

    pub fn toast_success(mut self, msg: impl Into<String>) -> Self {
        self.toast_success_msg = Some(msg.into());
        self
    }

    pub fn error_handler(
        mut self,
        handler: crate::hooks::error_handler::ErrorHandler,
        title: impl Into<String>,
    ) -> Self {
        self.error_handler = Some((handler, title.into()));
        self
    }

    /// Disable retries for this request (default: retry per `ApiConfig::max_retries`).
    ///
    /// Reserved for non-idempotent operations like `POST /api/series`, where the
    /// request can take a long time (metadata sync happens inside it) and a
    /// timeout-triggered retry would silently re-run the operation and duplicate
    /// the result (e.g. a second "Show (2)" series).
    pub fn no_retry(mut self) -> Self {
        self.retries = Some(0);
        self
    }

    // The two terminal methods: send() for typed responses, send_unit() for ().
    //
    // Auth token injection happens here, immediately before dispatch, so a token
    // freshly obtained elsewhere (e.g. another tab) is picked up rather than a
    // value cached at builder construction.
    //
    // Non-GET requests without an explicit body get an empty JSON object: reqwest
    // won't set Content-Type for bodiless POST/PUT/DELETE, and some proxies require it.

    /// Send the request and deserialize the response
    pub async fn send<T: DeserializeOwned>(self) -> Result<T, ApiError> {
        self.execute_request(|response| ApiClient::parse_json_response::<T>(response))
            .await
    }

    pub async fn send_unit(self) -> Result<(), ApiError> {
        self.execute_request(|response| ApiClient::parse_unit_response(response))
            .await
    }

    /// Shared request execution: builds the URL, constructs the request,
    /// dispatches it through the retry loop with the given response handler,
    /// and fires side effects (toasts, error handlers).
    async fn execute_request<H, HFut, T>(&self, handler: H) -> Result<T, ApiError>
    where
        H: Fn(reqwest::Response) -> HFut,
        HFut: std::future::Future<Output = Result<T, ApiError>>,
    {
        let url = format!("{}/{}", self.client.config.base_url, self.path);
        crate::debug_log!("API {} {}", self.method.as_str(), url);

        let res = self
            .client
            .execute_with_retry_inner(
                || async {
                    let mut req = self.client.client.request(self.method.clone(), &url);
                    if let Some(body) = &self.body {
                        req = req.json(body);
                    } else if self.method != reqwest::Method::GET {
                        req = req.json(&());
                    }

                    if let Some(token) = ApiClient::get_auth_token() {
                        req = req.header("Authorization", token);
                    }
                    req.send().await
                },
                handler,
                self.retries.unwrap_or(self.client.config.max_retries),
            )
            .await;

        match &res {
            Ok(_) => crate::debug_log!("API {} {} succeeded", self.method.as_str(), url),
            Err(e) => crate::debug_error!("API {} {} failed: {}", self.method.as_str(), url, e),
        }
        self.handle_toasts(&res);
        res
    }

    // After the request completes, fire the configured side effects.
    // Error toasts and handlers are non-blocking — the Result is returned to the
    // caller regardless, so a component can still handle the error itself.
    fn handle_toasts<T>(&self, res: &Result<T, ApiError>) {
        if let Err(e) = res {
            if let Some(msg) = self.toast_error_msg.as_ref() {
                crate::components::common::toast::show_error(format!(
                    "{}: {}",
                    msg,
                    e.user_message()
                ));
            }
            if let Some((handler, title)) = self.error_handler.as_ref() {
                handler.report.run((title.clone(), e.clone()));
            }
        } else if let Some(msg) = self.toast_success_msg.as_ref() {
            crate::components::common::toast::show_success(msg.to_string());
        }
    }
}

// API Client
//
// get_auth_token() is a static method: the token lives in browser localStorage
// and depends on no instance state, so RequestBuilder (which holds a
// &ApiClient) can read it without threading the token through as a field.

pub struct ApiClient {
    config: ApiConfig,
    client: Client,
}

impl ApiClient {
    pub fn new() -> Self {
        Self::with_config(ApiConfig::default())
    }

    pub fn with_config(config: ApiConfig) -> Self {
        // The browser Fetch API (reqwest's WASM backend) has no client-side
        // timeout configuration, so the timeout builder call is skipped on WASM.
        #[cfg(not(target_arch = "wasm32"))]
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .unwrap_or_else(|_| Client::new());

        #[cfg(target_arch = "wasm32")]
        let client = Client::builder().build().unwrap_or_else(|_| Client::new());

        Self { config, client }
    }

    // Auth token injection
    //
    // The token lives in localStorage so it persists across page reloads and
    // browser sessions (the backend's token lifetime can be weeks, so
    // sessionStorage's per-tab lifetime would force frequent relogins).
    // No "Bearer " prefix is added — the token is already stored in the format
    // the backend expects.

    fn get_auth_token() -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(window) = web_sys::window()
                && let Ok(Some(storage)) = window.local_storage()
                && let Ok(Some(token)) = storage.get_item("jb_auth_token")
            {
                return Some(token);
            }
        }
        None
    }

    pub fn request(&self, method: reqwest::Method, path: impl Into<String>) -> RequestBuilder<'_> {
        RequestBuilder::new(self, method, path)
    }

    pub fn get_request(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request(reqwest::Method::GET, path)
    }

    pub fn post_request(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request(reqwest::Method::POST, path)
    }

    pub fn put_request(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request(reqwest::Method::PUT, path)
    }

    pub fn delete_request(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request(reqwest::Method::DELETE, path)
    }

    // Convenience methods — thin wrappers over execute_with_retry for the common
    // case where no custom toast messages or error handlers are needed. They repeat
    // the auth-header injection from RequestBuilder, but the header logic is trivial
    // and the alternative (build-then-immediately-send) adds complexity.

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let url = format!("{}/{}", self.config.base_url, path);
        crate::debug_log!("API GET: {}", url);

        let result = self
            .execute_with_retry(|| async {
                let mut req = self.client.get(&url);
                if let Some(token) = Self::get_auth_token() {
                    req = req.header("Authorization", token);
                }
                req.send().await
            })
            .await;

        if let Err(ref _e) = result {
            crate::debug_error!("API GET {} failed: {}", url, _e);
        }
        result
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        let url = format!("{}/{}", self.config.base_url, path);
        crate::debug_log!("API POST: {}", url);

        let result = self
            .execute_with_retry(|| async {
                let mut req = self.client.post(&url).json(body);
                if let Some(token) = Self::get_auth_token() {
                    req = req.header("Authorization", token);
                }
                req.send().await
            })
            .await;

        if let Err(ref _e) = result {
            crate::debug_error!("API POST {} failed: {}", url, _e);
        }
        result
    }

    pub async fn post_unit<B: Serialize>(&self, path: &str, body: &B) -> Result<(), ApiError> {
        let url = format!("{}/{}", self.config.base_url, path);
        crate::debug_log!("API POST: {}", url);

        let result = self
            .execute_with_retry_unit(|| async {
                let mut req = self.client.post(&url).json(body);
                if let Some(token) = Self::get_auth_token() {
                    req = req.header("Authorization", token);
                }
                req.send().await
            })
            .await;

        if let Err(ref _e) = result {
            crate::debug_error!("API POST {} failed: {}", url, _e);
        }
        result
    }

    pub async fn put<B: Serialize>(&self, path: &str, body: &B) -> Result<(), ApiError> {
        let url = format!("{}/{}", self.config.base_url, path);
        crate::debug_log!("API PUT: {}", url);

        let result = self
            .execute_with_retry_unit(|| async {
                let mut req = self.client.put(&url).json(body);
                if let Some(token) = Self::get_auth_token() {
                    req = req.header("Authorization", token);
                }
                req.send().await
            })
            .await;

        if let Err(ref _e) = result {
            crate::debug_error!("API PUT {} failed: {}", url, _e);
        }
        result
    }

    /// PUT + typed response — mutate the resource, return deserialized JSON body.
    pub async fn put_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        let url = format!("{}/{}", self.config.base_url, path);
        crate::debug_log!("API PUT: {}", url);

        let result = self
            .execute_with_retry(|| async {
                let mut req = self.client.put(&url).json(body);
                if let Some(token) = Self::get_auth_token() {
                    req = req.header("Authorization", token);
                }
                req.send().await
            })
            .await;

        if let Err(ref _e) = result {
            crate::debug_error!("API PUT {} failed: {}", url, _e);
        }
        result
    }

    pub async fn delete(&self, path: &str) -> Result<(), ApiError> {
        let url = format!("{}/{}", self.config.base_url, path);
        crate::debug_log!("API DELETE: {}", url);

        let result = self
            .execute_with_retry_unit(|| async {
                let mut req = self.client.delete(&url);
                if let Some(token) = Self::get_auth_token() {
                    req = req.header("Authorization", token);
                }
                req.send().await
            })
            .await;

        if let Err(ref _e) = result {
            crate::debug_error!("API DELETE {} failed: {}", url, _e);
        }
        result
    }

    pub async fn delete_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let url = format!("{}/{}", self.config.base_url, path);
        crate::debug_log!("API DELETE: {}", url);

        let result = self
            .execute_with_retry(|| async {
                let mut req = self.client.delete(&url);
                if let Some(token) = Self::get_auth_token() {
                    req = req.header("Authorization", token);
                }
                req.send().await
            })
            .await;

        if let Err(ref _e) = result {
            crate::debug_error!("API DELETE {} failed: {}", url, _e);
        }
        result
    }

    /// DELETE with a JSON payload (non-standard but needed by some endpoints
    /// that accept a list of IDs to delete in the request body).
    pub async fn delete_with_body<B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<(), ApiError> {
        let url = format!("{}/{}", self.config.base_url, path);
        crate::debug_log!("API DELETE: {}", url);

        let result = self
            .execute_with_retry_unit(|| async {
                let mut req = self.client.delete(&url).json(body);
                if let Some(token) = Self::get_auth_token() {
                    req = req.header("Authorization", token);
                }
                req.send().await
            })
            .await;

        if let Err(ref _e) = result {
            crate::debug_error!("API DELETE {} failed: {}", url, _e);
        }
        result
    }

    // Retry Logic
    //
    // Exponential backoff (1s, 2s, 4s) balances responsiveness against hammering a
    // recovering server.
    // 4xx errors are NOT retried — the request itself is wrong, so retrying would just
    // waste time (and 429 is deliberate backend rate limiting).

    async fn execute_with_retry_unit<F, Fut>(&self, request_fn: F) -> Result<(), ApiError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Response, reqwest::Error>>,
    {
        self.execute_with_retry_inner(
            request_fn,
            |r| Box::pin(async move { Self::parse_unit_response(r).await }),
            self.config.max_retries,
        )
        .await
    }

    async fn execute_with_retry<F, Fut, T>(&self, request_fn: F) -> Result<T, ApiError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Response, reqwest::Error>>,
        T: DeserializeOwned,
    {
        self.execute_with_retry_inner(
            request_fn,
            |r| Box::pin(async move { Self::parse_json_response(r).await }),
            self.config.max_retries,
        )
        .await
    }

    // The inner loop takes a request factory and a response handler so the typed
    // and unit retry methods share the backoff/matching logic.
    async fn execute_with_retry_inner<F, Fut, H, HFut, T>(
        &self,
        request_fn: F,
        handler: H,
        max_retries: u32,
    ) -> Result<T, ApiError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Response, reqwest::Error>>,
        H: Fn(Response) -> HFut,
        HFut: std::future::Future<Output = Result<T, ApiError>>,
    {
        let mut attempts = 0;
        let mut last_error: Option<ApiError> = None;

        while attempts <= max_retries {
            match request_fn().await {
                Ok(response) => {
                    let status = response.status();
                    let _content_length = response.content_length();

                    if status.is_client_error() {
                        // 4xx: hand off to the handler immediately — it will
                        // produce an Api::Api error for the caller. No retry.
                        crate::debug_warn!(
                            "API client error: {} ({} bytes)",
                            status,
                            _content_length.unwrap_or(0)
                        );
                        return handler(response).await;
                    }
                    // Log the response before consuming the body
                    crate::debug_log!(
                        "API RESPONSE: {} ({} bytes)",
                        status,
                        _content_length.unwrap_or(0)
                    );
                    // 5xx or 2xx: the handler decides. On 5xx it returns Err,
                    // and we fall through to the retry logic below.
                    match handler(response).await {
                        Ok(val) => return Ok(val),
                        Err(e) => {
                            crate::debug_warn!("API handler returned error: {}", e);
                            last_error = Some(e);
                        }
                    }
                }
                Err(e) => {
                    last_error = Some(ApiError::from(e));
                }
            }

            attempts += 1;
            if attempts <= max_retries {
                let delay = self.config.retry_delay_ms * (2_u64.pow(attempts - 1));
                crate::debug_warn!(
                    "Request failed (attempt {}/{}), retrying in {}ms...",
                    attempts,
                    max_retries + 1,
                    delay
                );
                gloo_timers::future::TimeoutFuture::new(delay as u32).await;
            }
        }

        let err = last_error.expect("execute_with_retry_inner: loop exited without an error");
        crate::debug_error!("Request failed after {} attempts: {}", attempts, err);
        Err(err)
    }

    // Response Handlers
    //
    // Both handlers check status before reading the body: response.json() and
    // response.text() are mutually exclusive (a body can only be consumed once),
    // so the success path lets reqwest deserialize the stream directly.

    async fn parse_json_response<T: DeserializeOwned>(response: Response) -> Result<T, ApiError> {
        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .await
                .map_err(|e| ApiError::Deserialization(e.to_string()));
        }

        let body = response.text().await.unwrap_or_default();
        if !body.is_empty() {
            crate::debug_error!("API error {}: {}", status, body.trim());
        } else {
            crate::debug_error!("API error: {}", status);
        }

        Err(ApiError::Api {
            status,
            body: if body.is_empty() {
                status.to_string()
            } else {
                body
            },
        })
    }

    async fn parse_unit_response(response: Response) -> Result<(), ApiError> {
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }

        let body = response.text().await.unwrap_or_default();
        if !body.is_empty() {
            crate::debug_error!("API error {}: {}", status, body.trim());
        } else {
            crate::debug_error!("API error: {}", status);
        }

        Err(ApiError::Api {
            status,
            body: if body.is_empty() {
                status.to_string()
            } else {
                body
            },
        })
    }

    pub async fn handle_response<T: DeserializeOwned>(
        &self,
        response: Response,
    ) -> Result<T, ApiError> {
        Self::parse_json_response(response).await
    }

    pub async fn handle_unit_response(&self, response: Response) -> Result<(), ApiError> {
        Self::parse_unit_response(response).await
    }
}

impl Default for ApiClient {
    fn default() -> Self {
        Self::new()
    }
}

// Singleton ApiClient — connection pooling for all API calls.
//
// reqwest::Client maintains an internal connection pool, DNS cache, and HTTP/2
// multiplexing; creating one per request would discard all of that and open a
// fresh connection each time.
pub static API_CLIENT: LazyLock<ApiClient> =
    LazyLock::new(|| ApiClient::with_config(ApiConfig::default()));

/// Returns a reference to the singleton ApiClient — every API call should go
/// through this to keep connection pooling effective.
pub fn api_client() -> &'static ApiClient {
    &API_CLIENT
}

// BatchMoveProgressResponse — mirrored from the backend's BatchMoveProgress.
//
// Defined here rather than imported from jumbie_shared::types because
// BatchMoveProgress is backend-internal; the frontend only needs the
// deserialized shape.

/// Progress of a single batch-move operation, as returned by the status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMoveProgressResponse {
    pub total: usize,
    pub completed: usize,
    pub success_count: usize,
    pub failed: usize,
    pub finished: bool,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_config_builder() {
        let config = ApiConfig::default()
            .with_base_url("http://test:1234/api")
            .with_timeout(Duration::from_secs(10))
            .with_max_retries(5)
            .with_retry_delay_ms(500);

        assert_eq!(config.base_url, "http://test:1234/api");
        assert_eq!(config.timeout.as_secs(), 10);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.retry_delay_ms, 500);
    }

    #[test]
    fn test_api_config_defaults() {
        let config = ApiConfig::default();
        // Native testing has no Window, so detect_base_url yields the relative fallback
        assert_eq!(config.base_url, "/api");
        // 120s: `POST /api/series` carries the metadata sync inside the request.
        assert_eq!(config.timeout.as_secs(), 120);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.retry_delay_ms, 1000);
    }

    #[test]
    fn test_api_error_series_id() {
        let collision = ApiError::Api {
            status: reqwest::StatusCode::BAD_REQUEST,
            body: r#"{"error":"The path 'X' is already used by series 'S'","series_id":"abc-123"}"#
                .to_string(),
        };
        assert_eq!(collision.series_id(), Some("abc-123".to_string()));
        assert_eq!(
            collision.user_message(),
            "The path 'X' is already used by series 'S'"
        );

        // Envelopes without series_id (every non-collision error) still parse.
        let plain = ApiError::Api {
            status: reqwest::StatusCode::BAD_REQUEST,
            body: r#"{"error":"Invalid path"}"#.to_string(),
        };
        assert_eq!(plain.series_id(), None);
        assert_eq!(plain.user_message(), "Invalid path");

        // Non-envelope bodies fall back to the raw body.
        let raw = ApiError::Api {
            status: reqwest::StatusCode::BAD_GATEWAY,
            body: "proxy said no".to_string(),
        };
        assert_eq!(raw.user_message(), "proxy said no");
        assert_eq!(raw.series_id(), None);

        assert_eq!(ApiError::Network("boom".to_string()).series_id(), None);
    }
}
