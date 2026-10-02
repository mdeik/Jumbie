// qBittorrent Download Client
//
// Implements the downloader plugin for qBittorrent's Web API (v2), reachable
// over the network (localhost, Docker, or remote).
//
// Authentication is cookie-based: POST /api/v2/auth/login with username/password
// form fields; the server responds with a SID cookie. The reqwest client has
// `.cookie_store(true)` set, so later requests automatically carry the cookie.
//
// Path translation: a qBittorrent container sees paths from ITS perspective
// (/downloads/...) while the Jumbie container needs its own (/data/downloads/...).
// `organizer_path` maps them:
//
//   qBittorrent download_path: /downloads
//   organizer_path (local):    /data/downloads
//   qBittorrent reports:       /downloads/Series/MyShow/S01/E05.mkv
//   translated to:             /data/downloads/Series/MyShow/S01/E05.mkv
//
// Without this translation the organizer would access paths that don't exist on
// its filesystem. If `organizer_path` is unset, the raw qBittorrent path is used
// as-is (works for non-containerized setups).
//
// TLS: `verify_ssl` controls whether the reqwest client validates the server's
// certificate, via `.danger_accept_invalid_certs(!verify_ssl)` — needed because
// many reverse proxies use self-signed certificates.

use crate::plugins::extract::{extract_bool_opt, extract_str, extract_str_opt};
use crate::plugins::{PluginCallError, PluginInstance};
use crate::utils::describe_error_chain;
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, LazyLock};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct QBittorrentSettings {
    #[serde(default = "default_qb_name")]
    pub name: String,
    #[serde(default = "default_link")]
    pub link: String,
    #[serde(default = "default_username")]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_download_path")]
    pub download_path: std::path::PathBuf,
    #[serde(default)]
    pub use_separate_paths: bool,
    #[serde(default)]
    pub organizer_path: Option<std::path::PathBuf>,
    #[serde(default = "default_category")]
    pub default_category: String,
    #[serde(default = "default_true")]
    pub verify_ssl: bool,
}

fn default_qb_name() -> String {
    "qBittorrent".to_string()
}
fn default_true() -> bool {
    true
}
fn default_link() -> String {
    "http://localhost:8080".to_string()
}
fn default_username() -> String {
    "admin".to_string()
}
fn default_download_path() -> std::path::PathBuf {
    std::path::PathBuf::from("./downloads")
}
fn default_category() -> String {
    "Series".to_string()
}

impl QBittorrentSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.link.is_empty() {
            return Err("link cannot be empty".to_string());
        }
        if !self.link.starts_with("http://") && !self.link.starts_with("https://") {
            return Err("link must start with http:// or https://".to_string());
        }
        if self.username.is_empty() {
            return Err("username cannot be empty".to_string());
        }
        Ok(())
    }
}

/// HTTP transport for one qBittorrent instance: the reqwest client (Referer
/// header for CSRF, TLS acceptance) and the base URL, which both derive from
/// the instance's `link`/`verify_ssl` settings.
///
/// Mutable: a link/verify_ssl change rebuilds the transport IN PLACE during
/// `set_config` instead of forcing a factory replacement. `reqwest::Client`
/// is a cheap cloneable handle, so rebuilding it with the same global config
/// produces exactly what the factory would construct.
#[derive(Clone)]
struct QBittorrentHttp {
    client: Client,
    base_url: String,
}

pub struct QBittorrentClient {
    /// HTTP transport — behind an RwLock so in-placeset_config can rebuild it.
    http: std::sync::RwLock<QBittorrentHttp>,
    /// Stable per-instance ID derived from the plugin config key (e.g. "primary", "secondary").
    /// This allows multiple qBittorrent instances (e.g., one for 1080p, one for 4K).
    instance_id: String,
    /// Global config snapshot (proxy settings) used to rebuild the HTTP client
    /// during in-placeset_config — the SAME source the factory would use.
    global_config: Arc<jumbie_shared::config::Config>,
    // Mutable settings (in-placeset_config); reads go through `cfg()`.
    settings: std::sync::RwLock<QBittorrentSettings>,
}

impl QBittorrentClient {
    pub fn new(
        config: &QBittorrentSettings,
        instance_id: impl Into<String>,
        global_config: Arc<jumbie_shared::config::Config>,
    ) -> Self {
        let http = Self::build_http(config, &global_config);
        Self {
            http: std::sync::RwLock::new(http),
            instance_id: instance_id.into(),
            global_config,
            settings: std::sync::RwLock::new(config.clone()),
        }
    }

    /// Build the HTTP transport (client + base URL) from instance settings.
    /// SSoT: used by both `new` and in-place `set_config`, so a rebuilt
    /// transport is byte-identical to one the factory would construct.
    fn build_http(
        config: &QBittorrentSettings,
        global_config: &jumbie_shared::config::Config,
    ) -> QBittorrentHttp {
        let base_url = format!("{}/api/v2", config.link.trim_end_matches('/'));
        // Set Referer as a default header so every outgoing request includes it.
        // qBittorrent >= v4.3.0 enforces CSRF by checking Referer on state-changing
        // POSTs; without it, pause/resume/delete are rejected with HTTP 4xx.
        let mut default_headers = reqwest::header::HeaderMap::new();
        default_headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_str(&base_url)
                .expect("base_url is a valid header value"),
        );

        let client = crate::utils::http::create_client_builder(global_config)
            .cookie_store(true) // Required for qBittorrent's cookie-based auth
            .danger_accept_invalid_certs(!config.verify_ssl)
            .default_headers(default_headers)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        QBittorrentHttp { client, base_url }
    }

    /// Snapshot the current HTTP transport (the client is a cheap Arc handle)
    /// so the std lock is never held across an await.
    fn http(&self) -> QBittorrentHttp {
        self.http.read().unwrap().clone()
    }

    /// Snapshot of the current settings (SSoT: all config reads go through
    /// here so the std lock is never held across an await).
    fn cfg(&self) -> QBittorrentSettings {
        self.settings.read().unwrap().clone()
    }

    // qBittorrent's API requires a session cookie. Version differences:
    //  - Modern (libtorrent 2.x, qB 5.x): HTTP 204 on success, 401 on bad creds.
    //  - Older (qB 4.x): "200 Ok." body on success, "200 Fails." on bad creds.
    // Handle both by checking the HTTP status first, then the body.
    /// Shared login logic used by both `login()` and `test_connection()`.
    /// Takes an explicit `client` so callers can use different TLS configurations
    /// (e.g. with/without SSL verification) for audit purposes.
    async fn login_inner(
        client: &Client,
        base_url: &str,
        username: &str,
        password: &str,
    ) -> Result<()> {
        let params = [("username", username), ("password", password)];
        let res = match client
            .post(format!("{}/auth/login", base_url))
            .form(&params)
            .send()
            .await
        {
            // No handshake (connect/timeout) — transient network problems,
            // safe to retry with backoff (plugin spec §8).
            Ok(res) => res,
            Err(e) if e.is_connect() => {
                return Err(anyhow::Error::new(PluginCallError::Transient(format!(
                    "Cannot reach qBittorrent at {} — check that the server is running and the URL is correct. Underlying error: {}",
                    base_url.trim_end_matches("/api/v2"),
                    describe_error_chain(&e)
                ))));
            }
            Err(e) if e.is_timeout() => {
                return Err(anyhow::Error::new(PluginCallError::Transient(format!(
                    "Connection to qBittorrent at {} timed out — check the server status and network connectivity. Underlying error: {}",
                    base_url.trim_end_matches("/api/v2"),
                    describe_error_chain(&e)
                ))));
            }
            Err(e) => {
                return Err(anyhow::Error::new(PluginCallError::Transient(format!(
                    "Failed to connect to qBittorrent at {} — {}",
                    base_url.trim_end_matches("/api/v2"),
                    describe_error_chain(&e)
                ))));
            }
        };

        // Check HTTP status code first (modern qBittorrent builds)
        let status = res.status();

        if status == reqwest::StatusCode::NO_CONTENT {
            // HTTP 204 No Content — success (modern qBittorrent)
            return Ok(());
        }

        if status == reqwest::StatusCode::UNAUTHORIZED {
            // HTTP 401 Unauthorized — bad credentials (modern qBittorrent).
            // Classified as AuthFailed: the backend's failure policy (plugin
            // spec §8) never retries rejected credentials and enters a
            // cooldown instead of hammering the server on every operation.
            return Err(anyhow::Error::new(PluginCallError::AuthFailed(
                "Check your username and password. Server returned HTTP 401 Unauthorized."
                    .to_string(),
            )));
        }

        // Fallback: read the response body (older qBittorrent builds)
        let text = res.text().await.unwrap_or_default();

        if text.trim() == "Ok." {
            Ok(())
        } else if text.trim().to_lowercase().contains("fails") {
            // "200 Fails" body — bad credentials (older qBittorrent builds).
            // Same AuthFailed classification as the 401 branch above.
            Err(anyhow::Error::new(PluginCallError::AuthFailed(format!(
                "Check your username and password. Server responded: {}",
                text.trim()
            ))))
        } else {
            let msg = format!(
                "Unexpected response from qBittorrent login (HTTP {}){} {}",
                status.as_u16(),
                if text.trim().is_empty() { "" } else { ": " },
                if text.trim().is_empty() {
                    "(empty body)"
                } else {
                    text.trim()
                }
            );
            // 5xx → server hiccup, safe to retry; other codes (e.g. 404 = wrong
            // URL path) are deterministic and retrying won't help (plugin spec §8).
            if status.is_server_error() {
                Err(anyhow::Error::new(PluginCallError::Transient(msg)))
            } else {
                Err(anyhow::Error::new(PluginCallError::Permanent(msg)))
            }
        }
    }

    pub async fn login(&self) -> Result<()> {
        let cfg = self.cfg();
        let http = self.http();
        Self::login_inner(&http.client, &http.base_url, &cfg.username, &cfg.password).await
    }

    // Re-login before every operation: qBittorrent cookies expire after
    // inactivity, and the extra round trip is negligible next to torrent
    // operations — simpler than tracking cookie expiry.
    async fn ensure_logged_in(&self) -> Result<()> {
        self.login().await
    }

    // Generic API Helpers

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let http = self.http();
        let url = format!("{}{}", http.base_url, endpoint);
        let resp = http
            .client
            .get(&url)
            .query(query)
            .send()
            .await?
            .error_for_status()?;

        Ok(resp.json::<T>().await?)
    }

    async fn post_form(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<()> {
        // Referer header is set as a default header on the Client (SSoT in build_http).
        let http = self.http();
        let url = format!("{}{}", http.base_url, endpoint);
        http.client
            .post(&url)
            .form(params)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    async fn list_torrents(&self, query: &[(&str, &str)]) -> Result<Vec<serde_json::Value>> {
        self.get_json("/torrents/info", query).await
    }

    async fn get_torrent_info(&self, hash: &str) -> Result<Option<serde_json::Value>> {
        let res: Vec<serde_json::Value> =
            self.get_json("/torrents/info", &[("hashes", hash)]).await?;
        Ok(res.into_iter().next())
    }
}

// Inherent method implementations, dispatched through `handle_custom_method()`.
impl QBittorrentClient {
    pub fn name(&self) -> &str {
        "qBittorrent"
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    // Add a torrent via multipart form data (not JSON), which qBittorrent's
    // /torrents/add endpoint requires even for URL/magnet adds. autoTMM=false
    // prevents automatic torrent management from overriding our savepath and
    // category.
    async fn add_download_impl(
        &self,
        url: &str,
        _category: &str,
        _tag: Option<&str>,
        title: Option<&str>,
    ) -> Result<()> {
        self.ensure_logged_in().await?;

        // Always use a UUID subfolder to isolate each download.
        // This prevents cleanup (unneeded/unexpected file deletion) from
        // accidentally affecting other downloads sharing the same directory.
        let subdir = uuid::Uuid::new_v4().to_string();
        let resolved_save_path = self.cfg().download_path.join(&subdir);
        let save_path_str = resolved_save_path.to_string_lossy().to_string();

        // All downloads use `default_category`; series names become tags, not
        // categories.
        let resolved_category = self.cfg().default_category;

        let mut form = reqwest::multipart::Form::new()
            .text("urls", url.to_string())
            .text("category", resolved_category)
            .text("savepath", save_path_str)
            .text("autoTMM", "false");

        if let Some(rn) = title {
            form = form.text("rename", rn.to_string());
        }

        let http = self.http();
        http.client
            .post(format!("{}/torrents/add", http.base_url))
            // Referer header is set as a default header on the Client (SSoT in build_http).
            .multipart(form)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    // Queries qBittorrent for completed torrents via the server-side
    // "filter=completed" parameter.
    async fn get_completed_downloads_impl(&self) -> Result<Vec<String>> {
        self.ensure_logged_in().await?;

        let res = self.list_torrents(&[("filter", "completed")]).await?;

        let hashes = res
            .iter()
            .filter_map(|t| {
                t.get("hash")
                    .and_then(|h| h.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        Ok(hashes)
    }

    async fn get_download_progress_impl(&self, id: &str) -> Result<Option<f32>> {
        self.ensure_logged_in().await?;

        if let Some(t) = self.get_torrent_info(id).await?
            && let Some(progress) = t.get("progress").and_then(|p| p.as_f64())
        {
            return Ok(Some(progress as f32));
        }

        Ok(None)
    }

    async fn get_download_status_impl(&self, id: &str) -> Result<Option<String>> {
        self.ensure_logged_in().await?;

        if let Some(t) = self.get_torrent_info(id).await? {
            return Ok(t
                .get("state")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()));
        }

        Ok(None)
    }

    /// Report a hard failure for the torrent, if any. qBittorrent's `error` and
    /// `missingFiles` states cannot recover by waiting, so no-progress tracking
    /// must not treat them as merely slow.
    async fn get_download_failure_impl(&self, id: &str) -> Result<Option<String>> {
        self.ensure_logged_in().await?;

        let Some(t) = self.get_torrent_info(id).await? else {
            return Ok(None);
        };
        let state = t.get("state").and_then(|s| s.as_str()).unwrap_or("");
        Ok(download_failure_for_state(state))
    }

    // Returns the path to the downloaded file(s) on the ORGANIZER's filesystem,
    // translating qBittorrent's view (see module docs). Logging here is
    // intentionally verbose — path issues are the #1 source of Docker support
    // requests.
    async fn get_download_content_path_impl(&self, id: &str) -> Result<String> {
        self.ensure_logged_in().await?;
        let settings = self.cfg();

        if let Some(t) = self.get_torrent_info(id).await? {
            let save_path = t.get("save_path").and_then(|v| v.as_str()).unwrap_or("");
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let content_path_raw = t.get("content_path").and_then(|p| p.as_str()).unwrap_or("");

            tracing::debug!(
                "[content_path] id={} name={:?} save_path='{}' content_path='{}'",
                id,
                name,
                save_path,
                content_path_raw,
            );

            // SSoT: save_path + name are always present from the moment the
            // torrent is added.  The pipeline's metadata() check in Stage 2
            // handles file-not-on-disk; we don't gate on completion state here.

            // Use content_path if available (most accurate), otherwise construct by
            // joining save_path + name (works for single-file torrents). When
            // content_path exists but points to qBittorrent's temp/incomplete
            // directory (doesn't start with download_path), rebase it onto the
            // configured save_path + filename from content_path.
            let path_to_translate = if !content_path_raw.is_empty() {
                let dl_path = &settings.download_path;
                // Use Path::starts_with, not string starts_with (see
                // OrganizationConfig::contains_path): a string match would treat
                // "/dl/My Show" as a prefix of "/dl/My Showcase/file.mkv".
                if Path::new(content_path_raw).starts_with(dl_path) {
                    content_path_raw.to_string()
                } else {
                    // content_path is in a temp dir; construct from save_path +
                    // filename. save_path includes the UUID subfolder qBittorrent
                    // created; using download_path would lose it and point to a
                    // non-existent path.
                    let filename = std::path::Path::new(content_path_raw)
                        .file_name()
                        .unwrap_or_else(|| std::ffi::OsStr::new(name));
                    std::path::Path::new(save_path)
                        .join(filename)
                        .to_string_lossy()
                        .to_string()
                }
            } else {
                std::path::Path::new(save_path)
                    .join(name)
                    .to_string_lossy()
                    .to_string()
            };

            tracing::debug!(
                "[content_path] path_to_translate='{}' download_path='{}' \
                 organizer_path={:?} use_separate_paths={}",
                path_to_translate,
                settings.download_path.display(),
                settings.organizer_path,
                settings.use_separate_paths
            );

            // Translate only when use_separate_paths is enabled and an
            // organizer_path is configured; otherwise return the raw path.
            if settings.use_separate_paths
                && let Some(ref org_path) = settings.organizer_path
            {
                let dl_path = &settings.download_path;
                if Path::new(&path_to_translate).starts_with(dl_path) {
                    let relative_path = path_to_translate
                        .strip_prefix(&dl_path.to_string_lossy().to_string())
                        .unwrap_or("");
                    let relative_path = relative_path.trim_start_matches(['/', '\\']);
                    let translated_path = org_path.join(relative_path);
                    tracing::debug!(
                        "[content_path] Translated path: '{}' (relative: '{}')",
                        translated_path.display(),
                        relative_path
                    );
                    return Ok(translated_path.to_string_lossy().to_string());
                } else {
                    tracing::debug!(
                        "[content_path] path_to_translate '{}' does NOT start with \
                         download_path '{}'. Path translation skipped.",
                        path_to_translate,
                        dl_path.display()
                    );
                }
            }

            return Ok(path_to_translate);
        }

        Err(anyhow::anyhow!("No download info found for id={}", id))
    }

    async fn pause_download_impl(&self, id: &str) -> Result<()> {
        self.ensure_logged_in().await?;
        self.post_form("/torrents/stop", &[("hashes", id)]).await?;
        Ok(())
    }

    async fn resume_download_impl(&self, id: &str) -> Result<()> {
        self.ensure_logged_in().await?;
        self.post_form("/torrents/start", &[("hashes", id)]).await?;
        Ok(())
    }

    // Fully processed: remove the torrent without deleting files (the organizer
    // already moved them).
    async fn complete_download_impl(&self, id: &str) -> Result<()> {
        self.ensure_logged_in().await?;

        let params = [("hashes", id), ("deleteFiles", "false")];
        self.post_form("/torrents/delete", &params).await?;

        Ok(())
    }

    async fn delete_download_impl(&self, id: &str, delete_files: bool) -> Result<()> {
        self.ensure_logged_in().await?;

        let params = [
            ("hashes", id),
            ("deleteFiles", if delete_files { "true" } else { "false" }),
        ];
        self.post_form("/torrents/delete", &params).await?;

        Ok(())
    }

    /// Test connectivity and audit SSL certificate validity.
    ///
    /// Performs two checks:
    /// 1. Try the login using the user's configured `verify_ssl` setting.
    /// 2. If `verify_ssl` is disabled and login succeeds, do an audit: try with
    ///    SSL verification enabled and warn if the certificate is invalid.
    /// 3. If `verify_ssl` is enabled and login fails, try without verification to
    ///    determine if the failure is due to an invalid certificate vs. a real
    ///    connectivity issue.
    pub async fn test_impl(&self) -> Result<()> {
        let cfg = self.cfg();
        let http = self.http();
        let login_result = self.login().await;

        if cfg.verify_ssl {
            match login_result {
                Ok(()) => {
                    tracing::info!(
                        "qBittorrent connection verified with SSL (certificate is valid)"
                    );
                    Ok(())
                }
                Err(e) => {
                    // Login failed with SSL enabled. Try without verification
                    // to see if it's a cert issue.
                    let no_verify_client = reqwest::Client::builder()
                        .cookie_store(true)
                        .danger_accept_invalid_certs(true)
                        .build()
                        .map_err(|ce| anyhow::anyhow!("Failed to build HTTP client: {}", ce))?;

                    match Self::login_inner(
                        &no_verify_client,
                        &http.base_url,
                        &cfg.username,
                        &cfg.password,
                    )
                    .await
                    {
                        Ok(()) => {
                            // Connection works *without* SSL but fails *with* it
                            Err(anyhow::anyhow!(
                                "SSL verification failed — the server's TLS certificate is invalid or self-signed. You can disable SSL verification in the settings, but this is not recommended for production use. Original error: {}",
                                e
                            ))
                        }
                        Err(no_ssl_err) => Err(anyhow::anyhow!(
                            "Connection failed (with and without SSL verification): {}",
                            no_ssl_err
                        )),
                    }
                }
            }
        } else {
            match login_result {
                Ok(()) => {
                    // Audit: login worked with SSL verification disabled — retry with
                    // verification to check cert validity.
                    let verify_client = reqwest::Client::builder()
                        .cookie_store(true)
                        .build()
                        .map_err(|ce| anyhow::anyhow!("Failed to build HTTP client: {}", ce))?;

                    match Self::login_inner(
                        &verify_client,
                        &http.base_url,
                        &cfg.username,
                        &cfg.password,
                    )
                    .await
                    {
                        Ok(()) => {
                            tracing::debug!(
                                "qBittorrent SSL certificate is valid (verification disabled)"
                            );
                        }
                        Err(_) => {
                            // Works *without* SSL but would fail *with* it.
                            tracing::warn!(
                                "qBittorrent connection succeeded (SSL verification disabled), but the server's TLS certificate is invalid or self-signed. For security, consider using a valid certificate from a trusted Certificate Authority."
                            );
                        }
                    }

                    Ok(())
                }
                Err(e) => {
                    // Login failed even without SSL — return the error
                    Err(e)
                }
            }
        }
    }

    fn download_path_impl(&self) -> Option<std::path::PathBuf> {
        Some(self.cfg().download_path)
    }

    fn organizer_path_impl(&self) -> Option<std::path::PathBuf> {
        self.cfg().organizer_path
    }

    // Pre-requeue hook: remove the existing errored torrent (without deleting the
    // downloaded data) so the item can be re-dispatched cleanly. Otherwise
    // `add_download` would collide with the stale torrent, or the client would
    // keep reporting the errored state.
    async fn retry_impl(&self, id: &str) -> Result<()> {
        self.ensure_logged_in().await?;
        tracing::info!(
            "Removing existing torrent {} from client before requeue",
            id
        );
        let params = [("hashes", id), ("deleteFiles", "false")];
        self.post_form("/torrents/delete", &params).await?;
        Ok(())
    }

    // Search by exact name match; iterates all torrents (no server-side name
    // filter available).
    async fn get_download_id_by_name_impl(&self, name: &str) -> Result<Option<String>> {
        self.ensure_logged_in().await?;

        let res = self.list_torrents(&[]).await?;

        for t in res {
            if let Some(t_name) = t.get("name").and_then(|n| n.as_str())
                && t_name == name
                && let Some(hash) = t.get("hash").and_then(|h| h.as_str())
            {
                return Ok(Some(hash.to_string()));
            }
        }

        Ok(None)
    }
}

static SUPPORTED_PROTOCOLS: LazyLock<Vec<String>> =
    LazyLock::new(|| vec!["magnet:*".to_string(), "*://*.torrent".to_string()]);

#[async_trait]
impl PluginInstance for QBittorrentClient {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn priority(&self) -> i32 {
        0
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        Some(&SUPPORTED_PROTOCOLS)
    }

    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        Self::plugin_info()
    }

    async fn test_impl(&self) -> Result<()> {
        self.test_impl().await
    }

    async fn set_config(&self, config: serde_json::Value) -> Result<serde_json::Value> {
        // Parse-first; on error return Err with zero mutation (the manager
        // falls back to factory-replacement).
        let new: QBittorrentSettings = serde_json::from_value(config)
            .map_err(|e| anyhow::anyhow!("Invalid qBittorrent configuration: {}", e))?;
        if let Err(e) = new.validate() {
            tracing::warn!(
                "QBittorrent config validation failed (loading anyway): {}",
                e
            );
        }
        let old = self.cfg();
        // Everything is applied in place: settings go behind the RwLock, and a
        // link/verify_ssl change rebuilds the HTTP transport (Referer/CSRF base
        // + TLS acceptance) with the same global config the factory would use —
        // the rebuilt client is byte-identical to a factory-constructed one, so
        // no rebuild is ever needed for a valid config.
        let transport_changed = new.link.trim_end_matches('/') != old.link.trim_end_matches('/')
            || new.verify_ssl != old.verify_ssl;
        *self.settings.write().unwrap() = new.clone();
        if transport_changed {
            *self.http.write().unwrap() = Self::build_http(&new, &self.global_config);
            tracing::info!("Reconfigured qBittorrent transport (link/verify_ssl) in place");
        }
        // Cookie-based auth re-logins before every operation, so credential
        // changes need no explicit session invalidation.
        tracing::info!("Reconfigured qBittorrent plugin in place");
        Ok(serde_json::Value::Bool(true))
    }

    async fn handle_custom_method(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        match method {
            "add_download" => {
                let url = extract_str(&params, "url")?;
                anyhow::ensure!(!url.is_empty(), "url must not be empty");
                let category = extract_str(&params, "category")?;
                let tag = extract_str_opt(&params, "tag");
                let title = extract_str_opt(&params, "title");
                self.add_download_impl(&url, &category, tag.as_deref(), title.as_deref())
                    .await?;
                Ok(serde_json::Value::Null)
            }
            "get_completed_downloads" => {
                let hashes = self.get_completed_downloads_impl().await?;
                Ok(serde_json::to_value(hashes)?)
            }
            "get_download_progress" => {
                let id = params
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing id for get_download_progress"))?;
                let progress = self.get_download_progress_impl(id).await?;
                Ok(serde_json::to_value(progress)?)
            }
            "get_download_status" => {
                let id = params
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing id for get_download_status"))?;
                let status = self.get_download_status_impl(id).await?;
                Ok(serde_json::to_value(status)?)
            }
            "get_download_failure" => {
                let id = params
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing id for get_download_failure"))?;
                let failure = self.get_download_failure_impl(id).await?;
                Ok(serde_json::to_value(failure)?)
            }
            "get_download_content_path" => {
                let id = params
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing id for get_download_content_path"))?;
                let path = self.get_download_content_path_impl(id).await?;
                Ok(serde_json::to_value(path)?)
            }
            "pause_download" => {
                let id = params
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing id for pause_download"))?;
                self.pause_download_impl(id).await?;
                Ok(serde_json::Value::Null)
            }
            "resume_download" => {
                let id = params
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing id for resume_download"))?;
                self.resume_download_impl(id).await?;
                Ok(serde_json::Value::Null)
            }
            "delete_download" => {
                let id = extract_str(&params, "id")?;
                let delete_files = extract_bool_opt(&params, "delete_files", false);
                self.delete_download_impl(&id, delete_files).await?;
                Ok(serde_json::Value::Null)
            }
            "get_download_id_by_name" => {
                let name = params
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing name for get_download_id_by_name"))?;
                let id = self.get_download_id_by_name_impl(name).await?;
                Ok(serde_json::to_value(id)?)
            }
            "complete_download" => {
                let id = params
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing id for complete_download"))?;
                self.complete_download_impl(id).await?;
                Ok(serde_json::Value::Null)
            }
            "retry" => {
                let id = params
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing id for retry"))?;
                self.retry_impl(id).await?;
                Ok(serde_json::Value::Null)
            }
            "get_download_path" => Ok(serde_json::to_value(
                self.download_path_impl()
                    .map(|p| p.to_string_lossy().to_string()),
            )?),
            "get_organizer_path" => Ok(serde_json::to_value(
                self.organizer_path_impl()
                    .map(|p| p.to_string_lossy().to_string()),
            )?),
            "test_connection" => {
                // The manager calls "test_connection"; delegate to test_impl.
                self.test_impl().await?;
                Ok(serde_json::json!(format!(
                    "Successfully connected to {}",
                    self.name()
                )))
            }
            _ => Err(anyhow::anyhow!(
                crate::plugins::PluginCallError::MethodNotSupported(method.to_string())
            )),
        }
    }
}

// Factory and static info. Multiple instances can exist (e.g. "primary" and
// "secondary" qBittorrent).
impl QBittorrentClient {
    fn from_config(
        config: &serde_json::Value,
        instance_id: &str,
        global_config: &std::sync::Arc<jumbie_shared::config::Config>,
    ) -> Result<std::sync::Arc<dyn PluginInstance>> {
        let cfg: QBittorrentSettings = serde_json::from_value(config.clone())?;

        if let Err(e) = cfg.validate() {
            tracing::warn!(
                "QBittorrent config validation failed (loading anyway): {}",
                e
            );
        }

        Ok(std::sync::Arc::new(Self::new(
            &cfg,
            instance_id,
            global_config.clone(),
        )))
    }
}

// Static Info & Config Schema
// The "order" field in each property controls field display order in the UI's
// config editor. The `x-enum-options` pattern (used for Nyaa's category selector)
// is NOT used here because qBittorrent settings are simple key-value fields.

/// Independent version of the built-in qBittorrent plugin.
/// Bump when this plugin's behavior changes — it versions the plugin, not the app.
pub const PLUGIN_VERSION: &str = "1.0.0";

impl QBittorrentClient {
    /// SSoT: Static metadata for this plugin. Also registered as the
    /// registry-level info so the two can never diverge.
    pub fn plugin_info() -> jumbie_shared::plugin::PluginTypeInfo {
        jumbie_shared::plugin::PluginTypeInfo {
            display_name: "qBittorrent".to_string(),
            version: PLUGIN_VERSION.to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "qBittorrent Web API client".to_string(),
            capabilities: vec![
                jumbie_shared::plugin::Capability::Downloader,
                jumbie_shared::plugin::Capability::CanPauseResume,
            ],
            supported_protocols: Some(SUPPORTED_PROTOCOLS.clone()),
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: true,
        }
    }

    pub fn config_schema() -> serde_json::Value {
        serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "QBittorrentSettings",
            "type": "object",
            "required": ["link"],
            "properties": {
                "name": { "type": "string", "title": "Name", "default": "qBittorrent", "order": -25 },
                "link": {
                    "type": "string",
                    "title": "Link*",
                    "description": "qBittorrent Web UI URL (e.g. http://localhost:8080)",
                    "default": "http://localhost:8080",
                    "order": 10
                },
                "username": { "type": "string", "title": "Username", "default": "admin", "order": 30 },
                "password": { "type": "string", "title": "Password", "order": 40 },
                "download_path": { "type": "string", "title": "Download Path", "default": "./downloads", "order": 50 },
                "use_separate_paths": {
                    "type": "boolean",
                    "title": "Use separate paths for container and organizer",
                    "default": false,
                    "order": 60
                },
                "organizer_path": {
                    "type": "string",
                    "title": "Jumbie Path (Local)",
                    "order": 65,
                    "dependsOn": {
                        "field": "use_separate_paths",
                        "value": true
                    }
                },
                "default_category": { "type": "string", "title": "Default Category", "default": "Series", "order": 90 },
                "verify_ssl": { "type": "boolean", "title": "Verify SSL", "default": false, "order": 110 }
            }
        })
    }
}

// Self-contained registration so the factory lives next to the plugin code.
impl QBittorrentClient {
    pub async fn register() {
        use crate::plugins::registry::InternalPluginRegistry;

        InternalPluginRegistry::register_full(
            "downloader.qbittorrent",
            |config, id, _priority, _refresh_interval, global_cfg, _shutdown_token| {
                Self::from_config(&config, &id, &global_cfg)
            },
            Self::config_schema,
            Self::plugin_info,
        )
        .await;
    }
}

/// Map a qBittorrent torrent state to a hard-failure reason.
///
/// Only states that cannot recover by waiting (a dead torrent, or files removed
/// from disk) are failures; stall-like states such as `stalledDL`/`metaDL` are
/// left to the no-progress detection.
fn download_failure_for_state(state: &str) -> Option<String> {
    match state {
        "error" => Some("qBittorrent reports the torrent has errored".to_string()),
        "missingFiles" => Some("qBittorrent reports missing files".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SSoT for what qBittorrent counts as a hard failure. Kept pure so the mapping
    /// is testable without a live client.
    #[test]
    fn test_download_failure_for_state() {
        // Recoverable-in-theory states are not failures.
        for state in [
            "downloading",
            "stalledDL",
            "metaDL",
            "queuedDL",
            "pausedDL",
            "checkingDL",
            "checkingResumeData",
            "forcedDL",
            "moving",
            "unknown",
            "",
        ] {
            assert_eq!(
                download_failure_for_state(state),
                None,
                "state '{state}' must not be treated as a hard failure"
            );
        }

        assert!(
            download_failure_for_state("error")
                .unwrap()
                .contains("errored")
        );
        assert!(
            download_failure_for_state("missingFiles")
                .unwrap()
                .contains("missing files")
        );
    }

    fn default_cfg() -> QBittorrentSettings {
        serde_json::from_value(serde_json::json!({})).unwrap()
    }

    #[test]
    fn test_qbittorrent_settings_defaults() {
        let cfg = default_cfg();
        assert_eq!(cfg.link, "http://localhost:8080");
        assert!(cfg.verify_ssl);
    }

    #[test]
    fn test_qbittorrent_validation() {
        let mut cfg = default_cfg();
        assert!(
            cfg.validate().is_ok(),
            "Default cfg with valid host/user should pass"
        );

        cfg.link = "".to_string();
        assert!(cfg.validate().is_err(), "Empty link should fail");

        cfg.link = "http://127.0.0.1:8080".to_string();
        cfg.username = "".to_string();
        assert!(cfg.validate().is_err(), "Empty username should fail");
    }

    #[tokio::test]
    async fn test_set_config_applies_settings() {
        let client = QBittorrentClient::new(
            &default_cfg(),
            "inst",
            Arc::new(jumbie_shared::config::Config::default()),
        );

        client
            .set_config(serde_json::json!({
                "username": "newuser",
                "password": "newpass",
                "default_category": "Movies",
            }))
            .await
            .unwrap();

        let s = client.cfg();
        assert_eq!(s.username, "newuser");
        assert_eq!(s.password, "newpass");
        assert_eq!(s.default_category, "Movies");

        // Invalid config → Err with zero mutation.
        assert!(
            client
                .set_config(serde_json::json!({ "username": 42 }))
                .await
                .is_err()
        );
        assert_eq!(client.cfg().username, "newuser");
    }

    #[tokio::test]
    async fn test_set_config_link_or_ssl_change_applies_in_place() {
        let client = QBittorrentClient::new(
            &default_cfg(),
            "inst",
            Arc::new(jumbie_shared::config::Config::default()),
        );

        // link / verify_ssl are applied IN PLACE now: the HTTP transport is
        // rebuilt (Referer/CSRF base + TLS acceptance) rather than signaling a
        // factory replacement — no rebuild is ever needed for a valid config.
        client
            .set_config(serde_json::json!({ "link": "http://localhost:9090" }))
            .await
            .unwrap();
        assert_eq!(client.cfg().link, "http://localhost:9090");
        assert_eq!(
            client.http.read().unwrap().base_url,
            "http://localhost:9090/api/v2"
        );

        client
            .set_config(serde_json::json!({ "verify_ssl": false }))
            .await
            .unwrap();
        assert!(!client.cfg().verify_ssl);
    }
}
