pub mod auth;
pub mod health;

use axum::Router;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<crate::influxdb::InfluxDb>,
    pub auth: Arc<crate::tesla_auth::TeslaAuthClient>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .nest("/health", health::router())
        .nest("/api/auth", auth::router())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let db =
            crate::influxdb::InfluxDb::new("http://localhost:1", "bad-token", "tesla").unwrap();
        let auth = Arc::new(crate::tesla_auth::TeslaAuthClient::new(
            "test-client-id",
            "test-client-secret",
            "http://localhost:9999",
            "https://api.example.com",
        ));
        AppState {
            db: Arc::new(db),
            auth,
        }
    }

    #[tokio::test]
    async fn health_returns_200() {
        let app = create_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn ready_returns_503_when_db_unreachable() {
        let app = create_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "error");
        assert!(json["error"].is_string());
        assert!(!json["error"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_route_returns_404() {
        let app = create_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn health_under_subpath_not_found() {
        let app = create_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health/sub")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn options_request_returns_cors_headers() {
        let app = create_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/health")
                    .header("origin", "http://example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        // permissive CORS allows any origin
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_some()
        );
        assert!(
            response
                .headers()
                .get("access-control-allow-methods")
                .is_some()
        );
    }

    #[tokio::test]
    async fn health_with_trailing_slash_not_found() {
        let app = create_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Axum does not strip trailing slashes by default
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
