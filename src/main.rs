mod api;
mod config;
mod config_yaml;
mod elevation;
mod encryption;
mod influxdb;
mod tesla_api;
mod tesla_auth;
mod vehicles;

use anyhow::Context;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Environment configuration (load early so logging can use it) ─
    let env = config::Config::load()?;

    // ── Logging ─────────────────────────────────────────────────────
    let is_json = env.log_format == "json";

    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
        .from_env_lossy();

    let stdout_layer = if is_json {
        tracing_subscriber::fmt::layer()
            .json()
            .with_target(true)
            .with_span_events(FmtSpan::CLOSE)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .compact()
            .with_target(true)
            .with_span_events(FmtSpan::CLOSE)
            .boxed()
    };

    if let Some(ref log_file) = env.log_file {
        if let Some(parent) = std::path::Path::new(log_file).parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create log directory: {}", parent.display()))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
            .with_context(|| format!("failed to open log file: {log_file}"))?;
        let file = std::sync::Mutex::new(file);
        let file_layer = tracing_subscriber::fmt::layer()
            .compact()
            .with_ansi(false)
            .with_writer(file)
            .with_target(true)
            .with_span_events(FmtSpan::CLOSE);
        tracing_subscriber::registry()
            .with(env_filter)
            .with(stdout_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(stdout_layer)
            .init();
    }

    info!(config_dir = ?env.config_dir, "environment config loaded");

    // ── YAML config (with token encryption) ─────────────────────────
    let encryption_key = encryption::hex_to_key(&env.data_encryption_key)?;
    let yaml = Arc::new(Mutex::new(config_yaml::YamlConfigManager::load(
        &env.config_dir,
    )?));
    let (geo_len, cars_len, token_valid) = {
        let yaml = yaml.lock().unwrap();
        (
            yaml.geofences.geofences.len(),
            yaml.settings.cars.len(),
            yaml.decrypt_tokens(&encryption_key)
                .is_some_and(|r| r.is_ok()),
        )
    };
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

    // ── Token watch channel ─────────────────────────────────────────
    // Vehicle tasks receive token updates through this watch.
    let (token_tx, token_rx) = tokio::sync::watch::channel(None::<String>);

    // ── Startup token validation ────────────────────────────────────
    try_use_stored_tokens(&yaml, &auth, &encryption_key, &token_tx).await;

    // ── Vehicle discovery ───────────────────────────────────────────
    let vehicles = discover_vehicles(&yaml, &auth, &encryption_key, &env.tesla_api_url).await;

    // ── Vehicle state machines ──────────────────────────────────────
    let mut vm = vehicles::Vehicles::new(&env.tesla_api_url);
    let vehicle_count = vm.spawn_all(
        &vehicles,
        Arc::clone(&db),
        token_rx,
        Arc::clone(&yaml),
        Duration::from_secs(env.poll_interval_seconds),
    );
    info!(vehicle_count, "vehicle state machines started");
    let vehicle_manager = Arc::new(vm);

    // ── Background auto-refresh ─────────────────────────────────────
    let refresh_yaml = Arc::clone(&yaml);
    let refresh_auth = Arc::clone(&auth);
    let refresh_token_tx = token_tx.clone();
    tokio::spawn(async move {
        token_auto_refresh_loop(refresh_yaml, refresh_auth, encryption_key, refresh_token_tx).await;
    });

    // ── HTTP server ─────────────────────────────────────────────────
    let state = api::AppState {
        db,
        auth,
        yaml,
        encryption_key,
        vehicles,
        vehicle_manager: Arc::clone(&vehicle_manager),
    };
    let router = api::create_router(state);

    let listener = tokio::net::TcpListener::bind(env.listen_addr()).await?;
    info!(addr = %env.listen_addr(), "HTTP server started");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("shutting down vehicle state machines");
    vehicle_manager.shutdown_all();
    info!("shutdown complete");

    Ok(())
}

/// Refresh tokens and persist the result to the encrypted YAML store.
/// Returns `true` on success.
/// Refresh tokens via the Tesla API, persist them to disk, and return
/// the new access token on success (or `None` on failure).
async fn refresh_and_persist_tokens(
    yaml: &Arc<Mutex<config_yaml::YamlConfigManager>>,
    auth: &Arc<tesla_auth::TeslaAuthClient>,
    key: &[u8; 32],
    refresh_token: &str,
) -> Option<String> {
    let tokens = match auth.refresh_tokens(refresh_token).await {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "token refresh failed");
            return None;
        }
    };

    if let Ok(mut yaml) = yaml.lock()
        && let Err(e) = yaml.set_encrypted_tokens(
            key,
            &tokens.access_token,
            &tokens.refresh_token,
            tokens.expires_at(),
        )
    {
        warn!(error = %e, "failed to persist refreshed tokens");
    }

    Some(tokens.access_token)
}

/// Attempt to decrypt stored tokens and validate them at startup.
/// If expired or within 1 hour of expiry, refresh automatically.
/// Sends the current access token into the watch channel on success.
async fn try_use_stored_tokens(
    yaml: &Arc<Mutex<config_yaml::YamlConfigManager>>,
    auth: &Arc<tesla_auth::TeslaAuthClient>,
    key: &[u8; 32],
    token_tx: &tokio::sync::watch::Sender<Option<String>>,
) {
    let (access_token, refresh_token, expires_at) = {
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

    let valid_token = if expires_at - now <= 0 {
        info!("stored tokens expired, attempting refresh");
        None
    } else {
        let remaining = expires_at - now;
        if remaining <= 3600 {
            info!(
                remaining_secs = remaining,
                "tokens approaching expiry, attempting refresh"
            );
            None
        } else {
            info!(remaining_secs = remaining, "stored tokens valid");
            Some(access_token)
        }
    };

    if let Some(at) = valid_token {
        token_tx.send(Some(at)).ok();
    } else if let Some(at) = refresh_and_persist_tokens(yaml, auth, key, &refresh_token).await {
        info!("stored tokens refreshed successfully at startup");
        token_tx.send(Some(at)).ok();
    } else {
        warn!("failed to refresh stored tokens — manual sign-in required");
    }
}

/// Discover vehicles from the Tesla Owner API using stored tokens.
async fn discover_vehicles(
    yaml: &Arc<Mutex<config_yaml::YamlConfigManager>>,
    auth: &Arc<tesla_auth::TeslaAuthClient>,
    key: &[u8; 32],
    default_api_url: &str,
) -> Arc<HashMap<String, tesla_api::Vehicle>> {
    let access_token = {
        let yaml = yaml.lock().unwrap();
        match yaml.decrypt_tokens(key) {
            Some(Ok((at, _, _))) => at,
            _ => {
                info!("no stored tokens — skipping vehicle discovery");
                return Arc::new(HashMap::new());
            }
        }
    };

    let api_url = match auth.decode_region(&access_token) {
        Ok(region) => region.api_url.clone(),
        Err(_) => default_api_url.to_string(),
    };

    match tesla_api::list_products(&access_token, &api_url).await {
        Ok(vehicles) => {
            let count = vehicles.len();
            let vehicles_map: HashMap<_, _> =
                vehicles.into_iter().map(|v| (v.vin.clone(), v)).collect();
            info!(vehicle_count = count, "vehicle discovery complete");
            Arc::new(vehicles_map)
        }
        Err(e) => {
            warn!(error = %e, "vehicle discovery failed at startup");
            Arc::new(HashMap::new())
        }
    }
}

/// Background loop that checks token expiry every 60 seconds and refreshes
/// when within 1 hour of expiry. Sends the new access token to vehicle tasks
/// via the watch channel on each successful refresh.
async fn token_auto_refresh_loop(
    yaml: Arc<Mutex<config_yaml::YamlConfigManager>>,
    auth: Arc<tesla_auth::TeslaAuthClient>,
    key: [u8; 32],
    token_tx: tokio::sync::watch::Sender<Option<String>>,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;

        let (_access_token, refresh_token, expires_at) = {
            let yaml = yaml.lock().unwrap();
            match yaml.decrypt_tokens(&key) {
                None | Some(Err(_)) => continue,
                Some(Ok((at, rt, exp))) => (at, rt, exp),
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
            continue;
        }

        if let Some(at) = refresh_and_persist_tokens(&yaml, &auth, &key, &refresh_token).await {
            info!("auto-refresh: tokens refreshed successfully");
            token_tx.send(Some(at)).ok();
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
