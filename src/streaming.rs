use futures_util::{SinkExt, StreamExt};
use tracing::{info, warn};

/// A single data point from the Tesla streaming API.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StreamingData {
    pub timestamp: i64,
    pub speed: Option<f64>,
    pub soc: Option<f64>,
    pub odometer: Option<f64>,
    pub elevation: Option<f64>,
    pub heading: Option<f64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub power: Option<i64>,
    pub shift_state: Option<String>,
    pub range: Option<f64>,
}

/// Why a streaming connection ended.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StreamEndReason {
    VehicleOffline,
    TokenExpired,
    IoError(String),
    Shutdown,
}

/// Parse a single CSV line from the Tesla streaming API.
///
/// Format: `timestamp,speed,soc,odometer,elevation,heading,lat,lng,power,shift_state,range`
/// Empty fields represent missing/unknown values.
pub(crate) fn parse_csv_line(line: &str) -> Result<StreamingData, String> {
    let parts: Vec<&str> = line.split(',').collect();
    if parts.len() != 11 {
        return Err(format!("expected 11 fields, got {}", parts.len()));
    }

    let timestamp = parts[0]
        .parse::<i64>()
        .map_err(|e| format!("invalid timestamp: {e}"))?;

    let speed = parse_f64(parts[1]);
    let soc = parse_f64(parts[2]);
    let odometer = parse_f64(parts[3]);
    let elevation = parse_f64(parts[4]);
    let heading = parse_f64(parts[5]);
    let latitude = parse_f64(parts[6]);
    let longitude = parse_f64(parts[7]);
    let power = parse_i64(parts[8]);
    let range = parse_f64(parts[10]);

    let shift_state = if parts[9].is_empty() {
        None
    } else {
        Some(parts[9].to_string())
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
    })
}

fn parse_f64(s: &str) -> Option<f64> {
    if s.is_empty() { None } else { s.parse().ok() }
}

fn parse_i64(s: &str) -> Option<i64> {
    if s.is_empty() { None } else { s.parse().ok() }
}

/// Connect to the Tesla streaming API, subscribe to a vehicle, and forward
/// data points through the given channel until the stream ends.
pub(crate) async fn stream_vehicle_data(
    access_token: &str,
    vehicle_id: i64,
    data_tx: tokio::sync::mpsc::Sender<StreamingData>,
) -> StreamEndReason {
    use tokio_tungstenite::connect_async;

    let url = "wss://streaming.vn.teslamotors.com/streaming/";
    let (ws_stream, _response) = match connect_async(url).await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "streaming: connection failed");
            return StreamEndReason::IoError(e.to_string());
        }
    };

    info!("streaming: connected, subscribing");

    let (mut write, mut read) = ws_stream.split();

    let subscribe = serde_json::json!({
        "msg_type": "data:subscribe",
        "token": access_token,
        "value": vehicle_id.to_string(),
        "tag": vehicle_id.to_string(),
    })
    .to_string();

    if let Err(e) = write
        .send(tokio_tungstenite::tungstenite::Message::Text(subscribe))
        .await
    {
        warn!(error = %e, "streaming: subscribe send failed");
        return StreamEndReason::IoError(e.to_string());
    }

    let mut got_subscribe_ack = false;

    while let Some(msg) = read.next().await {
        let text = match msg {
            Ok(tokio_tungstenite::tungstenite::Message::Text(t)) => t,
            Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                info!("streaming: server closed connection");
                return StreamEndReason::Shutdown;
            }
            Ok(tokio_tungstenite::tungstenite::Message::Ping(p)) => {
                if write
                    .send(tokio_tungstenite::tungstenite::Message::Pong(p))
                    .await
                    .is_err()
                {
                    return StreamEndReason::IoError("pong failed".into());
                }
                continue;
            }
            Ok(_) => continue,
            Err(e) => {
                warn!(error = %e, "streaming: read error");
                return StreamEndReason::IoError(e.to_string());
            }
        };

        if !got_subscribe_ack {
            got_subscribe_ack = true;
            match handle_subscribe_response(&text) {
                Ok(()) => {
                    info!("streaming: subscribed successfully");
                    continue;
                }
                Err(reason) => return reason,
            }
        }

        match parse_csv_line(&text) {
            Ok(data) => {
                if data_tx.send(data).await.is_err() {
                    return StreamEndReason::Shutdown;
                }
            }
            Err(e) => {
                warn!(error = %e, line = %text, "streaming: failed to parse data");
            }
        }
    }

    StreamEndReason::Shutdown
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
            match error_type {
                "vehicle_offline" => Err(StreamEndReason::VehicleOffline),
                "token_expired" | "invalid_token" => Err(StreamEndReason::TokenExpired),
                _ => Err(StreamEndReason::IoError(error_msg.to_string())),
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_csv_line() {
        let line = "1700000000,65.0,85,50000.5,10.5,180,37.7749,-122.4194,12000,D,300";
        let data = parse_csv_line(line).unwrap();
        assert_eq!(data.timestamp, 1700000000);
        assert_eq!(data.speed, Some(65.0));
        assert_eq!(data.soc, Some(85.0));
        assert_eq!(data.odometer, Some(50000.5));
        assert_eq!(data.elevation, Some(10.5));
        assert_eq!(data.heading, Some(180.0));
        assert_eq!(data.latitude, Some(37.7749));
        assert_eq!(data.longitude, Some(-122.4194));
        assert_eq!(data.power, Some(12000));
        assert_eq!(data.shift_state.as_deref(), Some("D"));
        assert_eq!(data.range, Some(300.0));
    }

    #[test]
    fn parse_partial_csv_line() {
        let line = "1700000000,,,,,,,,,,";
        let data = parse_csv_line(line).unwrap();
        assert_eq!(data.timestamp, 1700000000);
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
    }

    #[test]
    fn parse_partial_with_some_fields() {
        let line = "1700000001,,80,,,,37.8,-122.5,,P,";
        let data = parse_csv_line(line).unwrap();
        assert_eq!(data.timestamp, 1700000001);
        assert!(data.speed.is_none());
        assert_eq!(data.soc, Some(80.0));
        assert_eq!(data.latitude, Some(37.8));
        assert_eq!(data.longitude, Some(-122.5));
        assert_eq!(data.shift_state.as_deref(), Some("P"));
        assert!(data.power.is_none());
        assert!(data.range.is_none());
    }

    #[test]
    fn parse_invalid_timestamp() {
        let line = "not-a-number,,,,,,,,,,";
        let err = parse_csv_line(line).unwrap_err();
        assert!(err.contains("invalid timestamp"));
    }

    #[test]
    fn parse_wrong_field_count() {
        let line = "1700000000,65.0,85";
        let err = parse_csv_line(line).unwrap_err();
        assert!(err.contains("expected 11 fields"));
    }

    #[test]
    fn parse_empty_line() {
        let err = parse_csv_line("").unwrap_err();
        assert!(err.contains("expected 11 fields, got 1"));
    }

    #[test]
    fn parse_negative_power() {
        let line = "1700000000,,,,,,,,-5000,P,280";
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
}
