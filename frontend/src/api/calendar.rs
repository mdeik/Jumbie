use super::{get, post};
use crate::api_client::ApiError as Error;

/// Fetch calendar entries for a date range. Both dates are inclusive.
pub async fn fetch_calendar(
    start_date: &str,
    end_date: &str,
) -> Result<jumbie_shared::types::CalendarResponse, Error> {
    crate::debug_log!("fetch_calendar({}, {})", start_date, end_date);
    let result: Result<jumbie_shared::types::CalendarResponse, Error> = get(&format!(
        "calendar?start_date={}&end_date={}",
        start_date, end_date
    ))
    .await;
    match &result {
        Ok(cal) => crate::debug_log!("fetch_calendar: got {} entries", cal.episodes.len()),
        Err(e) => crate::debug_error!("fetch_calendar failed: {}", e),
    }
    result
}

// Calendar Token Generation

/// Payload/response types are local to this module: they are exclusively used by
/// this front-end function, so moving them to the shared crate would add
/// backend coupling for no benefit.
#[derive(serde::Serialize)]
pub struct GenerateCalendarTokenPayload {
    pub name: String,
    #[serde(default)]
    pub hide_unmonitored: bool,
    #[serde(default)]
    pub show_as_all_day: bool,
}

#[derive(serde::Deserialize)]
pub struct GeneratedCalendarTokenResponse {
    pub id: String,
    pub token: String,
    pub hide_unmonitored: bool,
    pub show_as_all_day: bool,
}

pub async fn generate_calendar_token(
    name: String,
    hide_unmonitored: bool,
    show_as_all_day: bool,
) -> Result<GeneratedCalendarTokenResponse, Error> {
    crate::debug_log!("generate_calendar_token({})", name);
    let body = GenerateCalendarTokenPayload {
        name,
        hide_unmonitored,
        show_as_all_day,
    };
    let result: Result<GeneratedCalendarTokenResponse, Error> =
        post("config/auth/calendar_tokens/generate", &body).await;
    match &result {
        Ok(t) => crate::debug_log!(
            "generate_calendar_token: created token with prefix {}",
            &t.token[..t.token.len().min(8)]
        ),
        Err(e) => crate::debug_error!("generate_calendar_token failed: {}", e),
    }
    result
}
