mod api;
mod config;
mod config_yaml;
mod encryption;
mod influxdb;
mod tesla_auth;

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use tracing_subscriber::fmt::format::FmtSpan;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let builder = tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::builder()
            .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
            .from_env_lossy(),
    );

    if std::env::var("LOG_FORMAT").as_deref() == Ok("json") {
        builder
            .json()
            .with_target(true)
            .with_span_events(FmtSpan::CLOSE)
            .init();
    } else {
        builder
            .compact()
            .with_target(true)
            .with_span_events(FmtSpan::CLOSE)
            .init();
    }

    // ── Env configuration ───────────────────────────────────────────
    let env = config::Config::load()?;
    info!(config_dir = ?env.config_dir, "environment config loaded");

    // ── YAML config (with token encryption) ─────────────────────────
    let encryption_key = encryption::hex_to_key(&env.data_encryption_key)?;
    let yaml = Arc::new(Mutex::new(config_yaml::YamlConfigManager::load(
        &env.config_dir,
    )?));
    let geo_len = yaml.lock().unwrap().geofences.geofences.len();
    let cars_len = yaml.lock().unwrap().settings.cars.len();
    let token_valid = yaml
        .lock()
        .unwrap()
        .decrypt_tokens(&encryption_key)
        .is_some_and(|r| r.is_ok());
    info!(
        geofences = geo_len,
        cars = cars_len,
        authenticated = token_valid,
        "YAML config loaded"
    );

    // ── InfluxDB ────────────────────────────────────────────────────
    let db = Arc::new(influxdb::InfluxDb::new(
        &env.influxdb_url,
        &env.influxdb_token,
        &env.influxdb_database,
    )?);

    db.ping().await?;
    info!("InfluxDB connection OK");

    db.ensure_database().await?;
    info!(database = %env.influxdb_database, "InfluxDB database ready");

    // ── Tesla auth client ───────────────────────────────────────────
    let auth = Arc::new(tesla_auth::TeslaAuthClient::new(
        &env.tesla_api_client_id,
        &env.tesla_auth_url,
        &env.tesla_api_url,
    ));
    info!("Tesla auth client initialized");

    // ── Startup token validation ────────────────────────────────────
    try_use_stored_tokens(&yaml, &auth, &encryption_key).await;

    // ── Background auto-refresh ─────────────────────────────────────
    let refresh_yaml = Arc::clone(&yaml);
    let refresh_auth = Arc::clone(&auth);
    tokio::spawn(async move {
        token_auto_refresh_loop(refresh_yaml, refresh_auth, encryption_key).await;
    });

    // ── HTTP server ─────────────────────────────────────────────────
    let state = api::AppState {
        db,
        auth,
        yaml,
        encryption_key,
    };
    let router = api::create_router(state);

    let listener = tokio::net::TcpListener::bind(env.listen_addr()).await?;
    info!(addr = %env.listen_addr(), "HTTP server started");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Attempt to decrypt stored tokens and validate them at startup.
/// If expired or within 1 hour of expiry, refresh automatically.
async fn try_use_stored_tokens(
    yaml: &Arc<Mutex<config_yaml::YamlConfigManager>>,
    auth: &Arc<tesla_auth::TeslaAuthClient>,
    key: &[u8; 32],
) {
    let (_access_token, refresh_token, expires_at) = {
        let yaml = yaml.lock().unwrap();
        match yaml.decrypt_tokens(key) {
            None => {
                info!("no stored tokens found — require manual sign-in");
                return;
            }
            Some(Err(e)) => {
                warn!(error = %e, "failed to decrypt stored tokens — will overwrite");
                return;
            }
            Some(Ok(t)) => t,
        }
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    if expires_at - now <= 0 {
        info!("stored tokens expired, attempting refresh");
    } else {
        let remaining = expires_at - now;
        if remaining <= 3600 {
            info!(
                remaining_secs = remaining,
                "tokens approaching expiry, attempting refresh"
            );
        } else {
            info!(remaining_secs = remaining, "stored tokens valid");
            return;
        }
    }

    match auth.refresh_tokens(&refresh_token).await {
        Ok(tokens) => {
            let new_expires_at = now + tokens.expires_in as i64;
            if let Ok(mut yaml) = yaml.lock()
                && let Err(e) = yaml.set_encrypted_tokens(
                    key,
                    &tokens.access_token,
                    &tokens.refresh_token,
                    new_expires_at,
                )
            {
                warn!(error = %e, "failed to persist refreshed tokens");
            }
            info!("stored tokens refreshed successfully at startup");
        }
        Err(e) => {
            warn!(error = %e, "failed to refresh stored tokens — manual sign-in required");
        }
    }
}

/// Background loop that checks token expiry every 60 seconds and refreshes
/// when within 1 hour of expiry.
async fn token_auto_refresh_loop(
    yaml: Arc<Mutex<config_yaml::YamlConfigManager>>,
    auth: Arc<tesla_auth::TeslaAuthClient>,
    key: [u8; 32],
) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;

        let (refresh_token, expires_at) = {
            let yaml = yaml.lock().unwrap();
            match yaml.decrypt_tokens(&key) {
                None | Some(Err(_)) => continue,
                Some(Ok((_, rt, exp))) => (rt, exp),
            }
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let remaining = expires_at - now;
        if remaining <= 0 {
            info!("auto-refresh: tokens expired, refreshing");
        } else if remaining > 3600 {
            continue; // still well within validity
        }

        match auth.refresh_tokens(&refresh_token).await {
            Ok(tokens) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let new_expires_at = now + tokens.expires_in as i64;
                if let Ok(mut yaml) = yaml.lock() {
                    if let Err(e) = yaml.set_encrypted_tokens(
                        &key,
                        &tokens.access_token,
                        &tokens.refresh_token,
                        new_expires_at,
                    ) {
                        warn!(error = %e, "auto-refresh: failed to persist tokens");
                    } else {
                        info!("auto-refresh: tokens refreshed successfully");
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "auto-refresh: token refresh failed");
            }
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("shutdown signal received, starting graceful shutdown");
}
