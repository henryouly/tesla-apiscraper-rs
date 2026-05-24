pub mod health;

use axum::Router;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<crate::influxdb::InfluxDb>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .nest("/health", health::router())
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}
