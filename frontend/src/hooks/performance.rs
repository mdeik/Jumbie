use leptos::prelude::*;

pub fn use_track_mount(_name: &'static str) {
    let _start = js_sys::Date::now();
    on_cleanup(move || {
        let _end = js_sys::Date::now();
        crate::debug_log!("[Perf] {} mounted for {}ms", _name, _end - _start);
    });
}

pub async fn track_api_call<T, E, Fut>(_name: &'static str, fut: Fut) -> Result<T, E>
where
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let _start = js_sys::Date::now();
    let res = fut.await;
    let _end = js_sys::Date::now();
    crate::debug_log!("[Perf] API {} took {}ms", _name, _end - _start);
    res
}
