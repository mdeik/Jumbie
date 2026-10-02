use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StorageHealth {
    pub path: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PermissionCheck {
    pub path: String,
    pub can_read: bool,
    pub can_write: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SystemHealth {
    pub status: String,
    pub uptime_seconds: u64,
    pub memory_used: u64,
    pub memory_total: u64,
    pub cpu_usage: f32,
    pub db_integrity_status: String,
    pub storage_health: Vec<StorageHealth>,
    pub dir_permissions: Vec<PermissionCheck>,
    pub ffmpeg_installed: bool,
    #[serde(default)]
    pub ffmpeg_version: Option<String>,
    #[serde(default)]
    pub downloader_is_disabled: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AboutInfo {
    pub version: String,
    pub os: String,
    pub git_commit: String,
    pub build_date: String,
    pub license: String,
    pub description: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveOperation {
    pub id: String,
    pub operation_type: String,
    pub total: usize,
    pub completed: usize,
    pub finished: bool,
    #[serde(default)]
    pub finished_at_ms: Option<u64>,
    #[serde(default)]
    pub success_count: usize,
    #[serde(default)]
    pub failed: usize,
    #[serde(default)]
    pub errors: Vec<String>,
}
