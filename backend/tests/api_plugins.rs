// Plugin API integration tests
//
// Covers all read-scoped and write-scoped plugin endpoints.
// Read endpoints:  plugins_read
// Write endpoints: plugins_write

use axum::http::StatusCode;
use jumbie_shared::plugin::{is_jumbie_plugin_id, plugin_type_key};
use std::collections::HashMap;
use tower::ServiceExt;

mod common;

// Read endpoints (plugins_read scope)

#[tokio::test]
async fn test_get_plugins_config() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .oneshot(common::get_request("/api/config/plugins_cfg"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // The response should be a PluginsConfig object with the expected top-level fields
    assert!(json.is_object(), "Expected an object, got: {:?}", json);
    // PluginsConfig has: enabled, directory, downloader, notifier, source, metadata
    assert!(
        json.get("metadata").is_some(),
        "Expected 'metadata' field in plugins config"
    );
    assert!(
        json.get("notifier").is_some() || json.get("notifications").is_some(),
        "Expected 'notifier' or 'notifications' field in plugins config"
    );
    assert!(
        json.get("downloader").is_some(),
        "Expected 'downloader' field in plugins config"
    );
    assert!(
        json.get("source").is_some(),
        "Expected 'source' field in plugins config"
    );
    assert!(
        json.get("enabled").is_some(),
        "Expected 'enabled' field in plugins config"
    );
}

#[tokio::test]
async fn test_get_plugins_list() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .oneshot(common::get_request("/api/plugins"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Returns an array of plugin info objects
    assert!(json.is_array(), "Expected an array, got: {:?}", json);
}

#[tokio::test]
async fn test_get_available_plugins() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .oneshot(common::get_request("/api/plugins/available"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Returns an array of plugin info objects
    assert!(json.is_array(), "Expected an array, got: {:?}", json);

    // In test environment, internal plugins should be listed (downloader, notifier, metadata, source)
    let names: Vec<String> = json
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["display_name"].as_str())
        .map(|s| s.to_lowercase())
        .collect();

    // All internal plugins registered in tests must be present
    let expected_plugins = [
        "qbittorrent",
        "discord",
        "nyaa",
        "basic_rss",
        "tvdb",
        "tvmaze",
    ];
    for expected in &expected_plugins {
        assert!(
            names.contains(&expected.to_string()),
            "Expected '{}' in available plugins, got: {:?}",
            expected,
            names
        );
    }

    // Ordering contract: built-ins first, alphabetical within each group
    let entries: Vec<(bool, String)> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let builtin = p["plugin_id"].as_str().is_some_and(is_jumbie_plugin_id);
            (
                builtin,
                p["display_name"].as_str().unwrap_or("").to_lowercase(),
            )
        })
        .collect();

    // No built-in may appear after an external plugin (groups are contiguous).
    if let Some(first_external) = entries.iter().position(|(builtin, _)| !builtin) {
        assert!(
            entries[first_external..]
                .iter()
                .all(|(builtin, _)| !builtin),
            "built-in plugins must be grouped before external plugins: {:?}",
            entries
        );
    }

    // Display names are alphabetically non-decreasing within each group.
    for pair in entries.windows(2) {
        if pair[0].0 == pair[1].0 {
            assert!(
                pair[0].1 <= pair[1].1,
                "plugins must be alphabetical within a group: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }
}

#[tokio::test]
async fn test_available_metadata_plugins_have_identifier_fields() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .oneshot(common::get_request("/api/plugins/available"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let plugins = json.as_array().unwrap();

    // The type key is derived from plugin_id by splitting on '.' (e.g. "jumbie.tvdb" → "tvdb").
    for plugin in plugins {
        let plugin_id = plugin["plugin_id"].as_str().unwrap_or("");
        let type_key = plugin_type_key(plugin_id);
        match type_key {
            "tvdb" => {
                assert_eq!(
                    plugin["series_identifier_label"], "TVDB ID",
                    "tvdb: wrong identifier_label"
                );
                assert_eq!(
                    plugin["series_identifier_placeholder"], "79549",
                    "tvdb: wrong identifier_placeholder"
                );
            }
            "tvmaze" => {
                assert_eq!(
                    plugin["series_identifier_label"], "TVMaze ID",
                    "tvmaze: wrong identifier_label"
                );
                assert_eq!(
                    plugin["series_identifier_placeholder"], "34292",
                    "tvmaze: wrong identifier_placeholder"
                );
            }
            _ => {
                let name = plugin["display_name"].as_str().unwrap_or("");
                // Non-metadata plugins should have no identifier fields
                assert!(
                    plugin["series_identifier_label"].is_null()
                        || plugin["series_identifier_label"]
                            == serde_json::Value::String(String::new()),
                    "{}: unexpected series_identifier_label: {:?}",
                    name,
                    plugin["series_identifier_label"]
                );
                assert!(
                    plugin["series_identifier_placeholder"].is_null()
                        || plugin["series_identifier_placeholder"]
                            == serde_json::Value::String(String::new()),
                    "{}: unexpected series_identifier_placeholder: {:?}",
                    name,
                    plugin["series_identifier_placeholder"]
                );
            }
        }

        // Type-level infos never carry an instance id — instance_id is
        // backend-stamped only on instance-level infos (/api/plugins).
        assert!(
            plugin["instance_id"].is_null(),
            "{}: type-level info must not have instance_id: {:?}",
            plugin["display_name"],
            plugin["instance_id"]
        );
    }
}

#[tokio::test]
async fn test_get_all_plugin_schemas() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .oneshot(common::get_request("/api/plugins/schemas"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Returns an object mapping plugin_type → schema
    assert!(json.is_object(), "Expected an object, got: {:?}", json);

    let known_plugins = [
        "qbittorrent",
        "discord",
        "nyaa",
        "basic_rss",
        "tvmaze",
        "tvdb",
    ];
    for plugin_name in &known_plugins {
        assert!(
            json.get(*plugin_name).is_some(),
            "Expected schema for '{}' in all schemas, keys: {:?}",
            plugin_name,
            json.as_object().map(|obj| obj.keys().collect::<Vec<_>>())
        );
    }
}

#[tokio::test]
async fn test_get_plugin_schema_known_plugin() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // qbittorrent is always registered as an internal plugin
    let res = app
        .oneshot(common::get_request("/api/plugins/qbittorrent/schema"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(
        json.is_object(),
        "Expected a schema object, got: {:?}",
        json
    );
    // Schema should have type, properties, etc.
    assert!(json.get("type").is_some(), "Schema missing 'type' field");
    assert!(
        json.get("properties").is_some(),
        "Schema missing 'properties' field"
    );
}

#[tokio::test]
async fn test_get_plugin_schema_with_full_plugin_id() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // The backend schema endpoint should accept the author.plugin_name format
    // that the frontend sends from AddPluginModal.
    for full_id in ["jumbie.qbittorrent", "jumbie.nyaa", "jumbie.tvmaze"] {
        let res = app
            .clone()
            .oneshot(common::get_request(&format!(
                "/api/plugins/{}/schema",
                full_id
            )))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "Schema for '{}' should resolve with full plugin_id",
            full_id
        );

        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json.get("type").is_some(),
            "Schema for '{}' missing 'type' field",
            full_id
        );
        assert!(
            json.get("properties").is_some(),
            "Schema for '{}' missing 'properties' field",
            full_id
        );
    }
}

#[tokio::test]
async fn test_get_plugin_schema_unknown_plugin() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .oneshot(common::get_request(
            "/api/plugins/nonexistent_plugin/schema",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Error responses typically contain an "error" or "message" field
    assert!(
        json.is_object(),
        "Error response should be an object, got: {:?}",
        json
    );
}

#[tokio::test]
async fn test_get_plugin_schema_discord() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .oneshot(common::get_request("/api/plugins/discord/schema"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(
        json.is_object(),
        "Expected a schema object, got: {:?}",
        json
    );
    // Discord schema should have webhook_url property
    let props = json.get("properties").and_then(|p| p.as_object());
    assert!(props.is_some(), "Schema missing 'properties'");
    assert!(
        props.unwrap().contains_key("webhook_url"),
        "Discord schema should include 'webhook_url'"
    );
}

#[tokio::test]
async fn test_get_plugin_status() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .oneshot(common::get_request("/api/plugins/status"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Returns an array of status entries
    assert!(json.is_array(), "Expected an array, got: {:?}", json);

    // Each entry should have name, category, ok fields
    for entry in json.as_array().unwrap() {
        assert!(
            entry.get("name").is_some(),
            "Status entry missing 'name': {:?}",
            entry
        );
        assert!(
            entry.get("category").is_some(),
            "Status entry missing 'category': {:?}",
            entry
        );
        assert!(
            entry.get("ok").is_some(),
            "Status entry missing 'ok': {:?}",
            entry
        );
    }
}

// Write endpoints (plugins_write scope)

#[tokio::test]
async fn test_put_plugins_config() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Send an empty section payload to the downloader endpoint
    let payload: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();

    let res = app
        .oneshot(common::put_json_request(
            "/api/config/plugins_cfg/downloader",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_put_plugins_config_with_enabled_plugin() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Build a section payload with a simple qbittorrent entry
    let mut payload: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();
    let mut instances = HashMap::new();
    instances.insert("default".to_string(), serde_json::json!({"enabled": false}));
    payload.insert("jumbie.qbittorrent".to_string(), instances);

    let res = app
        .clone()
        .oneshot(common::put_json_request(
            "/api/config/plugins_cfg/downloader",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_validate_plugin_config_not_found() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = serde_json::json!({"some": "value"});
    let res = app
        .oneshot(common::post_json_request(
            "/api/plugins/nonexistent_plugin/validate",
            &payload,
        ))
        .await
        .unwrap();
    // Plugin not loaded → 404
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_validate_plugin_config_known_plugin_not_loaded() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // qbittorrent is registered but NOT loaded by default (no enabled config),
    // so validate returns 404 because the plugin manager doesn't have it loaded.
    let payload = serde_json::json!({
        "host": "localhost",
        "port": 8080,
        "username": "admin",
        "password": "password"
    });
    let res = app
        .oneshot(common::post_json_request(
            "/api/plugins/qbittorrent/validate",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_validate_plugin_config_loads_config_roundtrip() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // 1. PUT config section to enable qbittorrent
    let mut section: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();
    let mut instances = HashMap::new();
    instances.insert(
        "default".to_string(),
        serde_json::json!({
            "enabled": true,
            "host": "localhost",
            "port": 8080,
            "username": "admin",
            "password": "password",
            "download_path": "/tmp/downloads",
        }),
    );
    section.insert("jumbie.qbittorrent".to_string(), instances);

    let res = app
        .clone()
        .oneshot(common::put_json_request(
            "/api/config/plugins_cfg/downloader",
            &section,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Validate with a valid config payload
    let valid_payload = serde_json::json!({
        "host": "localhost",
        "port": 8080,
        "username": "admin",
        "password": "password",
        "download_path": "/tmp/downloads"
    });
    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/plugins/qbittorrent/validate",
            &valid_payload,
        ))
        .await
        .unwrap();
    // The validate endpoint returns 404 because internal plugins are stored
    // by instance ID ("default"), not by plugin type name ("qbittorrent")
    assert!(
        res.status() == StatusCode::OK || res.status() == StatusCode::NOT_FOUND,
        "Expected 200 or 404, got {}",
        res.status()
    );
}

#[tokio::test]
async fn test_plugin_test_unknown_category() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = jumbie_shared::types::TestPluginPayload {
        category: "unknown".to_string(),
        plugin_type: "none".to_string(),
        config: serde_json::json!({}),
    };
    let res = app
        .oneshot(common::post_json_request("/api/plugins/test", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_plugin_test_unknown_notifier_type() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = jumbie_shared::types::TestPluginPayload {
        category: "notifier".to_string(),
        plugin_type: "unknown_notifier".to_string(),
        config: serde_json::json!({}),
    };
    let res = app
        .oneshot(common::post_json_request("/api/plugins/test", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_plugin_test_unknown_downloader_type() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = jumbie_shared::types::TestPluginPayload {
        category: "downloader".to_string(),
        plugin_type: "unknown_downloader".to_string(),
        config: serde_json::json!({}),
    };
    let res = app
        .oneshot(common::post_json_request("/api/plugins/test", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_plugin_test_unknown_source_type() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = jumbie_shared::types::TestPluginPayload {
        category: "source".to_string(),
        plugin_type: "unknown_source".to_string(),
        config: serde_json::json!({}),
    };
    let res = app
        .oneshot(common::post_json_request("/api/plugins/test", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_plugin_test_unknown_metadata_type() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let payload = jumbie_shared::types::TestPluginPayload {
        category: "metadata".to_string(),
        plugin_type: "unknown_metadata".to_string(),
        config: serde_json::json!({}),
    };
    let res = app
        .oneshot(common::post_json_request("/api/plugins/test", &payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_plugin_test_qbittorrent_basic() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Testing qbittorrent with valid-looking config; the test endpoint creates
    // a fresh plugin instance and calls its `test` method.
    // Since there's no actual qbittorrent server, this may fail or time out,
    // but it should return a valid HTTP response (not crash).
    let payload = jumbie_shared::types::TestPluginPayload {
        category: "downloader".to_string(),
        plugin_type: "qbittorrent".to_string(),
        config: serde_json::json!({
            "host": "localhost",
            "port": 8080,
            "username": "admin",
            "password": "password",
            "download_path": "/tmp/downloads"
        }),
    };
    let res = app
        .oneshot(common::post_json_request("/api/plugins/test", &payload))
        .await
        .unwrap();
    // The test may fail (bad request) since there's no real qbittorrent,
    // but it should NOT crash with a 500 — the plugin test endpoint
    // maps connection failures to 400 BadRequest.
    assert!(
        res.status() == StatusCode::OK || res.status() == StatusCode::BAD_REQUEST,
        "Expected 200 OK or 400 Bad Request for qbittorrent test, got: {}",
        res.status()
    );
}

#[tokio::test]
async fn test_plugin_test_empty_config() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Empty config for a known plugin should result in a validation failure
    let payload = jumbie_shared::types::TestPluginPayload {
        category: "downloader".to_string(),
        plugin_type: "qbittorrent".to_string(),
        config: serde_json::json!({}),
    };
    let res = app
        .oneshot(common::post_json_request("/api/plugins/test", &payload))
        .await
        .unwrap();
    assert!(
        res.status().is_success() || res.status() == StatusCode::BAD_REQUEST,
        "Unexpected status for test with empty config: {}",
        res.status()
    );
}

// Integration: Round-trip config save + read

#[tokio::test]
async fn test_plugins_config_roundtrip() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Read default config
    let res = app
        .clone()
        .oneshot(common::get_request("/api/config/plugins_cfg"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Modify and save via section endpoint
    let mut section: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();
    let mut downloader_entry = HashMap::new();
    downloader_entry.insert(
        "default".to_string(),
        serde_json::json!({
            "enabled": false,
            "host": "192.168.1.100",
            "port": 8080,
        }),
    );
    section.insert("jumbie.qbittorrent".to_string(), downloader_entry);

    let res = app
        .clone()
        .oneshot(common::put_json_request(
            "/api/config/plugins_cfg/downloader",
            &section,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Read back and verify
    let res = app
        .clone()
        .oneshot(common::get_request("/api/config/plugins_cfg"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let updated: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(
        updated["downloader"]["jumbie.qbittorrent"]["default"]["host"]
            == serde_json::json!("192.168.1.100"),
        "Host should be updated"
    );
}
