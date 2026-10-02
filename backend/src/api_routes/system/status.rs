//! Unified system-status endpoint: a lightweight payload of indicator booleans
//! the frontend polls on a 60-second interval.
//!
//! Separate from /health because /health runs expensive checks (disk I/O, DB
//! integrity, ffprobe version) that shouldn't fire every 60s just to update an
//! icon. Rename-queue state is approximated from `failed_renames` /
//! `processing_renames` rather than recomputing every series' plan.

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::{ActiveOperation, AppState};
use crate::error::AppError;

#[derive(Serialize, Deserialize)]
pub struct SystemStatus {
    pub download_queue_has_failed: bool,
    pub download_queue_has_items: bool,
    pub rename_queue_has_failed: bool,
    pub rename_queue_populated: bool,
    pub downloader_disabled: bool,
    pub wanted_has_items: bool,
    #[serde(default)]
    pub locked_series: Vec<String>,
    #[serde(default)]
    pub active_operations: Vec<ActiveOperation>,
}

pub async fn get_system_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SystemStatus>, AppError> {
    let download_queue = state.db.get_download_queue().await.unwrap_or_default();
    let download_queue_has_failed = download_queue.iter().any(|item| item.status == "Failed");
    let download_queue_has_items = !download_queue.is_empty();

    let rename_queue_has_failed = !state.failed_renames.read().await.is_empty();
    let rename_queue_populated = {
        let failed = state.failed_renames.read().await;
        let processing = state.processing_renames.read().await;
        let has_pending = *state.rename_queue_has_pending.read().await;
        !failed.is_empty() || !processing.is_empty() || has_pending
    };

    let downloader_disabled = if let Some(dl) = &state.downloader {
        let lock = dl.read().await;
        lock.is_no_client().await
    } else {
        true
    };

    let wanted_has_items = state
        .db
        .has_wanted_episodes_by_preference()
        .await
        .unwrap_or(false);

    let locked_series: Vec<String> = state
        .modifying_series
        .read()
        .await
        .iter()
        .cloned()
        .collect();

    let active_operations = state.progress_tracker.all_active(std::time::Instant::now());

    Ok(Json(SystemStatus {
        download_queue_has_failed,
        download_queue_has_items,
        rename_queue_has_failed,
        rename_queue_populated,
        downloader_disabled,
        wanted_has_items,
        locked_series,
        active_operations,
    }))
}
