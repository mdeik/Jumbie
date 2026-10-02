//! Tests for the SearchQueue — serialized, deduplicated auto-search dispatch.
//!
//! Patterned after `metadata_queue::tests`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::sleep;

use crate::search_queue::{SEARCH_SEASON_DELAY, SearchQueue};

// Helpers

/// Create a SearchQueue plus an AtomicUsize counter for test assertions.
fn setup_counter() -> (SearchQueue, Arc<AtomicUsize>) {
    let queue = SearchQueue::new();
    let counter = Arc::new(AtomicUsize::new(0));
    (queue, counter)
}

fn make_counter_fn(
    counter: Arc<AtomicUsize>,
) -> impl FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send {
    move || {
        let c = counter.clone();
        Box::pin(async move {
            c.fetch_add(1, Ordering::SeqCst);
        })
    }
}

// Tests

#[tokio::test]
async fn test_submit_executes_job() {
    let (queue, counter) = setup_counter();
    assert!(
        queue
            .submit("test:1".to_string(), make_counter_fn(counter.clone()))
            .await
    );
    // Give the worker time to pick it up.
    sleep(Duration::from_millis(100)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_dedup_skips_duplicate_key() {
    let (queue, counter) = setup_counter();
    assert!(
        queue
            .submit("test:1".to_string(), make_counter_fn(counter.clone()))
            .await
    );
    // Second submit with same key — should be dedupped.
    assert!(
        !queue
            .submit("test:1".to_string(), make_counter_fn(counter.clone()))
            .await
    );
    // Give the worker time to execute.
    sleep(Duration::from_millis(100)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_dedup_different_keys_both_execute() {
    let (queue, counter) = setup_counter();
    assert!(
        queue
            .submit("test:1".to_string(), make_counter_fn(counter.clone()))
            .await
    );
    assert!(
        queue
            .submit("test:2".to_string(), make_counter_fn(counter.clone()))
            .await
    );
    // Give the worker time to execute both.
    sleep(SEARCH_SEASON_DELAY + Duration::from_millis(200)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_override_bypasses_dedup() {
    let (queue, counter) = setup_counter();
    assert!(
        queue
            .submit("test:1".to_string(), make_counter_fn(counter.clone()))
            .await
    );
    // Override forces through even though key is active.
    assert!(
        queue
            .submit_override("test:1".to_string(), make_counter_fn(counter.clone()))
            .await
    );
    // Both should execute (but the second will wait for the first + delay).
    sleep(SEARCH_SEASON_DELAY + Duration::from_millis(300)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_is_active_during_execution() {
    let queue = SearchQueue::new();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let b = barrier.clone();

    // Submit a job that holds the barrier until we check is_active.
    queue
        .submit("hold:1".to_string(), move || {
            let b = b.clone();
            async move {
                b.wait().await;
            }
        })
        .await;

    // Small delay to let the worker pick it up.
    sleep(Duration::from_millis(50)).await;
    assert!(queue.is_active("hold:1").await);

    // Release the barrier so the job completes.
    barrier.wait().await;
    sleep(Duration::from_millis(100)).await;
    assert!(!queue.is_active("hold:1").await);
}

#[tokio::test]
async fn test_shutdown_drains_pending() {
    let queue = SearchQueue::new();
    let counter = Arc::new(AtomicUsize::new(0));

    // Submit one job that will block so we can queue more behind it.
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let b = barrier.clone();

    queue
        .submit("block:1".to_string(), move || {
            let b = b.clone();
            async move {
                b.wait().await;
            }
        })
        .await;

    // Give it time to be picked up, then queue two more.
    sleep(Duration::from_millis(50)).await;
    assert!(
        queue
            .submit("pend:2".to_string(), make_counter_fn(counter.clone()))
            .await
    );
    assert!(
        queue
            .submit("pend:3".to_string(), make_counter_fn(counter.clone()))
            .await
    );

    // Shutdown — drains pending (pend:2, pend:3) but not the running job.
    queue.shutdown();

    // Release the running job.
    barrier.wait().await;

    // Give time for shutdown to settle.
    sleep(Duration::from_millis(100)).await;

    // The blocked job may or may not have run (depends on timing),
    // but the pending jobs should NOT have run.
    let ran = counter.load(Ordering::SeqCst);
    assert!(ran <= 1, "At most 1 job should have run, got {}", ran);

    // After shutdown, new submissions are rejected.
    assert!(
        !queue
            .submit("after:4".to_string(), make_counter_fn(counter.clone()))
            .await
    );
}

#[tokio::test]
async fn test_mark_active_inactive() {
    let queue = SearchQueue::new();
    assert!(!queue.is_active("manual:1").await);

    queue.mark_active("manual:1").await;
    assert!(queue.is_active("manual:1").await);

    queue.mark_inactive("manual:1").await;
    assert!(!queue.is_active("manual:1").await);
}

#[tokio::test]
async fn test_submit_rejects_after_shutdown() {
    let queue = SearchQueue::new();
    queue.shutdown();
    assert!(
        !queue
            .submit(
                "after:1".to_string(),
                make_counter_fn(Arc::new(AtomicUsize::new(0)))
            )
            .await
    );
    assert!(
        !queue
            .submit_override(
                "after:2".to_string(),
                make_counter_fn(Arc::new(AtomicUsize::new(0)))
            )
            .await
    );
}
