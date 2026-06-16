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

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub async fn list_products(
    access_token: &str,
    api_url: &str,
) -> Result<Vec<Vehicle>, crate::tesla_auth::AuthError> {
    let http_client = reqwest::Client::new();
    let url = format!("{}/api/1/products", api_url.trim_end_matches('/'));
    let resp = http_client
        .get(&url)
        .bearer_auth(access_token)
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

// ---------------------------------------------------------------------------
// Vehicle Data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleDataResponse {
    pub state: String,
    #[serde(default)]
    pub odometer: Option<f64>,
    #[serde(default, rename = "drive_state")]
    pub drive_state: Option<DriveState>,
    #[serde(default, rename = "charge_state")]
    pub charge_state: Option<ChargeState>,
    #[serde(default, rename = "climate_state")]
    pub climate_state: Option<ClimateState>,
    #[serde(default, rename = "vehicle_state")]
    pub vehicle_state: Option<VehicleStateData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveState {
    #[serde(default)]
    pub shift_state: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    #[serde(default)]
    pub heading: Option<i64>,
    #[serde(default)]
    pub power: Option<i64>,
    #[serde(default)]
    pub elevation: Option<f64>,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChargeState {
    #[serde(default)]
    pub battery_level: Option<i64>,
    #[serde(default)]
    pub battery_range: Option<f64>,
    #[serde(default)]
    pub ideal_battery_range: Option<f64>,
    #[serde(default)]
    pub est_battery_range: Option<f64>,
    #[serde(default)]
    pub usable_battery_level: Option<i64>,
    #[serde(default)]
    pub battery_heater_on: Option<bool>,
    #[serde(default)]
    pub charging_state: Option<String>,
    #[serde(default)]
    pub charge_energy_added: Option<f64>,
    #[serde(default)]
    pub charger_actual_current: Option<i64>,
    #[serde(default)]
    pub charger_voltage: Option<i64>,
    #[serde(default)]
    pub charger_power: Option<i64>,
    #[serde(default)]
    pub charger_phases: Option<i64>,
    #[serde(default)]
    pub fast_charger_brand: Option<String>,
    #[serde(default)]
    pub fast_charger_type: Option<String>,
    #[serde(default)]
    pub conn_charge_cable: Option<String>,
    #[serde(default)]
    pub charge_limit_soc: Option<i64>,
    #[serde(default)]
    pub time_to_full_charge: Option<f64>,
    #[serde(default)]
    pub charger_pilot_current: Option<i64>,
    #[serde(default)]
    pub fast_charger_present: Option<bool>,
    #[serde(default)]
    pub not_enough_power_to_heat: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClimateState {
    #[serde(default)]
    pub inside_temp: Option<f64>,
    #[serde(default)]
    pub outside_temp: Option<f64>,
    #[serde(default)]
    pub fan_status: Option<i64>,
    #[serde(default)]
    pub is_front_defroster_on: Option<bool>,
    #[serde(default)]
    pub is_rear_defroster_on: Option<bool>,
    #[serde(default)]
    pub is_climate_on: Option<bool>,
    #[serde(default)]
    pub driver_temp_setting: Option<f64>,
    #[serde(default)]
    pub passenger_temp_setting: Option<f64>,
    #[serde(default)]
    pub battery_heater: Option<bool>,
    #[serde(default)]
    pub battery_heater_no_power: Option<bool>,
    #[serde(default)]
    pub is_preconditioning: Option<bool>,
    #[serde(default, rename = "climate_keeper_mode")]
    pub climate_keeper_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleStateData {
    #[serde(default)]
    pub tpms_pressure_fl: Option<f64>,
    #[serde(default)]
    pub tpms_pressure_fr: Option<f64>,
    #[serde(default)]
    pub tpms_pressure_rl: Option<f64>,
    #[serde(default)]
    pub tpms_pressure_rr: Option<f64>,
    #[serde(default)]
    pub car_version: Option<String>,
    #[serde(default)]
    pub software_update: Option<SoftwareUpdate>,
    #[serde(default)]
    pub sentry_mode: Option<bool>,
    #[serde(default)]
    pub is_user_present: Option<bool>,
    #[serde(default)]
    pub df: Option<f64>,
    #[serde(default)]
    pub pf: Option<f64>,
    #[serde(default)]
    pub dr: Option<f64>,
    #[serde(default)]
    pub pr: Option<f64>,
    #[serde(default)]
    pub ft: Option<f64>,
    #[serde(default)]
    pub rt: Option<f64>,
    #[serde(default)]
    pub locked: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftwareUpdate {
    #[serde(default)]
    pub download_perc: Option<i64>,
    #[serde(default)]
    pub expected_duration_sec: Option<i64>,
    #[serde(default)]
    pub install_perc: Option<i64>,
    #[serde(default)]
    pub scheduled_time_ms: Option<i64>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

pub async fn fetch_vehicle_data(
    access_token: &str,
    api_url: &str,
    vehicle_id: i64,
) -> Result<VehicleDataResponse, crate::tesla_auth::AuthError> {
    let http_client = reqwest::Client::new();
    let url = format!(
        "{}/api/1/vehicles/{}/vehicle_data",
        api_url.trim_end_matches('/'),
        vehicle_id
    );
    let resp = http_client
        .get(&url)
        .bearer_auth(access_token)
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
            body: format!("invalid /api/1/vehicles/{{id}}/vehicle_data response: {e}"),
        }
    })
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

    const VEHICLE_DATA_RESPONSE: &str = r#"{
        "response": {
            "id": 12345,
            "state": "online",
            "odometer": 50000.5,
            "drive_state": {
                "shift_state": "D",
                "speed": 65.0,
                "latitude": 37.7749,
                "longitude": -122.4194,
                "heading": 180,
                "power": 12000,
                "elevation": 10.0,
                "timestamp": 1700000000000
            },
            "charge_state": {
                "battery_level": 85,
                "battery_range": 270.0,
                "ideal_battery_range": 300.0,
                "est_battery_range": 260.0,
                "usable_battery_level": 82,
                "battery_heater_on": false,
                "charging_state": "Disconnected",
                "charge_energy_added": 0.0,
                "charger_actual_current": 0,
                "charger_voltage": 0,
                "charger_power": 0,
                "charger_phases": null,
                "fast_charger_brand": null,
                "fast_charger_type": null,
                "conn_charge_cable": null,
                "charge_limit_soc": 90,
                "time_to_full_charge": null
            },
            "climate_state": {
                "inside_temp": 24.0,
                "outside_temp": 22.5,
                "fan_status": 5,
                "is_front_defroster_on": false,
                "is_rear_defroster_on": false,
                "is_climate_on": true,
                "driver_temp_setting": 22.0,
                "passenger_temp_setting": 22.0,
                "battery_heater": false,
                "battery_heater_no_power": false
            },
            "vehicle_state": {
                "tpms_pressure_fl": 42.0,
                "tpms_pressure_fr": 41.5,
                "tpms_pressure_rl": 40.0,
                "tpms_pressure_rr": 40.5
            }
        }
    }"#;

    #[tokio::test]
    async fn fetch_vehicle_data_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .and(matchers::header("authorization", "Bearer test-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(VEHICLE_DATA_RESPONSE, "application/json"),
            )
            .mount(&server)
            .await;

        let data = fetch_vehicle_data("test-token", &server.uri(), 12345)
            .await
            .unwrap();

        assert_eq!(data.state, "online");
        assert_eq!(data.odometer, Some(50000.5));

        let ds = data.drive_state.unwrap();
        assert_eq!(ds.shift_state.as_deref(), Some("D"));
        assert_eq!(ds.speed, Some(65.0));
        assert_eq!(ds.latitude, Some(37.7749));
        assert_eq!(ds.longitude, Some(-122.4194));
        assert_eq!(ds.heading, Some(180));
        assert_eq!(ds.power, Some(12000));
        assert_eq!(ds.elevation, Some(10.0));
        assert_eq!(ds.timestamp, Some(1700000000000));

        let cs = data.charge_state.unwrap();
        assert_eq!(cs.battery_level, Some(85));
        assert_eq!(cs.battery_range, Some(270.0));
        assert_eq!(cs.ideal_battery_range, Some(300.0));
        assert_eq!(cs.est_battery_range, Some(260.0));
        assert_eq!(cs.usable_battery_level, Some(82));
        assert_eq!(cs.battery_heater_on, Some(false));
        assert_eq!(cs.charging_state.as_deref(), Some("Disconnected"));
        assert_eq!(cs.charge_energy_added, Some(0.0));
        assert_eq!(cs.charger_actual_current, Some(0));
        assert_eq!(cs.charger_voltage, Some(0));
        assert_eq!(cs.charger_power, Some(0));
        assert_eq!(cs.charge_limit_soc, Some(90));
        assert!(cs.time_to_full_charge.is_none());

        let cl = data.climate_state.unwrap();
        assert_eq!(cl.inside_temp, Some(24.0));
        assert_eq!(cl.outside_temp, Some(22.5));
        assert_eq!(cl.fan_status, Some(5));
        assert_eq!(cl.is_front_defroster_on, Some(false));
        assert_eq!(cl.is_rear_defroster_on, Some(false));
        assert_eq!(cl.is_climate_on, Some(true));
        assert_eq!(cl.driver_temp_setting, Some(22.0));
        assert_eq!(cl.passenger_temp_setting, Some(22.0));
        assert_eq!(cl.battery_heater, Some(false));
        assert_eq!(cl.battery_heater_no_power, Some(false));

        let vs = data.vehicle_state.unwrap();
        assert_eq!(vs.tpms_pressure_fl, Some(42.0));
        assert_eq!(vs.tpms_pressure_fr, Some(41.5));
        assert_eq!(vs.tpms_pressure_rl, Some(40.0));
        assert_eq!(vs.tpms_pressure_rr, Some(40.5));
    }

    #[tokio::test]
    async fn fetch_vehicle_data_null_sub_objects() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 12345,
                    "state": "online",
                    "odometer": 100.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.0,
                        "longitude": -122.0,
                        "heading": null,
                        "power": null,
                        "elevation": null,
                        "timestamp": 1700000000000i64
                    },
                    "charge_state": null,
                    "climate_state": null,
                    "vehicle_state": null
                }
            })))
            .mount(&server)
            .await;

        let data = fetch_vehicle_data("token", &server.uri(), 12345)
            .await
            .unwrap();

        assert_eq!(data.state, "online");
        assert!(data.charge_state.is_none());
        assert!(data.climate_state.is_none());
        assert!(data.vehicle_state.is_none());
    }

    #[tokio::test]
    async fn fetch_vehicle_data_asleep() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 12345,
                    "state": "asleep",
                    "odometer": null,
                    "drive_state": null
                }
            })))
            .mount(&server)
            .await;

        let data = fetch_vehicle_data("token", &server.uri(), 12345)
            .await
            .unwrap();

        assert_eq!(data.state, "asleep");
        assert!(data.odometer.is_none());
        assert!(data.drive_state.is_none());
    }

    #[tokio::test]
    async fn fetch_vehicle_data_401_error() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let err = fetch_vehicle_data("bad-token", &server.uri(), 12345)
            .await
            .unwrap_err();

        match err {
            AuthError::Api { status, .. } => assert_eq!(status, 401),
            _ => panic!("expected Api error"),
        }
    }

    #[tokio::test]
    async fn fetch_vehicle_data_500_error() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&server)
            .await;

        let err = fetch_vehicle_data("token", &server.uri(), 12345)
            .await
            .unwrap_err();

        match err {
            AuthError::Api { status, .. } => assert_eq!(status, 500),
            _ => panic!("expected Api error"),
        }
    }

    #[tokio::test]
    async fn fetch_vehicle_data_no_drive_state_returns_none() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 12345,
                    "state": "online",
                    "odometer": 100.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": null,
                        "longitude": null,
                        "heading": null,
                        "power": null,
                        "elevation": null,
                        "timestamp": null
                    }
                }
            })))
            .mount(&server)
            .await;

        let data = fetch_vehicle_data("token", &server.uri(), 12345)
            .await
            .unwrap();

        let ds = data.drive_state.unwrap();
        assert!(ds.shift_state.is_none());
        assert!(ds.latitude.is_none());
        assert!(ds.longitude.is_none());
        assert!(ds.speed.is_none());
        assert!(ds.power.is_none());
    }
}
