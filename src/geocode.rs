use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use tracing::warn;

static CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("tesla-apiscraper-rs/0.1.0 (https://github.com/henryouly/tesla-apiscraper-rs)")
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest::Client builder is infallible")
});

#[cfg(not(test))]
const NOMINATIM_URL: &str = "https://nominatim.openstreetmap.org/reverse";
#[cfg(test)]
const NOMINATIM_URL: &str = "http://127.0.0.1:1/reverse";

fn cache_key(lat: f64, lng: f64) -> String {
    format!("{:.4}_{:.4}", lat, lng)
}

/// Resolve an address for a (lat, lng) pair.
/// Priority: cache > Nominatim API.
/// Returns a formatted address string, or `None` if unresolvable.
pub(crate) async fn resolve_address(lat: f64, lng: f64) -> Option<String> {
    let key = cache_key(lat, lng);
    {
        let cache = CACHE.lock().unwrap();
        match cache.get(&key) {
            Some(Some(v)) => return Some(v.clone()),
            Some(None) => return None,
            None => {}
        }
    }

    match lookup_address(lat, lng).await {
        Ok(Some(addr)) => {
            CACHE.lock().unwrap().insert(key, Some(addr.clone()));
            Some(addr)
        }
        Ok(None) => {
            CACHE.lock().unwrap().insert(key, None);
            None
        }
        Err(e) => {
            warn!(error = %e, lat, lng, "address lookup failed");
            None
        }
    }
}

/// Query the Nominatim reverse geocoding API for a single coordinate.
async fn lookup_address(lat: f64, lng: f64) -> Result<Option<String>> {
    let url = format!("{NOMINATIM_URL}?lat={lat}&lon={lng}&format=json&addressdetails=0");
    fetch_address(&url).await
}

/// Core HTTP + parsing logic (URL-injectable for testing).
async fn fetch_address(url: &str) -> Result<Option<String>> {
    let resp = CLIENT.get(url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Nominatim returned HTTP {}", resp.status());
    }
    let body: serde_json::Value = resp.json().await?;
    match body["display_name"].as_str() {
        Some(s) if !s.is_empty() => Ok(Some(s.to_string())),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_response() {
        let json = serde_json::json!({
            "place_id": 12345,
            "display_name": "1600 Amphitheatre Parkway, Mountain View, CA 94043, USA",
            "lat": "37.422",
            "lon": "-122.084"
        });
        let addr = json["display_name"].as_str().unwrap();
        assert_eq!(
            addr,
            "1600 Amphitheatre Parkway, Mountain View, CA 94043, USA"
        );
    }

    #[test]
    fn parse_empty_display_name() {
        let json = serde_json::json!({
            "place_id": 0,
            "display_name": "",
            "lat": "0.0",
            "lon": "0.0"
        });
        let addr = json["display_name"].as_str().unwrap();
        assert!(addr.is_empty());
    }

    #[tokio::test]
    async fn cache_hit_skips_http() {
        let mut cache = CACHE.lock().unwrap();
        cache.insert("37.7749_-122.4194".into(), Some("San Francisco".into()));
        drop(cache);

        let result = resolve_address(37.7749, -122.4194).await;
        assert_eq!(result, Some("San Francisco".into()));
    }

    #[tokio::test]
    async fn returns_address_from_api() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "display_name": "48 Rue de Rivoli, Paris, France"
                })),
            )
            .mount(&server)
            .await;

        let url = format!("{}/test", server.uri());
        let result = fetch_address(&url).await.unwrap();
        assert_eq!(result, Some("48 Rue de Rivoli, Paris, France".into()));
    }

    #[tokio::test]
    async fn returns_none_on_empty_response() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "display_name": ""
                })),
            )
            .mount(&server)
            .await;

        let url = format!("{}/test", server.uri());
        let result = fetch_address(&url).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn returns_error_on_server_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let url = format!("{}/test", server.uri());
        assert!(fetch_address(&url).await.is_err());
    }
}
