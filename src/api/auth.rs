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
        let db =
            crate::influxdb::InfluxDb::new("http://localhost:1", "bad-token", "tesla").unwrap();
        let auth = Arc::new(crate::tesla_auth::TeslaAuthClient::new(
            "test-client",
            mock_uri,
            "https://default.api",
        ));
        let state = AppState {
            db: Arc::new(db),
            auth,
        };
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
                            "access_token": "old-at",
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
                            "access_token": "old-at",
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
                            "access_token": "old-at",
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
                            "access_token": "at"
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
}
