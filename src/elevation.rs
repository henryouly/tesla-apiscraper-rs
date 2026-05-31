use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use tracing::warn;

static CACHE: LazyLock<Mutex<HashMap<String, Option<f64>>>> = LazyLock::new(Default::default);

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest::Client builder is infallible")
});

#[cfg(not(test))]
const SRTM_API_URL: &str = "https://api.opentopodata.org/v1/srtm30m";
#[cfg(test)]
const SRTM_API_URL: &str = "http://127.0.0.1:1/srtm30m";

fn cache_key(lat: f64, lng: f64) -> String {
    format!("{:.4}_{:.4}", lat, lng)
}

/// Resolve elevation for a (lat, lng) pair.
/// Priority: cache > SRTM API.
/// Returns the elevation in meters, or `None` if unresolvable.
pub(crate) async fn resolve_elevation(lat: f64, lng: f64) -> Option<f64> {
    let key = cache_key(lat, lng);
    {
        let cache = CACHE.lock().unwrap();
        match cache.get(&key) {
            Some(Some(v)) => return Some(*v),
            Some(None) => return None,
            None => {}
        }
    }

    match lookup_elevation(lat, lng).await {
        Ok(Some(e)) => {
            CACHE.lock().unwrap().insert(key, Some(e));
            Some(e)
        }
        Ok(None) => {
            CACHE.lock().unwrap().insert(key, None);
            None
        }
        Err(e) => {
            warn!(error = %e, lat, lng, "elevation lookup failed");
            None
        }
    }
}

/// Query the OpenTopoData SRTM API for elevation at a single coordinate.
async fn lookup_elevation(lat: f64, lng: f64) -> Result<Option<f64>> {
    let url = format!("{SRTM_API_URL}?locations={lat},{lng}");
    fetch_elevation(&url).await
}

/// Core HTTP + parsing logic (URL-injectable for testing).
async fn fetch_elevation(url: &str) -> Result<Option<f64>> {
    let resp = CLIENT.get(url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("SRTM API returned HTTP {}", resp.status());
    }
    let body: serde_json::Value = resp.json().await?;
    let results = body["results"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing results array"))?;
    if results.is_empty() {
        return Ok(None);
    }
    Ok(results[0]["elevation"].as_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_response() {
        let json = serde_json::json!({
            "results": [{
                "dataset": "srtm30m",
                "elevation": 10.0,
                "location": { "lat": 37.7749, "lng": -122.4194 }
            }],
            "status": "OK"
        });
        let results = json["results"].as_array().unwrap();
        let elevation = results[0]["elevation"].as_f64();
        assert_eq!(elevation, Some(10.0));
    }

    #[test]
    fn parse_no_data_response() {
        let json = serde_json::json!({
            "results": [],
            "status": "NO_DATA"
        });
        let results = json["results"].as_array().unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn parse_missing_elevation() {
        let json = serde_json::json!({
            "results": [{
                "dataset": "srtm30m",
                "location": { "lat": 0.0, "lng": 0.0 }
            }],
            "status": "OK"
        });
        let results = json["results"].as_array().unwrap();
        let elevation = results[0]["elevation"].as_f64();
        assert!(elevation.is_none());
    }

    #[tokio::test]
    async fn returns_elevation_from_api() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "results": [{
                        "dataset": "srtm30m",
                        "elevation": 52.0,
                        "location": { "lat": 48.8566, "lng": 2.3522 }
                    }],
                    "status": "OK"
                })),
            )
            .mount(&server)
            .await;

        let url = format!("{}/test?locations=48.8566,2.3522", server.uri());
        let result = fetch_elevation(&url).await.unwrap();
        assert_eq!(result, Some(52.0));
    }

    #[tokio::test]
    async fn returns_error_on_server_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let url = format!("{}/test", server.uri());
        assert!(fetch_elevation(&url).await.is_err());
    }
}
