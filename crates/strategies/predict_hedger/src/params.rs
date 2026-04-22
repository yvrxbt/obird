//! Hedge strategy configuration parameters.

use rust_decimal::Decimal;
use serde::Deserialize;

/// Runtime parameters for `PredictHedgeStrategy`.
///
/// Loaded from the `[hedge]` section of the market config TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct HedgeParams {
    /// Whether hedging is active. Setting to false is a kill-switch — all
    /// hedge logic is bypassed and no orders are placed.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Minimum unhedged notional (qty × poly_ask price in USDC) before a
    /// hedge order is placed. Batches small fills to reduce order count.
    #[serde(default = "default_hedge_min_notional")]
    pub hedge_min_notional: Decimal,

    /// Maximum unhedged notional (USDC) before urgency escalates.
    /// Above this threshold, the hedge order is placed at the ask
    /// instead of one tick inside the spread.
    #[serde(default = "default_max_unhedged_notional")]
    pub max_unhedged_notional: Decimal,

    /// Maximum time (seconds) an unhedged position is tolerated before
    /// escalating price aggression regardless of notional size.
    #[serde(default = "default_max_unhedged_duration_secs")]
    pub max_unhedged_duration_secs: u64,

    /// Maximum slippage above Polymarket mid accepted when placing a hedge order.
    ///
    /// Specifically: `poly_ask - poly_mid ≤ max_slippage_cents`.
    /// For a normal 0.01-tick market, the half-spread is 0.005, so the default
    /// of 0.05 allows crossing up to 5 ticks above mid. This should almost
    /// never be hit — Polymarket markets are tight.
    ///
    /// Note: this is NOT a check against break-even vs predict fill prices.
    /// We hedge for risk reduction regardless of venue divergence.
    #[serde(default = "default_max_slippage_cents")]
    pub max_slippage_cents: Decimal,
}

impl Default for HedgeParams {
    fn default() -> Self {
        Self {
            enabled: true,
            hedge_min_notional: default_hedge_min_notional(),
            max_unhedged_notional: default_max_unhedged_notional(),
            max_unhedged_duration_secs: default_max_unhedged_duration_secs(),
            max_slippage_cents: default_max_slippage_cents(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_hedge_min_notional() -> Decimal {
    rust_decimal_macros::dec!(5)
}
fn default_max_unhedged_notional() -> Decimal {
    rust_decimal_macros::dec!(100)
}
fn default_max_unhedged_duration_secs() -> u64 {
    60
}
fn default_max_slippage_cents() -> Decimal {
    rust_decimal_macros::dec!(0.05)
}
