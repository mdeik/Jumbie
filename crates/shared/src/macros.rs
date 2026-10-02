//! Centralizes the common test-module imports (`use super::*;`) so adding a new
//! shared import propagates to every test module automatically.

/// Standardizes the creation of a test module with common imports.
#[macro_export]
macro_rules! test_module {
    ($($content:tt)*) => {
        #[cfg(test)]
        mod tests {
            use super::*;

            $($content)*
        }
    };
}
