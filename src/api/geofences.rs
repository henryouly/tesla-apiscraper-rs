//! Geo-fence CRUD over `config/geofences.yml`.
//!
//! Billing changes apply to future sessions only — closed sessions keep
//! the cost computed at close time (recalculation is future work).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
};
use serde::Serialize;

use super::AppState;
use crate::config_yaml::Geofence;

#[derive(Serialize)]
pub struct GeofencesResponse {
    pub geofences: Vec<Geofence>,
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/", get(list_geofences).post(create_geofence))
        .route("/{name}", put(update_geofence).delete(delete_geofence))
}

fn validate(g: &Geofence) -> Result<(), String> {
    if g.name.trim().is_empty() {
        return Err("name must not be empty".into());
    }
    if !(-90.0..=90.0).contains(&g.latitude) {
        return Err("latitude must be within -90..90".into());
    }
    if !(-180.0..=180.0).contains(&g.longitude) {
        return Err("longitude must be within -180..180".into());
    }
    if !g.radius_meters.is_finite() || g.radius_meters <= 0.0 {
        return Err("radius_meters must be a positive number".into());
    }
    if let Some(ref b) = g.billing {
        if !b.cost_per_unit.is_finite() || b.cost_per_unit < 0.0 {
            return Err("cost_per_unit must not be negative".into());
        }
        if !b.session_fee.is_finite() || b.session_fee < 0.0 {
            return Err("session_fee must not be negative".into());
        }
    }
    Ok(())
}

async fn list_geofences(State(state): State<AppState>) -> Json<GeofencesResponse> {
    let geofences = state
        .yaml
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .geofences
        .geofences
        .clone();
    Json(GeofencesResponse { geofences })
}

async fn create_geofence(
    State(state): State<AppState>,
    Json(req): Json<Geofence>,
) -> Result<(StatusCode, Json<Geofence>), (StatusCode, Json<serde_json::Value>)> {
    if let Err(e) = validate(&req) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": e })),
        ));
    }
    let mut yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
    if yaml.geofences.geofences.iter().any(|g| g.name == req.name) {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "geofence name already exists" })),
        ));
    }
    yaml.geofences.geofences.push(req.clone());
    if let Err(e) = yaml.save_geofences() {
        // Roll back: a phantom in-memory entry would 404 nothing yet 409
        // every retry until restart.
        yaml.geofences.geofences.pop();
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ));
    }
    Ok((StatusCode::CREATED, Json(req)))
}

async fn update_geofence(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<Geofence>,
) -> Result<Json<Geofence>, (StatusCode, Json<serde_json::Value>)> {
    if req.name != name {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(
                serde_json::json!({ "error": "body name must match path (rename via delete + create)" }),
            ),
        ));
    }
    if let Err(e) = validate(&req) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": e })),
        ));
    }
    let mut yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pos) = yaml.geofences.geofences.iter().position(|g| g.name == name) else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "geofence not found" })),
        ));
    };
    let old = std::mem::replace(&mut yaml.geofences.geofences[pos], req.clone());
    if let Err(e) = yaml.save_geofences() {
        yaml.geofences.geofences[pos] = old;
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ));
    }
    Ok(Json(req))
}

async fn delete_geofence(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let mut yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pos) = yaml.geofences.geofences.iter().position(|g| g.name == name) else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "geofence not found" })),
        ));
    };
    let removed = yaml.geofences.geofences.remove(pos);
    if let Err(e) = yaml.save_geofences() {
        yaml.geofences.geofences.insert(pos, removed);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn fence(name: &str) -> Geofence {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "latitude": 37.7,
            "longitude": -122.4,
            "radius_meters": 150.0,
            "billing": {
                "type": "per_kwh",
                "cost_per_unit": 0.3,
                "session_fee": 1.0
            }
        }))
        .unwrap()
    }

    fn body_json(v: serde_json::Value) -> Body {
        Body::from(serde_json::to_vec(&v).unwrap())
    }

    #[tokio::test]
    async fn list_empty_initially() {
        let app = router().with_state(crate::api::test_helpers::test_state());
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["geofences"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn create_lists_round_trip() {
        let app = router().with_state(crate::api::test_helpers::test_state());
        let resp = app
            .clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .body(body_json(serde_json::to_value(fence("Home")).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["geofences"][0]["name"], "Home");
        assert_eq!(json["geofences"][0]["billing"]["type"], "per_kwh");
    }

    #[tokio::test]
    async fn create_duplicate_name_conflicts() {
        let state = crate::api::test_helpers::test_state();
        let app = router().with_state(state);
        for _ in 0..2 {
            let resp = app
                .clone()
                .oneshot(
                    Request::post("/")
                        .header("content-type", "application/json")
                        .body(body_json(serde_json::to_value(fence("Home")).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            if resp.status() != StatusCode::CREATED {
                assert_eq!(resp.status(), StatusCode::CONFLICT);
                return;
            }
        }
        panic!("second create should conflict");
    }

    #[tokio::test]
    async fn create_rejects_invalid() {
        let state = crate::api::test_helpers::test_state();
        let app = router().with_state(state);
        // Bad latitude, unknown billing type is a 422 from the extractor.
        for payload in [
            serde_json::json!({"name": "", "latitude": 0.0, "longitude": 0.0}),
            serde_json::json!({"name": "X", "latitude": 91.0, "longitude": 0.0}),
            serde_json::json!({"name": "X", "latitude": 0.0, "longitude": 0.0, "radius_meters": -5.0}),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::post("/")
                        .header("content-type", "application/json")
                        .body(body_json(payload))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    #[tokio::test]
    async fn update_and_delete_flow() {
        let app = router().with_state(crate::api::test_helpers::test_state());
        let created = fence("Work");
        app.clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .body(body_json(serde_json::to_value(&created).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let mut updated = created.clone();
        updated.radius_meters = 500.0;
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/Work")
                    .header("content-type", "application/json")
                    .body(body_json(serde_json::to_value(&updated).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["radius_meters"], 500.0);

        // Name mismatch rejected.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/Work")
                    .header("content-type", "application/json")
                    .body(body_json(serde_json::to_value(fence("Other")).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // Unknown name 404s on update and delete.
        for (method, uri) in [("PUT", "/Nope"), ("DELETE", "/Nope")] {
            let builder = Request::builder().method(method).uri(uri);
            let req = if method == "PUT" {
                builder
                    .header("content-type", "application/json")
                    .body(body_json(serde_json::to_value(fence("Nope")).unwrap()))
                    .unwrap()
            } else {
                builder.body(Body::empty()).unwrap()
            };
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{method}");
        }

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/Work")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    /// State with its config dir made read-only, so YAML saves fail.
    #[cfg(unix)]
    fn readonly_state() -> super::AppState {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join("tesla-test-geofence-ro").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        let mgr = crate::config_yaml::YamlConfigManager::load(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let mut state = crate::api::test_helpers::test_state();
        state.yaml = std::sync::Arc::new(std::sync::Mutex::new(mgr));
        state
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn create_rolls_back_on_save_failure() {
        let app = router().with_state(readonly_state());
        let resp = app
            .clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .body(body_json(serde_json::to_value(fence("Home")).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // No phantom entry: the list is empty and the name is reusable.
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["geofences"].as_array().unwrap().len(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_rolls_back_on_save_failure() {
        let dir = std::env::temp_dir().join("tesla-test-geofence-ro2").join(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_string(),
        );
        let mgr = crate::config_yaml::YamlConfigManager::load(&dir).unwrap();
        let mut state = crate::api::test_helpers::test_state();
        state.yaml = std::sync::Arc::new(std::sync::Mutex::new(mgr));
        let app = router().with_state(state);
        app.clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .body(body_json(serde_json::to_value(fence("Home")).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/Home")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Entry restored: still listed.
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["geofences"][0]["name"], "Home");
    }
}
