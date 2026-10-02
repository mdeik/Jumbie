// Test utility module — each integration-test binary links only the subset of
// helpers it uses, so the rest are legitimately dead from that binary's point of
// view. This cannot be expressed with `cfg(test)`: `dead_code` tracks
// per-crate reachability, and these helpers are compiled into each binary's own
// private module. Silencing it per binary is the idiomatic fix; removing it
// entirely would mean exposing the helpers through the `jumbie` library.
#![allow(dead_code)]

use axum::{Router, body::Body, http::Request};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use jumbie::api::{AppState, RouterConfig, create_router};
use jumbie::db::DbManager;
use jumbie::db::episodes::InsertEpisodeParams;
use jumbie::organizer::ContentOrganizer;
use jumbie::plugins::PluginInstance;
use jumbie_shared::config::Config;
use std::collections::HashMap;
use std::collections::HashSet;
use tempfile::TempDir;

/// Check whether `ffmpeg` (and `ffprobe`) are on PATH.
/// Delegates to `jumbie::ffmpeg_installed()` which reflects the cfg flag
/// set by build.rs — the detection logic lives in exactly one place.
pub fn ffprobe_available() -> bool {
    jumbie::ffmpeg_installed()
}

/// Set `collision_handling` ("rename" | "skip" | "overwrite") for a test app.
/// Tests that intentionally create a series at an already-existing folder use
/// "overwrite" to exercise the claim-existing-content path (the default is
/// "rename", which would suffix the folder instead).
///
/// Mirrors the real settings SSoT: the live in-memory config is updated and then
/// persisted to the DB (`ConfigManager::persist_db`). Both must agree — the
/// organizer reads the DB copy while API handlers read the in-memory one.
pub async fn set_collision_handling(state: &Arc<AppState>, mode: &str) {
    {
        let mut cfg = state.cfg.write().await;
        cfg.organization.collision_handling = mode.to_string();
    }
    state
        .cfg
        .persist_db()
        .await
        .expect("persist test organization config");
}

/// Create `InsertEpisodeParams` for a "missing" episode with a fixed meta_date
/// of 2024-01-01. Useful for tests that need a basic missing episode row.
pub fn make_missing_episode_params<'a>(
    episode_id: &'a str,
    series_id: &'a str,
    season: i32,
    episode: i32,
    metadata_ids: &'a HashMap<String, String>,
) -> InsertEpisodeParams<'a> {
    InsertEpisodeParams {
        episode_id,
        series_id,
        season,
        episode,
        quality_profile_id: None,
        status: "missing",
        meta_date: Some(chrono::NaiveDateTime::new(
            chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
        )),
        est_date: None,
        metadata_ids,
        description: None,
        runtime: None,
        image_url: None,
        metadata_source: None,
        numbering_mode: None,
        file_path: None,
        title: None,
    }
}

/// The three release dates of an episode, as used by episode-seeding helpers.
/// Mirrors `InsertEpisodeParams`'s date fields (meta_date / upload_date / est_date).
pub struct EpisodeDates {
    pub meta_date: Option<chrono::NaiveDateTime>,
    pub source_date: Option<chrono::NaiveDateTime>,
    pub est_date: Option<chrono::NaiveDateTime>,
}

pub fn to_update_payload(config: &Config) -> jumbie_shared::types::UpdateConfigPayload {
    jumbie_shared::types::UpdateConfigPayload {
        organization: Some(config.organization.clone()),
        sources: Some(config.sources.clone()),
        general: Some(config.general.clone()),
        proxy: Some(config.proxy.clone()),
        auth: Some(config.auth.clone()),
        security: Some(config.security.clone()),
    }
}

pub async fn get_json(app: &Router, path: &str) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let res = app.clone().oneshot(get_request(path)).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

pub async fn put_json<T: Serialize>(
    app: &Router,
    path: &str,
    payload: &T,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let res = app
        .clone()
        .oneshot(put_json_request(path, payload))
        .await
        .unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

pub async fn post_json<T: Serialize>(
    app: &Router,
    path: &str,
    payload: &T,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let res = app
        .clone()
        .oneshot(post_json_request(path, payload))
        .await
        .unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

#[path = "../../src/tests/fixtures.rs"]
pub mod test_fixtures;

/// Helper to create a temporary executable script for testing IPC.
/// The script loops, reads JSON-RPC requests from stdin, and writes responses to stdout.
pub struct TestScript {
    pub path: PathBuf,
}

impl TestScript {
    pub fn new(script_content: &str) -> Self {
        let dir = std::env::temp_dir();
        let filename = format!("series_org_test_{}.py", uuid::Uuid::new_v4());
        let path = dir.join(filename);

        fs::write(&path, script_content).expect("Failed to write test script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }

        Self { path }
    }
}

impl Drop for TestScript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Ensure Python is available, otherwise return None (SSoT: the interpreter
/// pick lives in `jumbie_shared::plugin::python_command`).
pub fn init_python_script() -> Option<TestScript> {
    jumbie_shared::plugin::python_command()?;

    let logic = r#"            if method == "echo":
                print(json.dumps({"jsonrpc": "2.0", "result": params.get("msg"), "id": req_id, "auth": req_auth}), flush=True)
            elif method == "crash":
                sys.exit(1)
            elif method == "health_check":
                print(json.dumps({"jsonrpc": "2.0", "result": "ok", "id": req_id, "auth": req_auth}), flush=True)
"#;
    let script = jumbie_shared::plugin::PYTHON_PLUGIN_SCRIPT.replace("__LOGIC__", logic);

    Some(TestScript::new(&script))
}

pub async fn setup_test_app() -> (Router, Arc<AppState>, TempDir) {
    setup_test_app_with_logs("/logs").await
}

/// Like [`setup_test_app`], but binds the log-file endpoints to `logs_dir`
/// (a fixture directory the test controls) instead of the default `/logs`.
pub async fn setup_test_app_with_logs(logs_dir: &str) -> (Router, Arc<AppState>, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");

    let db_manager = Arc::new(
        DbManager::new(&db_path)
            .await
            .expect("Failed to initialize test DB"),
    );

    // Built from JSON to bypass file/macro issues with Default.
    let config_json = serde_json::json!({
        "database": db_path.to_string_lossy().to_string(),
        "sources": [],
        "downloader": {
            "client_type": "qbittorrent",
            "host": "localhost",
            "port": 8080,
            "username": "admin",
            "password": "password",
            "download_path": temp_dir.path().join("downloads").to_string_lossy().to_string(),
            "enabled": true,
            "use_series_tags": true,
            "default_category": "Series",
            "verify_ssl": false,
            "priority": 0,
            "use_separate_paths": false,
            "name": "qBit"
        },
        "global_filters": {
            "required": [],
            "excluded": [],
            "regex_required": [],
            "regex_excluded": [],
            "case_sensitive": false,
            "match_mode": "substring",
            "search_in_description": false,
            "search_in_files": false
        },
        "series_mappings": {},
        "auth": {
            "password": null,
            "banned_ips": []
        },
        "organization": {
            "destination_roots": [{ "path": temp_dir.path().join("organized").to_string_lossy().to_string() }],
            "collision_handling": "rename",
            "season_folder_format": "S${season:02}",
            "episode_file_format": "${series} - S${season:02}E${episode:02} - ${title}",
            "season_folder_format_absolute": "S01",
            "episode_file_format_absolute": "${series} - S${season:02}E${episode:02} - ${title}",
            "rename_episodes": true,
            "auto_apply_renames": false
        }
    });
    let config: Config = serde_json::from_value(config_json).expect("Failed to parse mock config");

    jumbie::plugins::internal::register_all().await;

    let plugin_manager = Arc::new(RwLock::new(jumbie::plugins::PluginManager::new(
        temp_dir.path().join("plugins"),
    )));

    let modifying_series: Arc<RwLock<HashSet<String>>> = Arc::new(RwLock::new(HashSet::new()));
    let organizer = ContentOrganizer::new(
        &config.database,
        db_manager.clone(),
        plugin_manager.clone(),
        tokio_util::sync::CancellationToken::new(),
        modifying_series,
    )
    .await
    .expect("Failed to create organizer");

    // Minimal TOML config for ConfigManager to load. Forward slashes are used
    // because TOML double-quoted strings treat `\` as an escape, so Windows
    // paths like `C:\Users\...` would fail to parse; all major OSes accept
    // forward slashes.
    let config_path = temp_dir.path().join("config.toml");
    let db_path_toml = db_path.to_string_lossy().replace('\\', "/");
    let tmp_dir_toml = temp_dir
        .path()
        .join("jumbie_tmp")
        .to_string_lossy()
        .replace('\\', "/");
    let plugins_toml = temp_dir
        .path()
        .join("plugins")
        .to_string_lossy()
        .replace('\\', "/");
    let logs_toml = temp_dir
        .path()
        .join("logs")
        .to_string_lossy()
        .replace('\\', "/");
    std::fs::write(
        &config_path,
        format!(
            "database = \"{db_path_toml}\"\nunknown_files_tmp_dir = \"{tmp_dir_toml}\"\nplugins_dir = \"{plugins_toml}\"\nlogs_dir = \"{logs_toml}\"\n",
        ),
    )
    .expect("Failed to write test config.toml");

    // ConfigManager reads the organization section from the DB, so the test's
    // custom destination_root must be seeded there before it is constructed.
    db_manager
        .save_organization_config(&config.organization)
        .await
        .expect("Failed to seed organization config for test");

    let cfg_manager = Arc::new(
        jumbie::config_manager::ConfigManager::new(
            db_manager.clone(),
            config_path.to_str().unwrap(),
        )
        .await
        .expect("Failed to create ConfigManager"),
    );

    let (router, state) = create_router(RouterConfig {
        cfg: cfg_manager,
        db: db_manager,
        downloader: Some(organizer.downloader()),
        notifications: Some(organizer.notifications()),
        organizer: Some(organizer.clone()),
        plugin_manager,
        logs_dir: logs_dir.to_string(),
        log_level: "info".to_string(),
        log_buffer: Arc::new(std::sync::RwLock::new(jumbie_shared::types::LogRing::new())),
        shutdown_token: tokio_util::sync::CancellationToken::new(),
    })
    .await;

    (router, state, temp_dir)
}

pub async fn setup_authenticated_app() -> (Router, Arc<AppState>, TempDir) {
    let (app, state, temp_dir) = setup_test_app().await;

    let hash = jumbie::auth_utils::hash_password("password").unwrap();
    state
        .db
        .set_user_password_hash("admin", &hash)
        .await
        .unwrap();

    (app, state, temp_dir)
}

pub fn post_json_request<T: Serialize>(uri: &str, payload: &T) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(payload).unwrap()))
        .unwrap()
}

pub fn put_json_request<T: Serialize>(uri: &str, payload: &T) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(payload).unwrap()))
        .unwrap()
}

pub fn get_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

pub fn delete_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

pub fn delete_json_request<T: Serialize>(uri: &str, payload: &T) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(payload).unwrap()))
        .unwrap()
}

pub fn post_empty_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap()
}

pub async fn response_body(res: axum::response::Response<Body>) -> bytes::Bytes {
    axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
}

/// Parse the body into JSON without asserting on the HTTP status code — the
/// caller checks the status separately.
pub async fn response_json<T: serde::de::DeserializeOwned>(
    res: axum::response::Response<Body>,
) -> T {
    let body = response_body(res).await;
    if body.is_empty() {
        serde_json::from_str("null").unwrap()
    } else {
        serde_json::from_slice(&body).unwrap()
    }
}

/// Send a request to the app router and return the raw response.
pub async fn send_request(
    app: &axum::Router,
    req: Request<Body>,
) -> axum::response::Response<Body> {
    use tower::ServiceExt;
    app.clone().oneshot(req).await.unwrap()
}

/// GET returning (status, deserialized JSON); does NOT assert success.
pub async fn get_json_raw<T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    uri: &str,
) -> (axum::http::StatusCode, T) {
    let res = send_request(app, get_request(uri)).await;
    let status = res.status();
    let body = response_json(res).await;
    (status, body)
}

/// POST with a JSON payload returning (status, deserialized JSON); does NOT
/// assert success.
pub async fn post_json_raw<P: Serialize, T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    uri: &str,
    payload: &P,
) -> (axum::http::StatusCode, T) {
    let res = send_request(app, post_json_request(uri, payload)).await;
    let status = res.status();
    let body = response_json(res).await;
    (status, body)
}

/// PUT with a JSON payload returning (status, deserialized JSON); does NOT
/// assert success.
pub async fn put_json_raw<P: Serialize, T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    uri: &str,
    payload: &P,
) -> (axum::http::StatusCode, T) {
    let res = send_request(app, put_json_request(uri, payload)).await;
    let status = res.status();
    let body = response_json(res).await;
    (status, body)
}

pub async fn create_test_series(app: &axum::Router, title: &str) -> String {
    use tower::ServiceExt;
    let payload = test_fixtures::minimal_test_request(title);
    let req = post_json_request("/api/series", &payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        axum::http::StatusCode::CREATED,
        "Failed to create test series '{title}'"
    );
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let id: String = serde_json::from_slice(&body).unwrap();
    id.trim_matches('"').to_string()
}

#[async_trait::async_trait]
pub trait TestApp {
    async fn get_json<T: serde::de::DeserializeOwned>(&self, uri: &str) -> T;
    async fn post_json<P: Serialize + Sync, R: serde::de::DeserializeOwned>(
        &self,
        uri: &str,
        payload: &P,
    ) -> R;
    async fn put_json<P: Serialize + Sync, R: serde::de::DeserializeOwned>(
        &self,
        uri: &str,
        payload: &P,
    ) -> R;
    async fn delete_json<P: Serialize + Sync, R: serde::de::DeserializeOwned>(
        &self,
        uri: &str,
        payload: &P,
    ) -> R;
    async fn delete_request_ok(&self, uri: &str);
}

#[async_trait::async_trait]
impl TestApp for Router {
    async fn get_json<R: serde::de::DeserializeOwned>(&self, uri: &str) -> R {
        use tower::ServiceExt;
        let res = self.clone().oneshot(get_request(uri)).await.unwrap();
        assert!(
            res.status().is_success(),
            "GET {uri} failed with status {}",
            res.status()
        );
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        if body.is_empty() {
            return serde_json::from_str("null").unwrap();
        }
        serde_json::from_slice(&body).unwrap()
    }

    async fn post_json<P: Serialize + Sync, R: serde::de::DeserializeOwned>(
        &self,
        uri: &str,
        payload: &P,
    ) -> R {
        use tower::ServiceExt;
        let res = self
            .clone()
            .oneshot(post_json_request(uri, payload))
            .await
            .unwrap();
        assert!(
            res.status().is_success(),
            "POST {uri} failed with status {}",
            res.status()
        );
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        if body.is_empty() {
            return serde_json::from_str("null").unwrap();
        }
        serde_json::from_slice(&body).unwrap()
    }

    async fn put_json<P: Serialize + Sync, R: serde::de::DeserializeOwned>(
        &self,
        uri: &str,
        payload: &P,
    ) -> R {
        use tower::ServiceExt;
        let res = self
            .clone()
            .oneshot(put_json_request(uri, payload))
            .await
            .unwrap();
        assert!(
            res.status().is_success(),
            "PUT {uri} failed with status {}",
            res.status()
        );
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        if body.is_empty() {
            return serde_json::from_str("null").unwrap();
        }
        serde_json::from_slice(&body).unwrap()
    }

    async fn delete_json<P: Serialize + Sync, R: serde::de::DeserializeOwned>(
        &self,
        uri: &str,
        payload: &P,
    ) -> R {
        use tower::ServiceExt;
        let res = self
            .clone()
            .oneshot(delete_json_request(uri, payload))
            .await
            .unwrap();
        assert!(
            res.status().is_success(),
            "DELETE {uri} failed with status {}",
            res.status()
        );
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        if body.is_empty() {
            return serde_json::from_str("null").unwrap();
        }
        serde_json::from_slice(&body).unwrap()
    }

    async fn delete_request_ok(&self, uri: &str) {
        use tower::ServiceExt;
        let res = self.clone().oneshot(delete_request(uri)).await.unwrap();
        assert!(
            res.status().is_success(),
            "DELETE {uri} failed with status {}",
            res.status()
        );
    }
}

// Mock metadata plugin

/// A configurable mock metadata plugin for testing.
///
/// Construct this with the desired display name, capabilities, and response
/// values so each test file can customise the mock without duplicating the
/// [`PluginInstance`] implementation.
pub struct MockMetadataPlugin {
    pub instance_id: String,
    pub display_name: String,
    pub capabilities: Vec<jumbie_shared::plugin::Capability>,
    pub series_identifier_label: Option<String>,
    pub series_name: String,
    pub overview: String,
    pub aliases: Vec<String>,
    /// Episodes returned by `fetch_series_metadata`.  Empty by default — a
    /// metadata ID with zero episodes is treated as an invalid ID by
    /// `fetch_metadata_for_series` ("No episodes found").
    pub episodes: Vec<serde_json::Value>,
}

#[async_trait::async_trait]
impl PluginInstance for MockMetadataPlugin {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn plugin_info(&self) -> jumbie_shared::plugin::PluginTypeInfo {
        jumbie_shared::plugin::PluginTypeInfo {
            display_name: self.display_name.clone(),
            version: "0.0.0".to_string(),
            author: "test".to_string(),
            description: String::new(),
            capabilities: self.capabilities.clone(),
            supported_protocols: None,
            series_identifier_label: self.series_identifier_label.clone(),
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        }
    }

    fn priority(&self) -> i32 {
        0
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }

    async fn handle_custom_method(
        &self,
        method: &str,
        _params: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        match method {
            "fetch_series_metadata" => Ok(serde_json::json!({
                "episodes_and_seasons": {
                    "episodes": &self.episodes,
                    "seasons": []
                },
                "series_info": null
            })),
            "get_updated_series" => Ok(serde_json::json!([])),
            "fetch_series_info" => Ok(serde_json::json!({
                "name": self.series_name,
                "overview": self.overview,
                "original_country": null,
                "aliases": {}
            })),
            "fetch_series_aliases" => Ok(serde_json::json!(self.aliases)),
            _ => Err(anyhow::anyhow!(jumbie::plugins::PluginCallError::MethodNotSupported(method.to_string()))),
        }
    }
}

/// Minimal `PluginTypeInfo` for tests that mock the "available plugins" endpoint.
pub fn mock_metadata_plugin_info() -> Vec<jumbie_shared::plugin::PluginTypeInfo> {
    vec![jumbie_shared::plugin::PluginTypeInfo {
        display_name: "metadata_test".to_string(),
        version: "1.0.0".to_string(),
        description: String::new(),
        author: String::new(),
        capabilities: vec![jumbie_shared::plugin::Capability::MetadataProviderNormal],
        supported_protocols: None,
        series_identifier_label: None,
        series_identifier_placeholder: None,
        rate_limit: None,
        supports_test: false,
    }]
}
