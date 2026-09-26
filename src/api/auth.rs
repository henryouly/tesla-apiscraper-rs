use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde::Deserialize;
use serde::Serialize;
use tracing::warn;

use super::AppState;

/// Sign-in takes the refresh token only: the access token is short-lived and
/// always minted fresh from it, so asking the user for both is pointless.
#[derive(Deserialize)]
pub struct SignInRequest {
    pub refresh_token: String,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Serialize)]
pub struct AuthStatusResponse {
    pub authenticated: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sign_in", post(sign_in))
        .route("/refresh", post(refresh_tokens))
        .route("/status", get(auth_status))
}

/// Whether decryptable tokens are stored (no token material leaks).
async fn auth_status(State(state): State<AppState>) -> Json<AuthStatusResponse> {
    let authenticated = {
        let yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
        yaml.decrypt_tokens(&state.encryption_key)
            .is_some_and(|r| r.is_ok())
    };
    Json(AuthStatusResponse { authenticated })
}

async fn sign_in(
    State(state): State<AppState>,
    Json(req): Json<SignInRequest>,
) -> Result<Json<crate::tesla_auth::TokenResponse>, crate::tesla_auth::AuthError> {
    let resp = state.auth.refresh_tokens(&req.refresh_token).await?;

    if let Ok(mut yaml) = state.yaml.lock()
        && let Err(e) = yaml.set_encrypted_tokens(
            &state.encryption_key,
            &resp.access_token,
            &resp.refresh_token,
            resp.expires_at(),
        )
    {
        warn!(error = %e, "failed to persist tokens after sign_in");
    }

    Ok(Json(resp))
}

async fn refresh_tokens(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<crate::tesla_auth::TokenResponse>, crate::tesla_auth::AuthError> {
    let resp = state.auth.refresh_tokens(&req.refresh_token).await?;

    if let Ok(mut yaml) = state.yaml.lock()
        && let Err(e) = yaml.set_encrypted_tokens(
            &state.encryption_key,
            &resp.access_token,
            &resp.refresh_token,
            resp.expires_at(),
        )
    {
        warn!(error = %e, "failed to persist tokens after refresh");
    }

    Ok(Json(resp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    fn test_app(mock_uri: &str) -> Router {
        let state = crate::api::test_helpers::test_state_with_auth_url(mock_uri);
        router().with_state(state)
    }

    // -----------------------------------------------------------------------
    // POST /sign_in
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sign_in_returns_200_with_valid_tokens() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-new",
                "refresh_token": "rt-new",
                "expires_in": 28800
            })))
            .mount(&server)
            .await;

        let app = test_app(&server.uri());
        let response = app
            .oneshot(
                Request::post("/sign_in")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "refresh_token": "old-rt"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["access_token"], "at-new");
        assert_eq!(json["refresh_token"], "rt-new");
        assert_eq!(json["expires_in"], 28800);
    }

    #[tokio::test]
    async fn sign_in_persists_encrypted_tokens() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-persist",
                "refresh_token": "rt-persist",
                "expires_in": 28800
            })))
            .mount(&server)
            .await;

        let dir = std::env::temp_dir().join("tesla-test-auth").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        let db = crate::influxdb::InfluxDb::new("http://localhost:1", "", "", "tesla").unwrap();
        let auth = Arc::new(crate::tesla_auth::TeslaAuthClient::new(
            "test-client",
            &server.uri(),
            "https://default.api",
        ));
        let encryption_key = [0u8; 32];
        let yaml = Arc::new(std::sync::Mutex::new(
            crate::config_yaml::YamlConfigManager::load(&dir).unwrap(),
        ));
        let state = AppState {
            db: Arc::new(db),
            auth,
            yaml,
            encryption_key,
            vehicles: Arc::new(std::collections::HashMap::new()),
            vehicle_manager: Arc::new(crate::vehicles::Vehicles::new("http://localhost:1")),
        };
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::post("/sign_in")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "refresh_token": "old-rt"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify persisted tokens are encrypted (not plaintext)
        let loaded: crate::config_yaml::TokensConfig =
            crate::config_yaml::load_optional(&dir.join("tokens.yml"))
                .unwrap()
                .expect("tokens.yml should exist");
        assert_ne!(loaded.access_token, "at-persist", "should not be plaintext");
        assert_ne!(
            loaded.refresh_token, "rt-persist",
            "should not be plaintext"
        );

        // Verify tokens can be decrypted with the configured key
        let at = crate::encryption::decrypt(&encryption_key, &loaded.access_token).unwrap();
        let rt = crate::encryption::decrypt(&encryption_key, &loaded.refresh_token).unwrap();
        assert_eq!(at, "at-persist");
        assert_eq!(rt, "rt-persist");
    }

    #[tokio::test]
    async fn sign_in_returns_400_on_invalid_grant() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "The refresh token is invalid or expired"
            })))
            .mount(&server)
            .await;

        let app = test_app(&server.uri());
        let response = app
            .oneshot(
                Request::post("/sign_in")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "refresh_token": "bad-rt"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["error"]
                .as_str()
                .unwrap()
                .contains("invalid or expired")
        );
    }

    #[tokio::test]
    async fn sign_in_returns_502_on_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "login_required",
                "error_description": "Authentication required"
            })))
            .mount(&server)
            .await;

        let app = test_app(&server.uri());
        let response = app
            .oneshot(
                Request::post("/sign_in")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "refresh_token": "bad-rt"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("upstream"));
        assert!(json["error"].as_str().unwrap().contains("Tesla auth"));
    }

    #[tokio::test]
    async fn sign_in_returns_422_on_empty_body() {
        let server = MockServer::start().await;
        let app = test_app(&server.uri());
        let response = app
            .oneshot(
                Request::post("/sign_in")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn sign_in_returns_422_on_missing_fields() {
        let server = MockServer::start().await;
        let app = test_app(&server.uri());
        let response = app
            .oneshot(
                Request::post("/sign_in")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "unexpected": "value"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // -----------------------------------------------------------------------
    // POST /refresh
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn refresh_returns_200_with_valid_token() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-refreshed",
                "refresh_token": "rt-refreshed",
                "expires_in": 28800
            })))
            .mount(&server)
            .await;

        let app = test_app(&server.uri());
        let response = app
            .oneshot(
                Request::post("/refresh")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "refresh_token": "old-rt"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["access_token"], "at-refreshed");
        assert_eq!(json["refresh_token"], "rt-refreshed");
    }

    #[tokio::test]
    async fn refresh_returns_400_on_invalid_grant() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "expired"
            })))
            .mount(&server)
            .await;

        let app = test_app(&server.uri());
        let response = app
            .oneshot(
                Request::post("/refresh")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "refresh_token": "expired-rt"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("expired"));
    }

    #[tokio::test]
    async fn refresh_returns_422_on_empty_body() {
        let server = MockServer::start().await;
        let app = test_app(&server.uri());
        let response = app
            .oneshot(
                Request::post("/refresh")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn refresh_returns_422_on_unexpected_fields() {
        let server = MockServer::start().await;
        let app = test_app(&server.uri());
        let response = app
            .oneshot(
                Request::post("/refresh")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "unexpected": "value"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // -----------------------------------------------------------------------
    // GET /status
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn status_returns_false_without_tokens() {
        let server = MockServer::start().await;
        let app = test_app(&server.uri());
        let response = app
            .oneshot(Request::get("/status").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], false);
    }

    #[tokio::test]
    async fn status_returns_true_with_stored_tokens() {
        let server = MockServer::start().await;
        let dir = std::env::temp_dir().join("tesla-test-auth-status").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        let mut mgr = crate::config_yaml::YamlConfigManager::load(&dir).unwrap();
        mgr.set_encrypted_tokens(&[0u8; 32], "at", "rt", 9_999_999_999)
            .unwrap();
        let db = crate::influxdb::InfluxDb::new("http://localhost:1", "", "", "tesla").unwrap();
        let auth = Arc::new(crate::tesla_auth::TeslaAuthClient::new(
            "test-client",
            &server.uri(),
            "https://default.api",
        ));
        let state = crate::api::AppState {
            db: Arc::new(db),
            auth,
            yaml: Arc::new(std::sync::Mutex::new(mgr)),
            encryption_key: [0u8; 32],
            vehicles: Arc::new(std::collections::HashMap::new()),
            vehicle_manager: Arc::new(crate::vehicles::Vehicles::new("http://localhost:1")),
        };
        let app = router().with_state(state);
        let response = app
            .oneshot(Request::get("/status").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], true);
    }
}
