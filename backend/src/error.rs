use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use jumbie_shared::auth::ApiScope;
use jumbie_shared::types::ApiErrorEnvelope;

#[derive(Debug)]
pub enum AppError {
    // Catch-all for failures the server cannot recover from or attribute to the
    // client. Uses anyhow::Error to avoid coupling to a specific error library.
    Internal(anyhow::Error),
    // The 4xx variants carry a status-specific message so handlers need not
    // construct their own StatusCode. BadRequestWithSeries additionally carries
    // the id of the series claiming the path, so the frontend can link the user
    // to the existing series (timeout-then-re-add flow).
    BadRequest(String),
    BadRequestWithSeries {
        message: String,
        series_id: String,
    },
    NotFound(String),
    Conflict(String),
    ServiceUnavailable(String),
    RequestTimeout(String),
    TooManyRequests(String),
    /// Authenticated caller whose key lacks one or more required scopes. Renders
    /// as a 403 with the RFC 6750 challenge; build it with
    /// [`AppError::insufficient_scope`].
    InsufficientScope {
        required: Vec<String>,
        missing: Vec<String>,
    },
}

impl AppError {
    /// User-facing text for logging and activity events (e.g. add-series
    /// metadata sync failures that are recorded server-side without failing
    /// the request).
    ///
    /// `IntoResponse` deliberately does NOT use this for `Internal` errors — the
    /// HTTP body hides internal details for security. This method exists for
    /// server-side observability (logs, activity log).
    pub fn message(&self) -> String {
        match self {
            AppError::Internal(err) => format!("{:#}", err),
            AppError::BadRequest(msg) => msg.clone(),
            AppError::BadRequestWithSeries { message, .. } => message.clone(),
            AppError::NotFound(msg) => msg.clone(),
            AppError::Conflict(msg) => msg.clone(),
            AppError::ServiceUnavailable(msg) => msg.clone(),
            AppError::RequestTimeout(msg) => msg.clone(),
            AppError::TooManyRequests(msg) => msg.clone(),
            AppError::InsufficientScope { missing, .. } => {
                format!("Insufficient scope: missing {}", missing.join(", "))
            }
        }
    }

    /// Build an [`AppError::InsufficientScope`] from the required and granted
    /// scopes, computing the missing subset via [`ApiScope::permits`] (so a write
    /// scope satisfies a read requirement).
    pub fn insufficient_scope(required: &[ApiScope], granted: &[ApiScope]) -> Self {
        let missing = required
            .iter()
            .filter(|need| !granted.iter().any(|have| have.permits(**need)))
            .map(|scope| scope.as_str().to_string())
            .collect();
        AppError::InsufficientScope {
            required: required.iter().map(|s| s.as_str().to_string()).collect(),
            missing,
        }
    }
}

/// Canonical 403 for an authenticated caller missing required scopes: the
/// `WWW-Authenticate` challenge (RFC 6750 §3.1) plus the JSON envelope. Shared by
/// `AppError::InsufficientScope` and the router scope middleware so the wire
/// format is defined in one place.
pub fn insufficient_scope_response(required: &[String], missing: &[String]) -> Response {
    let body = ApiErrorEnvelope {
        error: format!("Insufficient scope: missing {}", missing.join(", ")),
        required_scopes: Some(required.to_vec()),
        missing_scopes: Some(missing.to_vec()),
        ..Default::default()
    };
    (
        StatusCode::FORBIDDEN,
        [(
            header::WWW_AUTHENTICATE,
            format!(
                "Bearer error=\"insufficient_scope\", scope=\"{}\"",
                required.join(" ")
            ),
        )],
        Json(body),
    )
        .into_response()
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Missing-scope failures carry a `WWW-Authenticate` challenge, so they
        // build their response separately from the plain `(status, Json)` arms.
        if let AppError::InsufficientScope { required, missing } = &self {
            tracing::warn!("Insufficient scope: missing {}", missing.join(", "));
            return insufficient_scope_response(required, missing);
        }

        // Internal errors use tracing::error (unexpected server faults needing
        // operator attention); client-caused 4xx failures use tracing::warn
        // (observable for monitoring, but not page-worthy).
        let (status, body) = match self {
            AppError::Internal(err) => {
                tracing::error!("Internal error: {:#}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ApiErrorEnvelope {
                        error: "Internal Server Error".to_string(),
                        series_id: None,
                        ..Default::default()
                    },
                )
            }
            AppError::BadRequest(msg) => {
                tracing::warn!("Bad request: {}", msg);
                (
                    StatusCode::BAD_REQUEST,
                    ApiErrorEnvelope {
                        error: msg,
                        series_id: None,
                        ..Default::default()
                    },
                )
            }
            AppError::BadRequestWithSeries { message, series_id } => {
                tracing::warn!(
                    "Bad request: {} (path claimed by series {})",
                    message,
                    series_id
                );
                (
                    StatusCode::BAD_REQUEST,
                    ApiErrorEnvelope {
                        error: message,
                        series_id: Some(series_id),
                        ..Default::default()
                    },
                )
            }
            AppError::NotFound(msg) => {
                tracing::warn!("Not found: {}", msg);
                (
                    StatusCode::NOT_FOUND,
                    ApiErrorEnvelope {
                        error: msg,
                        series_id: None,
                        ..Default::default()
                    },
                )
            }
            AppError::Conflict(msg) => {
                tracing::warn!("Conflict: {}", msg);
                (
                    StatusCode::CONFLICT,
                    ApiErrorEnvelope {
                        error: msg,
                        series_id: None,
                        ..Default::default()
                    },
                )
            }
            AppError::ServiceUnavailable(msg) => {
                tracing::warn!("Service unavailable: {}", msg);
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    ApiErrorEnvelope {
                        error: msg,
                        series_id: None,
                        ..Default::default()
                    },
                )
            }
            AppError::RequestTimeout(msg) => {
                tracing::warn!("Request timeout: {}", msg);
                (
                    StatusCode::REQUEST_TIMEOUT,
                    ApiErrorEnvelope {
                        error: msg,
                        series_id: None,
                        ..Default::default()
                    },
                )
            }
            AppError::TooManyRequests(msg) => {
                tracing::warn!("Too many requests: {}", msg);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    ApiErrorEnvelope {
                        error: msg,
                        series_id: None,
                        ..Default::default()
                    },
                )
            }
            // Handled by the early return above (it needs a custom header).
            AppError::InsufficientScope { .. } => {
                unreachable!("InsufficientScope is returned before the match")
            }
        };

        (status, Json(body)).into_response()
    }
}

// SAFETY WARNING: this blanket impl silently promotes every error to a 500.
// Convert domain errors that should be 4xx explicitly, via AppResultExt or
// `.map_err(AppError::...)`, before relying on `?`.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        AppError::Internal(err.into())
    }
}

/// Extension trait for Option and Result to easily map to AppError variants.
///
/// For `Result`, the original error is logged as a warning and then discarded —
/// internal details (SQL, paths, stack traces) must never leak to the client.
pub trait AppResultExt<T> {
    fn or_not_found(self, msg: impl Into<String>) -> Result<T, AppError>;
    fn or_bad_request(self, msg: impl Into<String>) -> Result<T, AppError>;
}

impl<T> AppResultExt<T> for Option<T> {
    fn or_not_found(self, msg: impl Into<String>) -> Result<T, AppError> {
        self.ok_or_else(|| AppError::NotFound(msg.into()))
    }
    fn or_bad_request(self, msg: impl Into<String>) -> Result<T, AppError> {
        self.ok_or_else(|| AppError::BadRequest(msg.into()))
    }
}

impl<T, E> AppResultExt<T> for Result<T, E>
where
    E: Into<anyhow::Error>,
{
    fn or_not_found(self, msg: impl Into<String>) -> Result<T, AppError> {
        self.map_err(|e| {
            tracing::debug!("Mapped error to NotFound: {:#}", e.into());
            AppError::NotFound(msg.into())
        })
    }
    fn or_bad_request(self, msg: impl Into<String>) -> Result<T, AppError> {
        self.map_err(|e| {
            tracing::debug!("Mapped error to BadRequest: {:#}", e.into());
            AppError::BadRequest(msg.into())
        })
    }
}

/// Collapses the common `Result<Json<T>, AppError>` pattern into a one-liner
/// at call sites. `into_json_response()` wraps the value in `Json<T>`;
/// `into_status_response()` discards it and returns 200 OK.
pub trait IntoApiResponse<T> {
    fn into_json_response(self) -> Result<Json<T>, AppError>;
    fn into_status_response(self) -> Result<StatusCode, AppError>;
}

impl<T, E> IntoApiResponse<T> for Result<T, E>
where
    E: Into<AppError>,
{
    fn into_json_response(self) -> Result<Json<T>, AppError> {
        self.map(Json).map_err(|e| e.into())
    }

    fn into_status_response(self) -> Result<StatusCode, AppError> {
        self.map(|_| StatusCode::OK).map_err(|e| e.into())
    }
}
