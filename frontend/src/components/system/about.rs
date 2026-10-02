use crate::components::common::SettingsPage;
use crate::components::common::structs::AboutInfo;
use crate::hooks::use_ui_config::use_time_format;
use crate::utils::format_datetime_local;
use leptos::prelude::*;

#[component]
pub fn SystemAbout() -> impl IntoView {
    let (about, set_about) = signal(Option::<AboutInfo>::None);

    crate::utils::use_api_cache(
        "fetch_about".to_string(),
        || crate::api::fetch_about(),
        move |a| {
            let _ = set_about.try_update(|s| *s = Some(a));
        },
    );

    let time_format = use_time_format();

    view! {
        <SettingsPage id="about">
              {move || if let Some(a) = about.get() {
                let tf = time_format.get();
                view! {
                    <div class="about-info">
                        <p><strong>"Version:"</strong> " " {a.version}</p>
                        <p><strong>"Operating System:"</strong> " " {a.os}</p>
                        <p><strong>"Git Commit:"</strong> " " {a.git_commit.get(..10).unwrap_or(&a.git_commit)}</p>
                        <p><strong>"Build Date:"</strong> " " {format_datetime_local(&a.build_date, &tf).split(' ').next().unwrap_or("").to_string()}</p>
                        <p><strong>"License:"</strong> " " {a.license}</p>
                        <div class="mt-xl">
                            <p>{a.description}</p>
                        </div>
                    </div>
                }.into_any()
            } else {
                 view! { <div class="p-xl text-center"><crate::components::common::loading_spinner::LoadingSpinner /></div> }.into_any()
            }}
        </SettingsPage>
    }
}
