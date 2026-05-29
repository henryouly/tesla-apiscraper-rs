pub mod auth;
pub mod health;
pub mod vehicles;

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::config_yaml::YamlConfigManager;
    use crate::influxdb::InfluxDb;
    use crate::tesla_api::Vehicle;
    use crate::tesla_auth::TeslaAuthClient;

    pub fn test_state() -> super::AppState {
        test_state_with_auth_url("http://localhost:9999")
    }

    pub fn test_state_with_auth_url(auth_url: &str) -> super::AppState {
        let db = InfluxDb::new("http://localhost:1", "bad-token", "tesla").unwrap();
        let auth = Arc::new(TeslaAuthClient::new(
            "test-client-id",
            auth_url,
            "https://api.example.com",
        ));
        let dir = std::env::temp_dir().join("tesla-test-state").join(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        let yaml = Arc::new(Mutex::new(YamlConfigManager::load(&dir).unwrap()));
        super::AppState {
            db: Arc::new(db),
            auth,
            yaml,
            encryption_key: [0u8; 32],
            vehicles: Arc::new(HashMap::new()),
            vehicle_manager: Arc::new(crate::vehicles::Vehicles::new("http://localhost:1")),
        }
    }

    pub fn test_state_with_vehicles(vehicles: Vec<Vehicle>) -> super::AppState {
        let mut state = test_state();
        state.vehicles = Arc::new(vehicles.into_iter().map(|v| (v.vin.clone(), v)).collect());
        state
    }
}

use axum::Router;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower_http::cors::CorsLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<crate::influxdb::InfluxDb>,
    pub auth: Arc<crate::tesla_auth::TeslaAuthClient>,
    pub yaml: Arc<Mutex<crate::config_yaml::YamlConfigManager>>,
    pub encryption_key: [u8; 32],
    pub vehicles: Arc<HashMap<String, crate::tesla_api::Vehicle>>,
    pub vehicle_manager: Arc<crate::vehicles::Vehicles>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .nest("/health", health::router())
        .nest("/api/auth", auth::router())
        .nest("/api/vehicles", vehicles::router())
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
    use super::test_helpers;
    use super::*;
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_200() {
        let app = create_router(test_helpers::test_state());
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
        let app = create_router(test_helpers::test_state());
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
        let app = create_router(test_helpers::test_state());
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
        let app = create_router(test_helpers::test_state());
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
        let app = create_router(test_helpers::test_state());
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
        let app = create_router(test_helpers::test_state());
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

    // -----------------------------------------------------------------------
    // /api/vehicles
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn vehicles_returns_200_empty() {
        let app = create_router(test_helpers::test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vehicles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["vehicles"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn vehicles_returns_all_fields() {
        let mut state = test_helpers::test_state();
        let vehicle = crate::tesla_api::Vehicle {
            id: 12345678901234567,
            vehicle_id: 987654321,
            vin: "5YJSA1E26MF123456".into(),
            display_name: Some("My Tesla".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };
        let mut map = HashMap::new();
        map.insert(vehicle.vin.clone(), vehicle);
        state.vehicles = Arc::new(map);
        let app = create_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vehicles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let v = &json["vehicles"][0];
        assert_eq!(v["id"], 12345678901234567i64);
        assert_eq!(v["vehicle_id"], 987654321);
        assert_eq!(v["vin"], "5YJSA1E26MF123456");
        assert_eq!(v["display_name"], "My Tesla");
        assert_eq!(v["state"], "online");
        assert_eq!(v["api_version"], 18);
        assert_eq!(v["in_service"], false);
    }

    #[tokio::test]
    async fn vehicles_subpath_not_found() {
        let app = create_router(test_helpers::test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vehicles/sub")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn vehicles_with_trailing_slash_not_found() {
        let app = create_router(test_helpers::test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/vehicles/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
