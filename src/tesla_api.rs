use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vehicle {
    pub id: i64,
    pub vehicle_id: i64,
    pub vin: String,
    pub display_name: Option<String>,
    pub state: String,
    pub api_version: i64,
    pub in_service: bool,
}

// ── Vehicle data types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DriveState {
    #[serde(default)]
    pub shift_state: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub power: Option<i64>,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    #[serde(default)]
    pub heading: Option<i64>,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChargeState {
    #[serde(default)]
    pub charging_state: Option<String>,
    #[serde(default)]
    pub battery_level: Option<i64>,
    #[serde(default)]
    pub battery_range: Option<f64>,
    #[serde(default)]
    pub est_battery_range: Option<f64>,
    #[serde(default)]
    pub charger_power: Option<i64>,
    #[serde(default)]
    pub charger_voltage: Option<i64>,
    #[serde(default)]
    pub charger_phases: Option<i64>,
    #[serde(default)]
    pub charge_energy_added: Option<f64>,
    #[serde(default)]
    pub charge_limit_soc: Option<i64>,
    #[serde(default)]
    pub time_to_full_charge: Option<f64>,
    #[serde(default)]
    pub charge_port_door_open: Option<bool>,
    #[serde(default)]
    pub fast_charger_brand: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VehicleSubState {
    #[serde(default)]
    pub odometer: Option<f64>,
    #[serde(default)]
    pub sentry_mode: Option<bool>,
    #[serde(default)]
    pub locked: Option<bool>,
    #[serde(default)]
    pub car_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClimateState {
    #[serde(default)]
    pub inside_temp: Option<f64>,
    #[serde(default)]
    pub outside_temp: Option<f64>,
    #[serde(default)]
    pub is_climate_on: Option<bool>,
    #[serde(default)]
    pub fan_status: Option<i64>,
    #[serde(default)]
    pub defrost_mode: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoftwareState {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub expected_duration_sec: Option<i64>,
    #[serde(default)]
    pub install_time: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VehicleData {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub drive_state: Option<DriveState>,
    #[serde(default)]
    pub charge_state: Option<ChargeState>,
    #[serde(default)]
    pub vehicle_state: Option<VehicleSubState>,
    #[serde(default)]
    pub climate_state: Option<ClimateState>,
    #[serde(default)]
    pub software_update: Option<SoftwareState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleDataResponse {
    pub response: VehicleData,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

fn default_api_url(api_url: &str) -> String {
    api_url.trim_end_matches('/').to_string()
}

fn api_request(
    access_token: &str,
    api_url: &str,
    path: &str,
) -> reqwest::RequestBuilder {
    let client = reqwest::Client::new();
    let url = format!("{}{}", default_api_url(api_url), path);
    client.get(&url).bearer_auth(access_token)
}

pub async fn list_products(
    access_token: &str,
    api_url: &str,
) -> Result<Vec<Vehicle>, crate::tesla_auth::AuthError> {
    let resp = api_request(access_token, api_url, "/api/1/products")
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(crate::tesla_auth::AuthError::Api { status, body });
    }

    let json: serde_json::Value = resp.json().await?;
    serde_json::from_value(json["response"].clone()).map_err(|e| {
        crate::tesla_auth::AuthError::Api {
            status: 502,
            body: format!("invalid /api/1/products response: {e}"),
        }
    })
}

/// Fetch full vehicle state from the Owner API.
/// Endpoint: `GET /api/1/vehicles/{vehicle_id}/vehicle_data`
pub async fn get_vehicle_data(
    access_token: &str,
    api_url: &str,
    vehicle_id: i64,
) -> Result<VehicleData, crate::tesla_auth::AuthError> {
    let path = format!("/api/1/vehicles/{vehicle_id}/vehicle_data");
    let resp = api_request(access_token, api_url, &path).send().await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(crate::tesla_auth::AuthError::Api { status, body });
    }

    let wrapper: VehicleDataResponse = resp.json().await.map_err(|e| {
        crate::tesla_auth::AuthError::Api {
            status: 502,
            body: format!("invalid vehicle_data response: {e}"),
        }
    })?;
    Ok(wrapper.response)
}

/// Wake a vehicle by sending a POST to the wake endpoint.
/// The endpoint returns vehicle data once the vehicle responds.
#[allow(dead_code)]
pub async fn wake_up(
    access_token: &str,
    api_url: &str,
    vehicle_id: i64,
) -> Result<VehicleData, crate::tesla_auth::AuthError> {
    let path = format!("/api/1/vehicles/{vehicle_id}/wake_up");
    let client = reqwest::Client::new();
    let url = format!("{}{}", default_api_url(api_url), path);
    let resp = client
        .post(&url)
        .bearer_auth(access_token)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(crate::tesla_auth::AuthError::Api { status, body });
    }

    let wrapper: VehicleDataResponse = resp.json().await.map_err(|e| {
        crate::tesla_auth::AuthError::Api {
            status: 502,
            body: format!("invalid wake_up response: {e}"),
        }
    })?;
    Ok(wrapper.response)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tesla_auth::AuthError;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    const EXPECTED_VEHICLE_JSON: &str = r#"{
        "id": 12345678901234567,
        "vehicle_id": 987654321,
        "vin": "5YJSA1E26MF123456",
        "display_name": "My Tesla",
        "state": "online",
        "api_version": 18,
        "in_service": false
    }"#;

    #[tokio::test]
    async fn list_products_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .and(matchers::header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": [serde_json::from_str::<serde_json::Value>(EXPECTED_VEHICLE_JSON).unwrap()],
                "count": 1
            })))
            .mount(&server)
            .await;

        let vehicles = list_products("test-token", &server.uri()).await.unwrap();

        assert_eq!(vehicles.len(), 1);
        assert_eq!(vehicles[0].vin, "5YJSA1E26MF123456");
        assert_eq!(vehicles[0].display_name.as_deref(), Some("My Tesla"));
        assert_eq!(vehicles[0].state, "online");
        assert_eq!(vehicles[0].api_version, 18);
    }

    #[tokio::test]
    async fn list_products_multiple_vehicles() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": [
                    serde_json::from_str::<serde_json::Value>(EXPECTED_VEHICLE_JSON).unwrap(),
                    {
                        "id": 9999999,
                        "vehicle_id": 111111111,
                        "vin": "5YJRE11234A567890",
                        "display_name": null,
                        "state": "asleep",
                        "api_version": 18,
                        "in_service": false
                    }
                ],
                "count": 2
            })))
            .mount(&server)
            .await;

        let vehicles = list_products("any-token", &server.uri()).await.unwrap();

        assert_eq!(vehicles.len(), 2);
        assert_eq!(vehicles[0].vin, "5YJSA1E26MF123456");
        assert_eq!(vehicles[1].vin, "5YJRE11234A567890");
        assert!(vehicles[1].display_name.is_none());
        assert_eq!(vehicles[1].state, "asleep");
    }

    #[tokio::test]
    async fn list_products_returns_empty_when_no_vehicles() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": [],
                "count": 0
            })))
            .mount(&server)
            .await;

        let vehicles = list_products("token", &server.uri()).await.unwrap();
        assert!(vehicles.is_empty());
    }

    #[tokio::test]
    async fn list_products_401_error() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let err = list_products("bad-token", &server.uri()).await.unwrap_err();

        match err {
            AuthError::Api { status, .. } => assert_eq!(status, 401),
            _ => panic!("expected Api error"),
        }
    }

    #[tokio::test]
    async fn list_products_500_error() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&server)
            .await;

        let err = list_products("token", &server.uri()).await.unwrap_err();

        match err {
            AuthError::Api { status, .. } => assert_eq!(status, 500),
            _ => panic!("expected Api error"),
        }
    }

    fn sample_vehicle_data_json() -> serde_json::Value {
        serde_json::json!({
            "response": {
                "state": "online",
                "drive_state": { "shift_state": "P", "speed": 0, "latitude": 37.77, "longitude": -122.42 },
                "charge_state": { "charging_state": "Disconnected", "battery_level": 80 },
                "vehicle_state": { "odometer": 50000.5, "sentry_mode": false, "locked": true },
                "climate_state": { "inside_temp": 23.0, "outside_temp": 15.0 },
                "software_update": { "status": "", "version": "2024.8" }
            }
        })
    }

    #[tokio::test]
    async fn get_vehicle_data_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .and(matchers::header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_vehicle_data_json()))
            .mount(&server)
            .await;

        let data = get_vehicle_data("test-token", &server.uri(), 12345)
            .await
            .unwrap();

        assert_eq!(data.state, "online");
        assert_eq!(data.drive_state.as_ref().unwrap().shift_state.as_deref(), Some("P"));
        assert_eq!(data.charge_state.as_ref().unwrap().battery_level, Some(80));
        assert_eq!(data.vehicle_state.as_ref().unwrap().odometer, Some(50000.5));
    }

    #[tokio::test]
    async fn get_vehicle_data_401() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let err = get_vehicle_data("bad", &server.uri(), 1).await.unwrap_err();
        match err {
            AuthError::Api { status, .. } => assert_eq!(status, 401),
            _ => panic!("expected Api error"),
        }
    }

    #[tokio::test]
    async fn get_vehicle_data_asleep() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": { "state": "asleep" }
            })))
            .mount(&server)
            .await;

        let data = get_vehicle_data("t", &server.uri(), 1).await.unwrap();
        assert_eq!(data.state, "asleep");
        assert!(data.drive_state.is_none());
    }

    #[tokio::test]
    async fn wake_up_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/wake_up"))
            .and(matchers::header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_vehicle_data_json()))
            .mount(&server)
            .await;

        let data = wake_up("test-token", &server.uri(), 12345).await.unwrap();
        assert_eq!(data.state, "online");
    }

    #[tokio::test]
    async fn wake_up_408_timeout() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/wake_up"))
            .respond_with(ResponseTemplate::new(408))
            .mount(&server)
            .await;

        let err = wake_up("t", &server.uri(), 1).await.unwrap_err();
        match err {
            AuthError::Api { status, .. } => assert_eq!(status, 408),
            _ => panic!("expected Api error"),
        }
    }
}
