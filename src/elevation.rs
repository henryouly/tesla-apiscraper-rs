use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

use anyhow::Result;
use tracing::warn;

static CACHE: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);

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
        if let Some(&e) = cache.get(&key) {
            return Some(e);
        }
    }

    match lookup_elevation(lat, lng).await {
        Ok(Some(e)) => {
            CACHE.lock().unwrap().insert(key, e);
            Some(e)
        }
        Ok(None) => {
            CACHE.lock().unwrap().insert(key, 0.0);
            None
        }
        Err(e) => {
            warn!(error = %e, lat, lng, "elevation lookup failed");
            None
        }
    }
}

#[cfg(not(test))]
const SRTM_API_URL: &str = "https://api.opentopodata.org/v1/srtm30m";
#[cfg(test)]
const SRTM_API_URL: &str = "http://127.0.0.1:1/srtm30m";

/// Query the OpenTopoData SRTM API for elevation at a single coordinate.
async fn lookup_elevation(lat: f64, lng: f64) -> Result<Option<f64>> {
    let url = format!("{SRTM_API_URL}?locations={lat},{lng}");
    let resp = reqwest::get(&url).await?;
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
    async fn elevation_api_success() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v1/srtm30m"))
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

        let url = format!("{}/v1/srtm30m?locations=48.8566,2.3522", server.uri());
        let resp = reqwest::get(&url).await.unwrap();
        let body: serde_json::Value = resp.json().await.unwrap();
        let elevation = body["results"][0]["elevation"].as_f64().unwrap();
        assert!((elevation - 52.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn elevation_api_server_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v1/srtm30m"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let url = format!("{}/v1/srtm30m?locations=0,0", server.uri());
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 500);
    }

}
