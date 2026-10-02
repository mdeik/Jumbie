mod common;

use axum::http::StatusCode;
use common::TestApp;
use jumbie_shared::types::{PathOperation, SeriesDetails, UpdateSeriesPayload};
use std::collections::HashMap;
use tower::ServiceExt;

// Helpers

/// Resolve the `${series}` template against the series title.
fn resolve_template(template: &str, title: &str) -> String {
    template.replace("${series}", title)
}

/// Create a series under the given destination root, both on disk and in the DB.
/// Returns the series ID.
///
/// WHY "overwrite" collision handling: The folder is pre-created on disk and the
/// series claims it — this is the claim-existing-content path. The org config's
/// default ("rename") would suffix the folder instead, which these template-
/// resolution tests don't intend to exercise.
async fn create_series_at_root(
    app: &axum::Router,
    state: &std::sync::Arc<jumbie::api::AppState>,
    root: &std::path::Path,
    title: &str,
) -> String {
    common::set_collision_handling(state, "overwrite").await;
    let series_dir = root.join(title);
    std::fs::create_dir_all(&series_dir).expect("Failed to create series dir");

    let create_payload = jumbie_shared::types::CreateSeriesRequest {
        path: series_dir.to_string_lossy().to_string(),
        series_name: Some(title.to_string()),
        scan_for_existing: Some(false),
        monitor_mode: Some(jumbie_shared::mapping::MonitorMode::All),
        quality_profile: None,
        release_profile: None,
        metadata_ids: Default::default(),
        search_missing_on_add: false,
        resolve_collisions: true,
        settings: Default::default(),
    };

    let req = common::post_json_request("/api/series", &create_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::CREATED,
        "Failed to create series '{}'",
        title
    );

    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let id: String = serde_json::from_slice(&body).unwrap();
    id.trim_matches('"').to_string()
}

// ${series} basic resolution (no spaces): a stored path of `{root}/${series}` must
// resolve to the same folder as the existing `{root}/MySeries`, so the Move is a
// no-op and the directory stays put.

#[tokio::test]
async fn test_series_variable_resolves_without_dollar_prefix() {
    let (app, state, temp_dir) = common::setup_test_app().await;

    let organized_root = temp_dir.path().join("organized");

    // Create a series under the actual destination root
    let series_id = create_series_at_root(&app, &state, &organized_root, "MySeries").await;

    let original_dir = organized_root.join("MySeries");
    assert!(
        original_dir.exists(),
        "Precondition: series directory should exist at the expected path under the root"
    );

    // Path::join (not format!) keeps OS-native separators; mixed separators on
    // Windows make PathBuf::eq treat identical paths as different.
    let template_path = organized_root
        .join("${series}")
        .to_string_lossy()
        .to_string();

    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("MySeries".to_string()),
        path_operation: Some(PathOperation::Move),
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some(template_path),
            monitor_mode: Some(jumbie_shared::mapping::MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "update_series should succeed when path contains ${{series}}"
    );

    // The directory MUST still be at `{root}/MySeries` — not `{root}/$MySeries`.
    assert!(
        original_dir.exists(),
        "Series directory should remain at original path when ${{series}} template resolves to the same value",
    );

    // Sanity-check: the buggy path should NOT be the one that exists
    let buggy_dir = organized_root.join("$MySeries");
    assert!(
        !buggy_dir.exists(),
        "Buggy code would have moved the directory to a '$'-prefixed path"
    );

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let stored = details.config.settings.path.as_deref().unwrap_or("");
    assert!(
        stored.contains("${series}"),
        "Stored path should contain the raw ${{series}} template, got: {}",
        stored
    );

    let resolved = resolve_template(stored, "MySeries");
    assert_eq!(
        resolved,
        organized_root
            .join("MySeries")
            .to_string_lossy()
            .to_string()
    );
}

// ${series} with spaces in the title

#[tokio::test]
async fn test_series_variable_handles_spaces() {
    let (app, state, temp_dir) = common::setup_test_app().await;

    let organized_root = temp_dir.path().join("organized");

    // Create series with a title containing spaces, under the actual root
    let series_id = create_series_at_root(&app, &state, &organized_root, "My Test Show").await;

    let original_dir = organized_root.join("My Test Show");
    assert!(
        original_dir.exists(),
        "Precondition: series directory should exist under root"
    );

    // Path::join keeps OS-native separators, avoiding Windows PathBuf comparison failures.
    let template_path = organized_root
        .join("${series}")
        .to_string_lossy()
        .to_string();

    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("My Test Show".to_string()),
        path_operation: Some(PathOperation::Move),
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some(template_path),
            monitor_mode: Some(jumbie_shared::mapping::MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "update_series should succeed with a space-containing title and ${{series}}"
    );

    // Directory MUST remain at `{root}/My Test Show` (not `{root}/$My Test Show`)
    assert!(
        original_dir.exists(),
        "Directory should remain at original path when ${{series}} resolves with spaces"
    );

    let buggy_dir = organized_root.join("$My Test Show");
    assert!(
        !buggy_dir.exists(),
        "Buggy code would have moved directory to a '$'-prefixed path"
    );

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let stored = details.config.settings.path.as_deref().unwrap_or("");
    assert!(
        stored.contains("${series}"),
        "Stored path should contain raw ${{series}}, got: {}",
        stored
    );

    let resolved = resolve_template(stored, "My Test Show");
    assert_eq!(
        resolved,
        organized_root
            .join("My Test Show")
            .to_string_lossy()
            .to_string(),
        "Spaces in title should be preserved"
    );
}

// ${series} in a compound path with a static prefix

#[tokio::test]
async fn test_series_variable_with_prefix_directory() {
    let (app, state, temp_dir) = common::setup_test_app().await;

    let organized_root = temp_dir.path().join("organized");

    // Create a series inside a subdirectory
    let series_id = create_series_at_root(
        &app,
        &state,
        &organized_root.join("shows"),
        "The Time Machine",
    )
    .await;

    let original_dir = organized_root.join("shows").join("The Time Machine");
    assert!(
        original_dir.exists(),
        "Precondition: series directory should exist"
    );

    // Path::join keeps OS-native separators, avoiding Windows PathBuf comparison failures.
    let template_path = organized_root
        .join("shows")
        .join("${series}")
        .to_string_lossy()
        .to_string();

    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("The Time Machine".to_string()),
        path_operation: Some(PathOperation::Move),
        settings: jumbie_shared::mapping::SeriesSettings {
            aliases: vec![],
            reg_patterns: vec![],
            season: HashMap::new(),
            season_absolute: HashMap::new(),
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            absolute_numbering: Some(false),
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            path: Some(template_path),
            monitor_mode: Some(jumbie_shared::mapping::MonitorMode::All),
            metadata_ids: HashMap::new(),
            metadata_last_synced_at: HashMap::new(),
            last_known_dir_mtimes: HashMap::new(),
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Path should remain at `{root}/shows/The Time Machine` (no move needed)
    let correct_path = organized_root.join("shows").join("The Time Machine");
    let literal_path = organized_root.join("shows").join("$The Time Machine");

    assert!(
        correct_path.exists(),
        "${{series}} should resolve to 'The Time Machine', directory stays at original path"
    );
    assert!(
        !literal_path.exists(),
        "Path with literal '$' prefix should not exist"
    );

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let stored = details.config.settings.path.as_deref().unwrap_or("");
    assert!(
        stored.contains("${series}"),
        "Stored path should contain raw ${{series}}, got: {}",
        stored
    );
}

// `${destination_root}` is unsupported: a path containing it must be treated as a
// literal directory name (no resolution). A `.replace("{destination_root}", root)`
// would match inside `${destination_root}`, consuming the braces and leaving `$`.

#[tokio::test]
async fn test_destination_root_variable_not_resolved() {
    let (app, state, temp_dir) = common::setup_test_app().await;

    let organized_root = temp_dir.path().join("organized");

    // Create a series under the root
    let series_id = create_series_at_root(&app, &state, &organized_root, "Legacy Show").await;

    let original_dir = organized_root.join("Legacy Show");
    assert!(
        original_dir.exists(),
        "Precondition: series dir should exist"
    );

    // The old `${destination_root}` variable is now a literal directory name (its
    // `$` sigil and braces are just characters).
    let template_path = format!(
        "{}/${{destination_root}}/Legacy Show",
        organized_root.to_string_lossy()
    );

    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Legacy Show".to_string()),
        path_operation: Some(PathOperation::Move),
        settings: jumbie_shared::mapping::SeriesSettings {
            aliases: vec![],
            reg_patterns: vec![],
            season: HashMap::new(),
            season_absolute: HashMap::new(),
            season_folder_format: None,
            episode_file_format: None,
            season_folder_format_absolute: None,
            episode_file_format_absolute: None,
            flatten_season_folders: None,
            absolute_numbering: Some(false),
            rename_episodes: None,
            search_format: None,
            search_format_absolute: None,
            path: Some(template_path),
            monitor_mode: Some(jumbie_shared::mapping::MonitorMode::All),
            metadata_ids: HashMap::new(),
            metadata_last_synced_at: HashMap::new(),
            last_known_dir_mtimes: HashMap::new(),
        },
    };

    let _ = app
        .clone()
        .oneshot(common::put_json_request(
            &format!("/api/series/{}", series_id),
            &update_payload,
        ))
        .await
        .unwrap();

    // This changes the path, so a Move occurs. Correct code keeps `${destination_root}`
    // as a literal directory name; buggy code partially resolves it to `$<root>`.
    let correct_new_dir = organized_root
        .join("${destination_root}")
        .join("Legacy Show");
    let root_path_str = organized_root.to_string_lossy().to_string();
    let root_path_trimmed = root_path_str.trim_start_matches('/');
    let buggy_new_dir = organized_root
        .join("$")
        .join(root_path_trimmed)
        .join("Legacy Show");
    // ^ e.g. `/tmp/jumbieXXXX/organized/$/tmp/jumbieXXXX/organized/Legacy Show`

    // Assert the move landed at the literal `${destination_root}` path (correct), not
    // the `$`-prefixed one (buggy behavior).
    if correct_new_dir.exists() {
        // Correct behavior - directory was moved to the literal path
        assert!(
            !buggy_new_dir.exists(),
            "Buggy code would have moved to a '$'-prefixed path instead of the literal"
        );
    } else if buggy_new_dir.exists() {
        panic!(
            "BUG: Directory was moved to '{}' instead of '{}'. \
             ${{destination_root}} was partially consumed.",
            buggy_new_dir.display(),
            correct_new_dir.display()
        );
    } else {
        // Neither exists — the move might not have happened because the
        // resolved paths matched. Check the stored template.
        let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
        let stored = details.config.settings.path.as_deref().unwrap_or("");

        // With CORRECT code, `${destination_root}` stays as-is in the template
        assert!(
            stored.contains("${destination_root}"),
            "The ${{destination_root}} variable must remain unresolved in the stored path, got: {}",
            stored
        );
    }
}

// Validation: stored template retains the raw variable form

#[tokio::test]
async fn test_path_template_stored_as_literal_with_series_variable() {
    let (app, state, temp_dir) = common::setup_test_app().await;

    let organized_root = temp_dir.path().join("organized");
    let series_id = create_series_at_root(&app, &state, &organized_root, "A Simple Show").await;

    // Update with `${series}` — no path_operation, just template storage
    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("A Simple Show".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("${series}".to_string()),
            monitor_mode: Some(jumbie_shared::mapping::MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Fetch details and verify the stored template is the literal form
    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let stored = details.config.settings.path.as_deref().unwrap_or("");

    assert_eq!(
        stored, "${series}",
        "The stored path must be the literal template ${{series}}, not resolved"
    );

    let resolved = resolve_template(stored, "A Simple Show");
    assert_eq!(
        resolved, "A Simple Show",
        "Resolution should yield the title verbatim when template is just ${{series}}"
    );
}

// Validation: template with ${destination_root} stored as-is

#[tokio::test]
async fn test_path_template_destination_root_stored_as_literal() {
    let (app, _state, _temp_dir) = common::setup_test_app().await;

    let series_id = common::create_test_series(&app, "Legacy Show").await;

    // Update with `${destination_root}` — no path_operation
    let update_payload = UpdateSeriesPayload {
        quality_profile: "Any".to_string(),
        release_profile: "Any".to_string(),
        title: Some("Legacy Show".to_string()),
        path_operation: None,
        settings: jumbie_shared::mapping::SeriesSettings {
            path: Some("${destination_root}/Legacy Show".to_string()),
            monitor_mode: Some(jumbie_shared::mapping::MonitorMode::All),
            absolute_numbering: Some(false),
            ..Default::default()
        },
    };

    let req = common::put_json_request(&format!("/api/series/{}", series_id), &update_payload);
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let details: SeriesDetails = app.get_json(&format!("/api/series/{}", series_id)).await;
    let stored = details.config.settings.path.as_deref().unwrap_or("");

    // The `${destination_root}` variable is REMOVED — it must be stored literally
    assert!(
        stored.contains("${destination_root}"),
        "The ${{destination_root}} variable was removed; the stored path must contain \
         the literal '${{destination_root}}' text, got: {}",
        stored
    );

    // Show the bug: current code resolves `{destination_root}` inside `${destination_root}`
    let buggy_resolved = stored.replace("{destination_root}", "/some/root");
    assert!(
        buggy_resolved.starts_with('$'),
        "Buggy resolution incorrectly produces '{}' which should start with a bare '$'",
        buggy_resolved
    );
    assert!(
        !buggy_resolved.contains("${destination_root}"),
        "Buggy code consumes the braces: '{}'",
        buggy_resolved
    );

    // Correct: no replacement → `${destination_root}` stays intact
    assert!(
        stored.contains("${destination_root}"),
        "Correct behavior: the literal ${{destination_root}} text is preserved"
    );
}
