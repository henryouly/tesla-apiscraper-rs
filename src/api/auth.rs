use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;

use super::AppState;

#[derive(Deserialize)]
pub struct SignInRequest {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sign_in", post(sign_in))
        .route("/refresh", post(refresh_tokens))
}

async fn sign_in(
    State(state): State<AppState>,
    Json(req): Json<SignInRequest>,
) -> Result<Json<crate::tesla_auth::TokenResponse>, crate::tesla_auth::AuthError> {
    let resp = state
        .auth
        .sign_in(&req.access_token, &req.refresh_token)
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
