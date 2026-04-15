//! Strategy parameters for the prediction market quoter.
//!
//! Spread is defined in **cents** (not basis points) because predict.fun prices
//! are probabilities in [0,1] — "cents" maps naturally to that range and gives
//! the strategy operator an intuitive handle on quote aggressiveness.
//!
//! ## Relationship between params and pricing
//!
//! ```text
//! YES mid = 0.60, spread_cents = 0.02, decimal_precision = 3 (tick = 0.001)
//!
//! Normal:   yes_bid = 0.60 - 0.02 = 0.58   no_bid = 1 - 0.58 = 0.42
//! Crossing: yes_bid clamped to bid_mkt + tick or ask_mkt - tick
//! ```
//!
//! `decimal_precision` is NOT configured here — it is fetched automatically from
//! the predict.fun API at startup and passed through `StrategyState`.

use rust_decimal::Decimal;
use serde::Deserialize;

/// Configuration for the `PredictionQuoter` strategy.
#[derive(Debug, Clone, Deserialize)]
pub struct QuoterParams {
    /// How far (in price units ≈ cents) to place bids from the YES mid.
    /// e.g. `0.02` places yes_bid 2 cents below mid.
    pub spread_cents: Decimal,

    /// USDT notional per order (each of YES and NO gets this much).
    pub order_size_usdt: Decimal,

    /// Optional manual join depth from the predict.fun YES mid.
    ///
    /// When fair-value pricing would place YES at/below the market best bid,
    /// and this field is set, we try:
    ///   `yes_bid = predict_mid - join_cents`
    /// then re-apply crossing guards.
    ///
    /// If omitted, the strategy uses automatic inside-join behavior (`best_bid + tick`).
    #[serde(default)]
    pub join_cents: Option<Decimal>,

    /// Minimum drift (in price units) before pulling and re-quoting.
    /// Avoids thrashing when mid barely moves.
    pub drift_cents: Decimal,

    /// After any fill, wait this many seconds before placing new quotes.
    pub fill_pause_secs: u64,

    /// Minimum time (seconds) orders must stay on the book before a drift-triggered
    /// requote is allowed. Prevents thrashing when the book bounces within drift range.
    /// Fill-triggered cancels always fire immediately regardless of this value.
    #[serde(default = "default_min_hold")]
    pub min_quote_hold_secs: u64,

    /// Maximum token exposure per outcome before stopping new orders.
    pub max_position_tokens: Decimal,

    // ── Points farming metadata ───────────────────────────────────────────────
    // NOT available via API — read manually from the "Activate Points" / "Points Active"
    // tooltip in the predict.fun UI for each market.
    /// Market's max allowed spread window (v in the scoring formula).
    /// `score_factor = ((v - spread) / v)^2`. Read from UI tooltip "Max spread ±Nc".
    /// e.g. 0.06 for a ±6¢ market. Orders beyond this earn zero points.
    #[serde(default = "default_spread_threshold")]
    pub spread_threshold_v: Decimal,

    /// Minimum shares per order side required to qualify for points.
    /// Read from UI tooltip "Min. shares: N". Orders below this earn zero points.
    #[serde(default = "default_min_shares")]
    pub min_shares_per_side: Decimal,
}

fn default_min_hold() -> u64 {
    5
}
fn default_spread_threshold() -> Decimal {
    rust_decimal_macros::dec!(0.06)
}
fn default_min_shares() -> Decimal {
    rust_decimal_macros::dec!(100)
}
