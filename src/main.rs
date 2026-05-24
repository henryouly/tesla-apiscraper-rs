mod api;
mod config;
mod config_yaml;
mod influxdb;

use std::io::IsTerminal;
use std::sync::Arc;
use tracing::info;

enum LogFormat {
    Json,
    Compact,
}

fn select_log_format(env_var: Option<&str>, is_tty: bool) -> LogFormat {
    match env_var {
        Some(v) if v.eq_ignore_ascii_case("json") => LogFormat::Json,
        Some(v) if v.eq_ignore_ascii_case("compact") => LogFormat::Compact,
        _ if is_tty => LogFormat::Compact,
        _ => LogFormat::Json,
    }
}

fn init_tracing() {
    let format = std::env::var("LOG_FORMAT").ok();
    let is_tty = std::io::stdout().is_terminal();

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with_target(true);

    match select_log_format(format.as_deref(), is_tty) {
        LogFormat::Json => subscriber.json().init(),
        LogFormat::Compact => subscriber.compact().init(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

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
        &env.influxdb_org,
        &env.influxdb_bucket,
    ));

    db.ping().await?;
    info!("InfluxDB connection OK");

    db.ensure_bucket().await?;
    info!(bucket = %env.influxdb_bucket, "InfluxDB bucket ready");

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_compact_on_tty() {
        assert!(matches!(select_log_format(None, true), LogFormat::Compact));
    }

    #[test]
    fn test_default_is_json_on_non_tty() {
        assert!(matches!(select_log_format(None, false), LogFormat::Json));
    }

    #[test]
    fn test_env_json_overrides_tty() {
        assert!(matches!(
            select_log_format(Some("json"), true),
            LogFormat::Json
        ));
    }

    #[test]
    fn test_env_json_overrides_non_tty() {
        assert!(matches!(
            select_log_format(Some("json"), false),
            LogFormat::Json
        ));
    }

    #[test]
    fn test_env_compact_overrides_tty() {
        assert!(matches!(
            select_log_format(Some("compact"), true),
            LogFormat::Compact
        ));
    }

    #[test]
    fn test_env_compact_overrides_non_tty() {
        assert!(matches!(
            select_log_format(Some("compact"), false),
            LogFormat::Compact
        ));
    }

    #[test]
    fn test_env_uppercase_json() {
        assert!(matches!(
            select_log_format(Some("JSON"), true),
            LogFormat::Json
        ));
    }

    #[test]
    fn test_env_mixed_case_json() {
        assert!(matches!(
            select_log_format(Some("jSON"), true),
            LogFormat::Json
        ));
    }

    #[test]
    fn test_env_uppercase_compact() {
        assert!(matches!(
            select_log_format(Some("COMPACT"), false),
            LogFormat::Compact
        ));
    }

    #[test]
    fn test_empty_env_falls_back_to_tty() {
        assert!(matches!(
            select_log_format(Some(""), true),
            LogFormat::Compact
        ));
    }

    #[test]
    fn test_empty_env_falls_back_to_non_tty() {
        assert!(matches!(
            select_log_format(Some(""), false),
            LogFormat::Json
        ));
    }

    #[test]
    fn test_unknown_env_falls_back_to_tty() {
        assert!(matches!(
            select_log_format(Some("pretty"), true),
            LogFormat::Compact
        ));
    }

    #[test]
    fn test_unknown_env_falls_back_to_non_tty() {
        assert!(matches!(
            select_log_format(Some("INVALID"), false),
            LogFormat::Json
        ));
    }
}
