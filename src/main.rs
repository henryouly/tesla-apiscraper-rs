mod api;
mod config;
mod config_yaml;
mod influxdb;

use std::sync::Arc;
use tracing::info;
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

    // ── YAML config ─────────────────────────────────────────────────
    let _yaml = config_yaml::YamlConfigManager::load(&env.config_dir)?;
    info!(
        geofences = _yaml.geofences.geofences.len(),
        cars = _yaml.settings.cars.len(),
        authenticated = _yaml.tokens.is_some(),
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

    // ── HTTP server ─────────────────────────────────────────────────
    let state = api::AppState { db };
    let router = api::create_router(state);

    let listener = tokio::net::TcpListener::bind(env.listen_addr()).await?;
    info!(addr = %env.listen_addr(), "HTTP server started");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
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
