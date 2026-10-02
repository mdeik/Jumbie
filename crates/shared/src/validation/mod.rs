//! Validation functions for user-provided input (titles, numbers, URLs, templates).
//!
//! This crate is pure-data with no I/O, so filesystem-dependent validation
//! (e.g. `validate_path`) lives in the crate that owns filesystem access
//! (`jumbie_core`). `validate_template` uses manual brace counting rather than
//! regex because `${name}` is a balanced-brace grammar regex cannot verify
//! (unmatched or partially formed braces).

pub mod config;
pub mod fields;
pub mod plugin_data;
pub mod template;

// Re-export all public API
pub use config::*;
pub use fields::*;
pub use plugin_data::*;
pub use template::*;

#[cfg(test)]
mod tests;
