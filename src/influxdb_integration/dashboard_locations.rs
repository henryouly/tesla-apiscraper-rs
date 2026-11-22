use super::{TestDb, get_json};
use axum::http::StatusCode;

#[tokio::test]
#[ignore = "requires a running InfluxDB 1.8 server (INFLUXDB_TEST_URL)"]
async fn dashboard_locations_against_real_influxdb() {
    use crate::influxdb::{Drive, Precision};
    use influxdb::{InfluxDbWriteable, Timestamp};

    let tdb = TestDb::new("dash_loc").await;
    let app = tdb.authed_app();

    let drives = [
        (
            1_699_956_000u128,
            "loc-1",
            Some("Office"),
            Some("123 Market St"),
            "2023-11-14T10:30:00Z",
            12_000.0,
        ),
        (
            1_699_960_000u128,
            "loc-2",
            Some("Office"),
            Some("123 Market St"),
            "2023-11-14T11:30:00Z",
            10_000.0,
        ),
        (
            1_699_964_000u128,
            "loc-3",
            None,
            Some("Golden Gate Park"),
            "2023-11-14T12:30:00Z",
            8_000.0,
        ),
    ];
    for (ts, id, gf, addr, end_time, dist) in drives {
        let d = Drive {
            time: Timestamp::Seconds(ts),
            vin: "VINLOC1".into(),
            drive_id: id.into(),
            start_lat: None,
            start_lng: None,
            end_lat: None,
            end_lng: None,
            start_address: None,
            end_address: addr.map(String::from),
            start_time: None,
            end_time: Some(end_time.into()),
            distance_meters: Some(dist),
            duration_seconds: Some(900),
            energy_used_wh: Some(1500.0),
            max_speed: None,
            average_speed: None,
            outside_temp_avg: None,
            inside_temp_avg: None,
            geofence_enter: None,
            geofence_exit: gf.map(String::from),
            is_merged: Some(false),
        };
        tdb.db
            .write_query(d.into_query("drives"), Precision::Seconds)
            .await
            .unwrap();
    }

    let (status, json) = get_json(
        &app,
        "/api/dashboards/locations?vin=VINLOC1&from=2023-11-14T00:00:00Z&to=2023-11-15T00:00:00Z&unit=km",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "locations: {json}");
    assert_eq!(json["drives_logged"], 3);

    let top = json["top"].as_array().unwrap();
    assert_eq!(top.len(), 2);
    assert_eq!(top[0]["place"], "Office");
    assert_eq!(top[0]["visits"], 2);
    assert_eq!(top[1]["place"], "Golden Gate Park");
    assert_eq!(top[1]["visits"], 1);

    let visited = json["last_visited"].as_array().unwrap();
    assert_eq!(visited.len(), 3);
    // Most recent end_time first.
    assert_eq!(visited[0]["address"], "Golden Gate Park");
    assert!((visited[0]["distance"].as_f64().unwrap() - 8.0).abs() < 1e-6);

    tdb.cleanup().await;
}
