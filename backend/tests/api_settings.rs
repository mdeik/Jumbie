mod common;

use common::TestApp;
use jumbie::db::automatic_profiles::SubmitterScoreInput;
use jumbie_shared::{
    config::{Theme, UIConfig},
    scoring::{Quality, QualityProfile, ReleaseProfile},
};
use std::collections::HashMap;
use tower::ServiceExt;

// Public theme

#[tokio::test]
async fn test_get_public_theme() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let json: serde_json::Value = app.get_json("/api/public/theme").await;
    assert!(json["theme"].is_string());
}

// Config

#[tokio::test]
async fn test_set_and_clear_password() {
    let (app, state, _tmp) = common::setup_authenticated_app().await;

    // Step 1: Verify password exists in DB
    let hash = state.db.get_user_password_hash("admin").await.unwrap();
    assert!(
        hash.is_some(),
        "Expected password hash to exist after setup_authenticated_app"
    );
    assert!(!hash.as_deref().unwrap().is_empty());

    // Helper: basic auth header for "admin:password"
    let basic_auth = format!(
        "Basic {}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "admin:password")
    );

    // Step 2: Read current config and set password to None
    use axum::body::Body;
    use tower::ServiceExt;

    let mut config = state.cfg.read().await.clone();
    config.auth.password = None; // clearing the password
    let payload = common::to_update_payload(&config);

    let req = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/config")
        .header("content-type", "application/json")
        .header("authorization", &basic_auth)
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert!(
        res.status().is_success(),
        "PUT /api/config (clear password) failed with status {}",
        res.status()
    );

    // Step 3: Verify password hash was removed from DB
    let hash_after_clear = state.db.get_user_password_hash("admin").await.unwrap();
    assert!(
        hash_after_clear.is_none() || hash_after_clear.as_deref().unwrap().is_empty(),
        "Expected password hash to be cleared, got: {:?}",
        hash_after_clear
    );

    // Step 4: Round-trip — set a new password again
    let mut config2 = state.cfg.read().await.clone();
    config2.auth.password = Some("new_password".to_string());
    let payload2 = common::to_update_payload(&config2);

    // Now auth is disabled (password was cleared), so we can send without auth header
    let req2 = common::put_json_request("/api/config", &payload2);
    let res2 = app.clone().oneshot(req2).await.unwrap();
    assert!(
        res2.status().is_success(),
        "PUT /api/config (set new password) failed with status {}",
        res2.status()
    );

    // Step 5: Verify new password hash exists in DB
    let hash_after_set = state.db.get_user_password_hash("admin").await.unwrap();
    assert!(
        hash_after_set.is_some(),
        "Expected password hash to exist after setting new password"
    );
    assert!(!hash_after_set.as_deref().unwrap().is_empty());

    // Step 6: Verify new password works for authentication
    let new_basic_auth = format!(
        "Basic {}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            "admin:new_password"
        )
    );

    let config3 = state.cfg.read().await.clone();
    let payload3 = jumbie_shared::types::UpdateConfigPayload {
        organization: Some(config3.organization),
        sources: None,
        general: None,
        proxy: None,
        auth: None,
        security: None,
    };

    let req3 = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/config")
        .header("content-type", "application/json")
        .header("authorization", &new_basic_auth)
        .body(Body::from(serde_json::to_string(&payload3).unwrap()))
        .unwrap();

    let res3 = app.clone().oneshot(req3).await.unwrap();
    assert!(
        res3.status().is_success(),
        "PUT /api/config (with new password) failed with status {}",
        res3.status()
    );

    // Step 7: Clear again to restore test isolation
    let mut config4 = state.cfg.read().await.clone();
    config4.auth.password = None;
    let payload4 = common::to_update_payload(&config4);

    // Use the new password for auth
    let req4 = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/config")
        .header("content-type", "application/json")
        .header("authorization", &new_basic_auth)
        .body(Body::from(serde_json::to_string(&payload4).unwrap()))
        .unwrap();

    let res4 = app.clone().oneshot(req4).await.unwrap();
    assert!(
        res4.status().is_success(),
        "PUT /api/config (final clear) failed with status {}",
        res4.status()
    );

    let final_hash = state.db.get_user_password_hash("admin").await.unwrap();
    assert!(
        final_hash.is_none() || final_hash.as_deref().unwrap().is_empty(),
        "Expected password hash to be cleared at end of test"
    );
}

#[tokio::test]
async fn test_get_config() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let json: serde_json::Value = app.get_json("/api/config").await;
    assert!(json["organization"].is_object());
}

#[tokio::test]
async fn test_get_config_preserves_calendar_tokens() {
    // Calendar tokens should be preserved in the response so the frontend can build URLs.
    // The UI masking is handled client-side by RevealableCode.
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Insert a calendar token
    db.insert_calendar_token(
        "test-id",
        "My Calendar",
        "cal_secret-token-value",
        true,
        false,
    )
    .await
    .unwrap();

    // Now fetch config — the token must be preserved
    let json: serde_json::Value = _app.get_json("/api/config").await;
    let tokens = json["auth"]["calendar_tokens"]
        .as_array()
        .expect("calendar_tokens should be an array");
    assert_eq!(tokens.len(), 1, "Should have 1 calendar token");
    let token_val = tokens[0]["token"]
        .as_str()
        .expect("token field should exist");
    assert_eq!(
        token_val, "cal_secret-token-value",
        "Calendar token should be preserved in config response (not masked)"
    );
    assert_eq!(
        tokens[0]["name"].as_str(),
        Some("My Calendar"),
        "Calendar token name should be preserved"
    );
}

#[tokio::test]
async fn test_update_config() {
    let (app, state, tmp) = common::setup_test_app().await;

    // Read the current config then PUT it back (round-trip)
    let config = state.cfg.read().await.clone();
    let payload = common::to_update_payload(&config);

    let _: serde_json::Value = app.put_json("/api/config", &payload).await;
    drop(tmp);
}

// Qualities

#[tokio::test]
async fn test_get_qualities() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let _: HashMap<String, Quality> = app.get_json("/api/config/qualities").await;
}

#[tokio::test]
async fn test_put_qualities() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let mut qualities: HashMap<String, Quality> = HashMap::new();
    qualities.insert(
        "Any".to_string(),
        Quality {
            name: "Any".to_string(),
            tags: vec!["mkv".to_string()],
        },
    );
    let _: serde_json::Value = app.put_json("/api/config/qualities", &qualities).await;
}

// Quality profiles

#[tokio::test]
async fn test_get_quality_profiles() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let _: HashMap<String, QualityProfile> = app.get_json("/api/config/quality_profiles").await;
}

#[tokio::test]
async fn test_put_quality_profiles() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let mut profiles: HashMap<String, QualityProfile> = HashMap::new();
    profiles.insert(
        "Any".to_string(),
        QualityProfile {
            name: "Any".to_string(),
            qualities: vec!["Any".to_string()],
            upgrade_only_qualities: vec![],
        },
    );
    let _: serde_json::Value = app
        .put_json("/api/config/quality_profiles", &profiles)
        .await;
}

#[tokio::test]
async fn test_put_quality_profile_with_upgrade_only() {
    let (app, state, _tmp) = common::setup_test_app().await;

    // Seed defaults so qualities exist
    state.db.seed_defaults().await.unwrap();

    // Fetch existing qualities to find a valid ID
    let qualities: HashMap<String, Quality> = app.get_json("/api/config/qualities").await;
    let first_id = qualities
        .keys()
        .next()
        .cloned()
        .expect("At least one quality exists from seeding");

    // Create a profile with this quality as upgrade-only
    let mut profiles: HashMap<String, QualityProfile> = HashMap::new();
    profiles.insert(
        "test_upgrade_only".to_string(),
        QualityProfile {
            name: "Upgrade Only Test".to_string(),
            qualities: vec![first_id.clone()],
            upgrade_only_qualities: vec![first_id],
        },
    );
    let _: serde_json::Value = app
        .put_json("/api/config/quality_profiles", &profiles)
        .await;

    // Read back and verify
    let read_back: HashMap<String, QualityProfile> =
        app.get_json("/api/config/quality_profiles").await;
    let saved = read_back
        .get("test_upgrade_only")
        .expect("Profile was saved");
    assert_eq!(saved.name, "Upgrade Only Test");
    assert_eq!(saved.qualities.len(), 1);
    assert_eq!(saved.upgrade_only_qualities.len(), 1);
    assert_eq!(saved.qualities, saved.upgrade_only_qualities);
}

#[tokio::test]
async fn test_put_quality_profile_rejects_invalid_upgrade_only() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Create a profile where upgrade_only_qualities contains an ID not in qualities
    let mut profiles: HashMap<String, QualityProfile> = HashMap::new();
    profiles.insert(
        "bad_profile".to_string(),
        QualityProfile {
            name: "Bad".to_string(),
            qualities: vec!["real_quality".to_string()],
            upgrade_only_qualities: vec!["nonexistent_quality".to_string()],
        },
    );
    let req = common::put_json_request("/api/config/quality_profiles", &profiles);
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(
        !res.status().is_success(),
        "Profile with invalid upgrade-only qualities should be rejected"
    );
}

// Release profiles

#[tokio::test]
async fn test_get_release_profiles() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let _: HashMap<String, ReleaseProfile> = app.get_json("/api/config/release_profiles").await;
}

#[tokio::test]
async fn test_put_release_profiles() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let profiles: HashMap<String, ReleaseProfile> = HashMap::new();
    let _: serde_json::Value = app
        .put_json("/api/config/release_profiles", &profiles)
        .await;
}

// UI preferences

#[tokio::test]
async fn test_get_ui_preferences() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let _: UIConfig = app.get_json("/api/config/ui_preferences").await;
}

#[tokio::test]
async fn test_put_ui_preferences() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let prefs = UIConfig {
        theme: Theme::Dark,
        ..Default::default()
    };
    let _: serde_json::Value = app.put_json("/api/config/ui_preferences", &prefs).await;
}

// Plugins config

#[tokio::test]
async fn test_get_plugins_config() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let _: jumbie_shared::config::PluginsConfig = app.get_json("/api/config/plugins_cfg").await;
}

#[tokio::test]
async fn test_put_plugins_config() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let empty = std::collections::HashMap::<
        String,
        std::collections::HashMap<String, serde_json::Value>,
    >::new();
    let _: jumbie_shared::types::SavePluginsSectionResponse = app
        .put_json("/api/config/plugins_cfg/downloader", &empty)
        .await;
}

// Automatic profiles

#[tokio::test]
async fn test_get_automatic_profiles() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let json: serde_json::Value = app.get_json("/api/automatic-profiles").await;
    assert!(json.is_array());
}

#[tokio::test]
async fn test_delete_nonexistent_automatic_profile() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    // Deleting a profile that doesn't exist should succeed (idempotent)
    app.delete_request_ok("/api/automatic-profiles/SomeGroup")
        .await;
}

#[tokio::test]
async fn test_get_automatic_profile_records() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let json: serde_json::Value = app
        .get_json("/api/automatic-profiles/SomeGroup/records")
        .await;
    assert!(json.is_array());
}

// Automatic profile offense lifecycle

#[tokio::test]
async fn test_offense_lifecycle_with_extension() {
    let (app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Record an offense with extension
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "TestGroup",
        description: "Included unknown file sample.mkv in download TestRelease",
        value: -5,
        category: "unexpected_files",
        bound: -50,
        source_identity: Some("hash_abc123"),
        extension: Some("mkv"),
    })
    .await
    .unwrap();

    let json: serde_json::Value = app
        .get_json("/api/automatic-profiles/TestGroup/records")
        .await;
    let offenses = json.as_array().unwrap();
    assert_eq!(offenses.len(), 1, "Should have 1 offense");
    assert_eq!(offenses[0]["extension"], "mkv");
    assert_eq!(offenses[0]["category"], "unexpected_files");
    assert_eq!(offenses[0]["score"], -5);

    let all_offenses = db
        .get_unexpected_file_records(&["unexpected_files".to_string()])
        .await
        .unwrap();
    assert_eq!(all_offenses.len(), 1);
    assert_eq!(all_offenses[0].3.as_deref(), Some("mkv"));

    // Record a second offense without extension (pre-migration style)
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "TestGroup",
        description: "Included unknown file other.txt",
        value: -5,
        category: "unexpected_files",
        bound: -50,
        source_identity: Some("hash_def456"),
        extension: None,
    })
    .await
    .unwrap();

    let all = db
        .get_unexpected_file_records(&["unexpected_files".to_string()])
        .await
        .unwrap();
    assert_eq!(all.len(), 2);

    // Clear the offense with the mkv extension and verify
    // (order by date_added DESC is non-deterministic at second precision
    //  on fast systems, so identify the record by extension)
    let mkv_id = all
        .iter()
        .find(|r| r.3.as_deref() == Some("mkv"))
        .unwrap()
        .0
        .clone();
    db.clear_record_by_id(&mkv_id).await.unwrap();

    let remaining = db
        .get_unexpected_file_records(&["unexpected_files".to_string()])
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1, "One offense should be cleared");
    assert_eq!(remaining[0].3, None, "Remaining offense has no extension");

    let profile = db.get_automatic_profile("TestGroup").await.unwrap();
    assert!(profile.is_some());
    // Started with 0, added -5 + -5 = -10, then cleared one -5 = -5
    assert_eq!(profile.unwrap().score, -5);
}

// Score parity: initial recording vs clearing produces consistent scores

#[tokio::test]
async fn test_offense_score_parity() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Simulate: download with 4 srt unknown files, max_penalty = -10
    // Each file has penalty -5, max_penalty for category is -10.
    // Files 1-2 should get -5 each (total -10), files 3-4 should be clamped to 0.
    let hashes = ["hash_01", "hash_02", "hash_03", "hash_04"];
    for (i, hash) in hashes.iter().enumerate() {
        db.record_submitter_score(SubmitterScoreInput {
            submitter: "ScoreGroup",
            description: &format!("Included unknown file file{}.srt in download Test", i + 1),
            value: -5,
            category: "unexpected_files",
            bound: -10,
            source_identity: Some(hash),
            extension: Some("srt"),
        })
        .await
        .unwrap();
    }

    // Total should be -10 (clamped), not -20
    let profile = db.get_automatic_profile("ScoreGroup").await.unwrap();
    assert_eq!(profile.as_ref().map(|p| p.score), Some(-10));

    let offenses = db
        .get_automatic_profile_records("ScoreGroup")
        .await
        .unwrap();
    let non_zero: Vec<_> = offenses.iter().filter(|o| o.score != 0).collect();
    assert_eq!(
        non_zero.len(),
        2,
        "Only 2 of 4 offenses should have non-zero penalty (clamped at -10)"
    );
    let total: i32 = offenses.iter().map(|o| o.score).sum();
    assert_eq!(total, -10, "Sum of all penalties should equal max_penalty");

    // Simulate: recording again with same source_identity (idempotent)
    // Re-recording the first file should dedup and NOT change total
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "ScoreGroup",
        description: "Included unknown file file1.srt in download Test (duplicate)",
        value: -5,
        category: "unexpected_files",
        bound: -10,
        source_identity: Some("hash_01"),
        extension: Some("srt"),
    })
    .await
    .unwrap();

    let profile_after_dup = db.get_automatic_profile("ScoreGroup").await.unwrap();
    assert_eq!(
        profile_after_dup.unwrap().score,
        -10,
        "Re-recording same source_identity should not change total"
    );

    // Simulate: clearing one non-zero offense
    // Clearing hash_01 should reduce total from -10 to -5
    db.clear_submitter_score("ScoreGroup", "unexpected_files", "hash_01")
        .await
        .unwrap();

    let profile_after_clear = db.get_automatic_profile("ScoreGroup").await.unwrap();
    assert_eq!(
        profile_after_clear.unwrap().score,
        -5,
        "Clearing one -5 offense should leave -5 total"
    );

    // Record with INI extension (in IGNORED_UNKNOWN_EXTS)
    // This should work fine — the ignore list only affects file classification
    // at smart_link time, not DB-level recording.
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "ScoreGroup",
        description: "Included unknown file info.nfo in download",
        value: -5,
        category: "unexpected_files",
        bound: -10,
        source_identity: Some("hash_nfo"),
        extension: Some("nfo"),
    })
    .await
    .unwrap();

    let profile_with_nfo = db.get_automatic_profile("ScoreGroup").await.unwrap();
    // -5 (from remaining srt) + -5 (nfo, clamped) = -10 (back to max)
    assert_eq!(profile_with_nfo.unwrap().score, -10);

    // Multiple submitters, independent scores
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "OtherGroup",
        description: "Included unknown file data.txt",
        value: -5,
        category: "unexpected_files",
        bound: -20,
        source_identity: Some("hash_txt"),
        extension: Some("txt"),
    })
    .await
    .unwrap();

    let other = db.get_automatic_profile("OtherGroup").await.unwrap();
    assert_eq!(other.unwrap().score, -5);

    let score_group = db.get_automatic_profile("ScoreGroup").await.unwrap();
    assert_eq!(score_group.unwrap().score, -10);
}

// Clear by ID preserves score correctly

#[tokio::test]
async fn test_clear_record_by_id_score_consistency() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Record two offenses for the same submitter
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "ClearGroup",
        description: "First file",
        value: -3,
        category: "unexpected_files",
        bound: -10,
        source_identity: Some("hash_a"),
        extension: Some("txt"),
    })
    .await
    .unwrap();
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "ClearGroup",
        description: "Second file",
        value: -7,
        category: "unexpected_files",
        bound: -10,
        source_identity: Some("hash_b"),
        extension: Some("log"),
    })
    .await
    .unwrap();

    let profile = db.get_automatic_profile("ClearGroup").await.unwrap();
    assert_eq!(profile.unwrap().score, -10);

    // Load offenses and clear by ID
    let offenses = db
        .get_unexpected_file_records(&["unexpected_files".to_string()])
        .await
        .unwrap();
    assert_eq!(offenses.len(), 2);

    // Find the offense for hash_b (penalty = -7, extension = "log")
    let offense_b_id = offenses
        .iter()
        .find(|(_, _, _, ext)| ext.as_deref() == Some("log"))
        .map(|(id, _, _, _)| id)
        .expect("hash_b record with extension 'log' should exist");
    db.clear_record_by_id(offense_b_id).await.unwrap();

    let profile_after = db.get_automatic_profile("ClearGroup").await.unwrap();
    assert_eq!(
        profile_after.unwrap().score,
        -3,
        "After clearing -7, only -3 should remain"
    );

    let remaining = db
        .get_unexpected_file_records(&["unexpected_files".to_string()])
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);

    // Clearing a non-existent ID should be a no-op (no error)
    db.clear_record_by_id("non_existent_id").await.unwrap();
    let profile_still = db.get_automatic_profile("ClearGroup").await.unwrap();
    assert_eq!(profile_still.unwrap().score, -3);
}

// Dedup by source_identity across different category names

#[tokio::test]
async fn test_offense_dedup_by_submitter_category_identity() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Same (submitter, category, source_identity) should produce one offense
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "DedupGroup",
        description: "First recording",
        value: -5,
        category: "unexpected_files",
        bound: -20,
        source_identity: Some("same_hash"),
        extension: Some("srt"),
    })
    .await
    .unwrap();

    db.record_submitter_score(SubmitterScoreInput {
        submitter: "DedupGroup",
        description: "Second recording (same identity)",
        value: -3, // different penalty — should update
        category: "unexpected_files",
        bound: -20,
        source_identity: Some("same_hash"),
        extension: Some("srt"),
    })
    .await
    .unwrap();

    // Only 1 offense, penalty should be updated to -3
    let offenses = db
        .get_automatic_profile_records("DedupGroup")
        .await
        .unwrap();
    assert_eq!(offenses.len(), 1);
    assert_eq!(offenses[0].score, -3);

    let profile = db.get_automatic_profile("DedupGroup").await.unwrap();
    assert_eq!(profile.unwrap().score, -3);

    // Same hash but DIFFERENT category → separate offense
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "DedupGroup",
        description: "Different category, same hash",
        value: -10,
        category: "audio_channels",
        bound: -50,
        source_identity: Some("same_hash"),
        extension: None,
    })
    .await
    .unwrap();

    let all = db
        .get_automatic_profile_records("DedupGroup")
        .await
        .unwrap();
    assert_eq!(
        all.len(),
        2,
        "Different categories should be separate offenses"
    );

    let profile2 = db.get_automatic_profile("DedupGroup").await.unwrap();
    assert_eq!(profile2.unwrap().score, -13); // -3 + -10
}

// IGNORED_UNKNOWN_EXTS override: user patterns take priority

#[tokio::test]
async fn test_ignored_unknown_ext_override_rescoring() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Record an offense with extension "nfo" (in IGNORED_UNKNOWN_EXTS)
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "OverrideGroup",
        description: "Included info.nfo",
        value: -5,
        category: "unexpected_files",
        bound: -20,
        source_identity: Some("hash_nfo"),
        extension: Some("nfo"),
    })
    .await
    .unwrap();

    // Record an offense with extension "txt" (NOT in IGNORED_UNKNOWN_EXTS)
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "OverrideGroup",
        description: "Included readme.txt",
        value: -5,
        category: "unexpected_files",
        bound: -20,
        source_identity: Some("hash_txt"),
        extension: Some("txt"),
    })
    .await
    .unwrap();

    // Rescore with no override: the ignore list must clear the nfo offense while
    // leaving the txt offense (not in the ignore list) intact.
    let all = db
        .get_unexpected_file_records(&["unexpected_files".to_string()])
        .await
        .unwrap();
    assert_eq!(all.len(), 2);

    assert!(jumbie_shared::media_format::is_ignored_unknown_ext("nfo"));
    assert!(!jumbie_shared::media_format::is_ignored_unknown_ext("txt"));

    // Clear nfo offense (simulating rescore without override).
    // Find it by checking against IGNORED_UNKNOWN_EXTS rather than
    // hardcoding the extension name.
    let nfo_offense = all
        .iter()
        .find(|(_, _, _, ext)| {
            ext.as_deref()
                .is_some_and(jumbie_shared::media_format::is_ignored_unknown_ext)
        })
        .unwrap();
    db.clear_record_by_id(&nfo_offense.0).await.unwrap();

    let remaining = db
        .get_unexpected_file_records(&["unexpected_files".to_string()])
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1, "Only txt offense should remain");
    assert_eq!(
        remaining[0].3.as_deref(),
        Some("txt"),
        "Remaining offense should be txt"
    );

    // Now test override: record nfo again, and simulate rescoring
    // with file_patterns containing nfo (user override)
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "OverrideGroup",
        description: "Included info.nfo",
        value: -5,
        category: "unexpected_files",
        bound: -20,
        source_identity: Some("hash_nfo"),
        extension: Some("nfo"),
    })
    .await
    .unwrap();

    let with_nfo = db
        .get_unexpected_file_records(&["unexpected_files".to_string()])
        .await
        .unwrap();
    assert_eq!(with_nfo.len(), 2, "nfo and txt should both exist");

    let exts: Vec<Option<String>> = with_nfo.iter().map(|o| o.3.clone()).collect();
    assert!(exts.contains(&Some("nfo".to_string())));
    assert!(exts.contains(&Some("txt".to_string())));

    let profile = db.get_automatic_profile("OverrideGroup").await.unwrap();
    assert_eq!(profile.unwrap().score, -10);
}

#[tokio::test]
async fn test_get_available_plugins() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let json: serde_json::Value = app.get_json("/api/plugins/available").await;
    assert!(json.is_array());
    // At minimum qbittorrent, discord, nyaa, basic_rss should be listed
    let names: Vec<String> = json
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["display_name"].as_str())
        .map(|s| s.to_lowercase())
        .collect();
    println!("Available plugins: {:?}", names);
    assert!(names.contains(&"qbittorrent".to_string()));
}

#[tokio::test]
async fn test_get_plugins() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let json: serde_json::Value = app.get_json("/api/plugins").await;
    assert!(json.is_array());
}

#[tokio::test]
async fn test_get_plugin_status() {
    let (app, _state, _tmp) = common::setup_test_app().await;
    let json: serde_json::Value = app.get_json("/api/plugins/status").await;
    assert!(json.is_array());
}

#[tokio::test]
async fn test_get_plugin_schema_not_found() {
    use tower::ServiceExt;
    let (app, _state, _tmp) = common::setup_test_app().await;
    // No WASM plugins loaded in tests, so schema will 404
    let res = app
        .oneshot(common::get_request(
            "/api/plugins/nonexistent_plugin/schema",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_validate_plugin_config_not_found() {
    use tower::ServiceExt;
    let (app, _state, _tmp) = common::setup_test_app().await;
    let payload = serde_json::json!({"some": "value"});
    let res = app
        .oneshot(common::post_json_request(
            "/api/plugins/nonexistent_plugin/validate",
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND);
}

// Test plugin (testing error cases only, no external connections)

#[tokio::test]
async fn test_plugin_test_unknown_category() {
    use tower::ServiceExt;
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
    assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_plugin_test_unknown_notifier_type() {
    use tower::ServiceExt;
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
    assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
}

// Unknown file extension count accumulation

#[tokio::test]
async fn test_unknown_file_counts_accumulate() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    db.record_unknown_file_counts(
        "CountGroup",
        &[("srt".to_string(), 3), ("txt".to_string(), 1)],
    )
    .await
    .unwrap();

    let all = db.get_unknown_file_extensions().await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.contains(&("CountGroup".to_string(), "srt".to_string(), 3)));
    assert!(all.contains(&("CountGroup".to_string(), "txt".to_string(), 1)));

    // Second walk adds more of the same extensions
    db.record_unknown_file_counts(
        "CountGroup",
        &[("srt".to_string(), 2), ("nfo".to_string(), 5)],
    )
    .await
    .unwrap();

    let all = db.get_unknown_file_extensions().await.unwrap();
    assert_eq!(all.len(), 3);
    // srt: 3 + 2 = 5
    assert!(all.contains(&("CountGroup".to_string(), "srt".to_string(), 5)));
    // txt: unchanged
    assert!(all.contains(&("CountGroup".to_string(), "txt".to_string(), 1)));
    // nfo: new
    assert!(all.contains(&("CountGroup".to_string(), "nfo".to_string(), 5)));
}

// Bulk clear of offenses by category

#[tokio::test]
async fn test_clear_records_by_categories_adjusts_score() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Record two offenses in "unexpected_files" category
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "ClearCatGroup",
        description: "file1.nfo",
        value: -5,
        category: "unexpected_files",
        bound: -20,
        source_identity: Some("hash_a"),
        extension: Some("nfo"),
    })
    .await
    .unwrap();
    db.record_submitter_score(SubmitterScoreInput {
        submitter: "ClearCatGroup",
        description: "file2.txt",
        value: -7,
        category: "unexpected_files",
        bound: -20,
        source_identity: Some("hash_b"),
        extension: Some("txt"),
    })
    .await
    .unwrap();

    let profile = db.get_automatic_profile("ClearCatGroup").await.unwrap();
    assert_eq!(profile.unwrap().score, -12);

    // Bulk-clear
    db.clear_records_by_categories(&["unexpected_files".to_string()])
        .await
        .unwrap();

    let profile = db.get_automatic_profile("ClearCatGroup").await.unwrap();
    assert_eq!(profile.unwrap().score, 0);

    let remaining = db
        .get_unexpected_file_records(&["unexpected_files".to_string()])
        .await
        .unwrap();
    assert_eq!(remaining.len(), 0);
}

// Rescoring produces consistent results regardless of how many times run

/// Record rescored submitter entries for testing idempotency.
async fn record_idempotent_scores(db: &jumbie::db::DbManager) {
    db.clear_records_by_categories(&["unexpected_files".to_string()])
        .await
        .unwrap();
    for i in 0..4 {
        db.record_submitter_score(SubmitterScoreInput {
            submitter: "IdempotentGroup",
            description: &format!("file{}.srt (rescored)", i + 1),
            value: -5,
            category: "unexpected_files",
            bound: -30,
            source_identity: Some(&format!("hash_srt_{}", i)),
            extension: Some("srt"),
        })
        .await
        .unwrap();
    }
    for i in 0..2 {
        db.record_submitter_score(SubmitterScoreInput {
            submitter: "IdempotentGroup",
            description: &format!("file{}.txt (rescored)", i + 1),
            value: -5,
            category: "unexpected_files",
            bound: -30,
            source_identity: Some(&format!("hash_txt_{}", i)),
            extension: Some("txt"),
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn test_rescoring_is_idempotent() {
    let (_app, state, _tmp) = common::setup_test_app().await;
    let db = &state.db;

    // Simulate initial scoring: record extensions and offenses
    // (Extensions are what rescoring uses; offenses are what it replaces)
    db.record_unknown_file_counts(
        "IdempotentGroup",
        &[("srt".to_string(), 4), ("txt".to_string(), 2)],
    )
    .await
    .unwrap();

    // Record initial offenses as a walk would
    for i in 0..4 {
        db.record_submitter_score(SubmitterScoreInput {
            submitter: "IdempotentGroup",
            description: &format!("file{}.srt", i + 1),
            value: -5,
            category: "unexpected_files",
            bound: -30,
            source_identity: Some(&format!("hash_srt_{}", i)),
            extension: Some("srt"),
        })
        .await
        .unwrap();
    }
    for i in 0..2 {
        db.record_submitter_score(SubmitterScoreInput {
            submitter: "IdempotentGroup",
            description: &format!("file{}.txt", i + 1),
            value: -5,
            category: "unexpected_files",
            bound: -30,
            source_identity: Some(&format!("hash_txt_{}", i)),
            extension: Some("txt"),
        })
        .await
        .unwrap();
    }

    let score_initial = db.get_automatic_profile("IdempotentGroup").await.unwrap();

    // Simulate rescoring round 1: bulk-clear + re-record using same extensions
    record_idempotent_scores(db).await;

    let score_round1 = db.get_automatic_profile("IdempotentGroup").await.unwrap();
    assert_eq!(
        score_initial.as_ref().map(|p| p.score),
        score_round1.as_ref().map(|p| p.score),
        "Score after first rescore should match initial"
    );

    // Simulate rescoring round 2: same extensions, should produce same score
    record_idempotent_scores(db).await;

    let score_round2 = db.get_automatic_profile("IdempotentGroup").await.unwrap();
    assert_eq!(
        score_round1.as_ref().map(|p| p.score),
        score_round2.as_ref().map(|p| p.score),
        "Score after second rescore should match first"
    );
}

// Config timestamp validation (strict RFC 3339)

#[tokio::test]
async fn test_config_rejects_zone_less_api_key_expiry() {
    let (app, _state, _tmp) = common::setup_test_app().await;

    // Config timestamps are plain strings, so they are validated explicitly.
    let payload = |expires_at: &str| {
        serde_json::json!({
            "auth": {
                "api_keys": [{
                    "id": "ts-key",
                    "name": "ts-key",
                    "key": "",
                    "prefix": "",
                    "scopes": ["config:read"],
                    "expires_at": expires_at,
                }]
            }
        })
    };

    for bad in ["2026-06-18", "2026-06-18T00:00:00", "2026-06-18 00:00:00"] {
        let (status, _) = common::put_json(&app, "/api/config", &payload(bad)).await;
        assert_eq!(
            status,
            axum::http::StatusCode::BAD_REQUEST,
            "{bad} should be rejected"
        );
    }

    let (status, _) =
        common::put_json(&app, "/api/config", &payload("2026-06-18T00:00:00+00:00")).await;
    assert!(
        status.is_success(),
        "RFC 3339 expiry should be accepted, got {status}"
    );
}
