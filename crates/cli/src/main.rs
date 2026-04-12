//! CLI entry point for the trading system.
//!
//! Usage:
//!   cargo run -- live --config configs/example.toml
//!   cargo run -- backtest --config configs/example.toml --data data/recordings/
//!   cargo run -- record --config configs/example.toml --duration 24h

use tracing_subscriber::EnvFilter;

mod live;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("help");

    match command {
        "live" => {
            tracing::info!("Starting live trading");
            live::run_once().await?;
        }
        "backtest" => {
            tracing::info!("Starting backtest");
            // TODO: Load config + data, run backtest harness
        }
        "record" => {
            tracing::info!("Starting market data recording");
            // TODO: Connect to exchanges, record to disk
        }
        _ => {
            println!("Usage: trading-cli <live|backtest|record> [options]");
        }
    }

    Ok(())
}
