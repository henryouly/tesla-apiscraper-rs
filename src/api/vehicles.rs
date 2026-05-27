use axum::{Json, extract::State, routing::get};
use serde::Serialize;

use super::AppState;

#[derive(Serialize)]
pub struct VehiclesResponse {
    pub vehicles: Vec<crate::tesla_api::Vehicle>,
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route("/", get(list_vehicles))
}

async fn list_vehicles(State(state): State<AppState>) -> Json<VehiclesResponse> {
    let vehicles: Vec<_> = state.vehicles.values().cloned().collect();
    Json(VehiclesResponse { vehicles })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tesla_api::Vehicle;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    fn test_state_with_vehicles(vehicles: Vec<Vehicle>) -> AppState {
        let db =
            crate::influxdb::InfluxDb::new("http://localhost:1", "bad-token", "tesla").unwrap();
        let auth = Arc::new(crate::tesla_auth::TeslaAuthClient::new(
            "test-client-id",
            "http://localhost:9999",
            "https://api.example.com",
        ));
        let dir = std::env::temp_dir().join("tesla-test-vehicles").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        let yaml = Arc::new(Mutex::new(
            crate::config_yaml::YamlConfigManager::load(&dir).unwrap(),
        ));

        let mut map = HashMap::new();
        for v in vehicles {
            map.insert(v.vin.clone(), v);
        }

        AppState {
            db: Arc::new(db),
            auth,
            yaml,
            encryption_key: [0u8; 32],
            vehicles: Arc::new(map),
        }
    }

    #[tokio::test]
    async fn list_vehicles_empty() {
        let state = test_state_with_vehicles(vec![]);
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

        let state = test_state_with_vehicles(vehicles);
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
}
