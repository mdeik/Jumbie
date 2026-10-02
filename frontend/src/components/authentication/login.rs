use crate::api::fetch_health;
use crate::components::common::form_fields::{FormGroup, TextInput};
use crate::components::common::toast::{NotificationType, show_error, show_toast};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn Login(on_success: Callback<()>) -> impl IntoView {
    let password = signal(String::new());
    let is_loading = signal(false);

    let handle_login = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        let p = password.0.get();

        if p.is_empty() {
            show_toast("Password is required", NotificationType::Error);
            return;
        }

        // Encode Basic Auth Base64 token (empty username)
        let credentials = format!(":{}", p);
        let b64 = STANDARD.encode(credentials.as_bytes());
        let token = format!("Basic {}", b64);

        // Save token to localStorage temporarily for verification
        crate::utils::set_auth_token(&token);

        is_loading.1.set(true);

        spawn_local(async move {
            match fetch_health().await {
                Ok(_) => {
                    show_toast("Login successful", NotificationType::Success);
                    on_success.run(());
                }
                Err(_) => {
                    crate::utils::clear_auth_storage();
                    show_error("Invalid password");
                    is_loading.1.set(false);
                }
            }
        });
    };

    view! {
        <div class="login-page">
            <div class="login-glass-card">
                <div class="text-center mb-xl">
                    <h2 class="login-title">"Jumbie"</h2>
                    <p class="login-subtitle">"Please sign in to continue"</p>
                </div>

                <form on:submit=handle_login class="flex flex-col gap-md">
                    <FormGroup label="Password".to_string()>
                        <TextInput
                            id="login-password"
                            value=password.0
                            set_value=move |v| password.1.set(v)
                            placeholder="Password"
                            type_="password"
                            password_placeholder_behaviour=true
                            show_toggle=true
                        />
                    </FormGroup>

                    <button
                        type="submit"
                        class="login-btn"
                        disabled=move || is_loading.0.get()
                    >
                        {move || if is_loading.0.get() { "Signing In..." } else { "Sign In" }}
                    </button>
                </form>
            </div>
        </div>
    }
}
