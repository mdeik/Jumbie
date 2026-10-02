use crate::components::common::form_fields::FormGroup;
use crate::components::common::form_fields::helpers::resolve_field_id;
use leptos::prelude::*;

/// Strip any character that isn't a digit, `+`, `-`, or in `extra_allowed`.
/// Text-mode numeric inputs need this because `type="number"` rejects partial
/// input like `-` before the digits. `extra_allowed` widens the set for decimal
/// separators or season ranges (e.g. `"1-5, 8, 10-12"`).
fn strip_non_numeric(raw: &str, extra_allowed: &str, allow_signs: bool) -> String {
    raw.chars()
        .filter(|c| {
            c.is_ascii_digit()
                || (allow_signs && (*c == '-' || *c == '+'))
                || extra_allowed.contains(*c)
        })
        .collect()
}

/// Characters permitted in an IPv4/IPv6 address entry: the `.` and `:`
/// separators plus hex letters for IPv6 (decimal digits are always kept).
const IP_ADDRESS_ALLOWED_CHARS: &str = ".:abcdefABCDEF";

/// Controls filtering behavior and the `inputmode` attribute.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum NumberInputMode {
    /// No filtering, no `inputmode` attribute. Default.
    #[default]
    Text,
    /// `inputmode="numeric"`, strips non-numeric — only digits `0-9` pass.
    Integer,
    /// Strips non-numeric — allows digits `0-9`, `+`, `-`. No `inputmode`
    /// since mobile numeric keypads lack a sign key.
    SignedInteger,
    /// `inputmode="decimal"`, strips non-numeric — only digits `0-9` and `.` pass.
    Decimal,
    /// `inputmode="decimal"`, strips non-numeric but allows `.`, `+`, `-`.
    SignedDecimal,
    /// Applies `strip_non_numeric` with `allow_chars` — the caller specifies
    /// every allowed character explicitly (digits are always kept). No
    /// `inputmode`, no integer/decimal validation — the caller handles parsing.
    /// Useful for season-range input like `"1-5, 8, 10-12"`.
    Custom,
    /// `inputmode` unset; strips characters that can't appear in an IPv4 or
    /// IPv6 address — digits `0-9`, the `.` and `:` separators, and hex
    /// letters `a-f`/`A-F` pass. No numeric parsing, so the caller validates
    /// the address.
    IpAddress,
}

/// Clamp a numeric string to [`min`, `max`] bounds. Returns the clamped
/// value as a string, or the original string if parsing fails (partial
/// input like `"-"`).
fn clamp_range(val: &str, min: f64, max: f64, integer: bool) -> String {
    if val.is_empty() {
        return val.to_string();
    }
    if let Ok(n) = val.parse::<f64>() {
        let clamped = n.clamp(min, max);
        if integer {
            (clamped as i64).to_string()
        } else {
            clamped.to_string()
        }
    } else {
        val.to_string()
    }
}

#[component]
pub fn NumberInput(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] set_value: Callback<String>,
    #[prop(optional, into)] min: Signal<String>,
    #[prop(optional, into)] max: Signal<String>,
    #[prop(optional, into)] placeholder: Signal<String>,
    #[prop(optional, into)] id: String,
    #[prop(optional, into)] class: Signal<String>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] label: Signal<String>,
    #[prop(optional, into)] help_text: Signal<String>,
    #[prop(optional, into)] error: MaybeProp<String>,
    #[prop(optional)] autofocus: bool,
    /// Controls `inputmode` and character filtering. Defaults to `Text`.
    #[prop(optional)]
    mode: NumberInputMode,
    /// Additional characters to preserve through the filter beyond
    /// digits. Only applies in `Integer`, `Decimal`, and `Custom` modes.
    #[prop(optional, into)]
    allow_chars: String,
) -> impl IntoView {
    let id_val = resolve_field_id(&id);
    let id_for_label = id_val.clone();

    let inputmode = move || match mode {
        NumberInputMode::Integer => Some("numeric"),
        NumberInputMode::Decimal => Some("decimal"),
        NumberInputMode::Text
        | NumberInputMode::SignedInteger
        | NumberInputMode::SignedDecimal
        | NumberInputMode::Custom
        | NumberInputMode::IpAddress => None,
    };

    let min_sig = Signal::derive(move || min.get().parse::<f64>().ok());
    let max_sig = Signal::derive(move || max.get().parse::<f64>().ok());

    let display = RwSignal::new(value.get_untracked());
    Effect::new(move |_| {
        display.set(value.get());
    });

    view! {
        <FormGroup label=label label_for=id_for_label help_text=help_text error=error>
            <input
                type="text"
                inputmode=inputmode
                class=move || format!("form-input {}", class.get())
                id=id_val
                placeholder=move || { let p = placeholder.get(); if p.is_empty() { None } else { Some(p) } }
                min=move || { let m = min.get(); if m.is_empty() { None } else { Some(m) } }
                max=move || { let m = max.get(); if m.is_empty() { None } else { Some(m) } }
                prop:value=display
                prop:disabled=disabled
                prop:autofocus=autofocus
                on:input=move |ev| {
                    let raw = event_target_value(&ev);
                    let filtered = match mode {
                        NumberInputMode::Integer => strip_non_numeric(&raw, &allow_chars, false),
                        NumberInputMode::SignedInteger => strip_non_numeric(&raw, &allow_chars, true),
                        NumberInputMode::Decimal => {
                            strip_non_numeric(&raw, &format!(".{}", allow_chars), false)
                        }
                        NumberInputMode::SignedDecimal => {
                            strip_non_numeric(&raw, &format!(".{}", allow_chars), true)
                        }
                        NumberInputMode::Custom => {
                            strip_non_numeric(&raw, &allow_chars, false)
                        }
                        NumberInputMode::IpAddress => {
                            strip_non_numeric(&raw, IP_ADDRESS_ALLOWED_CHARS, false)
                        }
                        NumberInputMode::Text => raw.clone(),
                    };
                    let is_integer = matches!(
                        mode,
                        NumberInputMode::Integer | NumberInputMode::SignedInteger
                    );
                    let result = match (min_sig.get(), max_sig.get()) {
                        (Some(lo), Some(hi)) => clamp_range(&filtered, lo, hi, is_integer),
                        (Some(lo), None) => clamp_range(&filtered, lo, f64::INFINITY, is_integer),
                        (None, Some(hi)) => clamp_range(&filtered, f64::NEG_INFINITY, hi, is_integer),
                        (None, None) => filtered,
                    };
                    // Force input display when filtering removed characters
                    if result != raw {
                        event_target::<web_sys::HtmlInputElement>(&ev).set_value(&result);
                    }
                    display.set(result.clone());
                    // Only propagate values the caller can handle. Block
                    // partial input that would be swallowed by downstream
                    // parsing (e.g. bare "-").
                    let is_complete = result.is_empty()
                        || match mode {
                            NumberInputMode::Text
                            | NumberInputMode::Custom
                            | NumberInputMode::IpAddress => true,
                            NumberInputMode::Integer | NumberInputMode::SignedInteger => {
                                result.parse::<i64>().is_ok()
                            }
                            NumberInputMode::Decimal | NumberInputMode::SignedDecimal => {
                                result.parse::<f64>().is_ok()
                            }
                        };
                    if is_complete {
                        set_value.run(result);
                    }
                }
            />
        </FormGroup>
    }
}
