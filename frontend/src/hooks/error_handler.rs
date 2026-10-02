use crate::api_client::ApiError;
use crate::components::common::toast::show_error;
use leptos::prelude::*;

#[derive(Clone, Copy)]
pub struct ErrorHandler {
    /// Report an error with a custom title and optional suggestion.
    pub report: Callback<(String, ApiError)>,
}

pub fn use_error_handler() -> ErrorHandler {
    let report = Callback::new(move |(title, err): (String, ApiError)| {
        // Api errors go through `user_message()` so the shared envelope is
        // stripped (raw body is the fallback for non-envelope bodies).
        let err_msg = match &err {
            ApiError::Api { .. } => err.user_message(),
            ApiError::Network(m) => format!("Network error: {}", m),
            ApiError::Deserialization(m) => format!("Data error: {}", m),
        };
        show_error(format!("{}: {}", title, err_msg));
        crate::debug_error!("{}: {}", title, err_msg);
    });

    ErrorHandler { report }
}

/// Extension trait for Results to easily report errors via the handler.
pub trait ResultReportExt<T> {
    fn report_err(self, handler: ErrorHandler, title: &str) -> Option<T>;
}

impl<T> ResultReportExt<T> for Result<T, ApiError> {
    fn report_err(self, handler: ErrorHandler, title: &str) -> Option<T> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                handler.report.run((title.to_string(), e));
                None
            }
        }
    }
}
