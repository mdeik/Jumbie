use crate::hooks::use_config::{ConfigContext, use_config};
use jumbie_shared::config::Config;
use leptos::prelude::*;

#[derive(Clone)]
pub struct ConfigBinding<T: Clone + Send + Sync + 'static> {
    pub value: Signal<T>,
    pub set_value: Callback<T>,
}

/// Core binding logic for `use_config_binding`: a local `RwSignal` (not
/// `Signal::derive`) for synchronous reads — no microtask delay when the user
/// types — kept in sync with the relevant global `config` field by an `Effect`.
/// Updates compare old/new values so unchanged fields don't save.
fn use_config_binding_inner<T, G, S>(
    getter: G,
    setter: S,
    trigger_save: impl Fn(bool) + Clone + Send + Sync + 'static,
    default_value: impl Fn() -> T + Clone + Send + Sync + 'static,
) -> ConfigBinding<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
    G: Fn(&Config) -> T + Clone + Send + Sync + 'static,
    S: Fn(&mut Config, T) + Clone + Send + Sync + 'static,
{
    let ConfigContext { config, set_config } = use_config();

    // Local writable signal — updates are synchronous, no microtask delay;
    // an Effect keeps it in sync with the global config signal.
    let value: RwSignal<T> = RwSignal::new(default_value());

    let getter_effect = getter.clone();
    Effect::new(move |_| {
        if let Some(cfg) = config.get() {
            value.set(getter_effect(&cfg));
        }
    });

    let set_value = Callback::new(move |new_val: T| {
        let changed = config.with(|c| {
            c.as_ref()
                .map(|curr| getter(&curr) != new_val)
                .unwrap_or(false)
        });

        if changed {
            value.set(new_val.clone());

            // Persist to the global config signal
            set_config.update(|c| {
                if let Some(c) = c {
                    setter(c, new_val);
                }
            });

            // Keep the API cache in sync
            config.with(|c| {
                if let Some(c) = c {
                    crate::utils::write_cache("fetch_config", c);
                }
            });

            trigger_save(false);
        }
    });

    ConfigBinding {
        value: value.into(),
        set_value,
    }
}

/// Creates a binding for a specific field in the global configuration.
///
/// - `getter`: How to read the value from the Config struct.
/// - `setter`: How to update the value in the Config struct.
/// - `trigger_save`: A callback (usually from `use_autosave`) to persist changes.
pub fn use_config_binding<T, G, S>(
    getter: G,
    setter: S,
    trigger_save: impl Fn(bool) + Clone + Send + Sync + 'static,
) -> ConfigBinding<T>
where
    T: Clone + PartialEq + Default + Send + Sync + 'static,
    G: Fn(&Config) -> T + Clone + Send + Sync + 'static,
    S: Fn(&mut Config, T) + Clone + Send + Sync + 'static,
{
    use_config_binding_inner(getter, setter, trigger_save, T::default)
}

/// Numeric config binding that owns the "blank means default" rule: blank input
/// stores `default`.
///
/// Use this for any settings field whose config value is a number and whose
/// blank state should resolve to a documented default (pair it with a placeholder
/// describing that default).
///
/// Blank is normalized to the default *before* the inner change detection, so
/// clearing a field that already holds the default is a no-op rather than a
/// redundant save.
pub fn use_number_binding<T, G, S>(
    getter: G,
    setter: S,
    default: T,
    trigger_save: impl Fn(bool) + Clone + Send + Sync + 'static,
) -> ConfigBinding<String>
where
    T: Copy + ToString + std::str::FromStr + Send + Sync + 'static,
    G: Fn(&Config) -> T + Clone + Send + Sync + 'static,
    S: Fn(&mut Config, T) + Clone + Send + Sync + 'static,
{
    let get = getter.clone();
    let set = setter.clone();
    let binding = use_config_binding(
        move |c: &Config| get(c).to_string(),
        move |c: &mut Config, v: String| {
            set(c, v.trim().parse::<T>().unwrap_or(default));
        },
        trigger_save,
    );

    let set_value = binding.set_value;
    ConfigBinding {
        value: binding.value,
        set_value: Callback::new(move |v: String| {
            set_value.run(if v.trim().is_empty() {
                default.to_string()
            } else {
                v
            });
        }),
    }
}
