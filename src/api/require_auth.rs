//! Server-side guard for telemetry routes.
//!
//! The SPA's `RequireAuth` is client-side only; with permissive CORS and a
//! default `0.0.0.0` bind, live GPS/battery data would otherwise be readable
//! by any network client. This middleware enforces the same usable-token
//! predicate as `GET /api/auth/status`. The bootstrap auth endpoints stay
//! public so a fresh instance can sign in.

use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::{AppState, auth::tokens_usable};

pub async fn require_usable_tokens(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let ok = {
        let yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
        tokens_usable(
            &yaml,
            &state.encryption_key,
            crate::vehicle_summary::now_unix(),
        )
    };
    if ok {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    fn authed_state() -> AppState {
        let state = crate::api::test_helpers::test_state();
        {
            let mut yaml = state.yaml.lock().unwrap_or_else(|e| e.into_inner());
            yaml.set_encrypted_tokens(&[0u8; 32], "at", "rt", 9_999_999_999)
                .unwrap();
        }
        state
    }

    #[tokio::test]
    async fn vehicles_rejects_without_tokens() {
        let app = crate::api::create_router(crate::api::test_helpers::test_state());
        for uri in ["/api/vehicles", "/api/vehicles/summaries", "/api/events"] {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{uri}");
        }
    }

    #[tokio::test]
    async fn vehicles_allows_with_fresh_tokens() {
        let app = crate::api::create_router(authed_state());
        for uri in ["/api/vehicles", "/api/vehicles/summaries"] {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        }
        // SSE endpoint: infinite stream — assert status only, then drop.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn public_routes_stay_open_without_tokens() {
        let app = crate::api::create_router(crate::api::test_helpers::test_state());
        for uri in ["/health", "/api/auth/status"] {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        }
    }
}
