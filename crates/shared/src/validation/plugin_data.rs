//! Validation for data flowing FROM plugins TO the backend.
//!
//! Distinct from `fields.rs` (scalar user input at the API boundary): this validates
//! structured *plugin output* entering trusted backend state, in bulk and with
//! filtering (skip bad entries rather than rejecting the batch). All plugin-output
//! validation flows through the [`Validate`] trait.

use crate::types::media::MediaEntry;
use crate::validation::fields::{MAX_SOURCE_LENGTH, MAX_TITLE_LENGTH, ValidationError};

/// Maximum length of a single plugin-reported search-query string.
pub const MAX_QUERY_LENGTH: usize = 2_000;

/// Maximum number of items a single plugin call may return. Defense-in-depth
/// against a runaway plugin: the per-item validators bound each element's size,
/// this bounds the count. Generous enough that no legitimate feed, alias list,
/// or metadata refresh is affected.
pub const MAX_PLUGIN_ITEMS: usize = 10_000;

/// Maximum number of query strings a plugin may report for one search.
pub const MAX_SEARCH_QUERIES: usize = 100;

/// The `Validate` trait — implemented by every type that crosses the plugin→backend boundary.
///
/// Returns `Ok(())` when the data is clean, or `Err(Vec<ValidationError>)` with one error
/// per problem found. Callers decide how to handle errors (log-and-skip vs. reject).
pub trait Validate {
    fn validate(&self) -> Result<(), Vec<ValidationError>>;
}

/// Convert a collected vector of errors into the standard `Validate` result.
///
/// Replaces the repetitive `if errors.is_empty() { Ok(()) } else { Err(errors) }`
/// pattern found in every `Validate` implementation.
pub fn validate_result(errors: Vec<ValidationError>) -> Result<(), Vec<ValidationError>> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// MediaEntry validation

impl Validate for MediaEntry {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        if self.title.trim().is_empty() {
            errors.push(ValidationError("MediaEntry title is empty".to_string()));
        }
        if self.title.len() > MAX_TITLE_LENGTH {
            errors.push(ValidationError(format!(
                "MediaEntry title too long ({} chars, max {})",
                self.title.len(),
                MAX_TITLE_LENGTH
            )));
        }
        if self.source.trim().is_empty() {
            errors.push(ValidationError("MediaEntry source is empty".to_string()));
        }
        if self.source.len() > MAX_SOURCE_LENGTH {
            errors.push(ValidationError(format!(
                "MediaEntry source too long ({} chars, max {})",
                self.source.len(),
                MAX_SOURCE_LENGTH
            )));
        }
        if let Some(s) = self.size
            && s > 1_000_000_000_000
        {
            errors.push(ValidationError(format!(
                "MediaEntry size ({}) exceeds 1TB",
                s
            )));
        }
        if let Some(s) = self.seeders
            && s > 1_000_000
        {
            errors.push(ValidationError(format!(
                "MediaEntry seeders ({}) exceeds 1M",
                s
            )));
        }

        // Validate link/magnet format if present
        if let Some(ref link) = self.link
            && !link.starts_with("http://")
            && !link.starts_with("https://")
            && !link.starts_with("magnet:?")
        {
            errors.push(ValidationError(format!(
                "MediaEntry link has unexpected scheme: {}",
                link
            )));
        }
        validate_result(errors)
    }
}

impl Validate for Vec<MediaEntry> {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut all_errors = Vec::new();
        for (i, entry) in self.iter().enumerate() {
            if let Err(errors) = entry.validate() {
                for err in errors {
                    all_errors.push(ValidationError(format!("Entry [{}]: {}", i, err.0)));
                }
            }
        }
        validate_result(all_errors)
    }
}

/// Filter a plugin-reported query list, dropping empty and over-long entries
/// and capping the count at [`MAX_SEARCH_QUERIES`].
pub fn sanitize_search_queries(queries: Vec<String>) -> Vec<String> {
    queries
        .into_iter()
        .filter(|q| !q.trim().is_empty() && q.len() <= MAX_QUERY_LENGTH)
        .take(MAX_SEARCH_QUERIES)
        .collect()
}

/// Filter out invalid entries from a list, returning the clean entries
/// alongside per-item validation errors.
///
/// Callers can choose how to handle rejected entries (log, discard, surface).
pub fn validate_and_filter<T: Validate>(
    items: Vec<T>,
) -> (Vec<T>, Vec<(usize, Vec<ValidationError>)>) {
    let mut clean = Vec::with_capacity(items.len());
    let mut rejected = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        match item.validate() {
            Ok(()) => clean.push(item),
            Err(errors) => rejected.push((i, errors)),
        }
    }
    (clean, rejected)
}

/// Helper: Strict validation that rejects the entire batch.
pub fn validate_all<T: Validate>(items: &[T]) -> Result<(), Vec<ValidationError>> {
    let mut all_errors = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if let Err(errors) = item.validate() {
            for err in errors {
                all_errors.push(ValidationError(format!("[{}]: {}", i, err.0)));
            }
        }
    }
    validate_result(all_errors)
}
