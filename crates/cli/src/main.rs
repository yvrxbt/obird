//! CLI entry point.
//!
//! Logging:
//!   - Terminal: human-readable (RUST_LOG controls level, default info)
//!   - File: JSON lines → logs/obird-YYYY-MM-DD.jsonl (always written, all levels ≥ debug)
//!     Every decision, order submission, fill, drift event is permanently recorded.
//!
//! Usage:
//!   cargo run --bin trading-cli -- live --config configs/quoter.toml
//!   cargo run --bin trading-cli -- backtest --config configs/example.toml
//!   cargo run --bin trading-cli -- record --config configs/example.toml

use std::path::Path;

use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

mod live;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Logging setup ─────────────────────────────────────────────────────────

    // File appender: daily rolling JSON lines, never discards anything
    std::fs::create_dir_all("logs")?;
    let file_appender = tracing_appender::rolling::daily("logs", "obird.jsonl");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // File layer: JSON, all events debug+
    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_filter(
            EnvFilter::new("debug,hyper=warn,reqwest=warn,h2=warn,rustls=warn")
        );

    // Terminal layer: human-readable, controlled by RUST_LOG (default: info for our crates)
    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_filter(
            EnvFilter::from_env("RUST_LOG")
                .add_directive("info".parse().unwrap())
        );

    tracing_subscriber::registry()
        .with(file_layer)
        .with(terminal_layer)
        .init();

    // ── Command dispatch ──────────────────────────────────────────────────────

    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("help");

    match command {
        "live" => {
            let config = flag_value(&args, "--config").unwrap_or("configs/quoter.toml");
            tracing::info!(config, "Starting live trading");
            live::run(config).await?;
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
            println!("Usage: trading-cli <live|backtest|record> [--config <path>]");
            println!();
            println!("  live      --config configs/quoter.toml");
            println!("  backtest  --config configs/example.toml --data data/");
            println!("  record    --config configs/example.toml");
            println!();
            println!("Logs: logs/obird-YYYY-MM-DD.jsonl  (all events, persistent)");
        }
    }

    Ok(())
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].as_str())
}
