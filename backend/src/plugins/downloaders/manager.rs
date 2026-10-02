use crate::plugins::bridge::BridgeError;
use crate::plugins::{PluginInstance, PluginManager};
use anyhow::Result;
use jumbie_shared::plugin::Capability;
use jumbie_shared::protocol::matches_protocol;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Log a downloader read that was skipped.
///
/// "Method not implemented" is expected for optional methods (debug); anything
/// else — a plugin error or a malformed response — is a real fault (warn), so a
/// broken plugin isn't silently mistaken for "no data".
fn log_skipped_read(method: &str, plugin: &dyn PluginInstance, err: &BridgeError) {
    let unsupported = matches!(
        err,
        BridgeError::Call(inner) if crate::plugins::policy::is_method_not_supported(inner)
    );
    if unsupported {
        tracing::debug!(
            "{}: plugin {} ({}) does not implement the method; skipping",
            method,
            plugin.plugin_info().display_name,
            plugin.instance_id()
        );
    } else {
        tracing::warn!(
            "{}: plugin {} ({}) returned an error; skipping: {}",
            method,
            plugin.plugin_info().display_name,
            plugin.instance_id(),
            err
        );
    }
}

/// Orchestrates multiple downloader clients with priority-based fallback.
///
/// Holds no direct references to downloader implementations — it queries the
/// central PluginManager for plugins advertising the Downloader capability, so
/// downloader config can hot-reload without rebuilding the manager and new
/// downloader types register themselves automatically.
///
/// Two query patterns:
///   - **Fallback (write/action):** try clients in priority order, stop at the
///     first success (`add_download`, `pause_download`, ...).
///   - **Aggregate (read):** collect from ALL clients and deduplicate
///     (`get_completed_downloads`, `get_download_paths`).
///
/// Disabled clients are tracked here (permanently vs. temporarily) so the manager
/// can skip them without mutating PluginManager state, enabling per-request
/// client exclusion (e.g. a client that recently hit a rate limit).
pub struct DownloadManager {
    plugin_manager: Arc<RwLock<PluginManager>>,
    pub permanently_disabled_clients: Vec<String>,
    pub temporarily_disabled_clients:
        std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
}

impl DownloadManager {
    pub fn new(plugin_manager: Arc<RwLock<PluginManager>>) -> Self {
        Self {
            plugin_manager,
            permanently_disabled_clients: Vec::new(),
            temporarily_disabled_clients: std::collections::HashMap::new(),
        }
    }

    /// Returns all downloader plugins in dispatch order.
    ///
    /// SSoT: `PluginManager` already orders by priority DESC, then `instance_id`
    /// ASC — so this is just a capability lookup. The "fallback (write/action)"
    /// pattern iterates this list and stops at the first success; the
    /// "aggregate (read)" pattern collects from all.
    async fn get_sorted_downloader_plugins(&self) -> Vec<Arc<dyn PluginInstance>> {
        self.plugin_manager
            .read()
            .await
            .get_plugins_by_capability(Capability::Downloader)
    }

    /// Try each downloader plugin (in priority order), return the first non-None progress.
    /// Delegates to the bridge for deserialization and validation (range 0.0..1.0).
    pub async fn get_download_progress(&self, hash: &str) -> Result<Option<f32>> {
        let plugins = self.get_sorted_downloader_plugins().await;
        for plugin in plugins {
            match crate::plugins::bridge::downloaders::get_download_progress(plugin.as_ref(), hash)
                .await
            {
                Ok(Some(p)) => return Ok(Some(p)),
                Ok(None) => continue,
                Err(e) => {
                    log_skipped_read("get_download_progress", plugin.as_ref(), &e);
                    continue;
                }
            }
        }
        Ok(None)
    }

    /// Try each downloader plugin (in priority order), return the first non-None status.
    pub async fn get_download_status(&self, hash: &str) -> Result<Option<String>> {
        let plugins = self.get_sorted_downloader_plugins().await;
        for plugin in plugins {
            match crate::plugins::bridge::downloaders::get_download_status(plugin.as_ref(), hash)
                .await
            {
                Ok(Some(s)) => return Ok(Some(s)),
                Ok(None) => continue,
                Err(e) => {
                    log_skipped_read("get_download_status", plugin.as_ref(), &e);
                    continue;
                }
            }
        }
        Ok(None)
    }

    /// Try each downloader plugin (in priority order), return the first reported
    /// hard failure reason. Plugins that don't implement the method are skipped.
    pub async fn get_download_failure(&self, hash: &str) -> Option<String> {
        let plugins = self.get_sorted_downloader_plugins().await;
        for plugin in plugins {
            match crate::plugins::bridge::downloaders::get_download_failure(plugin.as_ref(), hash)
                .await
            {
                Ok(Some(reason)) => return Some(reason),
                Ok(None) => continue,
                Err(e) => {
                    log_skipped_read("get_download_failure", plugin.as_ref(), &e);
                    continue;
                }
            }
        }
        None
    }

    /// Try each downloader plugin (in priority order), return the first content path.
    /// Delegates to the bridge for path validation (non-empty, ≤4096).
    ///
    /// SSoT: Returns an error if no plugin could provide a path — a missing hash
    /// is treated as a client error, not a silent None.
    pub async fn get_download_content_path(&self, hash: &str) -> Result<String> {
        let plugins = self.get_sorted_downloader_plugins().await;
        for plugin in plugins {
            match crate::plugins::bridge::downloaders::get_download_content_path(
                plugin.as_ref(),
                hash,
            )
            .await
            {
                Ok(p) => return Ok(p),
                Err(e) => {
                    tracing::debug!(
                        "get_download_content_path: plugin {} ({}) returned error: {}",
                        plugin.plugin_info().display_name,
                        plugin.instance_id(),
                        e
                    );
                    continue;
                }
            }
        }
        Err(anyhow::anyhow!(
            "No downloader plugin found content path for hash={}",
            hash
        ))
    }

    /// Check whether the download client with the given `client_id` supports pause/resume.
    pub async fn supports_pause_resume(&self, client_id: &str) -> bool {
        let plugins = self.get_sorted_downloader_plugins().await;
        let pm = self.plugin_manager.read().await;
        for plugin in plugins {
            if plugin.instance_id() == client_id {
                // Route through the manager's effective-capability chokepoint so
                // a future per-instance toggle for `CanPauseResume` is honored
                // here (SSoT: `plugins::capabilities`).
                return pm
                    .effective_capabilities_for(&plugin)
                    .contains(&Capability::CanPauseResume);
            }
        }
        false
    }

    /// Try each downloader plugin (in priority order), return the first matching download ID.
    /// Delegates to the bridge for ID validation (non-empty, ≤128).
    pub async fn get_download_id_by_name(&self, name: &str) -> Result<Option<String>> {
        let plugins = self.get_sorted_downloader_plugins().await;
        for plugin in plugins {
            match crate::plugins::bridge::downloaders::get_download_id_by_name(
                plugin.as_ref(),
                name,
            )
            .await
            {
                Ok(Some(id)) => return Ok(Some(id)),
                Ok(None) => continue,
                Err(e) => {
                    tracing::debug!(
                        "get_download_id_by_name: plugin {} ({}) returned error: {}",
                        plugin.plugin_info().display_name,
                        plugin.instance_id(),
                        e
                    );
                    continue;
                }
            }
        }
        Ok(None)
    }

    pub async fn pause_download(&self, hash: &str, client_id: Option<&str>) -> Result<()> {
        let plugins = self.get_sorted_downloader_plugins().await;
        for plugin in plugins {
            if let Some(cid) = client_id
                && plugin.instance_id() != cid
            {
                continue;
            }
            match crate::plugins::bridge::downloaders::pause_download(plugin.as_ref(), hash).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::debug!(
                        "pause_download failed for hash={} on plugin '{}' ({}): {}",
                        hash,
                        plugin.plugin_info().display_name,
                        plugin.instance_id(),
                        e
                    );
                }
            }
        }
        anyhow::bail!(
            "Failed to pause download — check that the download client is reachable and the torrent hasn't been removed"
        )
    }

    pub async fn resume_download(&self, hash: &str, client_id: Option<&str>) -> Result<()> {
        let plugins = self.get_sorted_downloader_plugins().await;
        for plugin in plugins {
            if let Some(cid) = client_id
                && plugin.instance_id() != cid
            {
                continue;
            }
            match crate::plugins::bridge::downloaders::resume_download(plugin.as_ref(), hash).await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::debug!(
                        "resume_download failed for hash={} on plugin '{}' ({}): {}",
                        hash,
                        plugin.plugin_info().display_name,
                        plugin.instance_id(),
                        e
                    );
                }
            }
        }
        anyhow::bail!(
            "Failed to resume download — check that the download client is reachable and the torrent hasn't been removed"
        )
    }

    /// Signal that a download has been fully processed and the client can clean up.
    ///
    /// Delegates directly to the identified download client's completion mechanism
    /// (for qBittorrent this removes the torrent entry without deleting files).
    /// Other clients may no-op. Like `retry_download`, requires an explicit
    /// `client_id` — there is no iteration over all plugins.
    pub async fn complete_download(&self, hash: &str, client_id: &str) -> Result<()> {
        let plugins = self.get_sorted_downloader_plugins().await;
        let plugin = plugins
            .into_iter()
            .find(|p| p.instance_id() == client_id)
            .ok_or_else(|| {
                anyhow::anyhow!("No downloader plugin found with client_id '{}'", client_id)
            })?;
        crate::plugins::bridge::downloaders::complete_download(plugin.as_ref(), hash)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }

    /// Pre-requeue hook: let the identified download client clean up or recover
    /// before the item re-enters the dispatch cycle.
    ///
    /// Requires an explicit `client_id` (with no iteration over all plugins) so a
    /// different client's no-op `retry` implementation can't swallow the call and
    /// prevent reaching the actual target. Errors if the plugin doesn't exist.
    pub async fn retry_download(&self, hash: &str, client_id: &str) -> Result<()> {
        let plugins = self.get_sorted_downloader_plugins().await;
        let plugin = plugins
            .into_iter()
            .find(|p| p.instance_id() == client_id)
            .ok_or_else(|| {
                anyhow::anyhow!("No downloader plugin found with client_id '{}'", client_id)
            })?;
        crate::plugins::bridge::downloaders::retry(plugin.as_ref(), hash)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }

    pub async fn delete_download(
        &self,
        hash: &str,
        delete_files: bool,
        client_id: Option<&str>,
    ) -> Result<()> {
        let plugins = self.get_sorted_downloader_plugins().await;
        for plugin in plugins {
            if let Some(cid) = client_id
                && plugin.instance_id() != cid
            {
                continue;
            }
            match crate::plugins::bridge::downloaders::delete_download(
                plugin.as_ref(),
                hash,
                delete_files,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::debug!(
                        "delete_download failed for hash={} on plugin '{}' ({}): {}",
                        hash,
                        plugin.plugin_info().display_name,
                        plugin.instance_id(),
                        e
                    );
                }
            }
        }
        anyhow::bail!(
            "Failed to delete download — check that the download client is reachable and the torrent hasn't been removed"
        )
    }

    pub async fn client_count(&self) -> usize {
        self.get_sorted_downloader_plugins().await.len()
    }

    /// Get all configured download paths across enabled clients.
    /// Delegates to the bridge for path validation (non-empty, ≤4096).
    pub async fn get_download_paths(&self) -> Vec<std::path::PathBuf> {
        let plugins = self.get_sorted_downloader_plugins().await;
        let mut paths = Vec::new();
        for plugin in plugins {
            if let Ok(Some(p)) =
                crate::plugins::bridge::downloaders::get_download_path(plugin.as_ref()).await
            {
                paths.push(std::path::PathBuf::from(p));
            }
        }
        paths
    }

    /// Get all configured organizer (staging) paths across enabled clients.
    /// Delegates to the bridge for path validation (non-empty, ≤4096).
    pub async fn get_organizer_paths(&self) -> Vec<std::path::PathBuf> {
        let plugins = self.get_sorted_downloader_plugins().await;
        let mut paths = Vec::new();
        for plugin in plugins {
            if let Ok(Some(p)) =
                crate::plugins::bridge::downloaders::get_organizer_path(plugin.as_ref()).await
            {
                paths.push(std::path::PathBuf::from(p));
            }
        }
        paths
    }

    /// All configured download roots: organizer (staging) paths where set, plus
    /// the raw download paths. These bound empty-staging-folder cleanup — a path
    /// under any of them has its per-download folder culled when empty, but the
    /// root itself is never removed.
    pub async fn get_download_roots(&self) -> Vec<std::path::PathBuf> {
        let mut roots = self.get_organizer_paths().await;
        roots.extend(self.get_download_paths().await);
        roots
    }

    /// Create a manager with no download client configured.
    ///
    /// The organizer pipeline always expects a `DownloadManager`; a dummy
    /// PluginManager with no registered plugins makes every capability query
    /// return empty, so `add_download` logs the intent and returns an empty
    /// client ID instead of failing.
    pub fn no_client() -> Self {
        let pm = Arc::new(RwLock::new(PluginManager::new(
            std::env::temp_dir().join("jb_dummy_plugins"),
        )));
        Self::new(pm)
    }

    /// Add a download to the download client with fallback, returning the
    /// `instance_id()` of the client that accepted it.
    ///
    /// Clients are tried in priority order; `matches_protocol` is checked first so
    /// a BitTorrent-only client isn't handed an HTTP(S) URL. If all clients fail,
    /// the LAST error is returned so the user sees the real failure reason.
    ///
    /// No retry on the same client: download client errors are typically config
    /// issues (wrong URL, auth), so retrying yields the same error with a delay.
    ///
    /// Delegates payload construction and URL validation to the bridge.
    pub async fn add_download(
        &self,
        url: &str,
        category: &str,
        tag: Option<&str>,
        title: Option<&str>,
    ) -> Result<String> {
        let plugins = self.get_sorted_downloader_plugins().await;

        if plugins.is_empty() {
            tracing::info!(
                "[NO-CLIENT MODE] Would add download: {} (category: {})",
                url,
                category
            );
            return Ok(String::new());
        }

        let mut last_error = None;
        for plugin in plugins {
            // Filter by protocol
            if let Some(supported) = plugin.supported_protocols() {
                let matches = supported
                    .iter()
                    .any(|pattern| matches_protocol(pattern, url));
                if !matches {
                    tracing::debug!(
                        "Skipping downloader '{}' ({}) as it doesn't support protocol for link",
                        plugin.plugin_info().display_name,
                        plugin.instance_id()
                    );
                    continue;
                }
            }

            match crate::plugins::bridge::downloaders::add_download(
                plugin.as_ref(),
                url,
                category,
                tag,
                title,
            )
            .await
            {
                Ok(()) => return Ok(plugin.instance_id().to_string()),
                Err(e) => {
                    tracing::debug!(
                        "Downloader plugin '{}' ({}) failed: {}",
                        plugin.plugin_info().display_name,
                        plugin.instance_id(),
                        e
                    );
                    last_error = Some(anyhow::anyhow!(e.to_string()));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No downloader plugins available")))
    }

    /// Get list of completed download IDs, aggregated from ALL active clients.
    /// Delegates to the bridge for ID validation (non-empty, ≤128).
    pub async fn get_completed_downloads(&self) -> Vec<String> {
        let plugins = self.get_sorted_downloader_plugins().await;
        let mut all_hashes = Vec::new();
        for plugin in plugins {
            if let Ok(ids) =
                crate::plugins::bridge::downloaders::get_completed_downloads(plugin.as_ref()).await
            {
                for hash in ids {
                    if !all_hashes.contains(&hash) {
                        all_hashes.push(hash);
                    }
                }
            }
        }
        all_hashes
    }

    /// Check if running in no-client mode
    pub async fn is_no_client(&self) -> bool {
        self.get_sorted_downloader_plugins().await.is_empty()
    }

    /// Test connectivity for each downloader client.
    /// Returns a list of (display_name, ok, error_message).
    pub async fn test_connections(&self) -> Vec<(String, bool, Option<String>)> {
        let plugins = self.get_sorted_downloader_plugins().await;
        let mut results = Vec::new();
        for plugin in plugins {
            let id = plugin.instance_id().to_string();
            match plugin.call("test_connection", None).await {
                Ok(_) => results.push((id, true, None)),
                Err(e) => results.push((id, false, Some(e.to_string()))),
            }
        }
        results
    }
}

impl std::fmt::Debug for DownloadManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadManager")
            .field("disabled_count", &self.permanently_disabled_clients.len())
            .finish()
    }
}

#[cfg(test)]
#[path = "tests/manager.rs"]
mod tests;
