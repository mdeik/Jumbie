// API endpoint module — thin wrappers over ApiClient for every backend route.
//
// Simple endpoints call a 1-line helper (get, post, post_unit, …); endpoints that
// need custom error messages, toast side effects, or non-standard serialization
// drop down to the RequestBuilder API directly. All calls go through the singleton
// ApiClient so the reqwest connection pool is reused.

use crate::api_client::{ApiError as Error, api_client};
use std::sync::Mutex;

pub(crate) mod calendar;
pub(crate) mod config;
pub(crate) mod episodes;
pub(crate) mod scanner;
pub(crate) mod search;
pub(crate) mod series;
pub(crate) mod system;

// Every public function is re-exported so callers using `crate::api::fetch_series()`
// continue to work unchanged.
pub use self::calendar::*;
pub use self::config::*;
pub use self::episodes::*;
pub use self::scanner::*;
pub use self::search::*;
pub use self::series::*;
pub use self::system::*;

// Partial-save state tracker for save_config.
//
// Purely internal to save_config (no component reads it). Holds the JSON-serialized
// sections of the last successfully saved config so unchanged sections can be
// omitted from the next request.
pub(crate) static LAST_SAVED_CONFIG: Mutex<Option<serde_json::Value>> = Mutex::new(None);

// Internal Helpers

pub(crate) async fn get<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, Error> {
    api_client().get(path).await
}

/// POST + typed response (returns deserialized JSON body).
pub(crate) async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
    path: &str,
    body: &B,
) -> Result<T, Error> {
    api_client().post(path, body).await
}

/// POST + unit response (the `_unit` suffix signals the caller gets back `()`).
pub(crate) async fn post_unit<B: serde::Serialize>(path: &str, body: &B) -> Result<(), Error> {
    api_client().post_unit(path, body).await
}

pub(crate) async fn put_unit<B: serde::Serialize>(path: &str, body: &B) -> Result<(), Error> {
    api_client().put(path, body).await
}

pub(crate) async fn put_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
    path: &str,
    body: &B,
) -> Result<T, Error> {
    api_client().put_json(path, body).await
}

pub(crate) async fn delete_unit(path: &str) -> Result<(), Error> {
    api_client().delete(path).await
}

pub(crate) async fn delete_json<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, Error> {
    api_client().delete_json(path).await
}

/// Compares each section of a partial config against the last saved state and
/// returns a new `UpdateConfigPayload` with only the changed sections populated.
///
/// The first save always sends all sections (cache is empty), which is correct
/// — the backend needs the full picture for the initial sync.
pub(crate) fn diff_sections(
    current: &jumbie_shared::types::UpdateConfigPayload,
) -> jumbie_shared::types::UpdateConfigPayload {
    let cache = LAST_SAVED_CONFIG.lock().ok();
    let last = cache.as_ref().and_then(|c| c.as_ref());

    /// Helper: true if the section differs from the cached value (or there is no cache).
    fn section_changed<B: serde::Serialize>(
        last: Option<&serde_json::Value>,
        name: &str,
        val: &Option<B>,
    ) -> bool {
        let Some(last) = last else { return true };
        let Some(val) = val.as_ref() else { return true };
        let Ok(json) = serde_json::to_value(val) else {
            return true;
        };
        last.get(name).map(|v| v != &json).unwrap_or(true)
    }

    jumbie_shared::types::UpdateConfigPayload {
        organization: if section_changed(last, "organization", &current.organization) {
            current.organization.clone()
        } else {
            None
        },
        sources: if section_changed(last, "sources", &current.sources) {
            current.sources.clone()
        } else {
            None
        },
        general: if section_changed(last, "general", &current.general) {
            current.general.clone()
        } else {
            None
        },
        proxy: if section_changed(last, "proxy", &current.proxy) {
            current.proxy.clone()
        } else {
            None
        },
        auth: if section_changed(last, "auth", &current.auth) {
            current.auth.clone()
        } else {
            None
        },
        security: if section_changed(last, "security", &current.security) {
            current.security.clone()
        } else {
            None
        },
    }
}
