//! CLI entry point.
//!
//! Logging:
//!   - Terminal: human-readable (RUST_LOG controls level, default info)
//!   - File: JSON lines → logs/obird-YYYY-MM-DD.jsonl (always written, all levels ≥ debug)
//!
//! Usage:
//!   cargo run --bin trading-cli -- live            --config configs/quoter.toml
//!   cargo run --bin trading-cli -- predict-check            # smoke test predict.fun connector
//!   cargo run --bin trading-cli -- predict-approve --config configs/predict_quoter.toml
//!   cargo run --bin trading-cli -- predict-approve --all --config configs/predict_quoter.toml
//!   cargo run --bin trading-cli -- backtest        --config configs/example.toml
//!   cargo run --bin trading-cli -- record          --config configs/example.toml

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

mod live;
mod poly_check;
mod predict_approve;
mod predict_check;
mod predict_liquidate;
mod predict_markets;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Logging setup ─────────────────────────────────────────────────────────

    std::fs::create_dir_all("logs")?;
    let file_appender = tracing_appender::rolling::daily("logs", "obird.jsonl");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_filter(EnvFilter::new(
            "debug,hyper=warn,reqwest=warn,h2=warn,rustls=warn",
        ));

    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_filter(EnvFilter::from_env("RUST_LOG").add_directive("info".parse().unwrap()));

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
        "predict-check" => {
            tracing::info!("Running predict.fun connector smoke test");
            predict_check::run().await?;
        }
        "poly-check" => {
            let live = args.contains(&"--live".to_string());
            tracing::info!(live, "Running Polymarket connector smoke test");
            poly_check::run(live).await?;
        }
        "predict-markets" => {
            let all = args.contains(&"--all".to_string());
            let write_configs = args.contains(&"--write-configs".to_string());
            let fail_on_missing_poly_token =
                args.contains(&"--fail-on-missing-poly-token".to_string());
            let output_dir = flag_value(&args, "--output-dir").unwrap_or("configs/markets_poly");
            predict_markets::run(all, write_configs, output_dir, fail_on_missing_poly_token)
                .await?;
        }
        "predict-approve" => {
            let config = flag_value(&args, "--config").unwrap_or("configs/predict_quoter.toml");
            let all = args.contains(&"--all".to_string());
            tracing::info!(config, all, "Setting on-chain approvals for predict.fun");
            predict_approve::run(config, all).await?;
        }
        "predict-liquidate" => {
            let config = flag_value(&args, "--config").unwrap_or("configs/predict_quoter.toml");
            let dry_run = args.contains(&"--dry-run".to_string());
            tracing::info!(
                config,
                dry_run,
                "Placing passive liquidation orders on predict.fun"
            );
            predict_liquidate::run(config, dry_run).await?;
        }
        "backtest" => {
            tracing::info!("Starting backtest");
            // TODO: wire up BacktestHarness
        }
        "record" => {
            tracing::info!("Starting market data recording");
            // TODO: wire up MarketDataRecorder
        }
        _ => {
            println!("Usage: trading-cli <command> [options]");
            println!();
            println!("Commands:");
            println!("  live            --config configs/quoter.toml      Run live market making");
            println!("  predict-check                                      Smoke test predict.fun connector");
            println!("  predict-markets                                    Show active boosted markets + config blocks");
            println!("  predict-markets --all                              Include non-boosted open markets");
            println!("  predict-markets --write-configs                    Auto-write per-market TOML files");
            println!("  predict-markets --write-configs --output-dir DIR   Write configs to DIR (default configs/markets_poly)");
            println!("  predict-markets --fail-on-missing-poly-token       Exit non-zero if any selected market lacks polymarket_yes_token_id");
            println!("  predict-approve --config configs/predict_quoter.toml  Set on-chain USDT approvals (run once)");
            println!("  predict-approve --all --config ...                 Approve all 4 contract variants");
            println!("  predict-liquidate --config configs/markets_poly/NNN.toml Place passive SELL limits at current ask for held YES/NO positions");
            println!("  predict-liquidate --dry-run --config ...           Preview prices/qty without placing orders");
            println!("  backtest        --config configs/example.toml     Run backtest");
            println!("  record          --config configs/example.toml     Record market data");
            println!();
            println!("Logs: logs/obird-YYYY-MM-DD.jsonl  (all events, persistent)");
        }
    }

    Ok(())
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].as_str())
}
