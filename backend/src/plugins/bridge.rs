//! Typed bridge between the generic `PluginInstance::call()` RPC dispatch and
//! the backend's type-safe domain layer.
//!
//! Every plugin method returns `serde_json::Value`, so callers must deserialize
//! to the expected type and validate the fields.  This module is the SSoT for
//! which Rust type each plugin method returns, what validation is applied, and
//! how errors are reported.  Each plugin category gets its own sub-module; the
//! generic [`call_typed`] helper handles call → deserialize → validate.

use crate::plugins::PluginInstance;
use crate::plugins::metadata::{SeriesMetadata, SeriesMetadataInfo};
use crate::validation::MAX_PATH_LENGTH;
use jumbie_shared::validation::{
    Validate,
    fields::{MAX_ID_LENGTH, MAX_TITLE_LENGTH},
};
use serde::de::DeserializeOwned;

/// Unified error type for bridge calls.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("Plugin call failed: {0}")]
    Call(#[from] anyhow::Error),
    #[error("Plugin response deserialization failed: {0}")]
    Deserialization(String),
    #[error("Plugin response validation failed: {0}")]
    Validation(String),
}

impl BridgeError {
    /// True if the bridge should treat this as a transient failure (network, timeout).
    pub fn is_transient(&self) -> bool {
        matches!(self, BridgeError::Call(_))
    }
}

/// Bound a plugin-returned collection to `MAX_PLUGIN_ITEMS`.
///
/// Per-item validators bound each element's size; this bounds the count so a
/// runaway plugin can't exhaust resources with an enormous list. Truncation is
/// logged at `warn` — a legitimate plugin never hits the limit.
fn cap_plugin_items<T>(mut items: Vec<T>, method: &str, plugin: &dyn PluginInstance) -> Vec<T> {
    use jumbie_shared::validation::plugin_data::MAX_PLUGIN_ITEMS;
    if items.len() > MAX_PLUGIN_ITEMS {
        tracing::warn!(
            "{}: plugin {} returned {} items; truncating to {}",
            method,
            plugin.plugin_info().display_name,
            items.len(),
            MAX_PLUGIN_ITEMS
        );
        items.truncate(MAX_PLUGIN_ITEMS);
    }
    items
}

impl From<jumbie_shared::validation::ValidationError> for BridgeError {
    /// Lets validation helpers be used with `?` directly — their `ValidationError`
    /// is a schema/value error, surfaced as [`BridgeError::Validation`].
    fn from(e: jumbie_shared::validation::ValidationError) -> Self {
        BridgeError::Validation(e.0)
    }
}

/// Call a plugin method, deserialize the response to `T`, and validate it.
/// The single typed entry point for plugin→backend communication.
async fn call_typed<T: DeserializeOwned + Validate>(
    plugin: &dyn PluginInstance,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<T, BridgeError> {
    let raw = plugin
        .call(method, params)
        .await
        .map_err(BridgeError::Call)?;
    let value: T =
        serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
    value.validate().map_err(|errors| {
        BridgeError::Validation(
            errors
                .into_iter()
                .map(|e| e.0)
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    Ok(value)
}

// Metadata bridge

pub mod metadata {
    use super::*;

    use crate::plugins::methods;

    /// Fetch episodes and seasons from a metadata plugin.
    ///
    /// The plugin returns `{ episodes_and_seasons: SeriesMetadata, series_info: ... }`.
    /// This function extracts and validates only the `episodes_and_seasons` payload.
    pub async fn fetch_episodes_and_seasons(
        plugin: &dyn PluginInstance,
        plugin_series_id: &str,
        absolute_numbering: bool,
    ) -> Result<SeriesMetadata, BridgeError> {
        let raw = plugin
            .call(
                methods::METHOD_FETCH_SERIES_METADATA,
                Some(serde_json::json!({
                    "id": plugin_series_id,
                    "absolute_numbering": absolute_numbering,
                })),
            )
            .await
            .map_err(BridgeError::Call)?;

        let inner = raw
            .get("episodes_and_seasons")
            .ok_or_else(|| {
                BridgeError::Deserialization(
                    "Missing 'episodes_and_seasons' in metadata response".to_string(),
                )
            })?
            .clone();

        let metadata: SeriesMetadata = serde_json::from_value(inner)
            .map_err(|e| BridgeError::Deserialization(e.to_string()))?;

        metadata.validate().map_err(|errors| {
            BridgeError::Validation(
                errors
                    .into_iter()
                    .map(|e| e.0)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;

        Ok(metadata)
    }

    /// Fetch series info (title, overview, image) from a metadata plugin.
    pub async fn fetch_series_info(
        plugin: &dyn PluginInstance,
        plugin_series_id: &str,
    ) -> Result<SeriesMetadataInfo, BridgeError> {
        call_typed::<SeriesMetadataInfo>(
            plugin,
            methods::METHOD_FETCH_SERIES_INFO,
            Some(serde_json::json!({ "id": plugin_series_id })),
        )
        .await
    }

    /// Fetch series aliases from a metadata plugin.
    /// Returns a validated list of alias strings (each non-empty, max 500 chars).
    pub async fn fetch_series_aliases(
        plugin: &dyn PluginInstance,
        plugin_series_id: &str,
    ) -> Result<Vec<String>, BridgeError> {
        let raw = plugin
            .call(
                methods::METHOD_FETCH_SERIES_ALIASES,
                Some(serde_json::json!({ "id": plugin_series_id })),
            )
            .await
            .map_err(BridgeError::Call)?;
        let aliases: Vec<String> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        let mut valid = Vec::with_capacity(aliases.len());
        for (i, alias) in aliases.iter().enumerate() {
            if alias.trim().is_empty() {
                tracing::debug!("fetch_series_aliases: skipping empty alias [{}]", i);
            } else if alias.len() > MAX_TITLE_LENGTH {
                tracing::debug!(
                    "fetch_series_aliases: skipping alias [{}] too long ({} chars, max {})",
                    i,
                    alias.len(),
                    MAX_TITLE_LENGTH
                );
            } else {
                valid.push(alias.clone());
            }
        }
        Ok(cap_plugin_items(valid, "fetch_series_aliases", plugin))
    }

    /// Fetch updated series IDs from a metadata plugin.
    /// Each returned ID is validated (non-empty, ≤512 chars); invalid ones are logged and skipped.
    pub async fn fetch_updated_series(
        plugin: &dyn PluginInstance,
    ) -> Result<Vec<String>, BridgeError> {
        let raw = plugin
            .call(
                methods::METHOD_GET_UPDATED_SERIES,
                Some(serde_json::json!({"days": 1})),
            )
            .await
            .map_err(BridgeError::Call)?;
        let ids: Vec<String> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        let mut valid = Vec::with_capacity(ids.len());
        for id in ids {
            if id.trim().is_empty() || id.len() > MAX_ID_LENGTH {
                tracing::warn!(
                    "fetch_updated_series: skipping invalid series ID (max {} chars): {:?}",
                    MAX_ID_LENGTH,
                    id.chars().take(20).collect::<String>()
                );
            } else {
                valid.push(id);
            }
        }
        Ok(cap_plugin_items(valid, "get_updated_series", plugin))
    }
}

// Source bridge
// Plugins validate `MediaEntry` internally; this re-validates at the consumer boundary.

pub mod sources {
    use super::*;
    use crate::plugins::methods;
    use crate::search::SearchLog;
    use jumbie_shared::types::media::MediaEntry;
    use jumbie_shared::validation::plugin_data::sanitize_search_queries;
    use plugin_sdk::query::SearchResponse;

    /// Search a source plugin. The plugin returns the [`SearchResponse`] envelope;
    /// its reported `queries` are logged via `log` and the entries validated.
    pub async fn search(
        plugin: &dyn PluginInstance,
        query: &str,
        log: &SearchLog,
    ) -> Result<Vec<MediaEntry>, BridgeError> {
        let raw = plugin
            .call(
                methods::METHOD_SEARCH,
                Some(serde_json::json!({ "query": query })),
            )
            .await
            .map_err(BridgeError::Call)?;

        let response: SearchResponse<Vec<MediaEntry>> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        log.emit(&sanitize_search_queries(response.queries));

        // Collect invalid entries rather than rejecting the whole batch.
        let (clean, rejected) = jumbie_shared::validation::validate_and_filter(response.entries);
        if !rejected.is_empty() {
            tracing::debug!(
                "source.search: filtered {} invalid entries from plugin {}",
                rejected.len(),
                plugin.plugin_info().display_name,
            );
            for (i, errors) in &rejected {
                for err in errors {
                    tracing::debug!("  entry [{}]: {}", i, err.0);
                }
            }
        }
        Ok(cap_plugin_items(clean, "search", plugin))
    }

    /// Auto-search a source plugin for specific episodes. The plugin reports the
    /// query string(s) it built; those are logged via `log`.
    pub async fn auto_search(
        plugin: &dyn PluginInstance,
        params: serde_json::Value,
        log: &SearchLog,
    ) -> Result<Vec<MediaEntry>, BridgeError> {
        let raw = plugin
            .call(methods::METHOD_AUTO_SEARCH, Some(params))
            .await
            .map_err(BridgeError::Call)?;

        let response: SearchResponse<Vec<MediaEntry>> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        log.emit(&sanitize_search_queries(response.queries));

        let (clean, rejected) = jumbie_shared::validation::validate_and_filter(response.entries);
        if !rejected.is_empty() {
            tracing::debug!(
                "source.auto_search: filtered {} invalid entries from plugin {}",
                rejected.len(),
                plugin.plugin_info().display_name,
            );
        }
        Ok(cap_plugin_items(clean, "auto_search", plugin))
    }

    /// Fetch all entries from a polling source plugin.
    pub async fn fetch_entries(
        plugin: &dyn PluginInstance,
    ) -> Result<Vec<MediaEntry>, BridgeError> {
        let raw = plugin
            .call(methods::METHOD_FETCH_ENTRIES, None)
            .await
            .map_err(BridgeError::Call)?;

        let entries: Vec<MediaEntry> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;

        let (clean, rejected) = jumbie_shared::validation::validate_and_filter(entries);
        if !rejected.is_empty() {
            tracing::debug!(
                "source.fetch_entries: filtered {} invalid entries from plugin {}",
                rejected.len(),
                plugin.plugin_info().display_name,
            );
        }
        Ok(cap_plugin_items(clean, "fetch_entries", plugin))
    }
}

// Downloader bridge

pub mod downloaders {
    use super::*;
    use crate::plugins::methods;
    use jumbie_shared::validation::fields::{
        MAX_FAILURE_REASON_LENGTH, MAX_MESSAGE_LENGTH, MAX_STATUS_LENGTH, sanitize_failure_reason,
        validate_max_length, validate_no_control_chars,
    };

    /// Validate a download client path (must be non-empty, reasonable length).
    fn validate_path(path: &str) -> Result<(), BridgeError> {
        if path.is_empty() {
            return Err(BridgeError::Validation(
                "Download path is empty".to_string(),
            ));
        }
        if path.len() > MAX_PATH_LENGTH {
            return Err(BridgeError::Validation(format!(
                "Download path too long: {} chars (max {})",
                path.len(),
                MAX_PATH_LENGTH
            )));
        }
        Ok(())
    }

    /// Validate a download identifier returned by the downloader.
    ///
    /// Download IDs are opaque strings — they can be info hashes (SHA-1 = 40 chars),
    /// numeric database IDs, or any client-native identifier.  We enforce only basic
    /// hygiene: non-empty and reasonable length.
    fn validate_download_id(id: &str) -> Result<(), BridgeError> {
        if id.is_empty() {
            return Err(BridgeError::Validation(
                "Download identifier is empty".to_string(),
            ));
        }
        if id.len() > 128 {
            return Err(BridgeError::Validation(format!(
                "Download identifier too long: {} chars (max 128)",
                id.len()
            )));
        }
        Ok(())
    }

    /// Add a download URL to the download client.
    ///
    /// The URL is validated as a download link (magnet or scheme-bearing URI) —
    /// the manager relies on this boundary for URL validation.
    pub async fn add_download(
        plugin: &dyn PluginInstance,
        url: &str,
        category: &str,
        tag: Option<&str>,
        title: Option<&str>,
    ) -> Result<(), BridgeError> {
        jumbie_shared::validation::validate_download_link(url)?;
        plugin
            .call(
                methods::METHOD_ADD_DOWNLOAD,
                Some(serde_json::json!({
                    "url": url,
                    "category": category,
                    "tag": tag,
                    "title": title,
                })),
            )
            .await
            .map_err(BridgeError::Call)?;
        Ok(())
    }

    /// Get completed download identifiers from the client.
    /// Each ID is validated (non-empty, ≤128 chars); invalid ones are logged and skipped.
    pub async fn get_completed_downloads(
        plugin: &dyn PluginInstance,
    ) -> Result<Vec<String>, BridgeError> {
        let raw = plugin
            .call(methods::METHOD_GET_COMPLETED_DOWNLOADS, None)
            .await
            .map_err(BridgeError::Call)?;
        let ids: Vec<String> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        let mut valid = Vec::with_capacity(ids.len());
        for (i, id) in ids.into_iter().enumerate() {
            match validate_download_id(&id) {
                Ok(()) => valid.push(id),
                Err(e) => tracing::warn!(
                    "get_completed_downloads[{}]: invalid download ID from {}: {}",
                    i,
                    plugin.plugin_info().display_name,
                    e,
                ),
            }
        }
        Ok(cap_plugin_items(valid, "get_completed_downloads", plugin))
    }

    /// Get download progress (0.0 to 1.0) for a specific hash.
    pub async fn get_download_progress(
        plugin: &dyn PluginInstance,
        hash: &str,
    ) -> Result<Option<f32>, BridgeError> {
        let raw = plugin
            .call(
                methods::METHOD_GET_DOWNLOAD_PROGRESS,
                Some(serde_json::json!(hash)),
            )
            .await
            .map_err(BridgeError::Call)?;
        let progress: Option<f32> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        if let Some(p) = progress
            && !(0.0..=1.0).contains(&p)
        {
            return Err(BridgeError::Validation(format!(
                "Download progress out of range: {} (expected 0.0..1.0)",
                p
            )));
        }
        Ok(progress)
    }

    /// Get download status string for a specific hash.
    ///
    /// The status is an opaque client token (e.g. `"downloading"`), bounded in
    /// length and free of control characters so it can be compared and displayed
    /// safely.
    pub async fn get_download_status(
        plugin: &dyn PluginInstance,
        hash: &str,
    ) -> Result<Option<String>, BridgeError> {
        let raw = plugin
            .call(
                methods::METHOD_GET_DOWNLOAD_STATUS,
                Some(serde_json::json!(hash)),
            )
            .await
            .map_err(BridgeError::Call)?;
        let status: Option<String> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        if let Some(ref s) = status {
            validate_max_length(s, "Download status", MAX_STATUS_LENGTH)?;
            validate_no_control_chars(s, "Download status")?;
        }
        Ok(status)
    }

    /// Ask the client whether a download has hard-failed, and why.
    ///
    /// SSoT: the plugin owns the definition of "failed" for its client
    /// (e.g. qBittorrent's `error` / `missingFiles` states). A null response or
    /// an unimplemented method means "no failure reported".
    ///
    /// The reason is a terminal signal, so a malformed one is *sanitized* rather
    /// than rejected (see [`sanitize_failure_reason`]): trimming, replacing
    /// control characters, and bounding the length while still honoring the
    /// failure. A sanitized reason is logged at `warn` as a sign of a buggy
    /// plugin.
    pub async fn get_download_failure(
        plugin: &dyn PluginInstance,
        hash: &str,
    ) -> Result<Option<String>, BridgeError> {
        let raw = plugin
            .call(
                methods::METHOD_GET_DOWNLOAD_FAILURE,
                Some(serde_json::json!(hash)),
            )
            .await
            .map_err(BridgeError::Call)?;
        let reason: Option<String> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;

        if let Some(raw_reason) = reason.as_deref()
            && (raw_reason.chars().any(char::is_control)
                || raw_reason.trim().len() > MAX_FAILURE_REASON_LENGTH)
        {
            tracing::warn!(
                "get_download_failure: sanitized {} failure reason from {} chars",
                plugin.plugin_info().display_name,
                raw_reason.len(),
            );
        }
        Ok(reason.as_deref().and_then(sanitize_failure_reason))
    }

    /// Get the content path on disk for a completed download.
    ///
    /// SSoT: Returns a non-empty validated path string.  Plugins must return a
    /// path for any active download; a missing/null response is treated as an
    /// error (plugin malfunction or hash unknown).
    pub async fn get_download_content_path(
        plugin: &dyn PluginInstance,
        hash: &str,
    ) -> Result<String, BridgeError> {
        let raw = plugin
            .call(
                methods::METHOD_GET_DOWNLOAD_CONTENT_PATH,
                Some(serde_json::json!(hash)),
            )
            .await
            .map_err(BridgeError::Call)?;
        let path: String =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        if path.is_empty() {
            return Err(BridgeError::Validation(
                "content path must not be empty".to_string(),
            ));
        }
        validate_path(&path)?;
        Ok(path)
    }

    /// Pause a download by hash.
    pub async fn pause_download(
        plugin: &dyn PluginInstance,
        hash: &str,
    ) -> Result<(), BridgeError> {
        plugin
            .call(
                methods::METHOD_PAUSE_DOWNLOAD,
                Some(serde_json::json!(hash)),
            )
            .await
            .map_err(BridgeError::Call)?;
        Ok(())
    }

    /// Resume a paused download by hash.
    pub async fn resume_download(
        plugin: &dyn PluginInstance,
        hash: &str,
    ) -> Result<(), BridgeError> {
        plugin
            .call(
                methods::METHOD_RESUME_DOWNLOAD,
                Some(serde_json::json!(hash)),
            )
            .await
            .map_err(BridgeError::Call)?;
        Ok(())
    }

    /// Delete a download by hash.
    pub async fn delete_download(
        plugin: &dyn PluginInstance,
        hash: &str,
        delete_files: bool,
    ) -> Result<(), BridgeError> {
        plugin
            .call(
                methods::METHOD_DELETE_DOWNLOAD,
                Some(serde_json::json!({
                    "id": hash,
                    "delete_files": delete_files,
                })),
            )
            .await
            .map_err(BridgeError::Call)?;
        Ok(())
    }

    /// Signal that a download has been fully processed and the client can clean up.
    /// Each plugin decides what that means — qBittorrent removes the torrent entry
    /// without deleting files; other clients may no-op.
    pub async fn complete_download(
        plugin: &dyn PluginInstance,
        hash: &str,
    ) -> Result<(), BridgeError> {
        plugin
            .call(
                methods::METHOD_COMPLETE_DOWNLOAD,
                Some(serde_json::json!(hash)),
            )
            .await
            .map_err(BridgeError::Call)?;
        Ok(())
    }

    /// Retry a failed download by hash.
    pub async fn retry(plugin: &dyn PluginInstance, hash: &str) -> Result<(), BridgeError> {
        plugin
            .call(methods::METHOD_RETRY, Some(serde_json::json!(hash)))
            .await
            .map_err(BridgeError::Call)?;
        Ok(())
    }

    /// Get download ID by name (e.g. for finding existing torrents by label).
    pub async fn get_download_id_by_name(
        plugin: &dyn PluginInstance,
        name: &str,
    ) -> Result<Option<String>, BridgeError> {
        let raw = plugin
            .call(
                methods::METHOD_GET_DOWNLOAD_ID_BY_NAME,
                Some(serde_json::json!(name)),
            )
            .await
            .map_err(BridgeError::Call)?;
        let id: Option<String> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        if let Some(ref id) = id {
            validate_download_id(id)?;
        }
        Ok(id)
    }

    /// Get the download client's download directory path.
    pub async fn get_download_path(
        plugin: &dyn PluginInstance,
    ) -> Result<Option<String>, BridgeError> {
        let raw = plugin
            .call(methods::METHOD_GET_DOWNLOAD_PATH, None)
            .await
            .map_err(BridgeError::Call)?;
        let path: Option<String> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        if let Some(ref p) = path {
            validate_path(p)?;
        }
        Ok(path)
    }

    /// Get the organizer path (where files should be moved after download).
    pub async fn get_organizer_path(
        plugin: &dyn PluginInstance,
    ) -> Result<Option<String>, BridgeError> {
        let raw = plugin
            .call(methods::METHOD_GET_ORGANIZER_PATH, None)
            .await
            .map_err(BridgeError::Call)?;
        let path: Option<String> =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        if let Some(ref p) = path {
            validate_path(p)?;
        }
        Ok(path)
    }

    /// Test the download client connection.
    pub async fn test_connection(plugin: &dyn PluginInstance) -> Result<String, BridgeError> {
        let raw = plugin
            .call(methods::METHOD_TEST_CONNECTION, None)
            .await
            .map_err(BridgeError::Call)?;
        let msg: String =
            serde_json::from_value(raw).map_err(|e| BridgeError::Deserialization(e.to_string()))?;
        if msg.trim().is_empty() {
            return Err(BridgeError::Validation(
                "Test connection returned empty message".to_string(),
            ));
        }
        validate_max_length(&msg, "Test connection message", MAX_MESSAGE_LENGTH)?;
        validate_no_control_chars(&msg, "Test connection message")?;
        Ok(msg)
    }
}

// Notifier bridge
// Methods are fire-and-forget; validation applies to the outgoing context, not the return value.

pub mod notifiers {
    use super::*;
    use crate::plugins::methods;
    use crate::plugins::notifiers::{NotifierContext, NotifierEvent};
    use jumbie_shared::validation::ValidationError;

    /// Notify a notifier plugin of an event. The context is validated before dispatch.
    pub async fn notify(
        plugin: &dyn PluginInstance,
        event: &NotifierEvent,
        context: &NotifierContext,
    ) -> Result<(), BridgeError> {
        let missing = event.validate_context(context);
        if !missing.is_empty() {
            return Err(BridgeError::Validation(format!(
                "Notifier context missing required fields for {:?}: {}",
                event,
                missing.join(", "),
            )));
        }

        let content_errors = validate_notifier_context(context);
        if !content_errors.is_empty() {
            tracing::warn!(
                "notifier context has invalid field values: {}",
                content_errors
                    .iter()
                    .map(|e| e.0.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }

        plugin
            .call(
                methods::METHOD_NOTIFY,
                Some(serde_json::json!({
                    "event": event.as_str(),
                    "context": context,
                })),
            )
            .await
            .map_err(BridgeError::Call)?;
        Ok(())
    }

    /// Validate content of notifier context fields beyond presence.
    fn validate_notifier_context(ctx: &NotifierContext) -> Vec<ValidationError> {
        use jumbie_shared::validation::{validate_episode_number, validate_season_number};
        let mut errors = Vec::new();

        if let Some(s) = &ctx.season
            && let Err(e) = validate_season_number(&s.to_string())
        {
            errors.push(e);
        }
        if let Some(ref ep) = ctx.episode
            && let Err(e) = validate_episode_number(*ep)
        {
            errors.push(e);
        }
        if let Some(ref ep) = ctx.episode_end
            && let Err(e) = validate_episode_number(*ep)
        {
            errors.push(e);
        }
        if let Some(s) = ctx.size_bytes
            && s > 1_000_000_000_000
        {
            errors.push(ValidationError(format!(
                "Notifier size_bytes exceeds 1TB: {}",
                s
            )));
        }
        if let Some(ref title) = ctx.series_title
            && title.trim().is_empty()
        {
            errors.push(ValidationError(
                "Notifier series_title is empty".to_string(),
            ));
        }
        errors
    }
}

#[cfg(test)]
#[path = "tests/bridge.rs"]
mod tests;
