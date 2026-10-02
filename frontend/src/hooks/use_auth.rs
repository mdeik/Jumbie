use crate::hooks::use_config::ConfigContext;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[derive(Clone, Copy)]
pub struct AuthContext {
    pub is_authenticated: ReadSignal<bool>,
    pub set_is_authenticated: WriteSignal<bool>,
    pub force_login: ReadSignal<bool>,
    pub set_force_login: WriteSignal<bool>,
}

pub fn use_auth() -> AuthContext {
    let ConfigContext {
        config: _,
        set_config,
    } = use_context::<ConfigContext>().expect("ConfigContext not found");

    // Pre-populate is_authenticated from localStorage
    let has_existing_token = crate::utils::get_auth_token().is_some();
    let (is_authenticated, set_is_authenticated) = signal(has_existing_token);

    // force_login is set when we detect a stale/invalid token (401 on startup),
    // OR when there is no token and an unauthenticated config fetch reveals the
    // server has a password set.
    let (force_login, set_force_login) = signal(false);
    provide_context(set_force_login);

    let handle_login_success = Callback::new(move |()| {
        set_is_authenticated.set(true);
        set_force_login.set(false);

        // Force a config reload now that we have a valid token
        spawn_local(async move {
            if let Ok(c) = crate::api::fetch_config().await {
                set_config.try_update(|cfg| *cfg = Some(c));
            }
        });

        // UI preferences are handled by provide_ui_config_context, which
        // self-populates from the API cache on creation and refreshes in
        // the background — no need to fetch them here.
    });

    provide_context(handle_login_success);

    // Initial config fetch and auth validation.
    //
    // With a token: fetch config; a 401 means the token is stale — clear it and
    // show the login screen.
    //
    // Without a token: try an unauthenticated config fetch. If the server has auth
    // disabled (empty password) the backend allows it through and we skip the login
    // screen; a 401 means auth is required, so force the login screen.
    if has_existing_token {
        LocalResource::new(move || async move {
            match crate::api::fetch_config().await {
                Ok(c) => {
                    let auth_disabled = c
                        .auth
                        .password
                        .as_deref()
                        .map(|p| p.is_empty())
                        .unwrap_or(true);
                    if auth_disabled {
                        set_is_authenticated.try_update(|auth| *auth = true);
                    }
                    set_config.try_update(|cfg| *cfg = Some(c));
                }
                Err(e) => {
                    // Only force login on an explicit 401. Network errors (timeout,
                    // DNS) should not trigger login — components handle missing
                    // config gracefully via their loading states.
                    let is_unauthorized = matches!(
                        &e,
                        crate::api_client::ApiError::Api {
                            status: reqwest::StatusCode::UNAUTHORIZED,
                            ..
                        }
                    );

                    if is_unauthorized {
                        crate::utils::clear_auth_storage();
                    }

                    set_config.try_update(|cfg| *cfg = None);
                    set_is_authenticated.try_update(|auth| *auth = false);
                    set_force_login.try_update(|fl| *fl = is_unauthorized);
                }
            }
        });
    } else {
        // No token — try an unauthenticated request to check if auth is required.
        // When the server has no password set, it allows all requests regardless
        // of credentials, so an unauthenticated fetch_config() will succeed.
        LocalResource::new(move || async move {
            match crate::api::fetch_config().await {
                Ok(c) => {
                    let auth_disabled = c
                        .auth
                        .password
                        .as_deref()
                        .map(|p| p.is_empty())
                        .unwrap_or(true);
                    if auth_disabled {
                        // Auth is disabled — skip login entirely
                        set_is_authenticated.try_update(|auth| *auth = true);
                    } else {
                        // Auth is enabled but we have no token — show login
                        set_force_login.try_update(|fl| *fl = true);
                    }
                    set_config.try_update(|cfg| *cfg = Some(c));
                }
                Err(e) => {
                    // Only force login on 401. Network errors (timeout, DNS in Docker)
                    // should not trigger login — the app already renders content via
                    // the AppLayout fix, and components handle missing config gracefully.
                    // If auth IS required, individual API calls will 401 which the error
                    // handlers can manage.
                    let is_unauthorized = matches!(
                        &e,
                        crate::api_client::ApiError::Api {
                            status: reqwest::StatusCode::UNAUTHORIZED,
                            ..
                        }
                    );
                    set_config.try_update(|cfg| *cfg = None);
                    set_force_login.try_update(|fl| *fl = is_unauthorized);
                }
            }
        });
    }

    let context = AuthContext {
        is_authenticated,
        set_is_authenticated,
        force_login,
        set_force_login,
    };
    provide_context(context);
    context
}
