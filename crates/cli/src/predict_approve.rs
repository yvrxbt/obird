//! `trading-cli predict-approve` — one-time on-chain approval setup for predict.fun.
//!
//! The CTF Exchange (and its NegRisk/YieldBearing variants) requires ERC-20 (USDT)
//! and ERC-1155 (ConditionalTokens) approvals before it can execute orders.
//! This is a ONE-TIME on-chain transaction per wallet per contract set.
//!
//! After running this once, the live bot can place orders without any further
//! on-chain setup. Requires a small amount of BNB for gas.
//!
//! Usage:
//!   source .env && cargo run --bin trading-cli -- predict-approve --config configs/predict_quoter.toml
//!
//! To approve non-negRisk non-yield-bearing markets too (covers all variants):
//!   source .env && cargo run --bin trading-cli -- predict-approve --all

use connector_predict_fun::PredictFunParams;
use trading_core::config::AppConfig;

pub async fn run(config_path: &str, all_variants: bool) -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    // Load market params from config.
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

    let signer = client
        .signer_address()
        .map_err(|e| anyhow::anyhow!("signer: {e}"))?;

    tracing::info!(
        wallet = %signer,
        is_neg_risk = market_params.is_neg_risk,
        is_yield_bearing = market_params.is_yield_bearing,
        testnet = pf_cfg.testnet,
        "Setting on-chain approvals for predict.fun",
    );

    // Always approve for the market variant specified in config.
    let variants: Vec<(bool, bool)> = if all_variants {
        // Cover all 4 contract combinations.
        vec![
            (false, false), // Standard CTFExchange
            (false, true),  // YieldBearing CTFExchange
            (true, false),  // NegRisk CTFExchange
            (true, true),   // YieldBearing NegRisk CTFExchange
        ]
    } else {
        vec![(market_params.is_neg_risk, market_params.is_yield_bearing)]
    };

    for (is_neg_risk, is_yield_bearing) in variants {
        tracing::info!(
            is_neg_risk,
            is_yield_bearing,
            "Setting approvals for variant…",
        );
        client
            .set_approvals(is_neg_risk, is_yield_bearing)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "set_approvals(neg_risk={is_neg_risk}, yield={is_yield_bearing}): {e}"
                )
            })?;
        tracing::info!(is_neg_risk, is_yield_bearing, "✓ Approvals set",);
    }

    tracing::info!(
        wallet = %signer,
        "✓ All approvals complete. You can now run: cargo run --bin trading-cli -- live --config {}",
        config_path,
    );

    Ok(())
}
