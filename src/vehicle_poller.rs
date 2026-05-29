use crate::tesla_api::VehicleData;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum VehicleState {
    Start,
    Online,
    Driving,
    Charging,
    Updating,
    Asleep,
    Offline,
}

impl fmt::Display for VehicleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VehicleState::Start => write!(f, "start"),
            VehicleState::Online => write!(f, "online"),
            VehicleState::Driving => write!(f, "driving"),
            VehicleState::Charging => write!(f, "charging"),
            VehicleState::Updating => write!(f, "updating"),
            VehicleState::Asleep => write!(f, "asleep"),
            VehicleState::Offline => write!(f, "offline"),
        }
    }
}

/// Classify a vehicle's state based on the latest `vehicle_data` response.
///
/// Priority order (highest first):
///   1. Updating  — software update in progress
///   2. Charging  — charge_state.charging_state == "Charging"
///   3. Driving   — shift_state is D, R, or N
///   4. Asleep    — vehicle state is "asleep"
///   5. Offline   — vehicle state is "offline"
///   6. Online    — everything else
pub fn classify_vehicle_state(data: &VehicleData) -> VehicleState {
    match data.state.as_str() {
        "online" => {
            // Updating takes priority
            if let Some(ref sw) = data.software_update
                && sw.status.as_deref() == Some("installing")
            {
                return VehicleState::Updating;
            }
            // Charging
            if let Some(ref cs) = data.charge_state
                && cs.charging_state.as_deref() == Some("Charging")
            {
                return VehicleState::Charging;
            }
            // Driving
            if let Some(ref ds) = data.drive_state {
                match ds.shift_state.as_deref() {
                    Some("D") | Some("R") | Some("N") => return VehicleState::Driving,
                    _ => {}
                }
            }
            VehicleState::Online
        }
        "asleep" => VehicleState::Asleep,
        "offline" => VehicleState::Offline,
        _ => VehicleState::Online,
    }
}

// ---------------------------------------------------------------------------
// Per-vehicle poll task
// ---------------------------------------------------------------------------

pub async fn vehicle_poll_task(
    vehicle: crate::tesla_api::Vehicle,
    current_token: Arc<RwLock<String>>,
    api_url: String,
    poll_interval: Duration,
) {
    let mut current_state = VehicleState::Start;
    let mut interval = tokio::time::interval(poll_interval);
    interval.tick().await;

    loop {
        interval.tick().await;

        let access_token = current_token.read().unwrap().clone();
        if access_token.is_empty() {
            continue;
        }

        match crate::tesla_api::get_vehicle_data(&access_token, &api_url, vehicle.id).await {
            Ok(data) => {
                let new_state = classify_vehicle_state(&data);
                debug!(
                    vin = %vehicle.vin,
                    state = %new_state,
                    shift_state = ?data.drive_state.as_ref().and_then(|d| d.shift_state.as_deref()),
                    battery = data.charge_state.as_ref().and_then(|c| c.battery_level),
                    charging = ?data.charge_state.as_ref().and_then(|c| c.charging_state.as_deref()),
                    "vehicle_data poll complete"
                );
                if new_state != current_state {
                    info!(
                        vin = %vehicle.vin,
                        from = %current_state,
                        to = %new_state,
                        "vehicle state transition"
                    );
                    current_state = new_state;
                }
            }
            Err(e) => {
                warn!(vin = %vehicle.vin, error = %e, "vehicle_data poll failed");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

/// Spawn one `vehicle_poll_task` per vehicle.
pub fn spawn_vehicle_tasks(
    vehicles: Arc<HashMap<String, crate::tesla_api::Vehicle>>,
    current_token: Arc<RwLock<String>>,
    api_url: String,
    poll_interval: Duration,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();
    for vehicle in vehicles.values() {
        let vehicle = vehicle.clone();
        let token = Arc::clone(&current_token);
        let url = api_url.clone();

        let handle = tokio::spawn(async move {
            vehicle_poll_task(vehicle, token, url, poll_interval).await;
        });
        handles.push(handle);
    }
    info!(count = handles.len(), "vehicle poll tasks spawned");
    handles
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tesla_api::{
        ChargeState, ClimateState, DriveState, SoftwareState, VehicleData, VehicleSubState,
    };

    fn online_data() -> VehicleData {
        VehicleData {
            state: "online".into(),
            drive_state: Some(DriveState {
                shift_state: Some("P".into()),
                ..Default::default()
            }),
            charge_state: Some(ChargeState {
                charging_state: Some("Disconnected".into()),
                ..Default::default()
            }),
            climate_state: Some(ClimateState::default()),
            vehicle_state: Some(VehicleSubState::default()),
            software_update: None,
        }
    }

    #[test]
    fn classify_online_when_awake_and_parked() {
        let data = online_data();
        assert_eq!(classify_vehicle_state(&data), VehicleState::Online);
    }

    #[test]
    fn classify_driving_when_in_drive() {
        let mut data = online_data();
        data.drive_state.as_mut().unwrap().shift_state = Some("D".into());
        assert_eq!(classify_vehicle_state(&data), VehicleState::Driving);
    }

    #[test]
    fn classify_driving_when_in_reverse() {
        let mut data = online_data();
        data.drive_state.as_mut().unwrap().shift_state = Some("R".into());
        assert_eq!(classify_vehicle_state(&data), VehicleState::Driving);
    }

    #[test]
    fn classify_driving_when_in_neutral() {
        let mut data = online_data();
        data.drive_state.as_mut().unwrap().shift_state = Some("N".into());
        assert_eq!(classify_vehicle_state(&data), VehicleState::Driving);
    }

    #[test]
    fn classify_park_is_not_driving() {
        let mut data = online_data();
        data.drive_state.as_mut().unwrap().shift_state = Some("P".into());
        assert_eq!(classify_vehicle_state(&data), VehicleState::Online);
    }

    #[test]
    fn classify_charging_when_charging() {
        let mut data = online_data();
        data.charge_state.as_mut().unwrap().charging_state = Some("Charging".into());
        assert_eq!(classify_vehicle_state(&data), VehicleState::Charging);
    }

    #[test]
    fn classify_asleep() {
        let data = VehicleData {
            state: "asleep".into(),
            ..Default::default()
        };
        assert_eq!(classify_vehicle_state(&data), VehicleState::Asleep);
    }

    #[test]
    fn classify_offline() {
        let data = VehicleData {
            state: "offline".into(),
            ..Default::default()
        };
        assert_eq!(classify_vehicle_state(&data), VehicleState::Offline);
    }

    #[test]
    fn classify_updating() {
        let mut data = online_data();
        data.software_update = Some(SoftwareState {
            status: Some("installing".into()),
            version: Some("2024.8".into()),
            ..Default::default()
        });
        assert_eq!(classify_vehicle_state(&data), VehicleState::Updating);
    }

    #[test]
    fn classify_charging_beats_driving() {
        let mut data = online_data();
        data.charge_state.as_mut().unwrap().charging_state = Some("Charging".into());
        data.drive_state.as_mut().unwrap().shift_state = Some("P".into());
        assert_eq!(classify_vehicle_state(&data), VehicleState::Charging);
    }

    #[test]
    fn classify_updating_beats_charging() {
        let mut data = online_data();
        data.charge_state.as_mut().unwrap().charging_state = Some("Charging".into());
        data.software_update = Some(SoftwareState {
            status: Some("installing".into()),
            version: Some("2024.14".into()),
            ..Default::default()
        });
        assert_eq!(classify_vehicle_state(&data), VehicleState::Updating);
    }

    #[test]
    fn classify_unknown_state_defaults_to_online() {
        let data = VehicleData {
            state: "unexpected".into(),
            ..Default::default()
        };
        assert_eq!(classify_vehicle_state(&data), VehicleState::Online);
    }

    #[test]
    fn classify_null_shift_state_is_not_driving() {
        let mut data = online_data();
        data.drive_state.as_mut().unwrap().shift_state = None;
        assert_eq!(classify_vehicle_state(&data), VehicleState::Online);
    }

    #[test]
    fn display_vehicle_state() {
        assert_eq!(VehicleState::Start.to_string(), "start");
        assert_eq!(VehicleState::Online.to_string(), "online");
        assert_eq!(VehicleState::Driving.to_string(), "driving");
        assert_eq!(VehicleState::Charging.to_string(), "charging");
        assert_eq!(VehicleState::Updating.to_string(), "updating");
        assert_eq!(VehicleState::Asleep.to_string(), "asleep");
        assert_eq!(VehicleState::Offline.to_string(), "offline");
    }

    #[test]
    fn classify_null_drive_state_handled() {
        let mut data = online_data();
        data.drive_state = None;
        assert_eq!(classify_vehicle_state(&data), VehicleState::Online);
    }

    #[test]
    fn classify_null_charge_state_handled() {
        let mut data = online_data();
        data.charge_state = None;
        assert_eq!(classify_vehicle_state(&data), VehicleState::Online);
    }

    #[test]
    fn classify_charging_complete_is_not_charging() {
        let mut data = online_data();
        data.charge_state.as_mut().unwrap().charging_state = Some("Complete".into());
        assert_eq!(classify_vehicle_state(&data), VehicleState::Online);
    }

    #[test]
    fn classify_null_software_update_handled() {
        let data = online_data();
        assert!(data.software_update.is_none());
        assert_eq!(classify_vehicle_state(&data), VehicleState::Online);
    }

    #[test]
    fn classify_installing_status_triggers_updating() {
        let mut data = online_data();
        data.software_update = Some(SoftwareState {
            status: Some("installing".into()),
            version: None,
            ..Default::default()
        });
        assert_eq!(classify_vehicle_state(&data), VehicleState::Updating);
    }

    #[test]
    fn classify_empty_status_not_updating() {
        let mut data = online_data();
        data.software_update = Some(SoftwareState {
            status: Some("".into()),
            ..Default::default()
        });
        assert_eq!(classify_vehicle_state(&data), VehicleState::Online);
    }

    // -----------------------------------------------------------------------
    // Integration: poll loop calls vehicle_data via HTTP
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn poll_task_makes_http_requests() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let server = MockServer::start().await;
        let json = serde_json::json!({
            "response": {
                "state": "online",
                "drive_state": { "shift_state": "P" },
                "charge_state": { "charging_state": "Disconnected" },
                "software_update": { "status": "" },
                "vehicle_state": {},
                "climate_state": {}
            }
        });
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json))
            .mount(&server)
            .await;

        let vehicle = crate::tesla_api::Vehicle {
            id: 12345,
            vehicle_id: 98765,
            vin: "TESTVIN".into(),
            display_name: None,
            state: "online".into(),
            api_version: 18,
            in_service: false,
        };

        let token = Arc::new(RwLock::new("bearer-token".to_string()));

        let handle = tokio::spawn(vehicle_poll_task(
            vehicle,
            Arc::clone(&token),
            server.uri(),
            Duration::from_millis(200),
        ));

        // Let the task run for several ticks
        tokio::time::sleep(Duration::from_millis(800)).await;

        handle.abort();

        let requests = server.received_requests().await.unwrap();
        let count = requests
            .iter()
            .filter(|r| r.url.path().contains("vehicle_data"))
            .count();
        assert!(count >= 2, "expected at least 2 vehicle_data calls, got {count}");
    }

    #[tokio::test]
    async fn poll_task_survives_empty_token() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": { "state": "online" }
            })))
            .mount(&server)
            .await;

        let vehicle = crate::tesla_api::Vehicle {
            id: 1, vehicle_id: 1, vin: "TOKENLESS".into(),
            display_name: None, state: "online".into(), api_version: 18, in_service: false,
        };
        let token = Arc::new(RwLock::new(String::new()));
        let handle = tokio::spawn(vehicle_poll_task(
            vehicle, Arc::clone(&token), server.uri(), Duration::from_millis(50),
        ));

        tokio::time::sleep(Duration::from_millis(150)).await;

        // Confirm no calls were made (task should skip with empty token)
        let before = server.received_requests().await.unwrap().len();
        assert_eq!(before, 0, "no calls expected with empty token");

        // Update the token — next tick should send requests
        *token.write().unwrap() = "fresh-token".to_string();
        tokio::time::sleep(Duration::from_millis(200)).await;

        handle.abort();
        let after = server.received_requests().await.unwrap().len();
        assert!(after >= 1, "expected requests after token was set, got {after}");
    }

    #[tokio::test]
    async fn spawn_vehicle_tasks_creates_one_task_per_vehicle() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};
        use std::collections::HashMap;

        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path_regex(r"/api/1/vehicles/\d+/vehicle_data"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": { "state": "online" }
            })))
            .mount(&server)
            .await;

        let cars: HashMap<String, crate::tesla_api::Vehicle> = [
            ("VIN1".into(), crate::tesla_api::Vehicle {
                id: 1, vehicle_id: 11, vin: "VIN1".into(),
                display_name: None, state: "online".into(), api_version: 18, in_service: false,
            }),
            ("VIN2".into(), crate::tesla_api::Vehicle {
                id: 2, vehicle_id: 22, vin: "VIN2".into(),
                display_name: None, state: "asleep".into(), api_version: 18, in_service: false,
            }),
        ].into();

        let token = Arc::new(RwLock::new("tok".to_string()));
        let handles = spawn_vehicle_tasks(
            Arc::new(cars),
            token,
            server.uri(),
            Duration::from_millis(100),
        );
        assert_eq!(handles.len(), 2);

        tokio::time::sleep(Duration::from_millis(400)).await;

        for h in handles { h.abort(); }

        let requests = server.received_requests().await.unwrap();
        let vids: std::collections::HashSet<_> = requests
            .iter()
            .filter_map(|r| {
                let p = r.url.path();
                // /api/1/vehicles/{id}/vehicle_data
                p.split('/').nth(4).map(|s| s.to_string())
            })
            .collect();
        assert!(vids.contains("1"), "expected vehicle id 1 to be polled, got {vids:?}");
        assert!(vids.contains("2"), "expected vehicle id 2 to be polled, got {vids:?}");
    }

    #[tokio::test]
    async fn vehicle_data_deserializes_tesla_api_format() {
        let json = serde_json::json!({
            "response": {
                "state": "online",
                "drive_state": {
                    "shift_state": null,
                    "speed": null,
                    "power": 0,
                    "latitude": 37.7749,
                    "longitude": -122.4194,
                    "heading": 180,
                    "timestamp": 1717000000
                },
                "charge_state": {
                    "charging_state": "Disconnected",
                    "battery_level": 85,
                    "battery_range": 270.3,
                    "est_battery_range": 265.1,
                    "charger_power": 0,
                    "charger_voltage": null,
                    "charger_phases": null,
                    "charge_energy_added": null,
                    "charge_limit_soc": 90,
                    "time_to_full_charge": null,
                    "charge_port_door_open": false,
                    "fast_charger_brand": null
                },
                "vehicle_state": {
                    "odometer": 50000.5,
                    "sentry_mode": false,
                    "locked": true,
                    "car_version": "2024.8.10"
                },
                "climate_state": {
                    "inside_temp": 23.0,
                    "outside_temp": 15.5,
                    "is_climate_on": false,
                    "fan_status": 0,
                    "defrost_mode": 0
                },
                "software_update": {
                    "status": "",
                    "version": "2024.8.10",
                    "expected_duration_sec": 2700,
                    "install_time": null
                }
            }
        });

        let wrapper: crate::tesla_api::VehicleDataResponse =
            serde_json::from_value(json).unwrap();
        let data = wrapper.response;

        assert_eq!(data.state, "online");
        assert_eq!(data.drive_state.as_ref().unwrap().latitude, Some(37.7749));
        assert_eq!(data.charge_state.as_ref().unwrap().battery_level, Some(85));
        assert_eq!(data.vehicle_state.as_ref().unwrap().odometer, Some(50000.5));
        assert_eq!(data.climate_state.as_ref().unwrap().outside_temp, Some(15.5));
        assert_eq!(data.software_update.as_ref().unwrap().version.as_deref(), Some("2024.8.10"));
    }
}
