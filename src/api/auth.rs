use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;

use super::AppState;

#[derive(Deserialize)]
pub struct PollRequest {
    pub device_code: String,
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
}

fn default_poll_interval() -> u64 {
    5
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/device", post(device_authorize))
        .route("/poll", post(poll_device))
        .route("/refresh", post(refresh_tokens))
}

async fn device_authorize(
    State(state): State<AppState>,
) -> Result<Json<crate::tesla_auth::DeviceAuthorizeResponse>, crate::tesla_auth::AuthError> {
    let resp = state.auth.device_authorize().await?;
    Ok(Json(resp))
}

async fn poll_device(
    State(state): State<AppState>,
    Json(req): Json<PollRequest>,
) -> Result<Json<crate::tesla_auth::TokenResponse>, crate::tesla_auth::AuthError> {
    let resp = state
        .auth
        .poll_device_token(&req.device_code, req.poll_interval)
        .await?;
    Ok(Json(resp))
}

async fn refresh_tokens(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<crate::tesla_auth::TokenResponse>, crate::tesla_auth::AuthError> {
    let resp = state.auth.refresh_tokens(&req.refresh_token).await?;
    Ok(Json(resp))
}
