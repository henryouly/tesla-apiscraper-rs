pub mod health;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{
    MakeRequestId, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::trace::{DefaultOnRequest, DefaultOnResponse, MakeSpan, TraceLayer};
use tracing::Level;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<crate::influxdb::InfluxDb>,
}

#[derive(Clone)]
struct RequestIdCounter(Arc<AtomicU64>);

impl RequestIdCounter {
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(1)))
    }
}

impl MakeRequestId for RequestIdCounter {
    fn make_request_id<B>(&mut self, _request: &axum::http::Request<B>) -> Option<RequestId> {
        let id = self.0.fetch_add(1, Ordering::Relaxed);
        let value = axum::http::HeaderValue::from_str(&format!("{:08x}", id)).ok()?;
        Some(RequestId::new(value))
    }
}

#[derive(Clone)]
struct RequestSpan;

impl<B> MakeSpan<B> for RequestSpan {
    fn make_span(&mut self, request: &axum::http::Request<B>) -> tracing::Span {
        let request_id = request
            .extensions()
            .get::<RequestId>()
            .and_then(|id: &RequestId| id.header_value().to_str().ok())
            .unwrap_or("unknown");

        tracing::info_span!(
            "http_request",
            method = %request.method(),
            uri = %request.uri(),
            request_id = %request_id,
        )
    }
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .nest("/health", health::router())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(RequestSpan)
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::new(
            axum::http::HeaderName::from_static("x-request-id"),
            RequestIdCounter::new(),
        ))
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
            crate::influxdb::InfluxDb::new("http://localhost:1", "bad-token", "tesla", "tesla");
        AppState { db: Arc::new(db) }
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

    #[tokio::test]
    async fn response_has_request_id_header() {
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

        let request_id = response.headers().get("x-request-id");
        assert!(
            request_id.is_some(),
            "response should have x-request-id header"
        );
        let value = request_id.unwrap().to_str().unwrap();
        assert!(!value.is_empty(), "request ID should not be empty");
        // Should be a hex string
        assert!(
            value.chars().all(|c| c.is_ascii_hexdigit()),
            "request ID should be hex"
        );
    }

    #[tokio::test]
    async fn request_id_is_monotonic() {
        let app = create_router(test_state());

        let response1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let response2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let id1 = response1
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let id2 = response2
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            id2 > id1,
            "request IDs should be monotonically increasing: {} >= {}",
            id1,
            id2
        );
    }
}
