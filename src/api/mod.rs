pub mod auth;
pub mod events;
pub mod health;
pub mod require_auth;
pub mod summary;
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
        let db = InfluxDb::new("http://localhost:1", "", "", "tesla").unwrap();
        // Default API URL points at the mock too, so region fallback in
        // discovery stays hermetic (no real-network products call).
        let auth = Arc::new(TeslaAuthClient::new("test-client-id", auth_url, auth_url));
        let dir = std::env::temp_dir().join("tesla-test-state").join(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        let yaml = Arc::new(Mutex::new(YamlConfigManager::load(&dir).unwrap()));
        let (token_tx, _token_rx) = tokio::sync::watch::channel(None);
        super::AppState {
            db: Arc::new(db),
            auth,
            yaml,
            encryption_key: [0u8; 32],
            vehicles: Arc::new(std::sync::RwLock::new(HashMap::new())),
            vehicle_manager: Arc::new(crate::vehicles::Vehicles::new("http://localhost:1")),
            token_tx,
            tesla_api_url: "http://localhost:1".into(),
            poll_interval: std::time::Duration::from_secs(15),
        }
    }

    pub fn test_state_with_vehicles(vehicles: Vec<Vehicle>) -> super::AppState {
        let state = test_state();
        *state.vehicles.write().unwrap_or_else(|e| e.into_inner()) =
            vehicles.into_iter().map(|v| (v.vin.clone(), v)).collect();
        state
    }

    /// State with fresh (unexpired) tokens so guarded routes return 200.
    pub fn test_state_authed() -> super::AppState {
        let state = test_state();
        state
            .yaml
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_encrypted_tokens(&[0u8; 32], "at", "rt", 9_999_999_999)
            .unwrap();
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
    /// Discovered vehicles, refreshed by sign-in as well as startup.
    pub vehicles: Arc<std::sync::RwLock<HashMap<String, crate::tesla_api::Vehicle>>>,
    pub vehicle_manager: Arc<crate::vehicles::Vehicles>,
    /// Broadcasts fresh access tokens to vehicle tasks (also fed by sign-in).
    pub token_tx: tokio::sync::watch::Sender<Option<String>>,
    pub tesla_api_url: String,
    pub poll_interval: std::time::Duration,
}

pub fn create_router(state: AppState) -> Router {
    use axum::middleware;
    // Telemetry routes require usable tokens server-side (the SPA's
    // RequireAuth is client-side only). Auth bootstrap + health stay public.
    let guard = middleware::from_fn_with_state(state.clone(), require_auth::require_usable_tokens);
    Router::new()
        .nest("/health", health::router())
        .nest("/api/auth", auth::router())
        .nest("/api/vehicles", vehicles::router().layer(guard.clone()))
        .nest("/api/events", events::router().layer(guard))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Whether `dist` holds a built SPA (presence of `index.html` decides).
pub fn spa_available(dist: &std::path::Path) -> bool {
    dist.join("index.html").is_file()
}

/// Serve a built SPA from `dist`: assets directly, unknown non-API paths
/// fall back to `index.html` for client-side routes. Takes the finalized
/// router (after `.with_state`), so registered API/health routes keep
/// precedence; unknown `/api/*` and `/health*` paths still 404 instead of
/// serving the shell. No-op when `spa_available` is false.
pub fn with_spa(router: Router, dist: &std::path::Path) -> Router {
    if !spa_available(dist) {
        return router;
    }
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        response::IntoResponse,
    };
    use tower::ServiceExt as _;
    use tower_http::services::{ServeDir, ServeFile};
    // Note: `fallback` (not `not_found_service`, which forces 404) so
    // client-side routes return 200 with the shell.
    let serve = ServeDir::new(dist).fallback(ServeFile::new(dist.join("index.html")));
    let fallback = move |req: Request<Body>| async move {
        if is_api_or_health(req.uri().path()) {
            return (StatusCode::NOT_FOUND, "not_found").into_response();
        }
        serve.clone().oneshot(req).await.into_response()
    };
    router.fallback(fallback)
}

/// API/health namespace for the SPA fallback: exact or slash-terminated
/// only, so bare `/api` (and `/api?x=1`, whose query `uri.path()` strips)
/// 404 instead of receiving the shell — without over-matching siblings.
fn is_api_or_health(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/health" || path.starts_with("/health/")
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
        let app = create_router(test_helpers::test_state_authed());
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
        let state = test_helpers::test_state_authed();
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
        *state.vehicles.write().unwrap_or_else(|e| e.into_inner()) = map;
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
        let app = create_router(test_helpers::test_state_authed());
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
        let app = create_router(test_helpers::test_state_authed());
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

    // -----------------------------------------------------------------------
    // SPA static serving
    // -----------------------------------------------------------------------

    fn spa_fixture() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("tesla-test-spa").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<html>spa-shell</html>").unwrap();
        std::fs::write(dir.join("app.js"), "console.log(1)").unwrap();
        dir
    }

    #[tokio::test]
    async fn spa_serves_index_at_root() {
        let dir = spa_fixture();
        let app = with_spa(create_router(test_helpers::test_state()), &dir);
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.windows(9).any(|w| w == b"spa-shell"));
    }

    #[tokio::test]
    async fn spa_falls_back_to_index_for_client_routes() {
        let dir = spa_fixture();
        let app = with_spa(create_router(test_helpers::test_state()), &dir);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/settings/car/123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.windows(9).any(|w| w == b"spa-shell"));
    }

    #[tokio::test]
    async fn spa_serves_assets_directly() {
        let dir = spa_fixture();
        let app = with_spa(create_router(test_helpers::test_state()), &dir);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"console.log(1)");
    }

    #[tokio::test]
    async fn spa_does_not_shadow_api_routes() {
        let dir = spa_fixture();
        let app = with_spa(create_router(test_helpers::test_state_authed()), &dir);
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
        assert!(json["vehicles"].is_array());
    }

    #[tokio::test]
    async fn spa_unknown_api_path_still_404s() {
        let dir = spa_fixture();
        let app = with_spa(create_router(test_helpers::test_state_authed()), &dir);
        for uri in ["/api/nope", "/api", "/api?x=1", "/health/ready-nope"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        }
    }

    #[tokio::test]
    async fn spa_missing_dist_stays_api_only() {
        let dir = std::env::temp_dir().join("tesla-test-spa-missing").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        assert!(!spa_available(&dir));
        let app = with_spa(create_router(test_helpers::test_state()), &dir);
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
