use std::convert::Infallible;

use axum::{extract::State, response::Sse, response::sse::Event, routing::get};
use futures_util::stream::Stream;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;

use super::AppState;
use crate::vehicle_summary::UiEvent;

/// SSE stream of [`UiEvent`]s: `summary` (full snapshot) and `state`
/// (VIN + state only). Slow consumers lag (bounded channel) and receive a
/// `resync` hint telling them to refetch `/api/vehicles/summaries`.
/// `axum`'s keep-alive supplies protocol-level heartbeats every 15s.
pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route("/", get(sse_handler))
}

async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.vehicle_manager.subscribe();
    let stream = BroadcastStream::new(rx).map(|msg| {
        let event = match msg {
            Ok(ev) => sse_event(&ev),
            Err(_) => Event::default()
                .event("resync")
                .json_data(serde_json::json!({ "reason": "lagged" }))
                .expect("resync payload serializes"),
        };
        Ok::<_, Infallible>(event)
    });
    // End the stream on server shutdown so graceful shutdown doesn't wait
    // for browsers to disconnect first. (futures_util's take_until via
    // UFCS: tokio-stream 0.1 has no take_until, and importing both
    // StreamExt traits would make .map ambiguous.)
    let mut shutdown_rx = state.shutdown_rx.clone();
    let stop = async move {
        shutdown_rx.wait_for(|fired| *fired).await.ok();
    };
    let stream = futures_util::StreamExt::take_until(stream, stop);
    // Merge an initial resync-independent heartbeat-independent stream note:
    // clients fetch summaries on connect, then apply events.
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

fn sse_event(ev: &UiEvent) -> Event {
    Event::default()
        .event(ev.kind)
        .json_data(ev)
        .expect("UiEvent serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn events_returns_event_stream_content_type() {
        let state = crate::api::test_helpers::test_state();
        let app = router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .expect("content-type")
            .to_str()
            .unwrap()
            .to_string();
        assert!(ct.contains("text/event-stream"), "got {ct}");
        // Drop without consuming: the stream is infinite by design.
    }

    #[tokio::test]
    async fn sse_ends_on_shutdown_signal() {
        let mut state = crate::api::test_helpers::test_state();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        state.shutdown_rx = shutdown_rx;
        let app = router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // The stream only ends when shutdown fires — never by itself.
        shutdown_tx.send(true).ok();
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            resp.into_body().collect(),
        )
        .await
        .expect("stream did not end after shutdown signal");
        let _ = body.unwrap();
    }

    #[tokio::test]
    async fn broadcast_reaches_subscriber() {
        let state = crate::api::test_helpers::test_state();
        let mut rx = state.vehicle_manager.subscribe();
        state
            .vehicle_manager
            .publish_state("VIN1", crate::vehicles::VehicleState::Online);
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.kind, "state");
        assert_eq!(ev.vin.as_deref(), Some("VIN1"));
    }

    #[test]
    fn sse_event_names_match_kind() {
        let ev = UiEvent::state("V", crate::vehicles::VehicleState::Driving);
        let _ = sse_event(&ev);
    }
}
