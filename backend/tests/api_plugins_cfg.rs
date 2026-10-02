// Plugins Config API Tests
//
// Covers the GET and PUT /api/config/plugins_cfg endpoints for reading and
// writing the application's plugin configuration.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use tower::ServiceExt;

// GET /api/config/plugins_cfg

#[tokio::test]
async fn test_get_plugins_config_ok() {
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

    assert!(json.is_object(), "Expected object, got: {:?}", json);
    assert!(json.get("enabled").is_some(), "Missing 'enabled' field");
    assert!(
        json.get("downloader").is_some(),
        "Missing 'downloader' field"
    );
    assert!(
        json.get("notifier").is_some() || json.get("notifications").is_some(),
        "Missing 'notifier'/'notifications' field"
    );
    assert!(json.get("source").is_some(), "Missing 'source' field");
    assert!(json.get("metadata").is_some(), "Missing 'metadata' field");
}

#[tokio::test]
async fn test_get_plugins_config_default_values() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let cfg: jumbie_shared::config::PluginsConfig = app.get_json("/api/config/plugins_cfg").await;

    // Default PluginsConfig should have disabled and empty categories
    assert!(!cfg.enabled, "Expected plugins to be disabled by default");
    assert!(
        cfg.downloader.is_empty(),
        "Expected empty downloader config"
    );
    assert!(cfg.source.is_empty(), "Expected empty source config");
    assert!(cfg.metadata.is_empty(), "Expected empty metadata config");
}

// PUT /api/config/plugins_cfg

#[tokio::test]
async fn test_put_plugins_section_empty_clears() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Seed a downloader instance
    let mut instances = std::collections::HashMap::new();
    let mut config = std::collections::HashMap::new();
    config.insert("name".to_string(), serde_json::json!("Test DL"));
    instances.insert("inst_001".to_string(), serde_json::json!(config));

    let mut section_data = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    section_data.insert("jumbie.qbittorrent".to_string(), instances);

    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/downloader", &section_data)
        .await;

    // Now clear the section with an empty payload
    let empty = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    let resp: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/downloader", &empty)
        .await;
    assert!(
        resp.config.downloader.is_empty(),
        "Downloader section should be empty after PUT with empty data"
    );

    let fetched: jumbie_shared::config::PluginsConfig =
        app.get_json("/api/config/plugins_cfg").await;
    assert!(
        fetched.downloader.is_empty(),
        "GET should confirm downloader section is empty"
    );
}

#[tokio::test]
async fn test_put_plugins_config_enabled() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Seed an enabled downloader instance via the section endpoint
    let mut instances = std::collections::HashMap::new();
    let mut config = std::collections::HashMap::new();
    config.insert("name".to_string(), serde_json::json!("qBit"));
    config.insert("enabled".to_string(), serde_json::json!(true));
    config.insert("host".to_string(), serde_json::json!("localhost"));
    instances.insert("inst_001".to_string(), serde_json::json!(config));

    let mut section_data = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    section_data.insert("jumbie.qbittorrent".to_string(), instances);

    let resp: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/downloader", &section_data)
        .await;

    let dl = &resp.config.downloader;
    assert!(dl.contains_key("jumbie.qbittorrent"));
    assert_eq!(dl["jumbie.qbittorrent"]["inst_001"]["enabled"], true);
}

#[tokio::test]
async fn test_put_plugins_config_roundtrip() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Seed a downloader instance via the section endpoint
    let mut instances = std::collections::HashMap::new();
    let mut config = std::collections::HashMap::new();
    config.insert("name".to_string(), serde_json::json!("qBit"));
    config.insert("host".to_string(), serde_json::json!("localhost"));
    instances.insert("inst_001".to_string(), serde_json::json!(config));

    let mut section_data = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    section_data.insert("jumbie.qbittorrent".to_string(), instances);

    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/downloader", &section_data)
        .await;

    // Read the config back
    let fetched: jumbie_shared::config::PluginsConfig =
        app.get_json("/api/config/plugins_cfg").await;
    assert!(
        fetched.downloader.contains_key("jumbie.qbittorrent"),
        "Downloader instance should survive round-trip"
    );
    assert_eq!(
        fetched.downloader["jumbie.qbittorrent"]["inst_001"]["name"],
        "qBit"
    );
}

// PUT /api/config/plugins_cfg/{section}

#[tokio::test]
async fn test_put_plugins_section_add_instance() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Insert a downloader instance via the section endpoint
    let mut instances = std::collections::HashMap::new();
    let mut config = std::collections::HashMap::new();
    config.insert("name".to_string(), serde_json::json!("Test Downloader"));
    config.insert("enabled".to_string(), serde_json::json!(true));
    config.insert("host".to_string(), serde_json::json!("localhost"));
    config.insert("port".to_string(), serde_json::json!(8080));
    instances.insert("inst_001".to_string(), serde_json::json!(config));

    let mut section_data = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    section_data.insert("jumbie.qbittorrent".to_string(), instances);

    let resp: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/downloader", &section_data)
        .await;
    let full_cfg = &resp.config;

    assert!(
        full_cfg.downloader.contains_key("jumbie.qbittorrent"),
        "Returned config should contain 'jumbie.qbittorrent'"
    );
    let qb_instances = full_cfg.downloader.get("jumbie.qbittorrent").unwrap();
    assert!(
        qb_instances.contains_key("inst_001"),
        "Returned config should contain 'inst_001'"
    );
    assert_eq!(qb_instances["inst_001"]["name"], "Test Downloader");

    let fetched: jumbie_shared::config::PluginsConfig =
        app.get_json("/api/config/plugins_cfg").await;
    assert!(
        fetched.downloader.contains_key("jumbie.qbittorrent"),
        "GET should show the new downloader instance"
    );
    assert!(
        fetched.source.is_empty(),
        "Source section should remain empty"
    );
    assert!(
        fetched.metadata.is_empty(),
        "Metadata section should remain empty"
    );
    assert!(
        fetched.notifier.is_empty(),
        "Notifier section should remain empty"
    );
}

#[tokio::test]
async fn test_put_plugins_section_overwrite_existing() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let mut instances = std::collections::HashMap::new();
    let mut cfg_v1 = std::collections::HashMap::new();
    cfg_v1.insert("name".to_string(), serde_json::json!("v1"));
    cfg_v1.insert("url".to_string(), serde_json::json!("http://old"));
    instances.insert("my_id".to_string(), serde_json::json!(cfg_v1));

    let mut section_data = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    section_data.insert("jumbie.nyaa".to_string(), instances);

    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/source", &section_data)
        .await;

    let mut instances_v2 = std::collections::HashMap::new();
    let mut cfg_v2 = std::collections::HashMap::new();
    cfg_v2.insert("name".to_string(), serde_json::json!("v2"));
    cfg_v2.insert("url".to_string(), serde_json::json!("http://new"));
    cfg_v2.insert("interval".to_string(), serde_json::json!(30));
    instances_v2.insert("my_id".to_string(), serde_json::json!(cfg_v2));

    let mut section_data_v2 = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    section_data_v2.insert("jumbie.nyaa".to_string(), instances_v2);

    let resp: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/source", &section_data_v2)
        .await;
    let updated = &resp.config;

    let nyaa_instances = updated.source.get("jumbie.nyaa").unwrap();
    assert_eq!(
        nyaa_instances.len(),
        1,
        "Should still be exactly 1 instance after overwrite"
    );
    assert_eq!(
        nyaa_instances["my_id"]["name"], "v2",
        "Name should reflect the overwrite"
    );
    assert_eq!(
        nyaa_instances["my_id"]["url"], "http://new",
        "URL should reflect the overwrite"
    );
    assert_eq!(
        nyaa_instances["my_id"]["interval"], 30,
        "New field should be present"
    );
}

#[tokio::test]
async fn test_put_plugins_section_clear_section() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // 1. Add an instance to the notifier section first
    let mut instances = std::collections::HashMap::new();
    let mut cfg = std::collections::HashMap::new();
    cfg.insert("name".to_string(), serde_json::json!("Discord Bot"));
    cfg.insert(
        "webhook".to_string(),
        serde_json::json!("https://discord.gg/xxx"),
    );
    instances.insert("bot_01".to_string(), serde_json::json!(cfg));

    let mut section_data = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    section_data.insert("jumbie.discord".to_string(), instances);

    let _: jumbie_shared::config::PluginsConfig = app
        .put_json("/api/config/plugins_cfg/notifier", &section_data)
        .await;

    let empty_section = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();

    let cleared: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/notifier", &empty_section)
        .await;

    assert!(
        cleared.config.notifier.is_empty(),
        "Notifier section should be empty after PUT with empty data"
    );

    let fetched: jumbie_shared::config::PluginsConfig =
        app.get_json("/api/config/plugins_cfg").await;
    assert!(
        fetched.notifier.is_empty(),
        "GET should confirm notifier section is empty"
    );
}

#[tokio::test]
async fn test_put_plugins_section_invalid_section() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let empty = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();

    let res = app
        .clone()
        .oneshot(common::put_json_request(
            "/api/config/plugins_cfg/invalid_section",
            &empty,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "PUT with invalid section name should return 400"
    );
}

#[tokio::test]
async fn test_put_plugins_section_preserves_other_sections() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let mut dl_config = std::collections::HashMap::new();
    dl_config.insert("name".to_string(), serde_json::json!("qBit"));
    let mut dl_instances = std::collections::HashMap::new();
    dl_instances.insert("q1".to_string(), serde_json::json!(dl_config));

    let mut dl_section = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    dl_section.insert("jumbie.qbittorrent".to_string(), dl_instances);

    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/downloader", &dl_section)
        .await;

    // Seed the metadata section
    let mut md_config = std::collections::HashMap::new();
    md_config.insert("name".to_string(), serde_json::json!("TVMaze"));
    let mut md_instances = std::collections::HashMap::new();
    md_instances.insert("m1".to_string(), serde_json::json!(md_config));

    let mut md_section = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    md_section.insert("jumbie.tvmaze".to_string(), md_instances);

    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/metadata", &md_section)
        .await;

    let mut md_v2 = std::collections::HashMap::new();
    md_v2.insert("name".to_string(), serde_json::json!("AniDB"));
    md_v2.insert("apikey".to_string(), serde_json::json!("abc123"));
    let mut md_instances_v2 = std::collections::HashMap::new();
    md_instances_v2.insert("m1".to_string(), serde_json::json!(md_v2));

    let mut section_data = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    section_data.insert("jumbie.anidb".to_string(), md_instances_v2);

    let resp: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/metadata", &section_data)
        .await;
    let updated = &resp.config;

    assert!(
        updated.downloader.contains_key("jumbie.qbittorrent"),
        "Downloader section should be preserved"
    );
    assert_eq!(
        updated.downloader["jumbie.qbittorrent"]["q1"]["name"], "qBit",
        "Downloader instance should be unchanged"
    );

    assert_eq!(
        updated.metadata.len(),
        1,
        "Metadata should have exactly one plugin type"
    );
    assert!(
        updated.metadata.contains_key("jumbie.anidb"),
        "Metadata should now contain 'jumbie.anidb' (replaced 'jumbie.tvmaze')"
    );
    assert_eq!(
        updated.metadata["jumbie.anidb"]["m1"]["name"], "AniDB",
        "Metadata instance name should reflect the update"
    );
}

// POST /api/config/plugins_cfg/{section}/{plugin_type}/instances
// The backend generates the instance ID atomically on save — the client never
// supplies or sees the ID before creation.

#[tokio::test]
async fn test_create_plugin_instance_generates_id() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Post a new downloader instance — no ID in the request
    let config = serde_json::json!({
        "name": "My Downloader",
        "enabled": true,
        "host": "localhost",
        "port": 8080
    });

    let full_cfg: jumbie_shared::config::PluginsConfig = app
        .post_json(
            "/api/config/plugins_cfg/downloader/jumbie.qbittorrent/instances",
            &config,
        )
        .await;

    let qb_instances = full_cfg
        .downloader
        .get("jumbie.qbittorrent")
        .expect("Should have jumbie.qbittorrent plugin type");
    assert_eq!(
        qb_instances.len(),
        1,
        "Should have exactly one qbittorrent instance"
    );

    // Extract the generated ID (it's the only key)
    let (gen_id, gen_config) = qb_instances.iter().next().unwrap();
    assert!(
        gen_id.len() == 12 && gen_id.chars().all(|c| c.is_ascii_hexdigit()),
        "Generated instance ID '{}' should be 12 hex characters",
        gen_id
    );
    assert_eq!(gen_config["name"], "My Downloader");
    assert_eq!(gen_config["host"], "localhost");
}

#[tokio::test]
async fn test_create_plugin_instance_persists_across_requests() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let config = serde_json::json!({"name": "Persistent Test", "enabled": false});

    let created: jumbie_shared::config::PluginsConfig = app
        .post_json(
            "/api/config/plugins_cfg/notifier/jumbie.discord/instances",
            &config,
        )
        .await;
    let gen_id = created.notifier["jumbie.discord"]
        .keys()
        .next()
        .cloned()
        .expect("Should have a generated ID");

    let fetched: jumbie_shared::config::PluginsConfig =
        app.get_json("/api/config/plugins_cfg").await;
    let discord_instances = &fetched.notifier["jumbie.discord"];
    assert!(
        discord_instances.contains_key(&gen_id),
        "Created instance with ID '{}' should persist",
        gen_id
    );
    assert_eq!(discord_instances[&gen_id]["name"], "Persistent Test");
}

#[tokio::test]
async fn test_create_plugin_instance_invalid_section() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .clone()
        .oneshot(common::post_json_request(
            "/api/config/plugins_cfg/invalid/type/instances",
            &serde_json::json!({"name": "x"}),
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "POST with invalid section should return 400"
    );
}

#[tokio::test]
async fn test_create_plugin_instance_each_call_gets_unique_id() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let config_a = serde_json::json!({"name": "Source A"});
    let config_b = serde_json::json!({"name": "Source B"});

    // Create two instances under the same plugin type
    let full_cfg_a: jumbie_shared::config::PluginsConfig = app
        .post_json(
            "/api/config/plugins_cfg/source/jumbie.nyaa/instances",
            &config_a,
        )
        .await;
    let id_a = full_cfg_a.source["jumbie.nyaa"]
        .keys()
        .next()
        .cloned()
        .expect("First instance should have an ID");

    let full_cfg_b: jumbie_shared::config::PluginsConfig = app
        .post_json(
            "/api/config/plugins_cfg/source/jumbie.nyaa/instances",
            &config_b,
        )
        .await;
    let id_b = full_cfg_b.source["jumbie.nyaa"]
        .keys()
        .find(|k| *k != &id_a)
        .cloned()
        .expect("Second instance should have a different ID");

    assert_ne!(id_a, id_b, "Each creation should get a unique instance ID");

    // Both instances should be in the final config
    assert!(full_cfg_b.source["jumbie.nyaa"].contains_key(&id_a));
    assert!(full_cfg_b.source["jumbie.nyaa"].contains_key(&id_b));
    assert_eq!(full_cfg_b.source["jumbie.nyaa"].len(), 2);
}

#[tokio::test]
async fn test_create_plugin_instance_other_sections_unaffected() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Seed a source instance first
    let src_config = serde_json::json!({"name": "Existing Source"});
    let seeded: jumbie_shared::config::PluginsConfig = app
        .post_json(
            "/api/config/plugins_cfg/source/jumbie.nyaa/instances",
            &src_config,
        )
        .await;
    let src_id = seeded.source["jumbie.nyaa"]
        .keys()
        .next()
        .cloned()
        .expect("Should have source instance ID");

    // Now create a metadata instance — should NOT affect the source section
    let md_config = serde_json::json!({"name": "TVMaze"});
    let after_md: jumbie_shared::config::PluginsConfig = app
        .post_json(
            "/api/config/plugins_cfg/metadata/jumbie.tvmaze/instances",
            &md_config,
        )
        .await;

    // Source section should be intact
    assert!(
        after_md.source.contains_key("jumbie.nyaa"),
        "Source section should still exist"
    );
    assert_eq!(
        after_md.source["jumbie.nyaa"][&src_id]["name"], "Existing Source",
        "Source instance should be unchanged"
    );

    // Metadata section should have the new instance
    assert!(
        after_md.metadata.contains_key("jumbie.tvmaze"),
        "Metadata section should have jumbie.tvmaze"
    );
}

// DELETE /api/config/plugins_cfg/{section}/{plugin_type}/{instance_id}

#[tokio::test]
async fn test_delete_plugin_instance_removes_single_instance() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let mut instances = std::collections::HashMap::new();
    let mut cfg_a = std::collections::HashMap::new();
    cfg_a.insert("name".to_string(), serde_json::json!("Source A"));
    instances.insert("a_001".to_string(), serde_json::json!(cfg_a));

    let mut cfg_b = std::collections::HashMap::new();
    cfg_b.insert("name".to_string(), serde_json::json!("Source B"));
    instances.insert("b_001".to_string(), serde_json::json!(cfg_b));

    let mut section_data = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    section_data.insert("jumbie.nyaa".to_string(), instances);

    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/source", &section_data)
        .await;

    let res = common::send_request(
        &app,
        common::delete_request("/api/config/plugins_cfg/source/jumbie.nyaa/a_001"),
    )
    .await;
    assert!(res.status().is_success());
    let remaining: jumbie_shared::config::PluginsConfig = common::response_json(res).await;

    let nyaa_instances = &remaining.source["jumbie.nyaa"];
    assert_eq!(
        nyaa_instances.len(),
        1,
        "Only one instance should remain after delete"
    );
    assert!(
        nyaa_instances.contains_key("b_001"),
        "Instance 'b_001' should survive deletion"
    );
    assert!(
        !nyaa_instances.contains_key("a_001"),
        "Instance 'a_001' should be gone"
    );

    let fetched: jumbie_shared::config::PluginsConfig =
        app.get_json("/api/config/plugins_cfg").await;
    let fetched_instances = &fetched.source["jumbie.nyaa"];
    assert_eq!(fetched_instances.len(), 1);
    assert!(fetched_instances.contains_key("b_001"));
}

#[tokio::test]
async fn test_delete_plugin_instance_idempotent() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Delete a non-existent instance — should not error (DELETE is idempotent)
    let res = common::send_request(
        &app,
        common::delete_request("/api/config/plugins_cfg/source/nonexistent/ghost"),
    )
    .await;
    assert!(res.status().is_success());
    let cfg: jumbie_shared::config::PluginsConfig = common::response_json(res).await;

    // Config should still be valid and empty
    assert!(cfg.source.is_empty());
}

#[tokio::test]
async fn test_delete_plugin_instance_invalid_section() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let res = app
        .clone()
        .oneshot(common::delete_request(
            "/api/config/plugins_cfg/wrongsection/type/id",
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "DELETE with invalid section should return 400"
    );
}

#[tokio::test]
async fn test_delete_plugin_instance_other_sections_unaffected() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let mut src_cfg = std::collections::HashMap::new();
    src_cfg.insert("name".to_string(), serde_json::json!("Nyaa"));
    let mut src_instances = std::collections::HashMap::new();
    src_instances.insert("n1".to_string(), serde_json::json!(src_cfg));

    let mut src_section = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    src_section.insert("jumbie.nyaa".to_string(), src_instances);

    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/source", &src_section)
        .await;

    let mut md_cfg = std::collections::HashMap::new();
    md_cfg.insert("name".to_string(), serde_json::json!("TVMaze"));
    let mut md_instances = std::collections::HashMap::new();
    md_instances.insert("t1".to_string(), serde_json::json!(md_cfg));

    let mut md_section = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    md_section.insert("jumbie.tvmaze".to_string(), md_instances);

    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/metadata", &md_section)
        .await;

    let res = common::send_request(
        &app,
        common::delete_request("/api/config/plugins_cfg/source/jumbie.nyaa/n1"),
    )
    .await;
    assert!(res.status().is_success());
    let updated: jumbie_shared::config::PluginsConfig = common::response_json(res).await;

    // 3. Verify source is cleared but metadata is untouched
    assert!(
        updated.source.is_empty(),
        "Source section should be empty after deleting the only instance"
    );
    assert!(
        updated.metadata.contains_key("jumbie.tvmaze"),
        "Metadata section should be untouched"
    );
    assert_eq!(
        updated.metadata["jumbie.tvmaze"]["t1"]["name"], "TVMaze",
        "Metadata instance should be unchanged"
    );
}

// Single-active metadata enforcement

/// A metadata config section: `plugin_key → instance_id → instance value`.
type MetadataSection =
    std::collections::HashMap<String, std::collections::HashMap<String, serde_json::Value>>;

/// Build a metadata section payload from `(plugin_key, instance_id, value)`.
fn metadata_section(entries: &[(&str, &str, serde_json::Value)]) -> MetadataSection {
    let mut section = MetadataSection::new();
    for (plugin_key, instance_id, value) in entries {
        section
            .entry(plugin_key.to_string())
            .or_default()
            .insert(instance_id.to_string(), value.clone());
    }
    section
}

/// Is the `(plugin_key, instance_id)` metadata instance enabled?
fn metadata_enabled(cfg: &jumbie_shared::config::PluginsConfig, key: &str, id: &str) -> bool {
    jumbie_shared::config::instance_is_enabled(&cfg.metadata[key][id])
}

/// A minimal valid TVDB instance config (`api_key` satisfies the schema's
/// required field so `validate_section_required_fields` does not auto-disable).
fn tvdb_instance(name: &str, enabled: bool) -> serde_json::Value {
    serde_json::json!({ "name": name, "enabled": enabled, "api_key": "test-key" })
}

#[tokio::test]
async fn test_put_plugins_section_keeps_only_newly_enabled_metadata_provider() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Seed: TVMaze enabled.
    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json(
            "/api/config/plugins_cfg/metadata",
            &metadata_section(&[(
                "jumbie.tvmaze",
                "t1",
                serde_json::json!({ "name": "TVMaze", "enabled": true }),
            )]),
        )
        .await;

    // Payload enables BOTH the previously-enabled TVMaze and a new TVDB.
    let resp: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json(
            "/api/config/plugins_cfg/metadata",
            &metadata_section(&[
                (
                    "jumbie.tvmaze",
                    "t1",
                    serde_json::json!({ "name": "TVMaze", "enabled": true }),
                ),
                ("jumbie.tvdb", "v1", tvdb_instance("TVDB", true)),
            ]),
        )
        .await;

    assert!(
        metadata_enabled(&resp.config, "jumbie.tvdb", "v1"),
        "the newly-enabled provider should stay enabled"
    );
    assert!(
        !metadata_enabled(&resp.config, "jumbie.tvmaze", "t1"),
        "the previously-enabled provider should be switched off"
    );

    // The change is persisted (not just in the response).
    let fetched: jumbie_shared::config::PluginsConfig =
        app.get_json("/api/config/plugins_cfg").await;
    assert!(metadata_enabled(&fetched, "jumbie.tvdb", "v1"));
    assert!(!metadata_enabled(&fetched, "jumbie.tvmaze", "t1"));
}

#[tokio::test]
async fn test_put_plugins_section_two_enabled_same_type_keeps_one() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Two instances of the SAME provider type, both enabled in one payload.
    let resp: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json(
            "/api/config/plugins_cfg/metadata",
            &metadata_section(&[
                ("jumbie.tvdb", "v1", tvdb_instance("TVDB A", true)),
                ("jumbie.tvdb", "v2", tvdb_instance("TVDB B", true)),
            ]),
        )
        .await;

    let enabled_count = ["v1", "v2"]
        .iter()
        .filter(|id| metadata_enabled(&resp.config, "jumbie.tvdb", id))
        .count();
    assert_eq!(enabled_count, 1, "exactly one instance must remain enabled");
}

#[tokio::test]
async fn test_create_plugin_instance_disables_other_metadata_provider() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Seed: TVMaze enabled.
    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json(
            "/api/config/plugins_cfg/metadata",
            &metadata_section(&[(
                "jumbie.tvmaze",
                "t1",
                serde_json::json!({ "name": "TVMaze", "enabled": true }),
            )]),
        )
        .await;

    // Create an enabled TVDB instance — the backend generates its id.
    let after: jumbie_shared::config::PluginsConfig = app
        .post_json(
            "/api/config/plugins_cfg/metadata/jumbie.tvdb/instances",
            &tvdb_instance("TVDB", true),
        )
        .await;

    let new_id = after.metadata["jumbie.tvdb"]
        .iter()
        .find(|(_, value)| jumbie_shared::config::instance_is_enabled(value))
        .map(|(id, _)| id.clone())
        .expect("the created TVDB instance should be enabled");

    assert!(
        !metadata_enabled(&after, "jumbie.tvmaze", "t1"),
        "creating an enabled metadata provider must disable the existing one"
    );
    assert!(metadata_enabled(&after, "jumbie.tvdb", &new_id));
}

#[tokio::test]
async fn test_create_disabled_metadata_provider_keeps_existing_enabled() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json(
            "/api/config/plugins_cfg/metadata",
            &metadata_section(&[(
                "jumbie.tvmaze",
                "t1",
                serde_json::json!({ "name": "TVMaze", "enabled": true }),
            )]),
        )
        .await;

    // Create a DISABLED TVDB instance — TVMaze must stay enabled.
    let after: jumbie_shared::config::PluginsConfig = app
        .post_json(
            "/api/config/plugins_cfg/metadata/jumbie.tvdb/instances",
            &tvdb_instance("TVDB", false),
        )
        .await;

    assert!(
        metadata_enabled(&after, "jumbie.tvmaze", "t1"),
        "creating a disabled provider must not disturb the enabled one"
    );
    assert!(
        !after.metadata["jumbie.tvdb"]
            .values()
            .any(jumbie_shared::config::instance_is_enabled),
        "the created instance should remain disabled"
    );
}

#[tokio::test]
async fn test_put_plugins_section_all_metadata_disabled_is_unchanged() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Both disabled → no enforcement, both persist as disabled.
    let resp: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json(
            "/api/config/plugins_cfg/metadata",
            &metadata_section(&[
                (
                    "jumbie.tvmaze",
                    "t1",
                    serde_json::json!({ "name": "TVMaze", "enabled": false }),
                ),
                ("jumbie.tvdb", "v1", tvdb_instance("TVDB", false)),
            ]),
        )
        .await;

    assert!(!metadata_enabled(&resp.config, "jumbie.tvmaze", "t1"));
    assert!(!metadata_enabled(&resp.config, "jumbie.tvdb", "v1"));
    assert_eq!(
        resp.config.metadata.len(),
        2,
        "both providers should persist"
    );
}
