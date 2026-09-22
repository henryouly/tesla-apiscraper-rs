use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tracing::{debug, info, warn};

/// A single data point from the Tesla streaming API.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StreamingData {
    /// Epoch milliseconds, like the poll API's `drive_state.timestamp`.
    pub timestamp: i64,
    pub speed: Option<f64>,
    pub soc: Option<f64>,
    pub odometer: Option<f64>,
    pub elevation: Option<f64>,
    /// Estimated heading. Intentionally the estimate (index 5), not the
    /// native heading (index 12): upstream merges `est_heading` into vehicle
    /// state and never consumes the native field.
    pub heading: Option<f64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub power: Option<i64>,
    pub shift_state: Option<String>,
    pub range: Option<f64>,
    pub est_range: Option<f64>,
}

/// Columns requested in the subscribe message, in wire order.
///
/// Must match [`parse_csv_line`]: `time` plus these twelve, thirteen values
/// total. Mirrors the documented streaming-API column list.
const COLUMNS: &[&str] = &[
    "speed",
    "odometer",
    "soc",
    "elevation",
    "est_heading",
    "est_lat",
    "est_lng",
    "power",
    "shift_state",
    "range",
    "est_range",
    "heading",
];

/// Why a streaming connection ended.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StreamEndReason {
    VehicleOffline,
    TokenExpired,
    IoError(String),
    Shutdown,
}

/// Parse the CSV payload of a `data:update` frame.
///
/// Wire order is `time` plus [`COLUMNS`]: `timestamp,speed,odometer,soc,
/// elevation,est_heading,est_lat,est_lng,power,shift_state,range,est_range,
/// heading`. Empty fields represent missing/unknown values.
pub(crate) fn parse_csv_line(line: &str) -> Result<StreamingData, String> {
    let parts: Vec<&str> = line.split(',').collect();
    if parts.len() != 13 {
        return Err(format!("expected 13 fields, got {}", parts.len()));
    }

    let timestamp = parts[0]
        .parse::<i64>()
        .map_err(|e| format!("invalid timestamp: {e}"))?;

    let speed = parse_f64(parts[1]);
    let odometer = parse_f64(parts[2]);
    let soc = parse_f64(parts[3]);
    let elevation = parse_f64(parts[4]);
    let heading = parse_f64(parts[5]);
    let latitude = parse_f64(parts[6]);
    let longitude = parse_f64(parts[7]);
    let power = parse_i64(parts[8]);
    let range = parse_f64(parts[10]);
    let est_range = parse_f64(parts[11]);

    let shift_state = {
        let s = parts[9].trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    };

    Ok(StreamingData {
        timestamp,
        speed,
        soc,
        odometer,
        elevation,
        heading,
        latitude,
        longitude,
        power,
        shift_state,
        range,
        est_range,
    })
}

fn parse_f64(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() { None } else { s.parse().ok() }
}

fn parse_i64(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() { None } else { s.parse().ok() }
}

/// Absolute deadline for the server's subscribe acknowledgment.
///
/// A single deadline shared by all pre-ack reads: a per-read timeout is not
/// enough because control frames (Ping, …) hit `continue` and would restart
/// it, letting a chatty-but-never-acking server hold the socket open forever.
const SUBSCRIBE_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Connect to the Tesla streaming API, subscribe to a vehicle, and forward
/// data points through the given channel until the stream ends.
pub(crate) async fn stream_vehicle_data(
    access_token: &str,
    vehicle_id: i64,
    vin: &str,
    data_tx: tokio::sync::mpsc::Sender<StreamingData>,
) -> StreamEndReason {
    stream_vehicle_data_with_url(
        access_token,
        vehicle_id,
        vin,
        data_tx,
        "wss://streaming.vn.teslamotors.com/streaming/",
    )
    .await
}

/// Same as [`stream_vehicle_data`], but against an explicit URL.
/// Production always uses the Tesla endpoint; tests point at a local server.
pub(crate) async fn stream_vehicle_data_with_url(
    access_token: &str,
    vehicle_id: i64,
    vin: &str,
    data_tx: tokio::sync::mpsc::Sender<StreamingData>,
    url: &str,
) -> StreamEndReason {
    use tokio_tungstenite::connect_async;

    let (ws_stream, _response) = match connect_async(url).await {
        Ok(c) => c,
        Err(e) => {
            warn!(%vin, error = %e, "streaming: connection failed");
            return StreamEndReason::IoError(e.to_string());
        }
    };

    info!(%vin, "streaming: connected, subscribing");

    let (mut write, mut read) = ws_stream.split();

    // OAuth tokens subscribe via `data:subscribe_oauth` with the column
    // list (`data:subscribe` expects the per-vehicle streaming token and is
    // silently ignored with an OAuth token).
    let subscribe = serde_json::json!({
        "msg_type": "data:subscribe_oauth",
        "token": access_token,
        "value": COLUMNS.join(","),
        "tag": vehicle_id.to_string(),
    })
    .to_string();

    if let Err(e) = write
        .send(tokio_tungstenite::tungstenite::Message::Text(subscribe))
        .await
    {
        warn!(%vin, error = %e, "streaming: subscribe send failed");
        return StreamEndReason::IoError(e.to_string());
    }

    // The server must acknowledge the subscription promptly: an asleep car
    // never answers, so bail instead of leaving the socket hanging (which
    // would also block the task's reconnect logic). The deadline is absolute
    // across all pre-ack reads — control frames answered with `continue`
    // below share it rather than restarting it.
    let ack_deadline = tokio::time::Instant::now() + SUBSCRIBE_ACK_TIMEOUT;
    let mut got_subscribe_ack = false;
    loop {
        let msg = if got_subscribe_ack {
            read.next().await
        } else {
            match tokio::time::timeout_at(ack_deadline, read.next()).await {
                Ok(v) => v,
                Err(_) => {
                    warn!(%vin, "streaming: subscribe response timeout");
                    break StreamEndReason::IoError("subscribe response timeout".into());
                }
            }
        };
        let Some(msg) = msg else {
            break StreamEndReason::Shutdown;
        };
        let text = match msg {
            Ok(tokio_tungstenite::tungstenite::Message::Text(t)) => t,
            Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                info!(%vin, "streaming: server closed connection");
                return StreamEndReason::Shutdown;
            }
            Ok(tokio_tungstenite::tungstenite::Message::Ping(p)) => {
                if let Err(e) = write
                    .send(tokio_tungstenite::tungstenite::Message::Pong(p))
                    .await
                {
                    warn!(%vin, error = %e, "streaming: pong failed");
                    return StreamEndReason::IoError(e.to_string());
                }
                continue;
            }
            Ok(_) => continue,
            Err(e) => {
                warn!(%vin, error = %e, "streaming: read error");
                return StreamEndReason::IoError(e.to_string());
            }
        };

        if !got_subscribe_ack {
            // The server greets with `control:hello` on connect: keep waiting
            // for proof of subscription instead of failing over it.
            if is_hello_frame(&text) {
                continue;
            }
            // Rejections can arrive before any ack: route them through the
            // shared error mapping so reconnects classify correctly.
            if let Some(reason) = termination_reason(&text) {
                return reason;
            }
            // Liveness is proven by a subscribe success or the first data
            // frame. Upstream never waits for an ack message at all (it keys
            // off data:update), so the latter must also end the wait —
            // otherwise a server that only sends data would time out here.
            match msg_type_of(&text).as_deref() {
                Some("data:subscribe:success") | Some("data:update") => {
                    got_subscribe_ack = true;
                    info!(%vin, "streaming: subscribed successfully");
                }
                _ => {
                    got_subscribe_ack = true;
                    match handle_subscribe_response(&text) {
                        Ok(()) => {
                            continue;
                        }
                        Err(reason) => return reason,
                    }
                }
            }
        }

        match classify_data_frame(&text) {
            DataFrame::Telemetry(csv) => match parse_csv_line(&csv) {
                Ok(data) => {
                    if data_tx.send(data).await.is_err() {
                        return StreamEndReason::Shutdown;
                    }
                }
                Err(e) => {
                    warn!(%vin, error = %e, line = %text, "streaming: failed to parse data");
                }
            },
            DataFrame::End(reason) => {
                warn!(%vin, reason = ?reason, "streaming: terminated mid-stream");
                return reason;
            }
            DataFrame::Ignored => {
                debug!(%vin, line = %text, "streaming: ignoring non-data frame");
            }
        }
    }
}

/// Whether a frame is the server's `control:hello` greeting.
fn is_hello_frame(text: &str) -> bool {
    msg_type_of(text) == Some("control:hello".to_string())
}

/// Extract `msg_type` from a JSON frame, if it is one.
fn msg_type_of(text: &str) -> Option<String> {
    if !text.trim_start().starts_with('{') {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|json| json["msg_type"].as_str().map(str::to_string))
}

/// One post-ack text frame, classified.
#[derive(Debug, PartialEq)]
enum DataFrame {
    /// CSV payload of a `data:update` frame (or a legacy raw CSV line).
    Telemetry(String),
    /// Termination frame carrying its end reason.
    End(StreamEndReason),
    /// Anything else (`control:hello`, unknown types): skip quietly.
    Ignored,
}

/// Classify a post-ack frame. Telemetry CSV hides in the `value` field of
/// `data:update` envelopes — parsing the raw frame as CSV was the reason no
/// data point ever survived even a working subscription.
fn classify_data_frame(text: &str) -> DataFrame {
    if let Some(reason) = termination_reason(text) {
        return DataFrame::End(reason);
    }
    let trimmed = text.trim_start();
    if !trimmed.starts_with('{') {
        return DataFrame::Telemetry(text.to_string());
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(json) => match json["msg_type"].as_str() {
            Some("data:update") => match json["value"].as_str() {
                Some(csv) => DataFrame::Telemetry(csv.to_string()),
                None => DataFrame::Ignored,
            },
            _ => DataFrame::Ignored,
        },
        Err(_) => DataFrame::Ignored,
    }
}

/// Parse the JSON response to the subscribe message.
/// Returns Ok(()) on success, or the appropriate `StreamEndReason` on error.
fn handle_subscribe_response(text: &str) -> Result<(), StreamEndReason> {
    let json: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            warn!(response = %text, "streaming: expected JSON subscribe response");
            return Err(StreamEndReason::IoError(
                "expected JSON subscribe response".into(),
            ));
        }
    };

    match json["msg_type"].as_str() {
        Some("data:subscribe:success") => Ok(()),
        Some("data:update:error") => {
            let error_type = json["error_type"].as_str().unwrap_or("unknown");
            let error_msg = json["error"].as_str().unwrap_or("unknown error");
            warn!(error_type, error = %error_msg, "streaming: subscribe error");
            Err(json_error_reason(&json))
        }
        Some(other) => {
            warn!(msg_type = %other, "streaming: unexpected subscribe response");
            Err(StreamEndReason::IoError(format!(
                "unexpected msg_type: {other}"
            )))
        }
        None => {
            warn!(response = %text, "streaming: subscribe response missing msg_type");
            Err(StreamEndReason::IoError("missing msg_type".into()))
        }
    }
}

/// Map a JSON error frame to its end reason.
///
/// Shared by the subscribe-ack path and mid-stream termination detection.
/// `vehicle_disconnected` arrives mid-stream (not at subscribe time) but
/// maps the same way as `vehicle_offline`.
fn json_error_reason(json: &serde_json::Value) -> StreamEndReason {
    let error_type = json["error_type"].as_str().unwrap_or("unknown");
    let error_msg = json["error"]
        .as_str()
        .or_else(|| json["value"].as_str())
        .unwrap_or("unknown error");
    match error_type {
        "vehicle_offline" | "vehicle_disconnected" => StreamEndReason::VehicleOffline,
        "token_expired" | "invalid_token" => StreamEndReason::TokenExpired,
        "client_error" if error_msg.starts_with("Can't validate token") => {
            StreamEndReason::TokenExpired
        }
        "vehicle_error" if error_msg == "Vehicle is offline" => StreamEndReason::VehicleOffline,
        _ => StreamEndReason::IoError(error_msg.to_string()),
    }
}

/// Detect a mid-stream JSON termination frame (`data:error` /
/// `data:update:error`, e.g. token expiry or vehicle disconnect after a
/// successful subscription). Returns `Some(reason)` only for error frames;
/// data frames (raw CSV) and anything else yield `None` so the caller falls
/// through to CSV parsing.
fn termination_reason(text: &str) -> Option<StreamEndReason> {
    if !text.trim_start().starts_with('{') {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    match json["msg_type"].as_str() {
        Some("data:error") | Some("data:update:error") => Some(json_error_reason(&json)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_csv_line() {
        // Real wire shape: time,speed,odometer,soc,elevation,est_heading,
        // est_lat,est_lng,power,shift_state,range,est_range,heading.
        let line = "1657180289188,2,17195.7,68,169,266,33.175985,-96.619818,1,D,235,245,268";
        let data = parse_csv_line(line).unwrap();
        assert_eq!(data.timestamp, 1657180289188);
        assert_eq!(data.speed, Some(2.0));
        assert_eq!(data.odometer, Some(17195.7));
        assert_eq!(data.soc, Some(68.0));
        assert_eq!(data.elevation, Some(169.0));
        assert_eq!(data.heading, Some(266.0));
        assert_eq!(data.latitude, Some(33.175985));
        assert_eq!(data.longitude, Some(-96.619818));
        assert_eq!(data.power, Some(1));
        assert_eq!(data.shift_state.as_deref(), Some("D"));
        assert_eq!(data.range, Some(235.0));
        assert_eq!(data.est_range, Some(245.0));
    }

    #[test]
    fn parse_partial_csv_line() {
        let line = "1700000000000,,,,,,,,,,,,";
        let data = parse_csv_line(line).unwrap();
        assert_eq!(data.timestamp, 1700000000000);
        assert!(data.speed.is_none());
        assert!(data.soc.is_none());
        assert!(data.odometer.is_none());
        assert!(data.elevation.is_none());
        assert!(data.heading.is_none());
        assert!(data.latitude.is_none());
        assert!(data.longitude.is_none());
        assert!(data.power.is_none());
        assert!(data.shift_state.is_none());
        assert!(data.range.is_none());
        assert!(data.est_range.is_none());
    }

    #[test]
    fn parse_partial_with_some_fields() {
        let line = "1700000000001,,17195.7,80,,,37.8,-122.5,,P,250,,";
        let data = parse_csv_line(line).unwrap();
        assert_eq!(data.timestamp, 1700000000001);
        assert!(data.speed.is_none());
        assert_eq!(data.odometer, Some(17195.7));
        assert_eq!(data.soc, Some(80.0));
        assert_eq!(data.latitude, Some(37.8));
        assert_eq!(data.longitude, Some(-122.5));
        assert_eq!(data.shift_state.as_deref(), Some("P"));
        assert_eq!(data.range, Some(250.0));
        assert!(data.power.is_none());
        assert!(data.est_range.is_none());
    }

    #[test]
    fn parse_invalid_timestamp() {
        let line = "not-a-number,,,,,,,,,,,,";
        let err = parse_csv_line(line).unwrap_err();
        assert!(err.contains("invalid timestamp"));
    }

    #[test]
    fn parse_wrong_field_count() {
        let line = "1700000000000,65.0,85";
        let err = parse_csv_line(line).unwrap_err();
        assert!(err.contains("expected 13 fields"));
    }

    #[test]
    fn parse_empty_line() {
        let err = parse_csv_line("").unwrap_err();
        assert!(err.contains("expected 13 fields, got 1"));
    }

    #[test]
    fn parse_negative_power() {
        let line = "1700000000000,,,,,,,,-5000,P,280,,";
        let data = parse_csv_line(line).unwrap();
        assert_eq!(data.power, Some(-5000));
        assert_eq!(data.shift_state.as_deref(), Some("P"));
        assert_eq!(data.range, Some(280.0));
    }

    #[test]
    fn handle_subscribe_success() {
        let json = r#"{"msg_type":"data:subscribe:success","tag":"12345"}"#;
        assert!(handle_subscribe_response(json).is_ok());
    }

    #[test]
    fn handle_subscribe_vehicle_offline() {
        let json = r#"{"msg_type":"data:update:error","tag":"12345","error_type":"vehicle_offline","error":"vehicle is offline or does not exist"}"#;
        let err = handle_subscribe_response(json).unwrap_err();
        assert_eq!(err, StreamEndReason::VehicleOffline);
    }

    #[test]
    fn handle_subscribe_token_expired() {
        let json = r#"{"msg_type":"data:update:error","tag":"12345","error_type":"token_expired","error":"token expired"}"#;
        let err = handle_subscribe_response(json).unwrap_err();
        assert_eq!(err, StreamEndReason::TokenExpired);
    }

    #[test]
    fn handle_subscribe_invalid_token() {
        let json = r#"{"msg_type":"data:update:error","tag":"12345","error_type":"invalid_token","error":"token is invalid"}"#;
        let err = handle_subscribe_response(json).unwrap_err();
        assert_eq!(err, StreamEndReason::TokenExpired);
    }

    #[test]
    fn handle_subscribe_unknown_error() {
        let json = r#"{"msg_type":"data:update:error","tag":"12345","error_type":"rate_limited","error":"too many requests"}"#;
        let err = handle_subscribe_response(json).unwrap_err();
        assert!(matches!(err, StreamEndReason::IoError(_)));
    }

    #[test]
    fn handle_subscribe_missing_msg_type() {
        let json = r#"{"tag":"12345"}"#;
        let err = handle_subscribe_response(json).unwrap_err();
        assert!(matches!(err, StreamEndReason::IoError(_)));
    }

    #[test]
    fn handle_subscribe_invalid_json() {
        let err = handle_subscribe_response("not json").unwrap_err();
        assert!(matches!(err, StreamEndReason::IoError(_)));
    }

    #[test]
    fn data_update_envelope_yields_csv() {
        let frame = r#"{"msg_type":"data:update","tag":"12345","value":"1657180289188,2,17195.7,68,169,266,33.175985,-96.619818,1,D,235,245,268"}"#;
        match classify_data_frame(frame) {
            DataFrame::Telemetry(csv) => {
                let data = parse_csv_line(&csv).unwrap();
                assert_eq!(data.timestamp, 1657180289188);
                assert_eq!(data.speed, Some(2.0));
                assert_eq!(data.latitude, Some(33.175985));
            }
            other => panic!("expected telemetry, got {other:?}"),
        }
    }

    #[test]
    fn data_update_without_value_is_ignored() {
        let frame = r#"{"msg_type":"data:update","tag":"12345"}"#;
        assert_eq!(classify_data_frame(frame), DataFrame::Ignored);
    }

    #[test]
    fn raw_csv_line_passes_through() {
        let line = "1657180289188,2,17195.7,68,169,266,33.175985,-96.619818,1,D,235,245,268";
        assert_eq!(
            classify_data_frame(line),
            DataFrame::Telemetry(line.to_string())
        );
    }

    #[test]
    fn hello_frame_detected() {
        let hello = r#"{"msg_type":"control:hello","connection_timeout":30}"#;
        assert!(is_hello_frame(hello));
        assert!(!is_hello_frame(
            r#"{"msg_type":"data:subscribe:success","tag":"12345"}"#
        ));
        assert!(!is_hello_frame("1657180289188,2,3"));
        assert_eq!(classify_data_frame(hello), DataFrame::Ignored);
    }

    #[test]
    fn error_frame_ends_with_reason() {
        let json = r#"{"msg_type":"data:error","tag":"12345","value":"disconnected","error_type":"vehicle_disconnected"}"#;
        assert_eq!(
            classify_data_frame(json),
            DataFrame::End(StreamEndReason::VehicleOffline)
        );
    }

    #[test]
    fn unvalidatable_token_maps_to_expired() {
        let json = r#"{"msg_type":"data:error","tag":"12345","value":"Can't validate token. ","error_type":"client_error"}"#;
        assert_eq!(
            termination_reason(json),
            Some(StreamEndReason::TokenExpired)
        );
    }

    #[test]
    fn offline_vehicle_error_maps_to_offline() {
        let json = r#"{"msg_type":"data:error","tag":"12345","value":"Vehicle is offline","error_type":"vehicle_error"}"#;
        assert_eq!(
            termination_reason(json),
            Some(StreamEndReason::VehicleOffline)
        );
    }

    /// Spins a local server speaking the documented greeting → data flow.
    /// Returns the client task handle and the data receiver.
    async fn hello_then(
        frames: Vec<tokio_tungstenite::tungstenite::Message>,
    ) -> (
        tokio::task::JoinHandle<StreamEndReason>,
        tokio::sync::mpsc::Receiver<StreamingData>,
    ) {
        use futures_util::SinkExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            // The subscribe frame is the protocol-critical part of this PR:
            // assert its shape instead of discarding it, so a regression in
            // msg_type, columns, token, or tag fails here rather than
            // silently in production.
            let first = futures_util::StreamExt::next(&mut ws)
                .await
                .expect("no subscribe frame")
                .expect("subscribe read failed");
            let text = match first {
                tokio_tungstenite::tungstenite::Message::Text(t) => t,
                other => panic!("expected text subscribe, got {other:?}"),
            };
            let subscribe: serde_json::Value =
                serde_json::from_str(&text).expect("subscribe is JSON");
            assert_eq!(subscribe["msg_type"], "data:subscribe_oauth");
            assert_eq!(subscribe["value"], COLUMNS.join(","));
            assert_eq!(subscribe["tag"], "123");
            assert_eq!(subscribe["token"], "token");
            for frame in frames {
                if ws.send(frame).await.is_err() {
                    return;
                }
            }
            // Hold the connection open; the test aborts the client when done.
            futures_util::future::pending::<()>().await;
        });

        let (data_tx, data_rx) = tokio::sync::mpsc::channel(64);
        // Test-only leak: `spawn` needs a `'static` future but the API takes
        // `&str` (production passes a literal).
        let url: &'static str = Box::leak(format!("ws://{addr}/").into_boxed_str());
        let join = tokio::spawn(stream_vehicle_data_with_url(
            "token", 123, "TESTVIN", data_tx, url,
        ));
        (join, data_rx)
    }

    #[tokio::test]
    async fn hello_then_data_update_forwards_point() {
        use tokio_tungstenite::tungstenite::Message;

        let csv = "1657180289188,2,17195.7,68,169,266,33.175985,-96.619818,1,D,235,245,268";
        let (join, mut data_rx) = hello_then(vec![
            Message::Text(r#"{"msg_type":"control:hello","connection_timeout":30}"#.into()),
            Message::Text(format!(
                r#"{{"msg_type":"data:update","tag":"123","value":"{csv}"}}"#
            )),
        ])
        .await;

        // No data:subscribe:success is ever sent: the first data frame must
        // end the ack wait on its own (upstream keys liveness off data).
        let point = tokio::time::timeout(Duration::from_secs(5), data_rx.recv())
            .await
            .expect("client hung")
            .expect("channel closed");
        assert_eq!(point.timestamp, 1657180289188);
        assert_eq!(point.speed, Some(2.0));
        assert_eq!(point.latitude, Some(33.175985));
        join.abort();
    }

    #[tokio::test]
    async fn pre_ack_error_returns_mapped_reason() {
        use tokio_tungstenite::tungstenite::Message;

        let (join, _data_rx) = hello_then(vec![Message::Text(
            r#"{"msg_type":"data:error","tag":"123","value":"Can't validate token. ","error_type":"client_error"}"#.into(),
        )])
        .await;

        // A rejection before any ack must classify (TokenExpired), not
        // IoError("unexpected msg_type").
        let reason = tokio::time::timeout(Duration::from_secs(5), join)
            .await
            .expect("client hung")
            .expect("client panicked");
        assert_eq!(reason, StreamEndReason::TokenExpired);
    }

    #[test]
    fn termination_mid_stream_token_expired() {
        let json = r#"{"msg_type":"data:update:error","tag":"12345","error_type":"token_expired","error":"token expired"}"#;
        assert_eq!(
            termination_reason(json),
            Some(StreamEndReason::TokenExpired)
        );
    }

    #[test]
    fn termination_mid_stream_vehicle_disconnected() {
        let json = r#"{"msg_type":"data:error","tag":"12345","value":"disconnected","error_type":"vehicle_disconnected"}"#;
        assert_eq!(
            termination_reason(json),
            Some(StreamEndReason::VehicleOffline)
        );
    }

    #[test]
    fn termination_mid_stream_unknown_error() {
        let json = r#"{"msg_type":"data:error","tag":"12345","error_type":"rate_limited","error":"too many requests"}"#;
        assert!(matches!(
            termination_reason(json),
            Some(StreamEndReason::IoError(_))
        ));
    }

    #[test]
    fn termination_ignores_data_frames() {
        assert_eq!(termination_reason("1700000000000,65.0,85"), None);
        assert_eq!(termination_reason("not json"), None);
        assert_eq!(
            termination_reason(r#"{"msg_type":"data:update","value":"1,2,3"}"#),
            None
        );
    }

    /// A server that keeps sending Pings without ever acknowledging the
    /// subscription must not hold the connection past the absolute deadline.
    /// Slow (~10s): run manually with
    /// `cargo test subscribe_ack_timeout -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "slow: waits out the ~10s subscribe-ack deadline"]
    async fn subscribe_ack_timeout_survives_ping_spam() {
        use tokio_tungstenite::tungstenite::Message;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            // Consume the subscribe message, then spam Pings and never ack.
            let _ = ws.next().await;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if ws.send(Message::Ping(vec![1, 2, 3])).await.is_err() {
                    break;
                }
            }
        });

        let (data_tx, _data_rx) = tokio::sync::mpsc::channel(64);
        let start = tokio::time::Instant::now();
        let reason = stream_vehicle_data_with_url(
            "token",
            123,
            "TESTVIN",
            data_tx,
            &format!("ws://{addr}/"),
        )
        .await;
        let elapsed = start.elapsed();

        assert!(
            matches!(reason, StreamEndReason::IoError(ref e) if e == "subscribe response timeout"),
            "unexpected end reason: {reason:?}"
        );
        assert!(
            elapsed >= SUBSCRIBE_ACK_TIMEOUT - Duration::from_secs(1)
                && elapsed < SUBSCRIBE_ACK_TIMEOUT + Duration::from_secs(20),
            "ack deadline not honored, elapsed: {elapsed:?}"
        );
    }
}
