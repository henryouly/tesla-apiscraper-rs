mod config;
mod config_yaml;
mod influxdb;

use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    // ── Env configuration ───────────────────────────────────────────
    let env = config::Config::load()?;
    info!(config_dir = ?env.config_dir, "environment config loaded");

    // ── YAML config ─────────────────────────────────────────────────
    let yaml = config_yaml::YamlConfigManager::load(&env.config_dir)?;
    info!(
        geofences = yaml.geofences.geofences.len(),
        cars = yaml.settings.cars.len(),
        authenticated = yaml.tokens.is_some(),
        "YAML config loaded"
    );

    // ── InfluxDB ────────────────────────────────────────────────────
    let db = influxdb::InfluxDb::new(
        &env.influxdb_url,
        &env.influxdb_token,
        &env.influxdb_org,
        &env.influxdb_bucket,
    );

    db.ping().await?;
    info!("InfluxDB connection OK");

    db.ensure_bucket().await?;
    info!(bucket = %env.influxdb_bucket, "InfluxDB bucket ready");

    info!(
        "tesla-apiscraper-rs started — listening on {}",
        env.listen_addr()
    );

    // Phase 1.4 will replace this with an axum HTTP server.
    // For now, the binary validates all connections and exits cleanly.
    Ok(())
}
