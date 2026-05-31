use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Serialize;

use super::AppState;
use crate::vehicles::{VehicleCommand, VehicleState};

#[derive(Serialize)]
pub struct VehiclesResponse {
    pub vehicles: Vec<crate::tesla_api::Vehicle>,
}

#[derive(Serialize)]
pub struct VehicleStateResponse {
    pub vin: String,
    pub state: Option<VehicleState>,
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/", get(list_vehicles))
        .route("/{vin}/state", get(vehicle_state))
        .route("/{vin}/suspend", post(suspend_logging))
        .route("/{vin}/resume", post(resume_logging))
}

async fn list_vehicles(State(state): State<AppState>) -> Json<VehiclesResponse> {
    let vehicles: Vec<_> = state.vehicles.values().cloned().collect();
    Json(VehiclesResponse { vehicles })
}

async fn vehicle_state(
    State(state): State<AppState>,
    Path(vin): Path<String>,
) -> Json<VehicleStateResponse> {
    let state = state.vehicle_manager.state_of(&vin);
    Json(VehicleStateResponse { vin, state })
}

async fn suspend_logging(
    State(state): State<AppState>,
    Path(vin): Path<String>,
) -> (StatusCode, &'static str) {
    let st = state.vehicle_manager.state_of(&vin);
    match st {
        None => (StatusCode::NOT_FOUND, "vehicle_not_found"),
        Some(ref s) => {
            if let Some(reason) = crate::vehicles::cannot_suspend_state(s) {
                (StatusCode::CONFLICT, reason)
            } else {
                state
                    .vehicle_manager
                    .send_cmd(&vin, VehicleCommand::Suspend);
                (StatusCode::NO_CONTENT, "")
            }
        }
    }
}

async fn resume_logging(
    State(state): State<AppState>,
    Path(vin): Path<String>,
) -> (StatusCode, &'static str) {
    if state.vehicle_manager.state_of(&vin).is_none() {
        return (StatusCode::NOT_FOUND, "vehicle_not_found");
    }
    state.vehicle_manager.send_cmd(&vin, VehicleCommand::Resume);
    (StatusCode::NO_CONTENT, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::test_state_with_vehicles;
    use crate::tesla_api::Vehicle;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn list_vehicles_empty() {
        let state = crate::api::test_helpers::test_state();
        let app = router().with_state(state);
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["vehicles"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_vehicles_with_data() {
        let vehicles = vec![
            Vehicle {
                id: 1,
                vehicle_id: 100,
                vin: "VIN001".into(),
                display_name: Some("Car One".into()),
                state: "online".into(),
                api_version: 18,
                in_service: false,
            },
            Vehicle {
                id: 2,
                vehicle_id: 200,
                vin: "VIN002".into(),
                display_name: None,
                state: "asleep".into(),
                api_version: 17,
                in_service: false,
            },
        ];

        let state = crate::api::test_helpers::test_state_with_vehicles(vehicles);
        let app = router().with_state(state);
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json["vehicles"].as_array().unwrap();

        let vins: Vec<&str> = arr.iter().map(|v| v["vin"].as_str().unwrap()).collect();
        assert!(vins.contains(&"VIN001"));
        assert!(vins.contains(&"VIN002"));

        let car1 = arr.iter().find(|v| v["vin"] == "VIN001").unwrap();
        assert_eq!(car1["display_name"], "Car One");
        assert_eq!(car1["state"], "online");
    }

    #[tokio::test]
    async fn vehicle_state_unknown_vin() {
        let state = test_state_with_vehicles(vec![]);
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/UNKNOWNVIN/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["vin"], "UNKNOWNVIN");
        assert!(json["state"].is_null());
    }
}
