//! API error envelope — the response body shape for every non-2xx response.

use serde::{Deserialize, Serialize};

/// Error envelope returned by the backend for every non-2xx response
/// (`{"error": ...}`).
///
/// SSoT for the error-body contract: the backend (`AppError::into_response`
/// and the scope middleware) constructs it and the frontend (`ApiError`)
/// deserializes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ApiErrorEnvelope {
    /// User-facing error message.
    pub error: String,
    /// Present when the failure is a path collision: the id of the series
    /// claiming the path, so the UI can link the user to the existing series
    /// (timeout-then-re-add flow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    /// Scopes the endpoint requires. Only set on an authorization failure
    /// (403), so an API client can tell what its key is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_scopes: Option<Vec<String>>,
    /// The subset of `required_scopes` the caller's key does not grant. Set
    /// alongside `required_scopes` on the same 403. The granted scopes are
    /// deliberately not echoed (least privilege).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_scopes: Option<Vec<String>>,
}
