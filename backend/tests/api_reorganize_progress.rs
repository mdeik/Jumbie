//! HTTP-contract tests for the async reorganize progress lifecycle.
//!
//! These assert the `/api/series/actions/reorganize_all*` and `/api/status`
//! contracts the frontend's progress toast reconciles against. They are
//! API-only (no browser), so they belong at the router layer with a per-test
//! temp DB rather than in the Playwright suite: `reorganize_all` mutates EVERY
//! series, so running it from a shared e2e server races sibling specs whose
//! files must stay where the scanner put them (e.g. `episode_original_path`).

use std::time::{Duration, Instant};

use jumbie::api::{ActiveOperation, BatchMoveProgress};
use jumbie_shared::types::BatchMoveResponse;

mod common;

use common::TestApp;

/// `/api/status` payload, narrowed to the field under test.
#[derive(serde::Deserialize)]
struct StatusPayload {
    #[serde(default)]
    active_operations: Vec<ActiveOperation>,
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Start an async reorganize and return its task_id.
async fn start_reorganize(app: &axum::Router) -> String {
    let resp: BatchMoveResponse = app
        .post_json("/api/series/actions/reorganize_all_async", &())
        .await;
    resp.task_id.expect("async reorganize returns a task_id")
}

async fn progress(app: &axum::Router, task_id: &str) -> BatchMoveProgress {
    app.get_json(&format!(
        "/api/series/actions/reorganize_all/{task_id}/status"
    ))
    .await
}

/// Poll until the task finishes, asserting the SSoT invariants on every
/// response (a finished task must report `completed == total`).
async fn poll_until_finished(app: &axum::Router, task_id: &str) -> BatchMoveProgress {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let p = progress(app, task_id).await;
        assert_eq!(p.operation_type.as_str(), "reorganize");
        if p.finished {
            assert_eq!(
                p.completed, p.total,
                "a finished task must report completed == total"
            );
            return p;
        }
        assert!(
            Instant::now() < deadline,
            "reorganize task {task_id} did not finish in time"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Find our task in `/api/status.active_operations` by id.
///
/// Matching by id (not `finished`) is deliberate: finished operations are
/// retained for a while, so several completed runs can be present at once.
async fn active_operation(app: &axum::Router, task_id: &str) -> ActiveOperation {
    let status: StatusPayload = app.get_json("/api/status").await;
    status
        .active_operations
        .into_iter()
        .find(|op| op.id == task_id)
        .unwrap_or_else(|| panic!("operation {task_id} missing from active_operations"))
}

#[tokio::test]
async fn reorganize_progress_completes_with_ssot_fields() {
    let (app, _state, _temp) = common::setup_test_app().await;
    common::create_test_series(&app, "Reorg Progress").await;

    let task_id = start_reorganize(&app).await;
    let p = poll_until_finished(&app, &task_id).await;

    assert!(p.finished);
    assert_eq!(p.completed, p.total);
    assert!(p.total >= 1, "one series exists, so total must cover it");
}

#[tokio::test]
async fn active_operations_include_the_started_reorganize() {
    let (app, _state, _temp) = common::setup_test_app().await;
    common::create_test_series(&app, "Reorg Active").await;

    let task_id = start_reorganize(&app).await;

    // Deserializing into `ActiveOperation` enforces the full field set the
    // frontend reconciler depends on; a dropped/renamed field fails here.
    let status: StatusPayload = app.get_json("/api/status").await;
    assert!(
        status
            .active_operations
            .iter()
            .any(|op| op.operation_type == "reorganize"),
        "at least one reorganize operation must be present"
    );

    let op = active_operation(&app, &task_id).await;
    assert_eq!(op.operation_type, "reorganize");
    assert!(op.total >= op.completed);
    poll_until_finished(&app, &task_id).await;
}

#[tokio::test]
async fn finished_reorganize_reports_finished_at_ms_within_window() {
    let (app, _state, _temp) = common::setup_test_app().await;
    common::create_test_series(&app, "Reorg Finished").await;

    let started_at = now_unix_ms();
    let task_id = start_reorganize(&app).await;
    poll_until_finished(&app, &task_id).await;
    let observed_at = now_unix_ms();

    let op = active_operation(&app, &task_id).await;
    assert!(op.finished);

    let finished_at_ms = op.finished_at_ms.expect("finished_at_ms is set on finish");
    assert!(finished_at_ms > 0);
    // Allow 5s of clock skew between the test and the server timestamp.
    assert!(finished_at_ms >= started_at.saturating_sub(5_000));
    assert!(finished_at_ms <= observed_at + 5_000);

    // The frontend recency gate (30 min) must accept a just-finished task.
    let age_ms = now_unix_ms().saturating_sub(finished_at_ms);
    assert!(age_ms < 30 * 60 * 1000);
}
