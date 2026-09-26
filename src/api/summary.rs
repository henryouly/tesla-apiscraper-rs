use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::Serialize;

use super::AppState;
use crate::vehicle_summary::VehicleSummary;

#[derive(Serialize)]
pub struct SummariesResponse {
    pub summaries: Vec<VehicleSummary>,
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/summaries", get(list_summaries))
        .route("/{vin}/summary", get(get_summary))
}

async fn list_summaries(State(state): State<AppState>) -> Json<SummariesResponse> {
    Json(SummariesResponse {
        summaries: state.vehicle_manager.all_summaries(),
    })
}

async fn get_summary(
    State(state): State<AppState>,
    Path(vin): Path<String>,
) -> Result<Json<VehicleSummary>, (StatusCode, Json<serde_json::Value>)> {
    match state.vehicle_manager.summary_of(&vin) {
        Some(s) => Ok(Json(s)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no summary yet for VIN" })),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tesla_api::Vehicle;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn vehicle(vin: &str) -> Vehicle {
        Vehicle {
            id: 1,
            vehicle_id: 100,
            vin: vin.into(),
            display_name: Some("Car".into()),
            state: "online".into(),
            api_version: 18,
            in_service: false,
        }
    }

    fn summary_for(vin: &str) -> VehicleSummary {
        let data: crate::tesla_api::VehicleDataResponse =
            serde_json::from_value(serde_json::json!({
                "state": "online",
                "drive_state": {"latitude": 1.0, "longitude": 2.0, "speed": 10.0},
                "charge_state": {"battery_level": 80, "battery_range": 250.0}
            }))
            .unwrap();
        VehicleSummary::from_data(
            &vehicle(vin),
            crate::vehicles::VehicleState::Driving,
            &data,
            1700000000,
        )
    }

    #[tokio::test]
    async fn list_summaries_empty() {
        let state = crate::api::test_helpers::test_state();
        let app = router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/summaries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["summaries"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_summary_round_trip() {
        let state = crate::api::test_helpers::test_state();
        state.vehicle_manager.publish_summary(summary_for("VIN1"));
        let app = router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/VIN1/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["vin"], "VIN1");
        assert_eq!(json["battery_level"], 80);
        assert_eq!(json["state"], "Driving");
    }

    #[tokio::test]
    async fn get_summary_unknown_vin_returns_404() {
        let state = crate::api::test_helpers::test_state();
        let app = router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/UNKNOWN/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
