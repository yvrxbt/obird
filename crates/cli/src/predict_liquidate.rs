//! `trading-cli predict-liquidate` — place passive SELL limit orders for held positions.
//!
//! Purpose: unwind predict.fun inventory without crossing spread / market-taking.
//! For each held YES/NO position on the configured market, this command places a
//! SELL LIMIT at the current ask (or one tick above bid if book is crossed/tight).

use connector_predict_fun::{normalize, PredictFunParams};
use predict_sdk::types::Side as PredictSide;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use trading_core::config::AppConfig;

pub async fn run(config_path: &str, dry_run: bool) -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let config = AppConfig::load(std::path::Path::new(config_path))
        .map_err(|e| anyhow::anyhow!("loading config: {e}"))?;

    let pf_cfg = config
        .exchanges
        .iter()
        .find(|e| e.name == "predict_fun")
        .ok_or_else(|| anyhow::anyhow!("no [exchanges] entry named 'predict_fun'"))?;

    let market_params: PredictFunParams = pf_cfg
        .params
        .clone()
        .try_into()
        .map_err(|e| anyhow::anyhow!("invalid [exchanges.params]: {e}"))?;

    let private_key = std::env::var(&pf_cfg.secret_key_env)
        .map_err(|_| anyhow::anyhow!("env var {} not set", pf_cfg.secret_key_env))?;
    let api_key = std::env::var(&pf_cfg.api_key_env)
        .map_err(|_| anyhow::anyhow!("env var {} not set", pf_cfg.api_key_env))?;

    let chain_id: u64 = if pf_cfg.testnet { 97 } else { 56 };
    let api_base = if pf_cfg.testnet {
        "https://api-testnet.predict.fun".to_string()
    } else {
        "https://api.predict.fun".to_string()
    };

    let client = predict_sdk::PredictClient::new(chain_id, &private_key, api_base, Some(api_key))
        .map_err(|e| anyhow::anyhow!("SDK init: {e}"))?;

    client
        .authenticate_and_store()
        .await
        .map_err(|e| anyhow::anyhow!("auth: {e}"))?;

    // Cancel any existing open orders on this market first (safer unwind).
    let open_orders = client
        .get_open_orders()
        .await
        .map_err(|e| anyhow::anyhow!("get_open_orders: {e}"))?;
    let target_token_ids = [&market_params.yes_token_id, &market_params.no_token_id];
    let cancel_ids: Vec<String> = open_orders
        .into_iter()
        .filter(|o| target_token_ids.contains(&&o.order.token_id))
        .map(|o| o.id)
        .collect();

    if !cancel_ids.is_empty() {
        tracing::info!(
            n = cancel_ids.len(),
            "Cancelling existing open orders on this market"
        );
        if !dry_run {
            client
                .cancel_orders(&cancel_ids)
                .await
                .map_err(|e| anyhow::anyhow!("cancel_orders: {e}"))?;
        }
    }

    // Fetch held positions for this market.
    let positions = client
        .get_positions()
        .await
        .map_err(|e| anyhow::anyhow!("get_positions: {e}"))?;

    let mut yes_qty = Decimal::ZERO;
    let mut no_qty = Decimal::ZERO;

    for p in positions {
        if p.market.id != market_params.market_id {
            continue;
        }
        let qty = normalize::from_wei(&p.amount);
        if qty <= Decimal::ZERO {
            continue;
        }
        if p.outcome.on_chain_id == market_params.yes_token_id {
            yes_qty += qty;
        } else if p.outcome.on_chain_id == market_params.no_token_id {
            no_qty += qty;
        }
    }

    if yes_qty <= Decimal::ZERO && no_qty <= Decimal::ZERO {
        tracing::info!(
            market_id = market_params.market_id,
            "No position to liquidate"
        );
        return Ok(());
    }

    // Get book + precision for passive ask pricing.
    let details = client
        .get_market_by_id(market_params.market_id)
        .await
        .map_err(|e| anyhow::anyhow!("get_market_by_id: {e}"))?;
    let decimal_precision = details.decimal_precision.unwrap_or(3);
    let tick = match decimal_precision {
        2 => dec!(0.01),
        _ => dec!(0.001),
    };

    let book = client
        .get_orderbook(&market_params.market_id.to_string())
        .await
        .map_err(|e| anyhow::anyhow!("get_orderbook: {e}"))?;

    let yes_bid = book
        .bids
        .first()
        .map(|(p, _)| *p)
        .ok_or_else(|| anyhow::anyhow!("orderbook has no bids"))?;
    let yes_ask = book
        .asks
        .first()
        .map(|(p, _)| *p)
        .ok_or_else(|| anyhow::anyhow!("orderbook has no asks"))?;

    let yes_sell_price = yes_ask.max(yes_bid + tick).round_dp_with_strategy(
        decimal_precision,
        rust_decimal::RoundingStrategy::AwayFromZero,
    );

    let no_bid = Decimal::ONE - yes_ask;
    let no_ask = Decimal::ONE - yes_bid;
    let no_sell_price = no_ask.max(no_bid + tick).round_dp_with_strategy(
        decimal_precision,
        rust_decimal::RoundingStrategy::AwayFromZero,
    );

    tracing::info!(
        market_id = market_params.market_id,
        decimal_precision,
        yes_qty = %yes_qty,
        no_qty = %no_qty,
        yes_bid = %yes_bid,
        yes_ask = %yes_ask,
        yes_sell_price = %yes_sell_price,
        no_sell_price = %no_sell_price,
        dry_run,
        "Prepared passive liquidation orders",
    );

    if dry_run {
        tracing::info!("DRY RUN enabled — no orders placed");
        return Ok(());
    }

    if yes_qty > Decimal::ZERO {
        let res = client
            .place_limit_order(
                &market_params.yes_token_id,
                PredictSide::Sell,
                yes_sell_price,
                yes_qty,
                market_params.is_neg_risk,
                market_params.is_yield_bearing,
                market_params.fee_rate_bps,
            )
            .await
            .map_err(|e| anyhow::anyhow!("place_limit_order YES sell: {e}"))?;
        tracing::info!(resp = ?res.data, "Placed YES liquidation order");
    }

    if no_qty > Decimal::ZERO {
        let res = client
            .place_limit_order(
                &market_params.no_token_id,
                PredictSide::Sell,
                no_sell_price,
                no_qty,
                market_params.is_neg_risk,
                market_params.is_yield_bearing,
                market_params.fee_rate_bps,
            )
            .await
            .map_err(|e| anyhow::anyhow!("place_limit_order NO sell: {e}"))?;
        tracing::info!(resp = ?res.data, "Placed NO liquidation order");
    }

    Ok(())
}
