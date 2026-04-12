//! Fair value service entry point.
//! Runs as a separate binary, publishes fair values over UDS.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("Fair value service starting");

    // TODO:
    // 1. Load config (which instruments, model params, UDS path)
    // 2. Connect to data sources (exchange prices, news feeds)
    // 3. Initialize model
    // 4. Start UDS server
    // 5. Loop: compute fair values, publish to connected clients

    Ok(())
}
