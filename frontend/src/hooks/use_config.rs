use jumbie_shared::config::Config;
use leptos::prelude::*;

#[derive(Clone, Copy)]
pub struct ConfigContext {
    pub config: ReadSignal<Option<Config>>,
    pub set_config: WriteSignal<Option<Config>>,
}

pub fn use_config() -> ConfigContext {
    use_context::<ConfigContext>().expect("ConfigContext should be provided in App")
}
