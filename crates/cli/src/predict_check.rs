//! `trading-cli predict-check` — smoke test for the predict.fun dual-outcome connector.
//!
//! Checks (in order):
//!   1. Env vars present (PREDICT_API_KEY, PREDICT_PRIVATE_KEY)
//!   2. Auth: JWT obtained via message-sign flow
//!   3. Markets: lists first 5 open markets with their outcome token IDs
//!   4. WS: subscribes to the YES orderbook for a market, waits for a BookUpdate,
//!          and verifies pricing::calculate() returns valid (non-crossing) prices.
//!
//! Set PREDICT_MARKET_ID=<id> to override which market is used.
//!
//! Usage:
//!   source .env && RUST_LOG=info cargo run --bin trading-cli -- predict-check

use std::sync::Arc;
use std::time::Duration;

use connector_predict_fun::{PredictFunClient, PredictFunMarketDataFeed, PredictFunParams};
use rust_decimal_macros::dec;
use strategy_prediction_quoter::pricing;
use trading_core::{Event, MarketDataSink};
use trading_engine::market_data_bus::MarketDataBus;

pub async fn run() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    // ── 1. Env vars ────────────────────────────────────────────────────────
    for var in &["PREDICT_API_KEY", "PREDICT_PRIVATE_KEY"] {
        std::env::var(var).map_err(|_| anyhow::anyhow!("{var} not set in environment"))?;
    }
    tracing::info!("✓ env vars present");

    // ── 2–3. Auth + market listing ─────────────────────────────────────────
    let sdk_client = predict_sdk::PredictClient::new(
        56u64,
        &std::env::var("PREDICT_PRIVATE_KEY").unwrap(),
        "https://api.predict.fun".to_string(),
        Some(std::env::var("PREDICT_API_KEY").unwrap()),
    )
    .map_err(|e| anyhow::anyhow!("SDK init: {e}"))?;

    let jwt: String = sdk_client
        .authenticate_and_store()
        .await
        .map_err(|e| anyhow::anyhow!("auth: {e}"))?;
    tracing::info!("✓ JWT obtained (len={})", jwt.len());

    let markets: Vec<predict_sdk::PredictMarket> = sdk_client
        .get_markets_filtered(Some("OPEN"))
        .await
        .map_err(|e| anyhow::anyhow!("get_markets: {e}"))?;

    let open: Vec<_> = markets
        .iter()
        .filter(|m| m.trading_status == "OPEN")
        .filter(|m| m.outcomes.len() >= 2)
        .collect();

    tracing::info!(
        "✓ {} markets returned, {} with tradingStatus=OPEN and ≥2 outcomes",
        markets.len(),
        open.len()
    );
    for m in open.iter() {
        tracing::info!(id = m.id, title = %m.title, is_neg_risk = m.is_neg_risk,
            is_yield_bearing = m.is_yield_bearing, fee_rate_bps = m.fee_rate_bps,
            trading_status = %m.trading_status, "market");
        for o in &m.outcomes {
            tracing::info!("  outcome '{}' on_chain_id={}", o.name, o.on_chain_id);
        }
    }

    // ── 4. WS + pricing smoke test ──────────────────────────────────────────
    let market_id: u64 = std::env::var("PREDICT_MARKET_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| open.first().map(|m| m.id))
        .ok_or_else(|| anyhow::anyhow!("no open market found; set PREDICT_MARKET_ID"))?;

    let target = markets
        .iter()
        .find(|m| m.id == market_id)
        .ok_or_else(|| anyhow::anyhow!("market {market_id} not in listing"))?;

    if target.outcomes.len() < 2 {
        anyhow::bail!("market {market_id} has fewer than 2 outcomes — cannot test dual-outcome");
    }

    // Use first two outcomes as YES and NO (may not literally be "YES"/"NO")
    let yes_o = &target.outcomes[0];
    let no_o = &target.outcomes[1];

    let params = PredictFunParams {
        market_id,
        yes_outcome_name: yes_o.name.clone(),
        yes_token_id: yes_o.on_chain_id.clone(),
        no_outcome_name: no_o.name.clone(),
        no_token_id: no_o.on_chain_id.clone(),
        is_neg_risk: target.is_neg_risk,
        is_yield_bearing: target.is_yield_bearing,
        fee_rate_bps: target.fee_rate_bps,
        polymarket_yes_token_id: None, // smoke test only — no Polymarket FV needed
    };

    tracing::info!(
        market_id,
        yes_outcome = %params.yes_outcome_name,
        yes_token_id = %params.yes_token_id,
        no_outcome  = %params.no_outcome_name,
        no_token_id  = %params.no_token_id,
        "connecting PredictFunClient (dual-outcome)",
    );

    let client =
        PredictFunClient::from_env("PREDICT_API_KEY", "PREDICT_PRIVATE_KEY", &params, false)
            .await
            .map_err(|e| anyhow::anyhow!("client init: {e}"))?;

    let (yes_inst, no_inst) = client.instruments();
    tracing::info!("✓ instruments: yes={}, no={}", yes_inst, no_inst);

    let md_bus = MarketDataBus::new();
    let _ = md_bus.sender(&yes_inst);
    let _ = md_bus.sender(&no_inst);
    let mut rx = md_bus.subscribe(&yes_inst);

    let feed = PredictFunMarketDataFeed::from_client(&client);
    let sink: Arc<dyn MarketDataSink> = md_bus.clone();

    tokio::spawn(async move { feed.run(sink).await });

    tracing::info!("waiting for first YES BookUpdate (10s timeout)…");
    let book = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Ok(Event::BookUpdate { book, .. }) => return Some(book),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out (10s) — check credentials and market ID"))?
    .ok_or_else(|| anyhow::anyhow!("WS stream closed before BookUpdate"))?;

    tracing::info!(
        best_bid = ?book.best_bid().map(|(p,_)| p.inner()),
        best_ask = ?book.best_ask().map(|(p,_)| p.inner()),
        bids = book.bids.len(),
        asks = book.asks.len(),
        "✓ YES BookUpdate received",
    );

    // ── Pricing sanity check ────────────────────────────────────────────────
    let spread_cents = dec!(0.02);
    let decimal_precision = 3u32; // use 3dp for check — works for both precision=2 and precision=3 markets

    // Use book mid as fair value for the smoke test (no external Polymarket feed).
    let fv = book
        .mid_price()
        .map(|m| m.inner())
        .unwrap_or(rust_decimal_macros::dec!(0.5));
    match pricing::calculate(&book, fv, spread_cents, None, decimal_precision) {
        Some(prices) => {
            let sum = prices.yes_bid.inner() + prices.no_bid.inner();
            tracing::info!(
                yes_bid = %prices.yes_bid,
                no_bid  = %prices.no_bid,
                sum     = %sum,
                "✓ pricing::calculate returned valid prices (sum = yes+no = 1.00)",
            );
            assert_eq!(sum, rust_decimal::Decimal::ONE, "yes+no must equal 1.00");
        }
        None => {
            tracing::warn!(
                "pricing::calculate returned None — book may be too narrow for \
                 spread_cents={spread_cents} decimal_precision={decimal_precision}. This is OK in thin markets.",
            );
        }
    }

    tracing::info!("✓ predict.fun dual-outcome connector check passed");
    Ok(())
}
