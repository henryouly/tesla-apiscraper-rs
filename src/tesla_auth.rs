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

const SCOPES: &str = "openid email offline_access";
const MAX_RETRIES: u32 = 3;
const BACKOFF_MAX: u64 = 120;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid or expired grant: {0}")]
    InvalidGrant(String),
    #[error("rate limited: {0}")]
    RateLimited(String),
    #[error("failed to decode region from access token: {0}")]
    RegionDecode(String),
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("upstream {server} error (HTTP {status}): {details}")]
    Upstream {
        server: String,
        status: u16,
        details: String,
    },
    #[error("API error (HTTP {status}): {body}")]
    Api { status: u16, body: String },
}

impl AuthError {
    fn is_retryable(&self) -> bool {
        match self {
            AuthError::Transport(_) | AuthError::RateLimited(_) => true,
            AuthError::Api { status, .. } | AuthError::Upstream { status, .. } => *status >= 500,
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
            AuthError::Upstream {
                server, details, ..
            } => (
                StatusCode::BAD_GATEWAY,
                format!("upstream {server} error: {details}"),
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

#[derive(Debug, Clone)]
pub struct Region {
    pub region: String,
    pub api_url: String,
}

impl Region {
    fn from_jwt_payload(payload: &str, default_api_url: &str) -> Result<Self, AuthError> {
        let json: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| AuthError::RegionDecode(format!("invalid JWT payload JSON: {e}")))?;

        let aud = json.get("aud").and_then(|v| v.as_str()).unwrap_or("");

        let (region, api_url) = if aud.ends_with(".cn") || aud.contains(".cn/") {
            ("cn", "https://owner-api.vn.cloud.tesla.cn")
        } else if aud.ends_with(".eu") || aud.contains(".eu/") {
            ("eu", "https://owner-api.vn.cloud.tesla.eu")
        } else if aud.contains("owner-api") {
            ("na", "https://owner-api.teslamotors.com")
        } else {
            ("global", default_api_url)
        };

        Ok(Self {
            region: region.to_string(),
            api_url: api_url.to_string(),
        })
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
            status: 502,
            body: format!("failed to parse success response: {e}"),
        });
    }

    let api_err = match serde_json::from_str::<TokenErrorBody>(&body) {
        Ok(err) => match err.error.as_str() {
            "invalid_grant" | "expired_token" => {
                AuthError::InvalidGrant(err.error_description.unwrap_or_default())
            }
            _ => {
                let details = match err.error_description {
                    Some(desc) if !desc.is_empty() => format!("{} ({})", err.error, desc),
                    _ => err.error,
                };
                AuthError::Upstream {
                    server: "Tesla auth".into(),
                    status: status.as_u16(),
                    details,
                }
            }
        },
        Err(_) => AuthError::Upstream {
            server: "Tesla auth".into(),
            status: status.as_u16(),
            details: body,
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
    auth_url: String,
    default_api_url: String,
    http_client: reqwest::Client,
    consecutive_failures: AtomicU64,
    last_failure_time: Mutex<Option<Instant>>,
}

impl TeslaAuthClient {
    pub fn new(client_id: &str, auth_url: &str, default_api_url: &str) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest::Client builder is infallible");

        Self {
            client_id: client_id.to_string(),
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
    // Sign in
    // -----------------------------------------------------------------------

    pub async fn sign_in(
        &self,
        _access_token: &str,
        refresh_token: &str,
    ) -> Result<TokenResponse, AuthError> {
        self.refresh_tokens(refresh_token).await
    }

    // -----------------------------------------------------------------------
    // Refresh tokens
    // -----------------------------------------------------------------------

    async fn try_refresh_tokens(&self, refresh_token: &str) -> Result<TokenResponse, AuthError> {
        let url = format!("{}/oauth2/v3/token", self.auth_url);
        let resp = self
            .http_client
            .post(&url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id.as_str()),
                ("refresh_token", refresh_token),
                ("scope", SCOPES),
            ])
            .send()
            .await?;

        let result = parse_token_response(resp).await;
        if let Err(ref e) = result {
            warn!(upstream = %url, error = %e, "upstream Tesla auth server returned error");
        }
        result
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
                    let is_client_error = matches!(
                        &e,
                        AuthError::InvalidGrant(_)
                            | AuthError::RegionDecode(_)
                            | AuthError::Upstream {
                                status: 400..=499,
                                ..
                            }
                    );
                    if !is_client_error {
                        self.record_failure();
                    }
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
        TeslaAuthClient::new("test-client", base_url, "https://default.api")
    }

    fn make_jwt(payload: &serde_json::Value) -> String {
        let header = serde_json::json!({"alg": "ES256", "typ": "JWT"});
        let enc = |v: &serde_json::Value| -> String {
            let json = serde_json::to_string(v).unwrap();
            jwt_engine().encode(json.as_bytes())
        };
        format!("{}.{}.dummysig", enc(&header), enc(payload))
    }

    // -----------------------------------------------------------------------
    // sign_in
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sign_in_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-signed-in",
                "refresh_token": "rt-signed-in",
                "expires_in": 28800
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client.sign_in("old-at", "old-rt").await.unwrap();
        assert_eq!(resp.access_token, "at-signed-in");
        assert_eq!(resp.refresh_token, "rt-signed-in");
    }

    #[tokio::test]
    async fn sign_in_invalid_refresh_token() {
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
        let err = client.sign_in("old-at", "bad-rt").await.unwrap_err();
        assert!(matches!(err, AuthError::InvalidGrant(_)));
        assert!(err.to_string().contains("expired"));
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

    #[tokio::test]
    async fn refresh_tokens_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "login_required",
                "error_description": "Authentication required"
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let err = client.refresh_tokens("bad-rt").await.unwrap_err();
        assert!(matches!(err, AuthError::Upstream { .. }));
        assert!(err.to_string().contains("Tesla auth"));
        assert!(err.to_string().contains("login_required"));
    }

    // -----------------------------------------------------------------------
    // decode_region
    // -----------------------------------------------------------------------

    #[test]
    fn decode_region_na() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"aud": "https://owner-api.teslamotors.com"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "na");
        assert_eq!(region.api_url, "https://owner-api.teslamotors.com");
    }

    #[test]
    fn decode_region_cn() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"aud": "https://owner-api.vn.cloud.tesla.cn"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "cn");
        assert_eq!(region.api_url, "https://owner-api.vn.cloud.tesla.cn");
    }

    #[test]
    fn decode_region_eu() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"aud": "https://owner-api.vn.cloud.tesla.eu"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "eu");
        assert_eq!(region.api_url, "https://owner-api.vn.cloud.tesla.eu");
    }

    #[test]
    fn decode_region_defaults_to_global() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"sub": "abc", "iss": "tesla"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "global");
        assert_eq!(region.api_url, "https://default.api");
    }

    #[test]
    fn decode_region_unknown_aud_uses_default() {
        let client = test_client("http://localhost");
        let jwt = make_jwt(&serde_json::json!({"aud": "https://strange-audience.example.com"}));
        let region = client.decode_region(&jwt).unwrap();
        assert_eq!(region.region, "global");
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
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        assert!(client.refresh_tokens("rt").await.is_err());
        assert_eq!(client.consecutive_failures(), 1);
    }

    #[tokio::test]
    async fn circuit_breaker_resets_on_success() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/oauth2/v3/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "expired"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
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
        assert!(client.refresh_tokens("rt").await.is_err());
        // InvalidGrant is not recorded as a breaker failure
        assert_eq!(client.consecutive_failures(), 0);

        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::time::resume();

        let ok = client.refresh_tokens("rt").await;
        assert!(ok.is_ok());
        assert_eq!(client.consecutive_failures(), 0);
    }
}
