use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

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

/// Whether stored tokens are usable right now: decryptable AND unexpired.
///
/// Shared by the status endpoint and the auth middleware so both agree.
/// An expired-but-refreshable pair reads `false` until the auto-refresh loop
/// mints fresh tokens — signing in again then just re-mints (harmless).
/// `now_unix` is a parameter (not read here) so tests control time.
pub fn tokens_usable(
    yaml: &crate::config_yaml::YamlConfigManager,
    key: &[u8; 32],
    now_unix: i64,
) -> bool {
    match yaml.decrypt_tokens(key) {
        Some(Ok((_, _, expires_at))) => expires_at > now_unix,
        _ => false,
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sign_in", post(sign_in))
        .route("/refresh", post(refresh_tokens))
        .route("/status", get(auth_status))
}

/// Whether usable tokens are stored (no token material leaks).
async fn auth_status(State(state): State<AppState>) -> Json<AuthStatusResponse> {
    let authenticated = {
        let yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
        tokens_usable(
            &yaml,
            &state.encryption_key,
            crate::vehicle_summary::now_unix(),
        )
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

    // Join the running lifecycle (no restart needed): broadcast the fresh
    // token so waiting tasks start, then discover and spawn tasks for any
    // vehicles not already tracked.
    state.token_tx.send(Some(resp.access_token.clone())).ok();
    let (discovered, api_url) = discover_vehicles(&state).await;
    // Region-resolved URL wins over the construction default, so tasks poll
    // the endpoint discovery itself succeeded against (issue #53).
    state.vehicle_manager.set_api_url(api_url);
    {
        let mut known = state.vehicles.write().unwrap_or_else(|e| e.into_inner());
        for (vin, v) in discovered.iter() {
            known.insert(vin.clone(), v.clone());
        }
    }
    let spawned = state.vehicle_manager.spawn_all(
        &discovered,
        Arc::clone(&state.db),
        state.token_tx.subscribe(),
        Arc::clone(&state.yaml),
        state.poll_interval,
    );
    if spawned > 0 {
        info!(spawned, "vehicle tasks started after sign-in");
    }

    Ok(Json(resp))
}

/// Discover vehicles with the currently stored access token (shared by
/// startup in `main` shape and sign-in; empty map when tokens are missing
/// or the API call fails). Also returns the region-resolved API URL the
/// tasks must poll.
async fn discover_vehicles(
    state: &AppState,
) -> (Arc<HashMap<String, crate::tesla_api::Vehicle>>, String) {
    let access_token = {
        let yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
        match yaml.decrypt_tokens(&state.encryption_key) {
            Some(Ok((at, _, _))) => at,
            _ => return (Arc::new(HashMap::new()), state.tesla_api_url.clone()),
        }
    };
    let api_url = match state.auth.decode_region(&access_token) {
        Ok(region) => region.api_url.clone(),
        Err(_) => state.tesla_api_url.clone(),
    };
    match crate::tesla_api::list_products(&access_token, &api_url).await {
        Ok(vehicles) => (
            Arc::new(vehicles.into_iter().map(|v| (v.vin.clone(), v)).collect()),
            api_url,
        ),
        Err(e) => {
            warn!(error = %e, "vehicle discovery failed");
            (Arc::new(HashMap::new()), api_url)
        }
    }
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
            vehicles: Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
            vehicle_manager: Arc::new(crate::vehicles::Vehicles::new("http://localhost:1")),
            token_tx: tokio::sync::watch::channel(None).0,
            tesla_api_url: "http://localhost:1".into(),
            poll_interval: std::time::Duration::from_secs(15),
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
            vehicles: Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
            vehicle_manager: Arc::new(crate::vehicles::Vehicles::new("http://localhost:1")),
            token_tx: tokio::sync::watch::channel(None).0,
            tesla_api_url: "http://localhost:1".into(),
            poll_interval: std::time::Duration::from_secs(15),
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

    // -----------------------------------------------------------------------
    // tokens_usable
    // -----------------------------------------------------------------------

    fn usable_mgr(expires_at: i64) -> crate::config_yaml::YamlConfigManager {
        let dir = std::env::temp_dir().join("tesla-test-usable").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string()
                + &expires_at.to_string(),
        );
        let mut mgr = crate::config_yaml::YamlConfigManager::load(&dir).unwrap();
        mgr.set_encrypted_tokens(&[0u8; 32], "at", "rt", expires_at)
            .unwrap();
        mgr
    }

    #[test]
    fn usable_with_fresh_tokens() {
        assert!(tokens_usable(
            &usable_mgr(9_999_999_999),
            &[0u8; 32],
            1_700_000_000
        ));
    }

    #[test]
    fn unusable_with_expired_tokens() {
        assert!(!tokens_usable(
            &usable_mgr(1_000),
            &[0u8; 32],
            1_700_000_000
        ));
    }

    #[test]
    fn unusable_without_tokens() {
        let dir = std::env::temp_dir().join("tesla-test-usable-empty").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        let mgr = crate::config_yaml::YamlConfigManager::load(&dir).unwrap();
        assert!(!tokens_usable(&mgr, &[0u8; 32], 1_700_000_000));
    }

    #[test]
    fn unusable_with_wrong_key() {
        assert!(!tokens_usable(
            &usable_mgr(9_999_999_999),
            &[1u8; 32],
            1_700_000_000
        ));
    }

    #[tokio::test]
    async fn status_returns_false_with_expired_tokens() {
        let server = MockServer::start().await;
        let mgr = usable_mgr(1_000);
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
            vehicles: Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
            vehicle_manager: Arc::new(crate::vehicles::Vehicles::new("http://localhost:1")),
            token_tx: tokio::sync::watch::channel(None).0,
            tesla_api_url: "http://localhost:1".into(),
            poll_interval: std::time::Duration::from_secs(15),
        };
        let app = router().with_state(state);
        let response = app
            .oneshot(Request::get("/status").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], false);
    }

    // -----------------------------------------------------------------------
    // POST /sign_in lifecycle: token broadcast + discovery + spawn
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sign_in_starts_vehicle_tasks_without_restart() {
        let token_server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-live",
                "refresh_token": "rt-live",
                "expires_in": 28800
            })))
            .mount(&token_server)
            .await;

        let api_server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": [{
                    "id": 1,
                    "vehicle_id": 100,
                    "vin": "LIVEVIN001",
                    "display_name": "Live Car",
                    "state": "online",
                    "api_version": 18,
                    "in_service": false
                }],
                "count": 1
            })))
            .mount(&api_server)
            .await;

        let dir = std::env::temp_dir()
            .join("tesla-test-signin-lifecycle")
            .join(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
                    .to_string(),
            );
        let yaml = Arc::new(std::sync::Mutex::new(
            crate::config_yaml::YamlConfigManager::load(&dir).unwrap(),
        ));
        let db = crate::influxdb::InfluxDb::new("http://localhost:1", "", "", "tesla").unwrap();
        let auth = Arc::new(crate::tesla_auth::TeslaAuthClient::new(
            "test-client",
            &token_server.uri(),
            &api_server.uri(),
        ));
        // "at-live" is not a JWT, so discovery falls back to tesla_api_url.
        let (token_tx, mut token_rx) = tokio::sync::watch::channel(None);
        let manager = Arc::new(crate::vehicles::Vehicles::new("http://localhost:1"));
        let state = crate::api::AppState {
            db: Arc::new(db),
            auth,
            yaml,
            encryption_key: [0u8; 32],
            vehicles: Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
            vehicle_manager: Arc::clone(&manager),
            token_tx,
            tesla_api_url: api_server.uri(),
            poll_interval: std::time::Duration::from_secs(15),
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

        // The fresh token reached the watch channel waiting tasks listen on.
        tokio::time::timeout(std::time::Duration::from_secs(2), token_rx.changed())
            .await
            .expect("token broadcast")
            .unwrap();
        assert_eq!(token_rx.borrow().as_deref(), Some("at-live"));

        // Discovery ran and the task spawned (summary seeded immediately).
        assert!(manager.summary_of("LIVEVIN001").is_some());

        manager.send_cmd("LIVEVIN001", crate::vehicles::VehicleCommand::Shutdown);
    }

    // -----------------------------------------------------------------------
    // Region URL: tasks poll the resolved endpoint, not the default
    // -----------------------------------------------------------------------

    fn neutral_jwt() -> String {
        use base64::Engine as _;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = enc.encode(r#"{"alg":"ES256","typ":"JWT"}"#);
        // No owner-api/.cn/.eu marker: decode falls back to default_api_url.
        let payload = enc.encode(r#"{"aud":"https://example.com/app"}"#);
        format!("{header}.{payload}.dummysig")
    }

    #[tokio::test]
    async fn sign_in_tasks_poll_region_resolved_url() {
        let access = neutral_jwt();
        let token_server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": access,
                "refresh_token": "rt-region",
                "expires_in": 28800
            })))
            .mount(&token_server)
            .await;

        let api_server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": [{
                    "id": 1,
                    "vehicle_id": 100,
                    "vin": "REGIONVIN001",
                    "display_name": "Region Car",
                    "state": "online",
                    "api_version": 18,
                    "in_service": false
                }],
                "count": 1
            })))
            .mount(&api_server)
            .await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "state": "online",
                    "odometer": 100.0,
                    "charge_state": {"battery_level": 77}
                }
            })))
            .mount(&api_server)
            .await;

        let dir = std::env::temp_dir().join("tesla-test-signin-region").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        let yaml = Arc::new(std::sync::Mutex::new(
            crate::config_yaml::YamlConfigManager::load(&dir).unwrap(),
        ));
        let db = crate::influxdb::InfluxDb::new("http://localhost:1", "", "", "tesla").unwrap();
        let auth = Arc::new(crate::tesla_auth::TeslaAuthClient::new(
            "test-client",
            &token_server.uri(),
            &api_server.uri(),
        ));
        let (token_tx, _token_rx) = tokio::sync::watch::channel(None);
        // Deliberately wrong: only the region-resolved URL must be polled.
        let manager = Arc::new(crate::vehicles::Vehicles::new("http://localhost:1"));
        let state = crate::api::AppState {
            db: Arc::new(db),
            auth,
            yaml,
            encryption_key: [0u8; 32],
            vehicles: Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
            vehicle_manager: Arc::clone(&manager),
            token_tx,
            tesla_api_url: api_server.uri(),
            poll_interval: std::time::Duration::from_millis(50),
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

        // The task's first poll hits the mock (not localhost:1): battery
        // telemetry arrives, proving the resolved URL won.
        let mut battery = None;
        for _ in 0..100 {
            battery = manager
                .summary_of("REGIONVIN001")
                .and_then(|s| s.battery_level);
            if battery == Some(77) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(battery, Some(77));

        manager.send_cmd("REGIONVIN001", crate::vehicles::VehicleCommand::Shutdown);
        tokio::time::timeout(std::time::Duration::from_secs(5), manager.join_all())
            .await
            .expect("join_all hung");
    }
}
