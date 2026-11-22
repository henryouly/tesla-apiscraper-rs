use super::{TestDb, get_json};
use axum::http::StatusCode;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_states_against_real_influxdb() {
    use crate::influxdb::{Precision, VehicleState as StatePoint};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_states").await;
    let app = tdb.authed_app();

    // Window: 2023-11-14T10:00:00Z (1699956000) .. 2023-11-14T11:00:00Z (1699959600)
    // Seed one state before `from`, two inside window, one after `to`.
    let points = [
        (1_699_955_000u128, "asleep"),
        (1_699_956_600, "online"),
        (1_699_957_200, "driving"),
        (1_699_960_000, "charging"),
    ];
    for (ts, st) in points {
        let pt = StatePoint {
            time: Timestamp::Seconds(ts),
            vin: "VINST1".into(),
            state: st.into(),
            inside_temp: None,
            outside_temp: None,
            battery_level: Some(75),
            locked: Some(true),
            sentry_mode: Some(false),
            dog_mode: None,
            cabin_overheat_protection: None,
        };
        tdb.db
            .write_query(pt.into_query("states"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/states?vin=VINST1&from=2023-11-14T10:00:00Z&to=2023-11-14T11:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "states response: {json}");
    assert_eq!(json["current"], "driving");
    assert_eq!(json["changes"], 2);

    let timeline = json["timeline"].as_array().unwrap();
    assert_eq!(
        timeline.len(),
        3,
        "prior state + 2 in-window transitions: {json}"
    );
    assert_eq!(timeline[0]["t"], 1_699_956_000_000i64);
    assert_eq!(timeline[0]["state"], "asleep");
    assert_eq!(timeline[1]["state"], "online");
    assert_eq!(timeline[2]["state"], "driving");

    tdb.cleanup().await;
}
