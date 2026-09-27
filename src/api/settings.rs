//! Global and per-car settings CRUD over `config/settings.yml`.
//!
//! Grafana URL stays environment-controlled (`GRAFANA_URL`) and is not part
//! of this API. Like geofences, in-memory mutations roll back when
//! persistence fails.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
};
use serde::Serialize;

use super::AppState;
use crate::config_yaml::{CarSettings, GlobalSettings, SettingsConfig};

#[derive(Serialize)]
pub struct SettingsResponse {
    pub settings: SettingsConfig,
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/", get(get_settings))
        .route("/global", put(put_global))
        .route("/cars/{vin}", put(put_car))
}

fn validate_global(g: &GlobalSettings) -> Result<(), String> {
    for (name, v, allowed) in [
        (
            "unit_length",
            g.unit_length.as_str(),
            ["km", "mi"].as_slice(),
        ),
        (
            "unit_temperature",
            g.unit_temperature.as_str(),
            ["C", "F"].as_slice(),
        ),
        (
            "unit_pressure",
            g.unit_pressure.as_str(),
            ["bar", "psi"].as_slice(),
        ),
        (
            "preferred_range",
            g.preferred_range.as_str(),
            ["rated", "ideal"].as_slice(),
        ),
        (
            "theme",
            g.theme.as_str(),
            ["light", "dark", "system"].as_slice(),
        ),
    ] {
        if !allowed.contains(&v) {
            return Err(format!("{name} must be one of {}", allowed.join("/")));
        }
    }
    if g.language.trim().is_empty() {
        return Err("language must not be empty".into());
    }
    Ok(())
}

fn validate_car(c: &CarSettings) -> Result<(), String> {
    for (name, v) in [
        ("suspend_after_idle_minutes", c.suspend_after_idle_minutes),
        ("suspend_minimum_minutes", c.suspend_minimum_minutes),
    ] {
        if v > 1440 {
            return Err(format!("{name} must be at most 1440"));
        }
    }
    Ok(())
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: impl Into<String>) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg.into() })))
}

async fn get_settings(State(state): State<AppState>) -> Json<SettingsResponse> {
    let settings = state
        .yaml
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .settings
        .clone();
    Json(SettingsResponse { settings })
}

async fn put_global(
    State(state): State<AppState>,
    Json(req): Json<GlobalSettings>,
) -> Result<Json<GlobalSettings>, ApiError> {
    if let Err(e) = validate_global(&req) {
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, e));
    }
    let mut yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
    let old = std::mem::replace(&mut yaml.settings.global, req.clone());
    if let Err(e) = yaml.save_settings() {
        yaml.settings.global = old;
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to persist settings: {e}"),
        ));
    }
    Ok(Json(req))
}

async fn put_car(
    State(state): State<AppState>,
    Path(vin): Path<String>,
    Json(req): Json<CarSettings>,
) -> Result<Json<CarSettings>, ApiError> {
    if let Err(e) = validate_car(&req) {
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, e));
    }
    // Only known vehicles get settings (matches discovery registry).
    let known = state
        .vehicles
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&vin);
    if !known {
        return Err(err(StatusCode::NOT_FOUND, "vehicle not found"));
    }
    let mut yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
    let old = yaml.settings.cars.insert(vin.clone(), req.clone());
    if let Err(e) = yaml.save_settings() {
        match old {
            Some(prev) => {
                yaml.settings.cars.insert(vin, prev);
            }
            None => {
                yaml.settings.cars.remove(&vin);
            }
        }
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to persist settings: {e}"),
        ));
    }
    Ok(Json(req))
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
    use tower::ServiceExt;

    fn global_valid() -> serde_json::Value {
        serde_json::json!({
            "unit_length": "mi",
            "unit_temperature": "F",
            "unit_pressure": "psi",
            "preferred_range": "ideal",
            "language": "de",
            "theme": "dark"
        })
    }

    fn car_valid() -> serde_json::Value {
        serde_json::json!({
            "suspend_after_idle_minutes": 30,
            "suspend_minimum_minutes": 10,
            "require_unlocked_for_wake": true,
            "free_supercharging": false,
            "use_streaming_api": true,
            "enabled": true,
            "lfp_battery": false
        })
    }

    fn state_with_vehicle(vin: &str) -> AppState {
        let state = crate::api::test_helpers::test_state();
        state
            .vehicles
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                vin.into(),
                Vehicle {
                    id: 1,
                    vehicle_id: 100,
                    vin: vin.into(),
                    display_name: Some("Car".into()),
                    state: "online".into(),
                    api_version: 18,
                    in_service: false,
                },
            );
        state
    }

    #[tokio::test]
    async fn get_settings_returns_defaults() {
        let app = router().with_state(crate::api::test_helpers::test_state());
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["settings"]["global"]["unit_length"], "km");
        assert_eq!(json["settings"]["cars"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn put_global_round_trip() {
        let app = router().with_state(crate::api::test_helpers::test_state());
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/global")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&global_valid()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["settings"]["global"]["unit_length"], "mi");
        assert_eq!(json["settings"]["global"]["theme"], "dark");
    }

    #[tokio::test]
    async fn put_global_rejects_invalid() {
        let app = router().with_state(crate::api::test_helpers::test_state());
        for (field, value) in [
            ("unit_length", "parsecs"),
            ("unit_temperature", "K"),
            ("unit_pressure", "atm"),
            ("preferred_range", "max"),
            ("theme", "neon"),
            ("language", ""),
        ] {
            let mut payload = global_valid();
            payload[field] = serde_json::json!(value);
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri("/global")
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{field}");
        }
    }

    #[tokio::test]
    async fn put_car_round_trip() {
        let app = router().with_state(state_with_vehicle("VIN1"));
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/cars/VIN1")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&car_valid()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["settings"]["cars"]["VIN1"]["suspend_after_idle_minutes"],
            30
        );
        assert_eq!(json["settings"]["cars"]["VIN1"]["use_streaming_api"], true);
    }

    #[tokio::test]
    async fn put_car_unknown_vin_returns_404() {
        let app = router().with_state(crate::api::test_helpers::test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/cars/NOPE")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&car_valid()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn put_car_rejects_absurd_suspend() {
        let app = router().with_state(state_with_vehicle("VIN1"));
        let mut payload = car_valid();
        payload["suspend_after_idle_minutes"] = serde_json::json!(99999);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/cars/VIN1")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
