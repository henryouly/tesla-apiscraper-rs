#![allow(dead_code)]

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::{
    Engine as _,
    engine::DecodePaddingMode,
    engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig},
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::warn;

const SCOPES: &str = "openid email offline_access vehicle_device_data";
const MAX_RETRIES: u32 = 3;
const BACKOFF_MAX: u64 = 120;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid or expired grant: {0}")]
    InvalidGrant(String),
    #[error("rate limited or authorization pending: {0}")]
    RateLimited(String),
    #[error("failed to decode region from access token: {0}")]
    RegionDecode(String),
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("API error (HTTP {status}): {body}")]
    Api { status: u16, body: String },
}

impl AuthError {
    fn is_retryable(&self) -> bool {
        match self {
            AuthError::Transport(_) | AuthError::RateLimited(_) => true,
            AuthError::Api { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AuthError::InvalidGrant(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AuthError::RateLimited(msg) => (StatusCode::TOO_MANY_REQUESTS, msg.clone()),
            AuthError::RegionDecode(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AuthError::Transport(e) => (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("upstream transport error: {e}"),
            ),
            AuthError::Api { status, body } => (
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY),
                body.clone(),
            ),
        };
        (status, Json(serde_json::json!({"error": message}))).into_response()
    }
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    #[serde(default)]
    pub id_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAuthorizeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone)]
pub struct Region {
    pub region: String,
    pub api_url: String,
}

impl Region {
    fn from_jwt_payload(payload: &str, default_api_url: &str) -> Result<Self, AuthError> {
        let json: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| AuthError::RegionDecode(format!("invalid JWT payload JSON: {e}")))?;

        let region = json
            .get("region")
            .and_then(|v| v.as_str())
            .or_else(|| json.get("cuc_region").and_then(|v| v.as_str()))
            .unwrap_or("global")
            .to_string();

        let api_url = match region.as_str() {
            "na" | "global" => "https://fleet-api.prd.na.vn.cloud.tesla.com",
            "eu" => "https://fleet-api.prd.eu.vn.cloud.tesla.com",
            "cn" => "https://fleet-api.prd.cn.vn.cloud.tesla.com",
            _ => default_api_url,
        }
        .to_string();

        Ok(Self { region, api_url })
    }
}

// ---------------------------------------------------------------------------
// Error response parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TokenErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

async fn parse_token_response(resp: reqwest::Response) -> Result<TokenResponse, AuthError> {
    let status = resp.status();
    let body = resp.text().await.map_err(AuthError::Transport)?;

    if status.is_success() {
        return serde_json::from_str(&body).map_err(|e| AuthError::Api {
            status: status.as_u16(),
            body: format!("failed to parse success response: {e}: {body}"),
        });
    }

    let api_err = match serde_json::from_str::<TokenErrorBody>(&body) {
        Ok(err) => match err.error.as_str() {
            "invalid_grant" | "expired_token" => {
                AuthError::InvalidGrant(err.error_description.unwrap_or_default())
            }
            "authorization_pending" | "slow_down" => {
                AuthError::RateLimited(err.error_description.unwrap_or_default())
            }
            _ => AuthError::Api {
                status: status.as_u16(),
                body: err.error,
            },
        },
        Err(_) => AuthError::Api {
            status: status.as_u16(),
            body,
        },
    };

    Err(api_err)
}

// ---------------------------------------------------------------------------
// JWT utilities
// ---------------------------------------------------------------------------

fn jwt_engine() -> GeneralPurpose {
    GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_padding_mode(DecodePaddingMode::Indifferent),
    )
}

fn decode_jwt_payload(token: &str) -> Result<String, AuthError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(AuthError::RegionDecode(
            "JWT must have exactly 3 dot-separated segments".into(),
        ));
    }
    let decoded = jwt_engine()
        .decode(parts[1])
        .map_err(|e| AuthError::RegionDecode(format!("invalid base64url payload: {e}")))?;
    String::from_utf8(decoded)
        .map_err(|e| AuthError::RegionDecode(format!("payload is not valid UTF-8: {e}")))
}

// ---------------------------------------------------------------------------
// TeslaAuthClient
// ---------------------------------------------------------------------------

pub struct TeslaAuthClient {
    client_id: String,
    client_secret: String,
    auth_url: String,
    default_api_url: String,
    http_client: reqwest::Client,
    consecutive_failures: AtomicU64,
    last_failure_time: Mutex<Option<Instant>>,
}

impl TeslaAuthClient {
    pub fn new(
        client_id: &str,
        client_secret: &str,
        auth_url: &str,
        default_api_url: &str,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest::Client builder is infallible");

        Self {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            auth_url: auth_url.trim_end_matches('/').to_string(),
            default_api_url: default_api_url.to_string(),
            http_client,
            consecutive_failures: AtomicU64::new(0),
            last_failure_time: Mutex::new(None),
        }
    }

    // -----------------------------------------------------------------------
    // Circuit breaker helpers
    // -----------------------------------------------------------------------

    fn check_breaker(&self) -> Result<(), AuthError> {
        let failures = self.consecutive_failures.load(Ordering::Relaxed);
        if failures == 0 {
            return Ok(());
        }
        if let Ok(Some(last)) = self.last_failure_time.lock().as_deref() {
            let backoff = (1u64 << failures.min(8)).min(BACKOFF_MAX);
            let elapsed = last.elapsed().as_secs();
            if elapsed < backoff {
                return Err(AuthError::RateLimited(format!(
                    "circuit breaker open ({failures} failures), retry in {}s",
                    backoff - elapsed
                )));
            }
        }
        Ok(())
    }

    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        if let Ok(mut last) = self.last_failure_time.lock() {
            *last = None;
        }
    }

    fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut last) = self.last_failure_time.lock() {
            *last = Some(Instant::now());
        }
    }

    // -----------------------------------------------------------------------
    // Device Authorization Grant
    // -----------------------------------------------------------------------

    async fn try_device_authorize(&self) -> Result<DeviceAuthorizeResponse, AuthError> {
        let resp = self
            .http_client
            .post(format!("{}/oauth2/v3/device/authorize", self.auth_url))
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("scope", SCOPES),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await.map_err(AuthError::Transport)?;

        if status.is_success() {
            return serde_json::from_str(&body).map_err(|e| AuthError::Api {
                status: status.as_u16(),
                body: format!("failed to parse device authorize response: {e}: {body}"),
            });
        }

        Err(match serde_json::from_str::<TokenErrorBody>(&body) {
            Ok(err) => AuthError::Api {
                status: status.as_u16(),
                body: format!(
                    "{}: {}",
                    err.error,
                    err.error_description.unwrap_or_default()
                ),
            },
            Err(_) => AuthError::Api {
                status: status.as_u16(),
                body,
            },
        })
    }

    pub async fn device_authorize(&self) -> Result<DeviceAuthorizeResponse, AuthError> {
        self.check_breaker()?;
        let mut delay = 1u64;
        for attempt in 0..=MAX_RETRIES {
            match self.try_device_authorize().await {
                Ok(resp) => {
                    self.record_success();
                    return Ok(resp);
                }
                Err(e) if attempt < MAX_RETRIES && e.is_retryable() => {
                    warn!(error = %e, attempt, "device authorize failed, retrying");
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    delay = (delay * 2).min(BACKOFF_MAX);
                }
                Err(e) => {
                    self.record_failure();
                    return Err(e);
                }
            }
        }
        unreachable!()
    }

    // -----------------------------------------------------------------------
    // Poll for device token
    // -----------------------------------------------------------------------

    async fn try_poll_device_token(&self, device_code: &str) -> Result<TokenResponse, AuthError> {
        let resp = self
            .http_client
            .post(format!("{}/oauth2/v3/token", self.auth_url))
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("device_code", device_code),
            ])
            .send()
            .await?;

        parse_token_response(resp).await
    }

    pub async fn poll_device_token(
        &self,
        device_code: &str,
        poll_interval: u64,
    ) -> Result<TokenResponse, AuthError> {
        self.check_breaker()?;
        let mut delay = poll_interval;
        let mut backoff = 1u64;
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }

            match self.try_poll_device_token(device_code).await {
                Ok(tokens) => {
                    self.record_success();
                    return Ok(tokens);
                }
                Err(AuthError::RateLimited(msg)) if attempt < MAX_RETRIES => {
                    warn!(%msg, attempt, "device auth not yet approved, retrying");
                    delay = (delay + backoff).min(30);
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
                Err(e) if attempt < MAX_RETRIES && e.is_retryable() => {
                    warn!(error = %e, attempt, "transient error polling device token, retrying");
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    delay = delay.max(backoff);
                }
                Err(e) => {
                    self.record_failure();
                    return Err(e);
                }
            }
        }
        unreachable!()
    }

    // -----------------------------------------------------------------------
    // Refresh tokens
    // -----------------------------------------------------------------------

    async fn try_refresh_tokens(&self, refresh_token: &str) -> Result<TokenResponse, AuthError> {
        let resp = self
            .http_client
            .post(format!("{}/oauth2/v3/token", self.auth_url))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await?;

        parse_token_response(resp).await
    }

    pub async fn refresh_tokens(&self, refresh_token: &str) -> Result<TokenResponse, AuthError> {
        self.check_breaker()?;
        let mut delay = 1u64;
        for attempt in 0..=MAX_RETRIES {
            match self.try_refresh_tokens(refresh_token).await {
                Ok(tokens) => {
                    self.record_success();
                    return Ok(tokens);
                }
                Err(e) if attempt < MAX_RETRIES && e.is_retryable() => {
                    warn!(error = %e, attempt, "token refresh failed, retrying");
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    delay = (delay * 2).min(BACKOFF_MAX);
                }
                Err(e) => {
                    self.record_failure();
                    return Err(e);
                }
            }
        }
        unreachable!()
    }

    // -----------------------------------------------------------------------
    // Region decode
    // -----------------------------------------------------------------------

    pub fn decode_region(&self, access_token: &str) -> Result<Region, AuthError> {
        let payload = decode_jwt_payload(access_token)?;
        Region::from_jwt_payload(&payload, &self.default_api_url)
    }

    // -----------------------------------------------------------------------
    // Accessors (for tests)
    // -----------------------------------------------------------------------

    pub fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    fn test_client(base_url: &str) -> TeslaAuthClient {
        TeslaAuthClient::new(
            "test-client",
            "test-secret",
            base_url,
            "https://default.api",
        )
    }

    fn make_jwt(payload: &serde_json::Value) -> String {
        let header = serde_json::json!({"alg": "ES256", "typ": "JWT"});
        let enc = |v: &serde_json::Value| -> String {
            let json = serde_json::to_string(v).unwrap();
            // Use unpadded encoding to match real JWTs (RFC 7515)
            jwt_engine().encode(json.as_bytes())
        };
        format!("{}.{}.dummysig", enc(&header), enc(payload))
    }

    // -----------------------------------------------------------------------
    // device_authorize
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn device_authorize_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/device/authorize"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "dc-123",
                "user_code": "ABC-DEF",
                "verification_uri": "https://tesla.com/activate",
                "verification_uri_complete": "https://tesla.com/activate?code=ABC-DEF",
                "expires_in": 1800,
                "interval": 5
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client.device_authorize().await.unwrap();
        assert_eq!(resp.device_code, "dc-123");
        assert_eq!(resp.user_code, "ABC-DEF");
        assert_eq!(resp.interval, 5);
    }

    #[tokio::test]
    async fn device_authorize_retries_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/device/authorize"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/device/authorize"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "dc-retry",
                "user_code": "XYZ-999",
                "verification_uri": "https://tesla.com/activate",
                "verification_uri_complete": "https://tesla.com/activate?code=XYZ-999",
                "expires_in": 1800,
                "interval": 5
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client.device_authorize().await.unwrap();
        assert_eq!(resp.device_code, "dc-retry");
    }

    #[tokio::test]
    async fn device_authorize_fails_after_max_retries() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/device/authorize"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let err = client.device_authorize().await.unwrap_err();
        assert!(matches!(err, AuthError::Api { status: 503, .. }));
    }

    // -----------------------------------------------------------------------
    // poll_device_token
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn poll_device_token_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-123",
                "refresh_token": "rt-456",
                "expires_in": 28800,
                "id_token": "id-789"
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client.poll_device_token("dc-123", 1).await.unwrap();
        assert_eq!(resp.access_token, "at-123");
        assert_eq!(resp.refresh_token, "rt-456");
        assert_eq!(resp.expires_in, 28800);
    }

    #[tokio::test]
    async fn poll_device_token_retries_on_pending() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "authorization_pending",
                "error_description": "User has not yet completed authorization"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-after-pending",
                "refresh_token": "rt-after-pending",
                "expires_in": 28800
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client.poll_device_token("dc-123", 0).await.unwrap();
        assert_eq!(resp.access_token, "at-after-pending");
    }

    #[tokio::test]
    async fn poll_device_token_fails_on_invalid_grant() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "The device code is invalid or expired"
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let err = client.poll_device_token("bad-code", 0).await.unwrap_err();
        assert!(matches!(err, AuthError::InvalidGrant(_)));
    }

    // -----------------------------------------------------------------------
    // refresh_tokens
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn refresh_tokens_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-refreshed",
                "refresh_token": "rt-refreshed",
                "expires_in": 28800
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client.refresh_tokens("old-rt").await.unwrap();
        assert_eq!(resp.access_token, "at-refreshed");
        assert_eq!(resp.refresh_token, "rt-refreshed");
    }

    #[tokio::test]
    async fn refresh_tokens_invalid_grant() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "The refresh token is invalid or expired"
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let err = client.refresh_tokens("expired-rt").await.unwrap_err();
        assert!(matches!(err, AuthError::InvalidGrant(_)));
        assert!(err.to_string().contains("expired"));
    }

    #[tokio::test]
    async fn refresh_tokens_retries_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(502))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-after-retry",
                "refresh_token": "rt-after-retry",
                "expires_in": 28800
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client.refresh_tokens("rt-retry").await.unwrap();
        assert_eq!(resp.access_token, "at-after-retry");
    }

    // -----------------------------------------------------------------------
    // decode_region
    // -----------------------------------------------------------------------

    #[test]
    fn decode_region_na() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"region": "na"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "na");
        assert_eq!(
            region.api_url,
            "https://fleet-api.prd.na.vn.cloud.tesla.com"
        );
    }

    #[test]
    fn decode_region_cn() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"region": "cn"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "cn");
        assert_eq!(
            region.api_url,
            "https://fleet-api.prd.cn.vn.cloud.tesla.com"
        );
    }

    #[test]
    fn decode_region_eu() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"region": "eu"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "eu");
        assert_eq!(
            region.api_url,
            "https://fleet-api.prd.eu.vn.cloud.tesla.com"
        );
    }

    #[test]
    fn decode_region_falls_back_to_cuc_region() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"cuc_region": "cn"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "cn");
    }

    #[test]
    fn decode_region_defaults_to_global() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"sub": "abc", "iss": "tesla"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "global");
        assert_eq!(
            region.api_url,
            "https://fleet-api.prd.na.vn.cloud.tesla.com"
        );
    }

    #[test]
    fn decode_region_unknown_region_uses_default() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"region": "au"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "au");
        assert_eq!(region.api_url, "https://default.api");
    }

    #[test]
    fn decode_region_invalid_jwt() {
        let client = test_client("http://localhost");
        let err = client.decode_region("not-a-jwt").unwrap_err();
        assert!(matches!(err, AuthError::RegionDecode(_)));
    }

    #[test]
    fn decode_region_invalid_base64() {
        let client = test_client("http://localhost");
        let jwt = format!("header.{}", jwt_engine().encode(b"not-json"));
        let jwt = format!("{jwt}.signature");
        let err = client.decode_region(&jwt).unwrap_err();
        assert!(matches!(err, AuthError::RegionDecode(_)));
    }

    // -----------------------------------------------------------------------
    // Circuit breaker
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn circuit_breaker_tracks_consecutive_failures() {
        let server = MockServer::start().await;
        // Always return 503 (for all retry attempts too)
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        // First call exhausts retries (4 attempts with MAX_RETRIES=3), records 1 failure
        assert!(client.refresh_tokens("rt").await.is_err());
        assert_eq!(client.consecutive_failures(), 1);
    }

    #[tokio::test]
    async fn circuit_breaker_resets_on_success() {
        let server = MockServer::start().await;
        // Use invalid_grant (non-retryable) so no retry loop interferes
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "expired"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Then success
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-reset",
                "refresh_token": "rt-reset",
                "expires_in": 28800
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        // First call fails immediately (invalid_grant is not retryable) → records 1 failure
        assert!(client.refresh_tokens("rt").await.is_err());
        assert_eq!(client.consecutive_failures(), 1);

        // Wait for circuit breaker to cool down (backoff for 1 failure = 2s)
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Now success resets counter
        let ok = client.refresh_tokens("rt").await;
        assert!(ok.is_ok());
        assert_eq!(client.consecutive_failures(), 0);
    }
}
