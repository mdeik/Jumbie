// Re-exports the time-input SSoT from `form_fields` and adds `TimeInputAdapter`
// for settings pages.

pub use crate::components::common::form_fields::time_input::{
    TimeInput, TimeParseResult, TimeUnit, format_duration, parse_time_input,
};

use crate::hooks::ConfigBinding;
use leptos::prelude::*;

/// Adapter that wraps a `ConfigBinding<String>` to provide time-input
/// display formatting, parsing, and inline error state.
///
/// Blank input is forwarded to the binding, which owns the default fallback;
/// `default` is used here only to render [`Self::placeholder`].
///
/// Usage:
/// ```ignore
/// let adapter = TimeInputAdapter::new(bind, TimeUnit::Minutes, 5, 1440, 60);
/// view! {
///     <TimeInput
///         label="My Field"
///         value=adapter.display
///         set_value=adapter.set_value
///         error=adapter.error
///     />
/// }
/// ```
#[derive(Clone)]
pub struct TimeInputAdapter {
    pub display: Signal<String>,
    pub set_value: Callback<String>,
    pub error: Signal<Option<String>>,
    /// Formatted `default`, suitable for the input's placeholder.
    pub placeholder: Signal<String>,
}

impl TimeInputAdapter {
    pub fn new(
        bind: ConfigBinding<String>,
        unit: TimeUnit,
        min: u64,
        max: u64,
        default: u64,
    ) -> Self {
        let error = RwSignal::new(None::<String>);

        let bind_for_display = bind.clone();
        let display = Signal::derive(move || {
            let raw = bind_for_display.value.get();
            if raw.trim().is_empty() {
                String::new()
            } else {
                format_duration(raw.parse().unwrap_or(default), unit)
            }
        });

        let placeholder = Signal::derive(move || format_duration(default, unit));

        // Clear error when the config value changes externally.
        let error_clear = error;
        let bind_for_effect = bind.clone();
        Effect::new(move |_| {
            bind_for_effect.value.get();
            error_clear.set(None);
        });

        let error_setter = error;
        let bind_for_cb = bind;
        let set_value = Callback::new(move |input: String| {
            // Blank is handled by the binding (which applies its default); only
            // non-blank input is parsed here.
            if input.trim().is_empty() {
                error_setter.set(None);
                bind_for_cb.set_value.run(input);
                return;
            }
            let result = parse_time_input(&input, unit, min, max);
            if let Some(err_msg) = result.error {
                error_setter.set(Some(err_msg));
            } else {
                error_setter.set(None);
                bind_for_cb.set_value.run(result.value.to_string());
            }
        });

        Self {
            display,
            set_value,
            error: error.into(),
            placeholder,
        }
    }
}
