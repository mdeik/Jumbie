use crate::api::{fetch_config, save_config};
use crate::components::common::SettingsBuilder;
use crate::components::common::form_fields::{FormGroup, TextInput};
use crate::components::common::modal_wrapper::{ModalWrapper, modal_footer_save};
use crate::components::common::toast::*;
use crate::hooks::use_auth::AuthContext;
use crate::hooks::use_config::{ConfigContext, use_config};

use base64::Engine;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn AccountSettings() -> impl IntoView {
    let ConfigContext { config, set_config } = use_config();
    let auth = use_context::<AuthContext>().expect("AuthContext missing");
    let (password, set_password) = signal(String::new());
    let (is_saving, set_is_saving) = signal(false);
    let (initialized, set_initialized) = signal(false);
    let password_is_set = Signal::derive(move || {
        config.with(|c| {
            c.as_ref()
                .map(|c| c.auth.password.is_some())
                .unwrap_or(false)
        })
    });

    // Initialize local state when config is first loaded
    Effect::new(move |_| {
        if !initialized.get_untracked() && config.get().is_some() {
            set_password.set(String::new());
            set_initialized.set(true);
        }
    });

    // Ensure config is fetched on reload if deep-linked to this page
    crate::utils::create_mount_resource(move || async move {
        if config.get_untracked().is_none() {
            match fetch_config().await {
                Ok(c) => set_config.set(Some(c)),
                Err(e) => crate::debug_error!("Failed to load config: {}", e),
            }
        }
    });

    let (show_verify_modal, set_show_verify_modal) = signal(false);
    let (verify_password, set_verify_password) = signal(String::new());
    let (shake_modal, set_shake_modal) = signal(false);

    let perform_save = move || {
        set_is_saving.set(true);
        let p = password.get();

        let mut new_config = match config.get_untracked() {
            Some(c) => c,
            None => {
                set_is_saving.set(false);
                return;
            }
        };
        new_config.auth.password = if p.is_empty() { None } else { Some(p.clone()) };

        let new_credentials = format!(":{}", p);
        let new_b64 = base64::engine::general_purpose::STANDARD.encode(new_credentials.as_bytes());
        let new_token = format!("Basic {}", new_b64);

        // Optimistic in-memory update BEFORE the async network call. When
        // clearing the password, updating the config signal first keeps
        // show_login false, lets other components' autosave Effects see the
        // final state (avoiding a conflicting PUT), and stays consistent if the
        // component unmounts.
        if p.is_empty() {
            set_config.set(Some(new_config.clone()));
        }

        let new_config_for_save = new_config.clone();
        spawn_local(async move {
            let save_result = save_config(new_config_for_save).await;

            if save_result
                .toast_on_err("Failed to save settings")
                .is_some()
            {
                batch(move || {
                    if p.is_empty() {
                        // In-memory config already updated optimistically above.
                        // Now tear down auth state: remove token and signal
                        // unauthenticated so the show_login gate opens.
                        if let Some(window) = web_sys::window()
                            && let Ok(Some(storage)) = window.local_storage()
                        {
                            let _ = storage.remove_item("jb_auth_token");
                        }
                        auth.set_is_authenticated.set(false);
                    } else {
                        // Setting password: persist the new token first, then
                        // update config so show_login stays false.
                        if let Some(window) = web_sys::window()
                            && let Ok(Some(storage)) = window.local_storage()
                        {
                            let _ = storage.set_item("jb_auth_token", &new_token);
                        }
                        auth.set_is_authenticated.set(true);
                        set_config.set(Some(new_config));
                    }
                });
                crate::components::common::toast::show_success("Account settings saved");
            } else {
                // Roll back the optimistic update if the save failed
                if p.is_empty()
                    && let Some(prev) = config.get_untracked()
                {
                    set_config.set(Some(prev));
                }
            }
            set_is_saving.set(false);
        });
    };

    let verify_submit = Callback::new(move |_: ()| {
        if verify_password.get() == password.get() {
            set_show_verify_modal.set(false);
            set_verify_password.set(String::new());
            perform_save();
        } else {
            set_shake_modal.set(true);
            crate::components::common::toast::show_error("Passwords do not match");
            set_timeout(
                move || set_shake_modal.set(false),
                std::time::Duration::from_millis(500),
            );
        }
    });

    SettingsBuilder::new("account")
        .raw_section(view! {
            <Show when=move || show_verify_modal.get() fallback=|| ()>
                <ModalWrapper
                    title=Signal::stored("Verify Password".to_string())
                    show=Signal::from(show_verify_modal)
                    on_close=Callback::new(move |_| {
                        set_show_verify_modal.set(false);
                        set_verify_password.set(String::new());
                    })
                    footer=modal_footer_save(verify_submit, Signal::derive(|| false)).into_any()
                >
                    <div class=move || if shake_modal.get() { "mb-md shake" } else { "mb-md" }>
                        <p class="text-sm text-muted">"Please enter your password again to confirm these changes."</p>
                        <FormGroup label="Verify Password".to_string() label_for="verifyPassword">
                            <TextInput
                                id="verifyPassword".to_string()
                                autofocus=true
                                value=verify_password
                                set_value=move |v| set_verify_password.set(v)
                                type_="password"
                                autocomplete="new-password"
                                show_toggle=true
                                placeholder="Enter password again"
                                password_placeholder_behaviour=true
                            />
                        </FormGroup>
                    </div>
                </ModalWrapper>
            </Show>
        })
        .section("Credentials", |section| {
            section.field(view! {
                <div class="mb-md p-md rounded auth-info-box">
                    <div class="flex items-center gap-sm mb-xs">
                        {move || if password_is_set.get() {
                            view! {
                                <span class="auth-status-dot auth-status-active"></span>
                                <span class="text-sm font-semibold text-primary">"A password is currently set"</span>
                            }.into_any()
                        } else {
                            view! {
                                <span class="auth-status-dot auth-status-inactive"></span>
                                <span class="text-sm font-semibold text-warning">"No password is currently set"</span>
                            }.into_any()
                        }}
                    </div>
                    <p class="text-xs text-muted m-0">
                        "Authentication is automatically enabled when a password is provided. Leave the password blank and save to disable authentication entirely."
                    </p>
                </div>
            })
            .field(view! {
                <FormGroup label="New Password".to_string() label_for="adminPassword" class="mb-0">
                    <TextInput
                        id="adminPassword".to_string()
                        value=password
                        set_value=move |v| set_password.set(v)
                        type_="password"
                        autocomplete="new-password"
                        show_toggle=true
                        password_placeholder_behaviour=true
                        placeholder="Password"
                    />
                </FormGroup>
            })
            .field(view! {
                <div class="flex justify-end mt-md pt-md gap-md">
                    <Show when=move || password_is_set.get() fallback=|| ()>
                        <button
                            class="btn auth-btn-danger"
                            on:click=move |_| {
                                crate::utils::clear_auth_and_logout();
                            }
                        >
                            "Logout"
                        </button>
                    </Show>
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_verify_modal.set(true)
                        disabled=move || is_saving.get()
                    >
                        {move || if is_saving.get() { "Saving..." } else { "Save Credentials" }}
                    </button>
                </div>
            })
        })
        .build()
}
