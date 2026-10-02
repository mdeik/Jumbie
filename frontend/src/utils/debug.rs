//! Frontend console logging — the only sanctioned API.
//!
//! Always log through [`debug_log!`], [`debug_warn!`], or [`debug_error!`]. Do
//! not call `leptos::logging` or `web_sys::console` directly: those are not
//! release-gated and would leak console output into production builds.
//!
//! Behavior by target:
//! - wasm32, debug builds: writes to the browser console.
//! - non-wasm32, debug builds (native `cargo test`): writes to stdout/stderr.
//! - release builds: no-op. Arguments still lower to a lazy `fmt::Arguments`,
//!   so bindings like `Err(e) => ...` stay used and call sites need no `#[cfg]`.

use std::fmt::Arguments;
#[cfg(all(debug_assertions, target_arch = "wasm32"))]
use web_sys::console;

macro_rules! define_logger {
    ($name:ident, $console_fn:ident, $native_macro:ident) => {
        /// Write the message. No-op in release builds.
        #[cfg(all(debug_assertions, target_arch = "wasm32"))]
        #[inline]
        pub fn $name(message: Arguments<'_>) {
            console::$console_fn(&message.to_string().into());
        }

        /// Write the message. No-op in release builds.
        #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
        #[inline]
        pub fn $name(message: Arguments<'_>) {
            $native_macro!("{message}");
        }

        /// No-op in release builds.
        #[cfg(not(debug_assertions))]
        #[inline]
        pub fn $name(_message: Arguments<'_>) {}
    };
}

define_logger!(log_fmt, log_1, println);
define_logger!(warn_fmt, warn_1, eprintln);
define_logger!(error_fmt, error_1, eprintln);

/// Log to the console (debug builds only). Accepts `format!` syntax.
#[macro_export]
macro_rules! debug_log {
    (? $val:expr $(,)?) => {
        $crate::utils::debug::log_fmt(format_args!("{:?}", $val))
    };
    ($($arg:tt)+) => {
        $crate::utils::debug::log_fmt(format_args!($($arg)+))
    };
}

/// Warn to the console (debug builds only). Accepts `format!` syntax.
#[macro_export]
macro_rules! debug_warn {
    ($($arg:tt)+) => {
        $crate::utils::debug::warn_fmt(format_args!($($arg)+))
    };
}

/// Log an error to the console (debug builds only). Accepts `format!` syntax.
#[macro_export]
macro_rules! debug_error {
    ($($arg:tt)+) => {
        $crate::utils::debug::error_fmt(format_args!($($arg)+))
    };
}

pub use {debug_error, debug_log, debug_warn};
