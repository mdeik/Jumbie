// Auth token persistence layer — sessionStorage-based credentials.
//
// sessionStorage (not localStorage) because the auth token is the single credential
// granting API access: sessionStorage clears on tab/window close, so an XSS-stolen
// token is only valid for the current session. Trade-off: users must re-login after
// closing the tab; normal SPA navigation and refresh still work.

const AUTH_TOKEN_KEY: &str = "jb_auth_token";
const AUTH_REFRESH_KEY: &str = "jb_refresh_token";

// SSoT: Auth token persistence layer

pub fn clear_auth_storage() {
    if let Some(window) = web_sys::window()
        && let Ok(Some(storage)) = window.session_storage()
    {
        let _ = storage.remove_item(AUTH_TOKEN_KEY);
        let _ = storage.remove_item(AUTH_REFRESH_KEY);
    }
    crate::utils::operation_ledger::clear();
}

pub fn set_auth_token(token: &str) {
    if let Some(window) = web_sys::window()
        && let Ok(Some(storage)) = window.session_storage()
    {
        let _ = storage.set_item(AUTH_TOKEN_KEY, token);
    }
}

pub fn get_auth_token() -> Option<String> {
    web_sys::window()
        .and_then(|w| w.session_storage().ok().flatten())
        .and_then(|s| s.get_item(AUTH_TOKEN_KEY).ok().flatten())
}

/// Clears authentication data and reloads the page. Preference-related data (theme,
/// UI state) uses localStorage separately and is intentionally NOT cleared here.
pub fn clear_auth_and_logout() {
    clear_auth_storage();
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}
