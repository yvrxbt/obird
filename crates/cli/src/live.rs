//! Live trading mode — wires the full engine stack end-to-end.
//!
//! On Ctrl+C: engine shuts down gracefully then the ShutdownHandle cancels
//! all open orders before the process exits.

use std::{collections::HashMap, sync::Arc};

use anyhow::Context;
use connector_hyperliquid::{HlMarketDataFeed, HyperliquidClient};
use strategy_hl_spread_quoter::{HlSpreadQuoter, QuoterParams};
use trading_core::{
    config::AppConfig,
    types::instrument::{Exchange, InstrumentId, InstrumentKind},
};
use trading_engine::{
    market_data_bus::MarketDataBus,
    runner::{EngineRunner, StrategyInstance},
};
use trading_telemetry::recorder::DataRecorder;

pub async fn run(config_path: &str) -> anyhow::Result<()> {
    let config = AppConfig::load(std::path::Path::new(config_path))
        .with_context(|| format!("loading config from {config_path}"))?;
    let _ = dotenvy::dotenv();

    let hl_cfg = config.exchanges.iter()
        .find(|e| e.name == "hyperliquid")
        .context("no [exchanges] entry named 'hyperliquid'")?;

    let strategy_cfg = config.strategies.first()
        .context("no [[strategies]] entry in config")?;

    if strategy_cfg.strategy_type != "hl_spread_quoter" {
        anyhow::bail!(
            "expected strategy_type = 'hl_spread_quoter', got '{}'",
            strategy_cfg.strategy_type
        );
    }

    let params: QuoterParams = strategy_cfg.params.clone()
        .try_into()
        .map_err(|e| anyhow::anyhow!("invalid [strategies.params]: {e}"))?;

    let instrument_str = strategy_cfg.instruments.first()
        .context("no instruments listed under [[strategies]]")?;
    let instrument = parse_instrument(instrument_str)?;
    let symbol = instrument.symbol.clone();

    tracing::info!(config = config_path, symbol = %symbol, testnet = hl_cfg.testnet,
        "Starting live quoter");

    let connector = HyperliquidClient::from_env(
        &hl_cfg.secret_key_env, &symbol, hl_cfg.testnet,
    ).await.context("building HyperliquidClient")?;

    // Extract shutdown handle BEFORE the connector is moved into the runner.
    // This lets us cancel all orders on Ctrl+C without restructuring the engine.
    let shutdown = connector.shutdown_handle(hl_cfg.testnet);

    let resolved_instrument = connector.instrument();
    let asset_info = connector.asset_info();
    let wallet_address = connector.wallet_address();

    let md_bus = MarketDataBus::new();
    let _ = md_bus.sender(&resolved_instrument);

    // DataRecorder: dedicated BBO + fill capture for quantitative analysis.
    // Subscribes to the same bus as the strategy — zero coupling, no overhead on hot path.
    // Writes to logs/data/bbo-YYYY-MM-DD.jsonl and logs/data/fills-YYYY-MM-DD.jsonl.
    let recorder_rx = md_bus.subscribe(&resolved_instrument);
    tokio::spawn(async move {
        if let Err(e) = DataRecorder::new(recorder_rx).run().await {
            tracing::error!("DataRecorder error: {e:#}");
        }
    });

    let feed = HlMarketDataFeed::new(asset_info, wallet_address, hl_cfg.testnet);
    let sink = md_bus.clone() as Arc<dyn trading_core::MarketDataSink>;
    tokio::spawn(async move { feed.run(sink).await });

    let quoter = HlSpreadQuoter::new(strategy_cfg.name.clone(), resolved_instrument, params);

    let mut connectors: HashMap<Exchange, Box<dyn trading_core::traits::ExchangeConnector>> =
        HashMap::new();
    connectors.insert(Exchange::Hyperliquid, Box::new(connector));

    let strategies = vec![StrategyInstance {
        id: strategy_cfg.name.clone(),
        strategy: Box::new(quoter),
    }];

    let runner = EngineRunner::new(connectors, strategies, md_bus);

    tracing::info!("Engine wired — starting run loop");
    runner.run().await?;

    // EngineRunner::run() returns after Ctrl+C — clean up orders before exit.
    tracing::info!("Engine stopped — cancelling all open orders");
    if let Err(e) = shutdown.cancel_all().await {
        tracing::error!("Shutdown cancel failed: {e:#}");
    }

    Ok(())
}

fn parse_instrument(s: &str) -> anyhow::Result<InstrumentId> {
    let parts: Vec<&str> = s.splitn(3, '.').collect();
    if parts.len() != 3 {
        anyhow::bail!("invalid instrument '{}': expected 'Exchange.Kind.Symbol'", s);
    }
    let exchange = match parts[0] {
        "Hyperliquid" => Exchange::Hyperliquid,
        other => anyhow::bail!("unknown exchange '{}'", other),
    };
    let kind = match parts[1] {
        "Perpetual" => InstrumentKind::Perpetual,
        "Spot" => InstrumentKind::Spot,
        "Binary" => InstrumentKind::Binary,
        other => anyhow::bail!("unknown instrument kind '{}'", other),
    };
    Ok(InstrumentId::new(exchange, kind, parts[2]))
}
