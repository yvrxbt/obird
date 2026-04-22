//! Live trading mode — wires the full engine stack end-to-end.
//!
//! Supported strategy types:
//!   - `hl_spread_quoter`    → HyperliquidClient + HlSpreadQuoter
//!   - `prediction_quoter`   → PredictFunClient + PredictionQuoter (negRisk + non-negRisk)
//!
//! On Ctrl+C: engine shuts down gracefully then the ShutdownHandle cancels
//! all open orders before the process exits.

use std::{collections::HashMap, sync::Arc};

use anyhow::Context;
use connector_hyperliquid::{HlMarketDataFeed, HyperliquidClient};
use connector_polymarket::{execution::PolymarketExecutionClient, market_data::PolymarketMarketDataFeed};
use connector_predict_fun::{PredictFunClient, PredictFunMarketDataFeed, PredictFunParams};
use strategy_hl_spread_quoter::{HlSpreadQuoter, QuoterParams as HlParams};
use strategy_predict_hedger::{HedgeParams, MarketMapping, PredictHedgeStrategy};
use strategy_prediction_quoter::{PredictionQuoter, QuoterParams as PredictParams};
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

    let strategy_cfg = config
        .strategies
        .first()
        .context("no [[strategies]] entry in config")?;

    match strategy_cfg.strategy_type.as_str() {
        "hl_spread_quoter" => run_hl(config).await,
        "prediction_quoter" => run_predict(config).await,
        other => anyhow::bail!(
            "unknown strategy_type '{}' — supported: hl_spread_quoter, prediction_quoter",
            other
        ),
    }
}

// ── Hyperliquid spread quoter ─────────────────────────────────────────────────

async fn run_hl(config: AppConfig) -> anyhow::Result<()> {
    let hl_cfg = config
        .exchanges
        .iter()
        .find(|e| e.name == "hyperliquid")
        .context("no [exchanges] entry named 'hyperliquid'")?;

    let strategy_cfg = config.strategies.first().unwrap();

    let params: HlParams = strategy_cfg
        .params
        .clone()
        .try_into()
        .map_err(|e| anyhow::anyhow!("invalid [strategies.params]: {e}"))?;

    let instrument_str = strategy_cfg
        .instruments
        .first()
        .context("no instruments listed under [[strategies]]")?;
    let instrument = parse_instrument(instrument_str)?;
    let symbol = instrument.symbol.clone();

    tracing::info!(symbol = %symbol, testnet = hl_cfg.testnet, "Starting HL spread quoter");

    let connector = HyperliquidClient::from_env(&hl_cfg.secret_key_env, &symbol, hl_cfg.testnet)
        .await
        .context("building HyperliquidClient")?;

    let shutdown = connector.shutdown_handle(hl_cfg.testnet);
    let resolved_instrument = connector.instrument();
    let asset_info = connector.asset_info();
    let wallet_address = connector.wallet_address();

    let md_bus = MarketDataBus::new();
    let _ = md_bus.sender(&resolved_instrument);

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

    let runner = EngineRunner::new(connectors, strategies, md_bus)
        .with_shutdown_flag(shutdown.shutting_down.clone());

    tracing::info!("HL engine wired — starting run loop");
    runner.run().await?;

    tracing::info!("Engine stopped — cancelling all open orders");
    if let Err(e) = shutdown.cancel_all().await {
        tracing::error!("Shutdown cancel failed: {e:#}");
    }
    Ok(())
}

// ── predict.fun points-farming quoter ────────────────────────────────────────

async fn run_predict(config: AppConfig) -> anyhow::Result<()> {
    let pf_cfg = config
        .exchanges
        .iter()
        .find(|e| e.name == "predict_fun")
        .context("no [exchanges] entry named 'predict_fun'")?;

    let strategy_cfg = config.strategies.first().unwrap();

    // Deserialize market params from [exchanges.params].
    let market_params: PredictFunParams = pf_cfg
        .params
        .clone()
        .try_into()
        .map_err(|e| anyhow::anyhow!("invalid [exchanges.params] for predict_fun: {e}"))?;

    // Deserialize strategy params from [strategies.params].
    let strategy_params: PredictParams = strategy_cfg
        .params
        .clone()
        .try_into()
        .map_err(|e| anyhow::anyhow!("invalid [strategies.params]: {e}"))?;

    // Instruments: expect exactly 2 entries — [YES, NO].
    if strategy_cfg.instruments.len() < 2 {
        anyhow::bail!("prediction_quoter requires exactly 2 instruments: [YES, NO]");
    }
    let yes_instrument = parse_instrument(&strategy_cfg.instruments[0])?;
    let no_instrument = parse_instrument(&strategy_cfg.instruments[1])?;

    // Resolve Polymarket FV instrument from the token ID in [exchanges.params].
    // Invariant #5: we never quote without a Polymarket fair-value feed.
    let polymarket_yes_token_id = market_params
        .polymarket_yes_token_id
        .as_ref()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "[exchanges.params] polymarket_yes_token_id is required — prediction_quoter \
                 refuses to start without a Polymarket FV feed (invariant #5). Regenerate with \
                 `trading-cli predict-markets --all --write-configs --fail-on-missing-poly-token`."
            )
        })?;
    let polymarket_fv_instrument = Some(InstrumentId::new(
        Exchange::Polymarket,
        InstrumentKind::Binary,
        polymarket_yes_token_id.clone(),
    ));

    tracing::info!(
        market_id = market_params.market_id,
        yes = %yes_instrument,
        no  = %no_instrument,
        polymarket_fv = ?polymarket_fv_instrument,
        is_neg_risk = market_params.is_neg_risk,
        is_yield_bearing = market_params.is_yield_bearing,
        fee_rate_bps = market_params.fee_rate_bps,
        testnet = pf_cfg.testnet,
        "Starting predict.fun points-farming quoter",
    );

    let client = PredictFunClient::from_env(
        &pf_cfg.api_key_env,
        &pf_cfg.secret_key_env,
        &market_params,
        pf_cfg.testnet,
    )
    .await
    .context("building PredictFunClient")?;

    // Extract shutdown handle BEFORE the client moves into the engine.
    // Shares active_orders and shutting_down with the connector so we can
    // block new places and cancel resting orders after the engine drains.
    let shutdown = client.shutdown_handle();

    let md_bus = MarketDataBus::new();
    // Pre-register predict.fun instruments so the feed can publish to them.
    let _ = md_bus.sender(&yes_instrument);
    let _ = md_bus.sender(&no_instrument);

    // ── Polymarket CLOB WS market data ─────────���─────────────────────────────
    //
    // Subscribe to YES token (for FV) and optionally NO token (for hedge pricing).
    // A single PolymarketMarketDataFeed handles all tokens over one WS connection.

    // Build the NO token instrument if configured — needed for hedge price data.
    let polymarket_no_instrument = market_params
        .polymarket_no_token_id
        .as_ref()
        .map(|token_id| {
            InstrumentId::new(
                Exchange::Polymarket,
                InstrumentKind::Binary,
                token_id.clone(),
            )
        });

    let mut poly_subscriptions: Vec<(String, InstrumentId)> = Vec::new();
    if let Some(ref poly_instrument) = polymarket_fv_instrument {
        let _ = md_bus.sender(poly_instrument);
        let token_id = market_params.polymarket_yes_token_id.clone().unwrap();
        poly_subscriptions.push((token_id, poly_instrument.clone()));
    }
    if let Some(ref poly_no_inst) = polymarket_no_instrument {
        let _ = md_bus.sender(poly_no_inst);
        let token_id = market_params.polymarket_no_token_id.clone().unwrap();
        poly_subscriptions.push((token_id, poly_no_inst.clone()));
    }

    // poly_subscriptions is guaranteed non-empty: polymarket_yes_token_id is required above.
    let poly_feed = PolymarketMarketDataFeed::new(poly_subscriptions);
    let poly_sink = md_bus.clone() as Arc<dyn trading_core::MarketDataSink>;
    tokio::spawn(async move { poly_feed.run(poly_sink).await });
    tracing::info!(
        yes_inst = ?polymarket_fv_instrument,
        no_inst  = ?polymarket_no_instrument,
        "Polymarket CLOB WS feed spawned",
    );

    let feed = PredictFunMarketDataFeed::from_client(&client);
    let sink = md_bus.clone() as Arc<dyn trading_core::MarketDataSink>;
    tokio::spawn(async move { feed.run(sink).await });

    let quoter = PredictionQuoter::new(
        strategy_cfg.name.clone(),
        yes_instrument.clone(),
        no_instrument.clone(),
        strategy_params,
        polymarket_fv_instrument.clone(),
    );

    // ── Polymarket execution client + hedge strategy ──────────────────────────
    //
    // Enabled when both polymarket_yes_token_id and polymarket_no_token_id are
    // present in [exchanges.params]. Reads POLY_API_KEY, POLY_SECRET,
    // POLY_PASSPHRASE from env; signing uses the same PREDICT_PRIVATE_KEY.

    let (poly_connector, hedge_strategy) =
        if let (Some(ref poly_yes_inst), Some(ref poly_no_inst)) =
            (&polymarket_fv_instrument, &polymarket_no_instrument)
        {
            match PolymarketExecutionClient::from_env(&pf_cfg.secret_key_env).await
            {
                Ok(poly_client) => {
                    let hedge_params = HedgeParams::default();
                    let mapping = MarketMapping {
                        predict_yes: yes_instrument.clone(),
                        predict_no: no_instrument.clone(),
                        poly_yes: poly_yes_inst.clone(),
                        poly_no: poly_no_inst.clone(),
                    };
                    let hedger = PredictHedgeStrategy::new(
                        format!("{}_hedge", strategy_cfg.name),
                        vec![mapping],
                        hedge_params,
                    );
                    tracing::info!("Polymarket hedge strategy enabled");
                    (Some(poly_client), Some(hedger))
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        env_var = %pf_cfg.secret_key_env,
                        "Polymarket execution client init failed — hedge disabled. \
                         SDK derives the poly API key from the predict.fun private key; \
                         verify the env var named above is set and funded.",
                    );
                    (None, None)
                }
            }
        } else {
            tracing::info!("polymarket_no_token_id not configured — hedge strategy disabled");
            (None, None)
        };

    let mut connectors: HashMap<Exchange, Box<dyn trading_core::traits::ExchangeConnector>> =
        HashMap::new();
    connectors.insert(Exchange::PredictFun, Box::new(client));
    if let Some(poly_client) = poly_connector {
        connectors.insert(Exchange::Polymarket, Box::new(poly_client));
    }

    let mut strategies: Vec<StrategyInstance> = vec![StrategyInstance {
        id: strategy_cfg.name.clone(),
        strategy: Box::new(quoter),
    }];
    if let Some(hedger) = hedge_strategy {
        strategies.push(StrategyInstance {
            id: format!("{}_hedge", strategy_cfg.name),
            strategy: Box::new(hedger),
        });
    }

    let runner = EngineRunner::new(connectors, strategies, md_bus)
        .with_shutdown_flag(shutdown.shutting_down.clone());

    tracing::info!("predict.fun engine wired — starting run loop");
    runner.run().await?;

    tracing::info!("Engine stopped — cancelling all open orders");
    if let Err(e) = shutdown.cancel_all().await {
        tracing::error!("Shutdown cancel failed: {e:#}");
    }
    Ok(())
}

// ── Instrument parsing ────────────────────────────────────────────────────────

fn parse_instrument(s: &str) -> anyhow::Result<InstrumentId> {
    // Format: "Exchange.Kind.Symbol" — symbol may contain dots or special chars.
    // Split on first two dots only.
    let mut parts = s.splitn(3, '.');
    let exchange_str = parts.next().unwrap_or("");
    let kind_str = parts.next().unwrap_or("");
    let symbol = parts.next().unwrap_or("");

    if symbol.is_empty() {
        anyhow::bail!(
            "invalid instrument '{}': expected 'Exchange.Kind.Symbol'",
            s
        );
    }

    let exchange = match exchange_str {
        "Hyperliquid" => Exchange::Hyperliquid,
        "Binance" => Exchange::Binance,
        "PredictFun" => Exchange::PredictFun,
        "Polymarket" => Exchange::Polymarket,
        other => anyhow::bail!("unknown exchange '{}' in instrument '{}'", other, s),
    };

    let kind = match kind_str {
        "Perpetual" => InstrumentKind::Perpetual,
        "Spot" => InstrumentKind::Spot,
        "Binary" => InstrumentKind::Binary,
        other => anyhow::bail!("unknown instrument kind '{}' in instrument '{}'", other, s),
    };

    Ok(InstrumentId::new(exchange, kind, symbol))
}
