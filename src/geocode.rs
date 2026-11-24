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

fn nominatim_url(lat: f64, lng: f64) -> String {
    format!("{NOMINATIM_URL}?lat={lat}&lon={lng}&format=json&addressdetails=1")
}

/// Resolve an address for a (lat, lng) pair.
/// Priority: cache > Nominatim API.
/// Returns a city-level formatted address (`"<street>, <city>"`) built from
/// Nominatim's structured `address` object, falling back to `display_name`
/// only when both street and city components are absent, or `None` if
/// unresolvable.
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
    let url = nominatim_url(lat, lng);
    fetch_address(&url).await
}

fn first_non_empty<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter()
        .filter_map(|k| obj.get(*k).and_then(|v| v.as_str()))
        .map(str::trim)
        .find(|s| !s.is_empty())
}

/// Extract a `"<street>, <city>"` label from a Nominatim reverse-geocode JSON
/// response, falling back to `display_name` when neither component is present.
fn format_address(body: &serde_json::Value) -> Option<String> {
    if let Some(addr) = body.get("address").and_then(|v| v.as_object()) {
        let road = first_non_empty(
            addr,
            &[
                "road",
                "pedestrian",
                "footway",
                "residential",
                "cycleway",
                "path",
            ],
        );
        let house_number = first_non_empty(addr, &["house_number"]);
        let street = road.map(|r| match house_number {
            Some(num) => format!("{num} {r}"),
            None => r.to_string(),
        });
        let city = first_non_empty(addr, &["city", "town", "village", "municipality"]);

        match (street, city) {
            (Some(s), Some(c)) => return Some(format!("{s}, {c}")),
            (Some(s), None) => return Some(s),
            (None, Some(c)) => return Some(c.to_string()),
            (None, None) => {}
        }
    }

    body.get("display_name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Core HTTP + parsing logic (URL-injectable for testing).
async fn fetch_address(url: &str) -> Result<Option<String>> {
    let resp = CLIENT.get(url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Nominatim returned HTTP {}", resp.status());
    }
    let body: serde_json::Value = resp.json().await?;
    Ok(format_address(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominatim_url_requests_addressdetails() {
        let url = nominatim_url(37.422, -122.084);
        assert!(url.contains("format=json"), "got: {url}");
        assert!(url.contains("addressdetails=1"), "got: {url}");
        assert!(url.contains("lat=37.422"), "got: {url}");
        assert!(url.contains("lon=-122.084"), "got: {url}");
    }

    #[test]
    fn formats_street_and_city_from_structured_address() {
        let json = serde_json::json!({
            "place_id": 12345,
            "display_name": "1600 Amphitheatre Parkway, Mountain View, California, USA, 95014",
            "address": {
                "house_number": "1600",
                "road": "Amphitheatre Parkway",
                "city": "Mountain View",
                "state": "California",
                "postcode": "95014",
                "country": "USA"
            }
        });
        assert_eq!(
            format_address(&json),
            Some("1600 Amphitheatre Parkway, Mountain View".into())
        );
    }

    #[test]
    fn formats_road_without_house_number_and_city_fallbacks() {
        for (city_key, city_val) in [
            ("city", "San Francisco"),
            ("town", "Los Gatos"),
            ("village", "Carmel"),
            ("municipality", "Palo Alto"),
        ] {
            let mut addr = serde_json::Map::new();
            addr.insert("road".into(), serde_json::json!("Main Street"));
            addr.insert(city_key.into(), serde_json::json!(city_val));
            let json = serde_json::json!({ "address": addr });
            assert_eq!(
                format_address(&json),
                Some(format!("Main Street, {city_val}")),
                "city_key={city_key}"
            );
        }

        // Non-road street fallback (e.g. pedestrian mall)
        let pedestrian = serde_json::json!({
            "address": {
                "pedestrian": "Castro Street",
                "city": "Mountain View"
            }
        });
        assert_eq!(
            format_address(&pedestrian),
            Some("Castro Street, Mountain View".into())
        );
    }

    #[test]
    fn formats_street_only_or_city_only_without_stray_comma() {
        let street_only = serde_json::json!({
            "address": {
                "road": "Interstate 280",
                "county": "San Mateo County"
            },
            "display_name": "Interstate 280, San Mateo County, California, USA"
        });
        assert_eq!(format_address(&street_only), Some("Interstate 280".into()));

        let city_only = serde_json::json!({
            "address": {
                "house_number": "42",
                "city": "Paris",
                "country": "France"
            },
            "display_name": "42, Paris, France"
        });
        assert_eq!(format_address(&city_only), Some("Paris".into()));
    }

    #[test]
    fn falls_back_to_display_name_when_structured_address_absent_or_empty() {
        let no_addr = serde_json::json!({
            "display_name": "Pacific Ocean"
        });
        assert_eq!(format_address(&no_addr), Some("Pacific Ocean".into()));

        let no_street_or_city = serde_json::json!({
            "address": {
                "state": "California",
                "country": "USA"
            },
            "display_name": "Yosemite National Park, California, USA"
        });
        assert_eq!(
            format_address(&no_street_or_city),
            Some("Yosemite National Park, California, USA".into())
        );

        let blank = serde_json::json!({
            "address": { "road": "  ", "city": "" },
            "display_name": "   "
        });
        assert_eq!(format_address(&blank), None);
    }

    #[tokio::test]
    async fn cache_hit_skips_http() {
        {
            let mut cache = CACHE.lock().unwrap();
            cache.insert("37.7749_-122.4194".into(), Some("San Francisco".into()));
        }

        let result = resolve_address(37.7749, -122.4194).await;
        assert_eq!(result, Some("San Francisco".into()));
    }

    #[tokio::test]
    async fn returns_address_from_api() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "display_name": "48 Rue de Rivoli, Paris, Île-de-France, 75004, France",
                    "address": {
                        "house_number": "48",
                        "road": "Rue de Rivoli",
                        "city": "Paris",
                        "state": "Île-de-France",
                        "postcode": "75004",
                        "country": "France"
                    }
                })),
            )
            .mount(&server)
            .await;

        let url = format!("{}/test", server.uri());
        let result = fetch_address(&url).await.unwrap();
        assert_eq!(result, Some("48 Rue de Rivoli, Paris".into()));
    }

    #[tokio::test]
    async fn returns_street_and_city_for_us_address_dropping_state_and_postcode() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "display_name": "1600 Amphitheatre Parkway, Mountain View, Santa Clara County, California, 94043, USA",
                    "address": {
                        "house_number": "1600",
                        "road": "Amphitheatre Parkway",
                        "city": "Mountain View",
                        "county": "Santa Clara County",
                        "state": "California",
                        "postcode": "94043",
                        "country": "United States"
                    }
                })),
            )
            .mount(&server)
            .await;

        let url = format!("{}/test", server.uri());
        let result = fetch_address(&url).await.unwrap();
        assert_eq!(
            result,
            Some("1600 Amphitheatre Parkway, Mountain View".into())
        );
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
