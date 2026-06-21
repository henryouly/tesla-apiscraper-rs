use super::*;
use crate::config_yaml::{BillingConfig, BillingType, Geofence, YamlConfigManager};
use crate::influxdb::InfluxDb;
use crate::tesla_api::Vehicle;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

fn test_vehicle() -> Vehicle {
    Vehicle {
        id: 1,
        vehicle_id: 100,
        vin: "TESTVIN000000001".into(),
        display_name: Some("Test Car".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    }
}

fn test_api_url() -> String {
    "http://localhost:1".into()
}

fn test_db() -> Arc<InfluxDb> {
    Arc::new(InfluxDb::new("http://localhost:1", "none", "test").unwrap())
}

fn test_settings() -> Arc<Mutex<YamlConfigManager>> {
    let dir = std::env::temp_dir().join("tesla-test-vehicles").join(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string(),
    );
    Arc::new(Mutex::new(YamlConfigManager::load(&dir).unwrap()))
}

// -----------------------------------------------------------------------
// Vehicles supervisor
// -----------------------------------------------------------------------

#[test]
fn new_supervisor_is_empty() {
    let vm = Vehicles::new("http://localhost:1");
    assert_eq!(vm.state_of("any"), None);
}

#[tokio::test]
async fn spawn_one_then_remove() {
    let mut vm = Vehicles::new(&test_api_url());
    let vehicle = test_vehicle();
    let vin = vehicle.vin.clone();
    let (_, token_rx) = watch::channel(Some("token".into()));

    vm.spawn_one(
        vehicle,
        test_db(),
        token_rx,
        test_settings(),
        Duration::from_secs(30),
    );

    assert!(vm.state_of(&vin).is_some());

    assert!(vm.send_cmd(&vin, VehicleCommand::Shutdown));

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
}

#[tokio::test]
async fn spawn_all_skips_existing() {
    let mut vm = Vehicles::new(&test_api_url());
    let vehicle = test_vehicle();
    let vin = vehicle.vin.clone();
    let (_, token_rx) = watch::channel(Some("token".into()));

    vm.spawn_one(
        vehicle.clone(),
        test_db(),
        token_rx,
        test_settings(),
        Duration::from_secs(30),
    );

    let mut vehicles = HashMap::new();
    vehicles.insert(vin.clone(), vehicle);
    let (_, token_rx2) = watch::channel(Some("token".into()));
    let count = vm.spawn_all(
        &vehicles,
        test_db(),
        token_rx2,
        test_settings(),
        Duration::from_secs(30),
    );
    assert_eq!(count, 0, "should not spawn already-tracked vehicle");

    vm.shutdown_all();
}

#[tokio::test]
async fn send_cmd_to_unknown_vin_returns_false() {
    let vm = Vehicles::new("http://localhost:1");
    assert!(!vm.send_cmd("UNKNOWN", VehicleCommand::Shutdown));
}

#[tokio::test]
async fn state_tracks_suspend_resume() {
    let mut vm = Vehicles::new(&test_api_url());
    let vehicle = test_vehicle();
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        test_db(),
        token_rx,
        test_settings(),
        Duration::from_millis(10),
    );

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));

    vm.send_cmd(&vin, VehicleCommand::Suspend);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(vm.state_of(&vin), Some(VehicleState::Suspended));

    vm.send_cmd(&vin, VehicleCommand::Resume);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
}

// -----------------------------------------------------------------------
// Polling integration tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn poll_transitions_to_asleep() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 1,
                    "state": "asleep",
                    "odometer": null,
                    "drive_state": null
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 1,
        vehicle_id: 100,
        vin: "POLLTEST01".into(),
        display_name: Some("Poll Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    // Wait for a few poll ticks
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Asleep));

    vm.shutdown_all();
}

#[tokio::test]
async fn poll_writes_position_on_tick() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 2,
                    "state": "online",
                    "odometer": 50000.0,
                    "drive_state": {
                        "shift_state": "D",
                        "speed": 55.0,
                        "latitude": 37.7749,
                        "longitude": -122.4194,
                        "heading": 180,
                        "power": 8000,
                        "elevation": 15.0,
                        "timestamp": 1700000100000i64
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 2,
        vehicle_id: 200,
        vin: "POLLTEST02".into(),
        display_name: Some("Poll Write Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Driving));

    vm.shutdown_all();
}

#[tokio::test]
async fn poll_skips_unchanged_position() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 3,
                    "state": "online",
                    "odometer": 50000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": 0.0,
                        "latitude": 37.7749,
                        "longitude": -122.4194,
                        "heading": 180,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700000100000i64
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    // Catch-all: handle non-position writes.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    // Specific: match only position writes via the car_id tag (higher priority).
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("car_id="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 3,
        vehicle_id: 300,
        vin: "DEDUP0001".into(),
        display_name: Some("Dedup Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(300)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn poll_retries_after_write_failure() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 5,
                    "state": "online",
                    "odometer": 50000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.7749,
                        "longitude": -122.4194,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700000100000i64
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    // Always return 500 — every tick should still retry (at least 2 attempts)
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .expect(2..)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 5,
        vehicle_id: 500,
        vin: "RETRY001".into(),
        display_name: Some("Retry Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // .expect(2) verifies both ticks attempted a write
    // despite both failing — dedup did NOT skip the second
    vm.shutdown_all();
}

#[tokio::test]
async fn poll_position_includes_all_fields() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 4,
                    "state": "online",
                    "odometer": 50000.0,
                    "drive_state": {
                        "shift_state": "D",
                        "speed": 55.0,
                        "latitude": 37.7749,
                        "longitude": -122.4194,
                        "heading": 180,
                        "power": 8000,
                        "elevation": 15.0,
                        "timestamp": 1700000100000i64
                    },
                    "charge_state": {
                        "battery_level": 85,
                        "battery_range": 270.0,
                        "ideal_battery_range": 300.0,
                        "est_battery_range": 260.0,
                        "usable_battery_level": 82,
                        "battery_heater_on": false
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
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    // Mock only matches if the body contains all the new fields
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            "battery_level=85i",
        ))
        .and(wiremock::matchers::body_string_contains(
            "rated_battery_range_km=270",
        ))
        .and(wiremock::matchers::body_string_contains(
            "ideal_battery_range_km=300",
        ))
        .and(wiremock::matchers::body_string_contains(
            "est_battery_range_km=260",
        ))
        .and(wiremock::matchers::body_string_contains(
            "usable_battery_level=82i",
        ))
        .and(wiremock::matchers::body_string_contains("inside_temp=24"))
        .and(wiremock::matchers::body_string_contains(
            "outside_temp=22.5",
        ))
        .and(wiremock::matchers::body_string_contains("fan_status=5i"))
        .and(wiremock::matchers::body_string_contains(
            "is_front_defroster_on=false",
        ))
        .and(wiremock::matchers::body_string_contains(
            "is_rear_defroster_on=false",
        ))
        .and(wiremock::matchers::body_string_contains(
            "is_climate_on=true",
        ))
        .and(wiremock::matchers::body_string_contains(
            "driver_temp_setting=22",
        ))
        .and(wiremock::matchers::body_string_contains(
            "passenger_temp_setting=22",
        ))
        .and(wiremock::matchers::body_string_contains(
            "battery_heater=false",
        ))
        .and(wiremock::matchers::body_string_contains(
            "battery_heater_on=false",
        ))
        .and(wiremock::matchers::body_string_contains(
            "battery_heater_no_power=false",
        ))
        .and(wiremock::matchers::body_string_contains(
            "tpms_pressure_fl=42",
        ))
        .and(wiremock::matchers::body_string_contains(
            "tpms_pressure_fr=41.5",
        ))
        .and(wiremock::matchers::body_string_contains(
            "tpms_pressure_rl=40",
        ))
        .and(wiremock::matchers::body_string_contains(
            "tpms_pressure_rr=40.5",
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 4,
        vehicle_id: 400,
        vin: "FIELDS01".into(),
        display_name: Some("Fields Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // .expect(1) above verifies exactly one write matched
    vm.shutdown_all();
}

#[tokio::test]
async fn drive_starts_when_driving() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 10,
                    "state": "online",
                    "odometer": 60000.0,
                    "drive_state": {
                        "shift_state": "D",
                        "speed": 60.0,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": 90,
                        "power": 10000,
                        "elevation": 20.0,
                        "timestamp": 1700000200000i64
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    // Catch-all for position writes and other writes.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    // Specific: match only drive writes via the drive_id tag (higher priority).
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("drive_id="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 10,
        vehicle_id: 1000,
        vin: "DRIVE001".into(),
        display_name: Some("Drive Start Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // .expect(1) above verifies exactly one drive write matched
    vm.shutdown_all();
}

#[tokio::test]
async fn no_drive_writes_when_parked() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 11,
                    "state": "online",
                    "odometer": 61000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700000300000i64
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    // Catch-all for position writes.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    // Specific: match only drive writes (higher priority) — expect 0.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("drive_id="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(0)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 11,
        vehicle_id: 1100,
        vin: "PARKED01".into(),
        display_name: Some("Parked Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Vehicle should be Online, not Driving
    assert_eq!(vm.state_of("PARKED01"), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn charge_starts_when_charging() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 20,
                    "state": "online",
                    "odometer": 70000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700000400000i64
                    },
                    "charge_state": {
                        "battery_level": 55,
                        "battery_range": 150.0,
                        "ideal_battery_range": 180.0,
                        "charging_state": "Charging",
                        "charge_energy_added": 2.5,
                        "charger_actual_current": 32,
                        "charger_voltage": 230,
                        "charger_power": 7000,
                        "charger_phases": 3
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    // Catch-all for any writes
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    // Specific: match only charging_sessions writes via the charge_id tag (higher priority)
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("charge_id="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 20,
        vehicle_id: 2000,
        vin: "CHARGE01".into(),
        display_name: Some("Charge Start Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn no_charge_writes_when_disconnected() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 21,
                    "state": "online",
                    "odometer": 71000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700000500000i64
                    },
                    "charge_state": {
                        "battery_level": 80,
                        "battery_range": 220.0,
                        "ideal_battery_range": 250.0,
                        "charging_state": "Disconnected",
                        "charge_energy_added": 0.0,
                        "charger_actual_current": 0,
                        "charger_voltage": 0,
                        "charger_power": 0,
                        "charger_phases": null
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    // Catch-all for position writes
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    // Specific: match only charge writes (higher priority) — expect 0
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("charge_id="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(0)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 21,
        vehicle_id: 2100,
        vin: "DISCONN01".into(),
        display_name: Some("Disconnected Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of("DISCONN01"), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn charge_writes_reading_every_tick() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 22,
                    "state": "online",
                    "odometer": 72000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700000600000i64
                    },
                    "charge_state": {
                        "battery_level": 60,
                        "battery_range": 160.0,
                        "ideal_battery_range": 190.0,
                        "charging_state": "Charging",
                        "charge_energy_added": 5.0,
                        "charger_actual_current": 32,
                        "charger_voltage": 230,
                        "charger_power": 7000,
                        "charger_phases": 3,
                        "fast_charger_brand": "Tesla",
                        "fast_charger_type": "Supercharger",
                        "conn_charge_cable": "CCS"
                    },
                    "climate_state": {
                        "outside_temp": 22.5,
                        "inside_temp": 24.0
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    // Catch-all for any writes.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 22,
        vehicle_id: 2200,
        vin: "READING01".into(),
        display_name: Some("Charge Reading Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify we're in Charging state (confirms lifecycle started)
    assert_eq!(vm.state_of("READING01"), Some(VehicleState::Charging));
    vm.shutdown_all();
}

#[tokio::test]
async fn charge_ends_with_aggregated_write() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    // 3-response sequence: Charging → Charging → Complete
    let charging_resp_a = serde_json::json!({
        "response": {
            "id": 23,
            "state": "online",
            "odometer": 73000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700000700000i64
            },
            "charge_state": {
                "battery_level": 50,
                "battery_range": 130.0,
                "ideal_battery_range": 160.0,
                "charging_state": "Charging",
                "charge_energy_added": 0.0,
                "charger_actual_current": 32,
                "charger_voltage": 230,
                "charger_power": 7000,
                "charger_phases": 3,
                "conn_charge_cable": "CCS"
            },
            "climate_state": {
                "outside_temp": 20.0,
                "inside_temp": 22.0
            }
        }
    });

    let charging_resp_b = serde_json::json!({
        "response": {
            "id": 23,
            "state": "online",
            "odometer": 73000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700000705000i64
            },
            "charge_state": {
                "battery_level": 55,
                "battery_range": 145.0,
                "ideal_battery_range": 175.0,
                "charging_state": "Charging",
                "charge_energy_added": 2.0,
                "charger_actual_current": 32,
                "charger_voltage": 230,
                "charger_power": 7000,
                "charger_phases": 3,
                "conn_charge_cable": "CCS"
            },
            "climate_state": {
                "outside_temp": 20.0,
                "inside_temp": 22.0
            }
        }
    });

    let complete_resp = serde_json::json!({
        "response": {
            "id": 23,
            "state": "online",
            "odometer": 73000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700000700000i64
            },
            "charge_state": {
                "battery_level": 80,
                "battery_range": 220.0,
                "ideal_battery_range": 260.0,
                "charging_state": "Complete",
                "charge_energy_added": 15.0,
                "charger_actual_current": 0,
                "charger_voltage": 0,
                "charger_power": 0,
                "charger_phases": null
            },
            "climate_state": {
                "outside_temp": 20.0,
                "inside_temp": 22.0
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(charging_resp_a.clone())
            } else if count == 1 {
                wiremock::ResponseTemplate::new(200).set_body_json(charging_resp_b.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(complete_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    // Catch-all for any writes (unlimited).
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    // Assert at least one per-tick charge reading was written.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("charge_readings"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;
    // Assert the aggregated charge session write with duration/energy/battery.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            "charging_sessions",
        ))
        .and(wiremock::matchers::body_string_contains("energy_added_wh="))
        .and(wiremock::matchers::body_string_contains(
            "duration_seconds=",
        ))
        .and(wiremock::matchers::body_string_contains(
            "end_battery_level=80i",
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 23,
        vehicle_id: 2300,
        vin: "ENDCHRG01".into(),
        display_name: Some("Charge End Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    // Wait for START tick to fire (Response 0 → start session)
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First Suspend/Resume: force next tick at poll_interval.
    // Response 1 → ACCUMULATE (writes a charge_reading)
    assert!(vm.send_cmd("ENDCHRG01", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("ENDCHRG01", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second Suspend/Resume: next tick fires at poll_interval.
    // Response 2+ → Complete → END (writes aggregated charging_sessions)
    assert!(vm.send_cmd("ENDCHRG01", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("ENDCHRG01", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    vm.shutdown_all();
}

fn test_geofence_settings() -> Arc<Mutex<YamlConfigManager>> {
    let dir = std::env::temp_dir().join("tesla-test-geofences").join(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string(),
    );
    let mut mgr = YamlConfigManager::load(&dir).unwrap();
    mgr.geofences.geofences = vec![Geofence {
        name: "Home".into(),
        latitude: 37.8,
        longitude: -122.4,
        radius_meters: 100.0,
        billing: None,
    }];
    Arc::new(Mutex::new(mgr))
}

fn test_geofence_settings_with_billing(
    billing_type: BillingType,
    cost_per_unit: f64,
    session_fee: f64,
) -> Arc<Mutex<YamlConfigManager>> {
    let dir = std::env::temp_dir()
        .join("tesla-test-geofences-billing")
        .join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
    let mut mgr = YamlConfigManager::load(&dir).unwrap();
    mgr.geofences.geofences = vec![Geofence {
        name: "Home".into(),
        latitude: 37.8,
        longitude: -122.4,
        radius_meters: 100.0,
        billing: Some(BillingConfig {
            billing_type,
            cost_per_unit,
            session_fee,
        }),
    }];
    Arc::new(Mutex::new(mgr))
}

#[tokio::test]
async fn drive_close_with_geofence() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let drive_resp = serde_json::json!({
        "response": {
            "id": 30,
            "state": "online",
            "odometer": 75000.0,
            "drive_state": {
                "shift_state": "D",
                "speed": 50.0,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": 90,
                "power": 8000,
                "elevation": null,
                "timestamp": 1700000800000i64
            }
        }
    });

    let parked_resp = serde_json::json!({
        "response": {
            "id": 30,
            "state": "online",
            "odometer": 75001.0,
            "drive_state": {
                "shift_state": "P",
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": 90,
                "power": 0,
                "elevation": null,
                "timestamp": 1700000900000i64
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(drive_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(parked_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            "geofence_enter=\"Home\"",
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 30,
        vehicle_id: 3000,
        vin: "GEOFDRV1".into(),
        display_name: Some("Geofence Drive Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_geofence_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(vm.send_cmd("GEOFDRV1", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("GEOFDRV1", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn drive_close_without_geofence() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let drive_resp = serde_json::json!({
        "response": {
            "id": 31,
            "state": "online",
            "odometer": 76000.0,
            "drive_state": {
                "shift_state": "D",
                "speed": 50.0,
                "latitude": 37.9,
                "longitude": -122.5,
                "heading": 90,
                "power": 8000,
                "elevation": null,
                "timestamp": 1700001000000i64
            }
        }
    });

    let parked_resp = serde_json::json!({
        "response": {
            "id": 31,
            "state": "online",
            "odometer": 76001.0,
            "drive_state": {
                "shift_state": "P",
                "speed": null,
                "latitude": 37.9,
                "longitude": -122.5,
                "heading": 90,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001100000i64
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(drive_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(parked_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("geofence_enter="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(0)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 31,
        vehicle_id: 3100,
        vin: "GEOFDRV2".into(),
        display_name: Some("Geofence Drive Miss Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_geofence_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(vm.send_cmd("GEOFDRV2", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("GEOFDRV2", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn charge_close_with_geofence() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let charging_resp = serde_json::json!({
        "response": {
            "id": 32,
            "state": "online",
            "odometer": 77000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001200000i64
            },
            "charge_state": {
                "battery_level": 40,
                "battery_range": 100.0,
                "ideal_battery_range": 130.0,
                "charging_state": "Charging",
                "charge_energy_added": 0.0,
                "charger_actual_current": 32,
                "charger_voltage": 230,
                "charger_power": 7000,
                "charger_phases": 3,
                "conn_charge_cable": "CCS"
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let complete_resp = serde_json::json!({
        "response": {
            "id": 32,
            "state": "online",
            "odometer": 77000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001300000i64
            },
            "charge_state": {
                "battery_level": 80,
                "battery_range": 220.0,
                "ideal_battery_range": 260.0,
                "charging_state": "Complete",
                "charge_energy_added": 15.0,
                "charger_actual_current": 0,
                "charger_voltage": 0,
                "charger_power": 0,
                "charger_phases": null
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(charging_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(complete_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            "charging_sessions",
        ))
        .and(wiremock::matchers::body_string_contains(
            "geofence_id=\"Home\"",
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 32,
        vehicle_id: 3200,
        vin: "GEOFCHG1".into(),
        display_name: Some("Geofence Charge Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_geofence_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(vm.send_cmd("GEOFCHG1", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("GEOFCHG1", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn charge_close_without_geofence() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let charging_resp = serde_json::json!({
        "response": {
            "id": 33,
            "state": "online",
            "odometer": 78000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.9,
                "longitude": -122.5,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001400000i64
            },
            "charge_state": {
                "battery_level": 20,
                "battery_range": 50.0,
                "ideal_battery_range": 80.0,
                "charging_state": "Charging",
                "charge_energy_added": 0.0,
                "charger_actual_current": 32,
                "charger_voltage": 230,
                "charger_power": 7000,
                "charger_phases": 3,
                "conn_charge_cable": "CCS"
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let complete_resp = serde_json::json!({
        "response": {
            "id": 33,
            "state": "online",
            "odometer": 78000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.9,
                "longitude": -122.5,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001500000i64
            },
            "charge_state": {
                "battery_level": 60,
                "battery_range": 160.0,
                "ideal_battery_range": 190.0,
                "charging_state": "Complete",
                "charge_energy_added": 12.0,
                "charger_actual_current": 0,
                "charger_voltage": 0,
                "charger_power": 0,
                "charger_phases": null
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(charging_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(complete_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            "charging_sessions",
        ))
        .and(wiremock::matchers::body_string_contains("geofence_id="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(0)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 33,
        vehicle_id: 3300,
        vin: "GEOFCHG2".into(),
        display_name: Some("Geofence Charge Miss Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_geofence_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(vm.send_cmd("GEOFCHG2", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("GEOFCHG2", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn charge_close_missing_end_coords() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let charging_resp = serde_json::json!({
        "response": {
            "id": 34,
            "state": "online",
            "odometer": 79000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001600000i64
            },
            "charge_state": {
                "battery_level": 30,
                "battery_range": 80.0,
                "ideal_battery_range": 100.0,
                "charging_state": "Charging",
                "charge_energy_added": 0.0,
                "charger_actual_current": 32,
                "charger_voltage": 230,
                "charger_power": 7000,
                "charger_phases": 3,
                "conn_charge_cable": "CCS"
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let complete_resp = serde_json::json!({
        "response": {
            "id": 34,
            "state": "online",
            "odometer": 79000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": null,
                "longitude": null,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001700000i64
            },
            "charge_state": {
                "battery_level": 70,
                "battery_range": 200.0,
                "ideal_battery_range": 230.0,
                "charging_state": "Complete",
                "charge_energy_added": 14.0,
                "charger_actual_current": 0,
                "charger_voltage": 0,
                "charger_power": 0,
                "charger_phases": null
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(charging_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(complete_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            "charging_sessions",
        ))
        .and(wiremock::matchers::body_string_contains("geofence_id="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(0)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 34,
        vehicle_id: 3400,
        vin: "GEOFCHG3".into(),
        display_name: Some("Geofence Charge Null Coords".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_geofence_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(vm.send_cmd("GEOFCHG3", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("GEOFCHG3", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn charge_close_with_cost_per_kwh() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let charging_resp = serde_json::json!({
        "response": {
            "id": 35,
            "state": "online",
            "odometer": 80000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001800000i64
            },
            "charge_state": {
                "battery_level": 20,
                "battery_range": 50.0,
                "ideal_battery_range": 80.0,
                "charging_state": "Charging",
                "charge_energy_added": 0.0,
                "charger_actual_current": 32,
                "charger_voltage": 230,
                "charger_power": 7000,
                "charger_phases": 3,
                "conn_charge_cable": "CCS"
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let complete_resp = serde_json::json!({
        "response": {
            "id": 35,
            "state": "online",
            "odometer": 80000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001900000i64
            },
            "charge_state": {
                "battery_level": 60,
                "battery_range": 160.0,
                "ideal_battery_range": 190.0,
                "charging_state": "Complete",
                "charge_energy_added": 15.0,
                "charger_actual_current": 0,
                "charger_voltage": 0,
                "charger_power": 0,
                "charger_phases": null
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(charging_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(complete_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            "charging_sessions",
        ))
        .and(wiremock::matchers::body_string_contains("cost=2.25"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 35,
        vehicle_id: 3500,
        vin: "GEOFCOST1".into(),
        display_name: Some("Cost Per Kwh".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_geofence_settings_with_billing(BillingType::PerKwh, 0.15, 0.0),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(vm.send_cmd("GEOFCOST1", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("GEOFCOST1", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn charge_close_with_cost_per_minute() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let charging_resp = serde_json::json!({
        "response": {
            "id": 36,
            "state": "online",
            "odometer": 81000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700002000000i64
            },
            "charge_state": {
                "battery_level": 30,
                "battery_range": 80.0,
                "ideal_battery_range": 100.0,
                "charging_state": "Charging",
                "charge_energy_added": 0.0,
                "charger_actual_current": 32,
                "charger_voltage": 230,
                "charger_power": 7000,
                "charger_phases": 3,
                "conn_charge_cable": "CCS"
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let complete_resp = serde_json::json!({
        "response": {
            "id": 36,
            "state": "online",
            "odometer": 81000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700002100000i64
            },
            "charge_state": {
                "battery_level": 70,
                "battery_range": 200.0,
                "ideal_battery_range": 230.0,
                "charging_state": "Complete",
                "charge_energy_added": 15.0,
                "charger_actual_current": 0,
                "charger_voltage": 0,
                "charger_power": 0,
                "charger_phases": null
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(charging_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(complete_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            "charging_sessions",
        ))
        .and(wiremock::matchers::body_string_contains("cost="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 36,
        vehicle_id: 3600,
        vin: "GEOFCOST2".into(),
        display_name: Some("Cost Per Minute".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_geofence_settings_with_billing(BillingType::PerMinute, 0.10, 0.0),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(vm.send_cmd("GEOFCOST2", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("GEOFCOST2", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn charge_close_with_session_fee() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let charging_resp = serde_json::json!({
        "response": {
            "id": 37,
            "state": "online",
            "odometer": 82000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700002200000i64
            },
            "charge_state": {
                "battery_level": 25,
                "battery_range": 60.0,
                "ideal_battery_range": 90.0,
                "charging_state": "Charging",
                "charge_energy_added": 0.0,
                "charger_actual_current": 32,
                "charger_voltage": 230,
                "charger_power": 7000,
                "charger_phases": 3,
                "conn_charge_cable": "CCS"
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let complete_resp = serde_json::json!({
        "response": {
            "id": 37,
            "state": "online",
            "odometer": 82000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700002300000i64
            },
            "charge_state": {
                "battery_level": 65,
                "battery_range": 180.0,
                "ideal_battery_range": 210.0,
                "charging_state": "Complete",
                "charge_energy_added": 15.0,
                "charger_actual_current": 0,
                "charger_voltage": 0,
                "charger_power": 0,
                "charger_phases": null
            },
            "climate_state": {
                "outside_temp": 21.0,
                "inside_temp": 23.0
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(charging_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(complete_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            "charging_sessions",
        ))
        .and(wiremock::matchers::body_string_contains("cost=3.25"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 37,
        vehicle_id: 3700,
        vin: "GEOFCOST3".into(),
        display_name: Some("Cost Session Fee".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_geofence_settings_with_billing(BillingType::PerKwh, 0.15, 1.00),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(vm.send_cmd("GEOFCOST3", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("GEOFCOST3", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    vm.shutdown_all();
}

#[tokio::test]
async fn update_starts_when_installing() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 30,
                    "state": "online",
                    "odometer": 80000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700000800000i64
                    },
                    "vehicle_state": {
                        "car_version": "2024.8",
                        "software_update": {
                            "download_perc": 100,
                            "expected_duration_sec": 2700,
                            "install_perc": 50,
                            "status": "installing",
                            "version": "2024.12"
                        }
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            r#"status="installing""#,
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 30,
        vehicle_id: 3000,
        vin: "UPDSTRT01".into(),
        display_name: Some("Update Start Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Updating));
    vm.shutdown_all();
}

#[tokio::test]
async fn update_completes_when_installed() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let installing_resp = serde_json::json!({
        "response": {
            "id": 31,
            "state": "online",
            "odometer": 81000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700000900000i64
            },
            "vehicle_state": {
                "car_version": "2024.8",
                "software_update": {
                    "status": "installing",
                    "version": "2024.12"
                }
            }
        }
    });

    let done_resp = serde_json::json!({
        "response": {
            "id": 31,
            "state": "online",
            "odometer": 81000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700000900000i64
            },
            "vehicle_state": {
                "car_version": "2024.12",
                "software_update": {
                    "status": "",
                    "version": "2024.12"
                }
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(installing_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(done_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            r#"status="installing""#,
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("install_end="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 31,
        vehicle_id: 3100,
        vin: "UPDDONE01".into(),
        display_name: Some("Update Done Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(150)).await;

    assert!(vm.send_cmd("UPDDONE01", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("UPDDONE01", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn no_update_when_no_software_update() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 33,
                    "state": "online",
                    "odometer": 83000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700001100000i64
                    },
                    "vehicle_state": {
                        "tpms_pressure_fl": 42.0,
                        "tpms_pressure_fr": 41.5,
                        "tpms_pressure_rl": 40.0,
                        "tpms_pressure_rr": 40.5
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("updates"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(0)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 33,
        vehicle_id: 3300,
        vin: "NOUPD001".into(),
        display_name: Some("No Update Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn update_keeps_state_when_software_update_absent() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let installing_resp = serde_json::json!({
        "response": {
            "id": 34,
            "state": "online",
            "odometer": 84000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001200000i64
            },
            "vehicle_state": {
                "car_version": "2024.8",
                "software_update": {
                    "status": "installing",
                    "version": "2024.12"
                }
            }
        }
    });

    let absent_resp = serde_json::json!({
        "response": {
            "id": 34,
            "state": "online",
            "odometer": 84000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001200000i64
            },
            "vehicle_state": {
                "car_version": "2024.8",
                "tpms_pressure_fl": 42.0,
                "tpms_pressure_fr": 41.5,
                "tpms_pressure_rl": 40.0,
                "tpms_pressure_rr": 40.5
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(installing_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(absent_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            r#"status="installing""#,
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("install_end="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 34,
        vehicle_id: 3400,
        vin: "UPDPERS01".into(),
        display_name: Some("Update Persist Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn update_cancelled_when_available() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let installing_resp = serde_json::json!({
        "response": {
            "id": 35,
            "state": "online",
            "odometer": 85000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001300000i64
            },
            "vehicle_state": {
                "car_version": "2024.8",
                "software_update": {
                    "status": "installing",
                    "version": "2024.12"
                }
            }
        }
    });

    let available_resp = serde_json::json!({
        "response": {
            "id": 35,
            "state": "online",
            "odometer": 85000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001300000i64
            },
            "vehicle_state": {
                "car_version": "2024.8",
                "software_update": {
                    "status": "available",
                    "version": "2024.12"
                }
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(installing_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(available_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            r#"status="installing""#,
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            r#"status="cancelled""#,
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 35,
        vehicle_id: 3500,
        vin: "UPDCNCL01".into(),
        display_name: Some("Update Cancel Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(150)).await;

    assert!(vm.send_cmd("UPDCNCL01", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("UPDCNCL01", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn update_cannot_suspend() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 36,
                    "state": "online",
                    "odometer": 86000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700001400000i64
                    },
                    "vehicle_state": {
                        "car_version": "2024.8",
                        "software_update": {
                            "status": "installing",
                            "version": "2024.12"
                        }
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 36,
        vehicle_id: 3600,
        vin: "UPDSUSP01".into(),
        display_name: Some("Update Suspend Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Updating));

    vm.send_cmd(&vin, VehicleCommand::Suspend);
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Updating));
    vm.shutdown_all();
}

#[tokio::test]
async fn update_survives_offline_resume() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let installing_resp = serde_json::json!({
        "response": {
            "id": 37,
            "state": "online",
            "odometer": 87000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001500000i64
            },
            "vehicle_state": {
                "car_version": "2024.8",
                "software_update": {
                    "status": "installing",
                    "version": "2024.12"
                }
            }
        }
    });

    let offline_resp = serde_json::json!({
        "response": {
            "id": 37,
            "state": "offline",
            "odometer": null,
            "drive_state": null,
            "charge_state": null,
            "climate_state": null,
            "vehicle_state": null
        }
    });

    let done_resp = serde_json::json!({
        "response": {
            "id": 37,
            "state": "online",
            "odometer": 87000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001500000i64
            },
            "vehicle_state": {
                "car_version": "2024.12",
                "software_update": {
                    "status": "",
                    "version": "2024.12"
                }
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match count {
                0 => wiremock::ResponseTemplate::new(200).set_body_json(installing_resp.clone()),
                1 => wiremock::ResponseTemplate::new(200).set_body_json(offline_resp.clone()),
                _ => wiremock::ResponseTemplate::new(200).set_body_json(done_resp.clone()),
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            r#"status="installing""#,
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("install_end="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 37,
        vehicle_id: 3700,
        vin: "UPDRSM01".into(),
        display_name: Some("Update Resume Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Updating));

    assert!(vm.send_cmd("UPDRSM01", VehicleCommand::Suspend));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(vm.send_cmd("UPDRSM01", VehicleCommand::Resume));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn update_finalizes_when_driving_detected() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let installing_resp = serde_json::json!({
        "response": {
            "id": 38,
            "state": "online",
            "odometer": 88000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001600000i64
            },
            "vehicle_state": {
                "car_version": "2024.8",
                "software_update": {
                    "status": "installing",
                    "version": "2024.12"
                }
            }
        }
    });

    let driving_resp = serde_json::json!({
        "response": {
            "id": 38,
            "state": "online",
            "odometer": 88000.0,
            "drive_state": {
                "shift_state": "D",
                "speed": 60.0,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": 90,
                "power": 10000,
                "elevation": 20.0,
                "timestamp": 1700001605000i64
            },
            "vehicle_state": {
                "car_version": "2024.12",
                "tpms_pressure_fl": 42.0,
                "tpms_pressure_fr": 41.5,
                "tpms_pressure_rl": 40.0,
                "tpms_pressure_rr": 40.5
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(installing_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(driving_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            r#"status="installing""#,
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("install_end="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 38,
        vehicle_id: 3800,
        vin: "UPDRV01".into(),
        display_name: Some("Update Drive Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Should eventually be Driving (after update finalized)
    assert_eq!(vm.state_of("UPDRV01"), Some(VehicleState::Driving));
    vm.shutdown_all();
}

#[tokio::test]
async fn update_finalizes_when_vehicle_state_absent_software_update() {
    let tesla_server = wiremock::MockServer::start().await;
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let installing_resp = serde_json::json!({
        "response": {
            "id": 39,
            "state": "online",
            "odometer": 89000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001700000i64
            },
            "vehicle_state": {
                "car_version": "2024.8",
                "software_update": {
                    "status": "installing",
                    "version": "2024.12"
                }
            }
        }
    });

    let absent_resp = serde_json::json!({
        "response": {
            "id": 39,
            "state": "online",
            "odometer": 89000.0,
            "drive_state": {
                "shift_state": null,
                "speed": null,
                "latitude": 37.8,
                "longitude": -122.4,
                "heading": null,
                "power": 0,
                "elevation": null,
                "timestamp": 1700001700000i64
            },
            "vehicle_state": {
                "car_version": "2024.12",
                "tpms_pressure_fl": 42.0,
                "tpms_pressure_fr": 41.5,
                "tpms_pressure_rl": 40.0,
                "tpms_pressure_rr": 40.5
            }
        }
    });

    let counter_clone = std::sync::Arc::clone(&counter);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                wiremock::ResponseTemplate::new(200).set_body_json(installing_resp.clone())
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(absent_resp.clone())
            }
        })
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains(
            r#"status="installing""#,
        ))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .and(wiremock::matchers::body_string_contains("install_end="))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 39,
        vehicle_id: 3900,
        vin: "UPDSGAP01".into(),
        display_name: Some("Update SU Gone Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

// -----------------------------------------------------------------------
// Auto-suspend tests
// -----------------------------------------------------------------------

/// Creates test settings with very short idle timeout (0 minutes) for
/// auto-suspend tests.
fn test_settings_with_auto_suspend() -> Arc<Mutex<YamlConfigManager>> {
    let dir = std::env::temp_dir().join("tesla-test-autosuspend").join(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string(),
    );
    let mut mgr = YamlConfigManager::load(&dir).unwrap();
    let car_settings = crate::config_yaml::CarSettings {
        suspend_after_idle_minutes: 0,
        suspend_minimum_minutes: 0,
        require_unlocked_for_wake: false,
        ..Default::default()
    };
    for vin in [
        "AUTOSUSP01",
        "SENTRY01",
        "PRECOND01",
        "DOGMODE01",
        "DOORSOPEN01",
        "POWER01",
    ] {
        mgr.settings.cars.insert(vin.into(), car_settings.clone());
    }
    Arc::new(Mutex::new(mgr))
}

#[tokio::test]
async fn auto_suspend_after_idle() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 40,
                    "state": "online",
                    "odometer": 90000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700001800000i64
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 40,
        vehicle_id: 4000,
        vin: "AUTOSUSP01".into(),
        display_name: Some("Auto Suspend Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings_with_auto_suspend(),
        Duration::from_millis(50),
    );

    // Wait for poll tick to fire and auto-suspend
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Suspended));
    vm.shutdown_all();
}

#[tokio::test]
async fn auto_suspend_skipped_when_sentry_active() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 41,
                    "state": "online",
                    "odometer": 91000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700001900000i64
                    },
                    "vehicle_state": {
                        "sentry_mode": true
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 41,
        vehicle_id: 4100,
        vin: "SENTRY01".into(),
        display_name: Some("Sentry Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings_with_auto_suspend(),
        Duration::from_millis(50),
    );

    // Wait enough for multiple poll ticks
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Should stay online despite zero idle timeout because sentry is active
    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn auto_suspend_skipped_when_preconditioning() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 42,
                    "state": "online",
                    "odometer": 92000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700002000000i64
                    },
                    "climate_state": {
                        "is_preconditioning": true
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 42,
        vehicle_id: 4200,
        vin: "PRECOND01".into(),
        display_name: Some("Preconditioning Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings_with_auto_suspend(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn auto_suspend_skipped_when_dog_mode() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 43,
                    "state": "online",
                    "odometer": 93000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700002100000i64
                    },
                    "climate_state": {
                        "climate_keeper_mode": "dog"
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 43,
        vehicle_id: 4300,
        vin: "DOGMODE01".into(),
        display_name: Some("Dog Mode Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings_with_auto_suspend(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn auto_suspend_skipped_when_doors_open() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 44,
                    "state": "online",
                    "odometer": 94000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700002200000i64
                    },
                    "vehicle_state": {
                        "df": 1.0,
                        "pf": 0.0,
                        "dr": 0.0,
                        "pr": 0.0
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 44,
        vehicle_id: 4400,
        vin: "DOORSOPEN01".into(),
        display_name: Some("Doors Open Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings_with_auto_suspend(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn auto_suspend_skipped_when_power_usage() {
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 45,
                    "state": "online",
                    "odometer": 95000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 500,
                        "elevation": null,
                        "timestamp": 1700002300000i64
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    let mut vm = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 45,
        vehicle_id: 4500,
        vin: "POWER01".into(),
        display_name: Some("Power Usage Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (tx, token_rx) = watch::channel(Some("token".into()));
    tx.send(Some("token".into())).ok();

    vm.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings_with_auto_suspend(),
        Duration::from_millis(50),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(vm.state_of(&vin), Some(VehicleState::Online));
    vm.shutdown_all();
}

#[tokio::test]
async fn http_suspend_resume_endpoints() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // Set up wiremock servers for the vehicle task
    let tesla_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"/api/1/vehicles/\d+/vehicle_data",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": {
                    "id": 46,
                    "state": "online",
                    "odometer": 96000.0,
                    "drive_state": {
                        "shift_state": null,
                        "speed": null,
                        "latitude": 37.8,
                        "longitude": -122.4,
                        "heading": null,
                        "power": 0,
                        "elevation": null,
                        "timestamp": 1700002400000i64
                    }
                }
            })),
        )
        .mount(&tesla_server)
        .await;

    let db_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v3/write_lp"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&db_server)
        .await;

    // Build app state with a real vehicle task
    let mut vehicle_manager = Vehicles::new(&tesla_server.uri());
    let vehicle = Vehicle {
        id: 46,
        vehicle_id: 4600,
        vin: "HTTPTEST01".into(),
        display_name: Some("HTTP Test".into()),
        state: "online".into(),
        api_version: 18,
        in_service: false,
    };
    let vin = vehicle.vin.clone();
    let (_, token_rx) = watch::channel(Some("token".into()));

    vehicle_manager.spawn_one(
        vehicle,
        Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        token_rx,
        test_settings(),
        Duration::from_millis(50),
    );

    // Wait for initial poll
    tokio::time::sleep(Duration::from_millis(100)).await;

    let state = crate::api::AppState {
        db: Arc::new(InfluxDb::new(&db_server.uri(), "none", "test").unwrap()),
        auth: std::sync::Arc::new(crate::tesla_auth::TeslaAuthClient::new(
            "client",
            "https://example.com",
            "https://example.com",
        )),
        yaml: test_settings(),
        encryption_key: [0u8; 32],
        vehicles: Arc::new(HashMap::new()),
        vehicle_manager: Arc::new(vehicle_manager),
    };
    let app = crate::api::vehicles::router().with_state(state);

    // Suspend
    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/{vin}/suspend"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // Wait for vehicle task to process suspend command
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify suspended
    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/{vin}/state"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["state"], "Suspended");

    // Resume
    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/{vin}/resume"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // Wait for vehicle task to process resume command
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify online
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/{vin}/state"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["state"], "Online");
}

#[tokio::test]
async fn http_suspend_unknown_vin_returns_404() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let state = crate::api::test_helpers::test_state();
    let app = crate::api::vehicles::router().with_state(state);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/UNKNOWNVIN/suspend")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"vehicle_not_found");
}

#[tokio::test]
async fn http_resume_unknown_vin_returns_404() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let state = crate::api::test_helpers::test_state();
    let app = crate::api::vehicles::router().with_state(state);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/UNKNOWNVIN/resume")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"vehicle_not_found");
}
