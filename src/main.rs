mod config;

use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    let config = config::Config::load()?;

    info!(
        "tesla-apiscraper-rs starting — listening on {}",
        config.listen_addr()
    );

    Ok(())
}
