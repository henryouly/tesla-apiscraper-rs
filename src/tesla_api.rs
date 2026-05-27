use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vehicle {
    pub id: i64,
    pub vehicle_id: i64,
    pub vin: String,
    pub display_name: Option<String>,
    pub state: String,
    pub api_version: i64,
    pub in_service: bool,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub async fn list_products(
    access_token: &str,
    api_url: &str,
) -> Result<Vec<Vehicle>, crate::tesla_auth::AuthError> {
    let http_client = reqwest::Client::new();
    let url = format!("{}/api/1/products", api_url.trim_end_matches('/'));
    let resp = http_client
        .get(&url)
        .bearer_auth(access_token)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(crate::tesla_auth::AuthError::Api { status, body });
    }

    let json: serde_json::Value = resp.json().await?;
    serde_json::from_value(json["response"].clone()).map_err(|e| {
        crate::tesla_auth::AuthError::Api {
            status: 502,
            body: format!("invalid /api/1/products response: {e}"),
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tesla_auth::AuthError;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    const EXPECTED_VEHICLE_JSON: &str = r#"{
        "id": 12345678901234567,
        "vehicle_id": 987654321,
        "vin": "5YJSA1E26MF123456",
        "display_name": "My Tesla",
        "state": "online",
        "api_version": 18,
        "in_service": false
    }"#;

    #[tokio::test]
    async fn list_products_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .and(matchers::header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": [serde_json::from_str::<serde_json::Value>(EXPECTED_VEHICLE_JSON).unwrap()],
                "count": 1
            })))
            .mount(&server)
            .await;

        let vehicles = list_products("test-token", &server.uri()).await.unwrap();

        assert_eq!(vehicles.len(), 1);
        assert_eq!(vehicles[0].vin, "5YJSA1E26MF123456");
        assert_eq!(vehicles[0].display_name.as_deref(), Some("My Tesla"));
        assert_eq!(vehicles[0].state, "online");
        assert_eq!(vehicles[0].api_version, 18);
    }

    #[tokio::test]
    async fn list_products_multiple_vehicles() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": [
                    serde_json::from_str::<serde_json::Value>(EXPECTED_VEHICLE_JSON).unwrap(),
                    {
                        "id": 9999999,
                        "vehicle_id": 111111111,
                        "vin": "5YJRE11234A567890",
                        "display_name": null,
                        "state": "asleep",
                        "api_version": 18,
                        "in_service": false
                    }
                ],
                "count": 2
            })))
            .mount(&server)
            .await;

        let vehicles = list_products("any-token", &server.uri()).await.unwrap();

        assert_eq!(vehicles.len(), 2);
        assert_eq!(vehicles[0].vin, "5YJSA1E26MF123456");
        assert_eq!(vehicles[1].vin, "5YJRE11234A567890");
        assert!(vehicles[1].display_name.is_none());
        assert_eq!(vehicles[1].state, "asleep");
    }

    #[tokio::test]
    async fn list_products_returns_empty_when_no_vehicles() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": [],
                "count": 0
            })))
            .mount(&server)
            .await;

        let vehicles = list_products("token", &server.uri()).await.unwrap();
        assert!(vehicles.is_empty());
    }

    #[tokio::test]
    async fn list_products_401_error() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let err = list_products("bad-token", &server.uri()).await.unwrap_err();

        match err {
            AuthError::Api { status, .. } => assert_eq!(status, 401),
            _ => panic!("expected Api error"),
        }
    }

    #[tokio::test]
    async fn list_products_500_error() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/api/1/products"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&server)
            .await;

        let err = list_products("token", &server.uri()).await.unwrap_err();

        match err {
            AuthError::Api { status, .. } => assert_eq!(status, 500),
            _ => panic!("expected Api error"),
        }
    }
}
