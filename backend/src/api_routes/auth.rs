// API key generation. The key is returned ONCE and never stored in plaintext —
// only a hash is persisted, so a lost key must be revoked and regenerated.
//
// Security:
//   • 32 bytes (256 bits) of CSPRNG output, URL-safe base64, no padding.
//   • "jb_" prefix makes the key identifiable in logs/config without context.
//   • The first 8 chars (plus "jb_") are stored as a prefix so the UI can show
//     which key is which without exposing the full key.
//   • Expiration is optional — `duration_days: null` means "never expires".
//
// The `unsafe` raw-pointer casts fill a fixed-size `[u64; N]` buffer from a
// byte slice to avoid a heap allocation per key; alignment and length match
// exactly, so the cast is sound.

use crate::api::AppState;
use axum::{Json, extract::State, http::StatusCode};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jumbie_shared::auth::ApiScope;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct GenerateApiKeyPayload {
    pub name: String,
    pub scopes: Vec<ApiScope>,
    pub duration_days: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct GeneratedApiKeyResponse {
    pub id: String,
    pub key: String,
    pub prefix: String,
}

#[derive(Debug, Deserialize)]
pub struct GenerateCalendarTokenPayload {
    pub name: String,
    #[serde(default)]
    pub hide_unmonitored: bool,
    #[serde(default)]
    pub show_as_all_day: bool,
}

#[derive(Debug, Serialize)]
pub struct GeneratedCalendarTokenResponse {
    pub id: String,
    pub token: String,
    pub hide_unmonitored: bool,
    pub show_as_all_day: bool,
}

pub async fn generate_api_key(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<GenerateApiKeyPayload>,
) -> Result<Json<GeneratedApiKeyResponse>, (StatusCode, String)> {
    tracing::debug!(
        "generate_api_key called: name={}, scopes={:?}, duration_days={:?}",
        payload.name,
        payload.scopes,
        payload.duration_days
    );
    // Reject empty names / empty scope lists before doing CSPRNG work.
    if let Err(e) = crate::validation::validate_name(&payload.name, "Key name", 255) {
        return Err((StatusCode::BAD_REQUEST, e.to_string()));
    }
    if payload.scopes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "At least one scope must be selected".to_string(),
        ));
    }

    let id = jumbie_shared::config::generate_uuid();

    // Stack-allocated 32-byte buffer; cast to `&[u8]` because `fill_bytes` takes
    // a byte slice.
    let mut bytes = [0u64; 4]; // 32 bytes
    rand::rng()
        .fill_bytes(unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut u8, 32) });

    let raw_key = URL_SAFE_NO_PAD
        .encode(unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u8, 32) });
    let key = format!("jb_{}", raw_key);
    let prefix = format!("jb_{}", &raw_key[0..8]);
    let key_hash = crate::auth_utils::hash_api_key(&key);

    // `Some(0)` or a negative day count means "never expires" (defensive rather
    // than rejecting outright); durations are capped at ~100 years.
    if payload.duration_days.is_some_and(|d| d > 36500) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Duration cannot exceed 36500 days (~100 years)".to_string(),
        ));
    }
    let expires_at = payload.duration_days.and_then(|days| {
        if days > 0 {
            Some(
                crate::datetime::UtcDateTime::from_chrono_utc(
                    chrono::Utc::now() + chrono::Duration::days(days),
                )
                .to_rfc3339_utc(),
            )
        } else {
            None
        }
    });

    state
        .db
        .insert_api_key(
            &id,
            payload.name.trim(),
            &key_hash,
            &prefix,
            &payload.scopes,
            expires_at.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::error!("generate_api_key DB error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    // A new key must be usable immediately, bypassing any cached negative results.
    state.auth_cache.invalidate_api_keys().await;

    tracing::info!(
        "Generated API key: {} (scope: {:?})",
        prefix,
        payload.scopes
    );
    Ok(Json(GeneratedApiKeyResponse { id, key, prefix }))
}

// Calendar token generation. Unlike API keys, calendar tokens grant no scoped
// access — purely reading episode air dates in calendar format. The separate
// "cal_" prefix makes tokens auditable without inspecting the DB. 24 bytes (vs
// 32 for API keys) suffices: the exposed data is limited and the shorter base64
// output is nicer in URLs.

pub async fn generate_calendar_token(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<GenerateCalendarTokenPayload>,
) -> Result<Json<GeneratedCalendarTokenResponse>, (StatusCode, String)> {
    tracing::debug!("generate_calendar_token called: name={}", payload.name);
    if let Err(e) = crate::validation::validate_name(&payload.name, "Token name", 255) {
        tracing::debug!("generate_calendar_token: invalid name: {}", e);
        return Err((StatusCode::BAD_REQUEST, e.to_string()));
    }

    let id = jumbie_shared::config::generate_uuid();

    let mut bytes = [0u64; 3];
    rand::rng()
        .fill_bytes(unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut u8, 24) });

    let raw_token = URL_SAFE_NO_PAD
        .encode(unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u8, 24) });
    let token = format!("cal_{}", raw_token);

    state
        .db
        .insert_calendar_token(
            &id,
            payload.name.trim(),
            &token,
            !payload.hide_unmonitored,
            payload.show_as_all_day,
        )
        .await
        .map_err(|e| {
            tracing::error!("generate_calendar_token DB error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    tracing::info!("Generated calendar token: cal_{}...", &token[4..12]);
    Ok(Json(GeneratedCalendarTokenResponse {
        id,
        token,
        hide_unmonitored: payload.hide_unmonitored,
        show_as_all_day: payload.show_as_all_day,
    }))
}
