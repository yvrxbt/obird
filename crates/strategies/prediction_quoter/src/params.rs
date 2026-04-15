//! Strategy parameters for the prediction market quoter.
//!
//! See `PREDICT_QUOTING_DESIGN.md` for the full pricing decision tree and tuning guide.

use rust_decimal::Decimal;
use serde::Deserialize;

/// Configuration for the `PredictionQuoter` strategy.
#[derive(Debug, Clone, Deserialize)]
pub struct QuoterParams {
    /// Distance from the effective fair value to place each bid (in price units ≈ cents).
    ///
    /// The effective FV for each side is `min(poly_mid, predict_mid)` for YES and
    /// `1 - max(poly_mid, predict_mid)` for NO — the conservative choice that keeps
    /// bids below BOTH venues' mids, minimising adverse selection.
    ///
    /// Score factor = `((v - spread_cents) / v)²` where `v = spread_threshold_v`.
    ///
    /// | spread_cents | score_factor (v=0.06) | fill risk |
    /// |---|---|---|
    /// | 0.01 | 69% | high |
    /// | 0.02 | 44% | moderate (default) |
    /// | 0.03 | 25% | low |
    ///
    /// Rule of thumb: start at 0.02. Increase if getting filled too often.
    pub spread_cents: Decimal,

    /// USDT notional per order (each of YES and NO gets this much).
    pub order_size_usdt: Decimal,

    /// Minimum drift (in price units) before pulling and re-quoting.
    /// Avoids thrashing when mid barely moves. Set ≥ `spread_cents`.
    pub drift_cents: Decimal,

    /// Trigger a defensive requote when a resting bid gets too close to the ask
    /// (i.e., likely to be lifted/hit) by this distance or less.
    ///
    /// Example:
    /// - `0.00`: trigger only at/through ask (very permissive).
    /// - `0.01`: trigger when within 1 cent of ask.
    #[serde(default = "default_touch_trigger_cents")]
    pub touch_trigger_cents: Decimal,

    /// On defensive touch-triggered requotes, target at least this much distance
    /// from ask for each side.
    #[serde(default = "default_touch_retreat_cents")]
    pub touch_retreat_cents: Decimal,

    /// After any fill, wait this many seconds before placing new quotes.
    pub fill_pause_secs: u64,

    /// Minimum time (seconds) orders must stay on the book before a drift-triggered
    /// requote is allowed. Fill-triggered cancels bypass this.
    #[serde(default = "default_min_hold")]
    pub min_quote_hold_secs: u64,

    /// Seconds since the last Polymarket heartbeat (PONG or book update) before the
    /// FV is considered stale and quoting pauses.
    ///
    /// **Must be > 60** (the WS recv-timeout). The feed sends a TEXT PING every 10s
    /// and re-publishes the last known book on each PONG, so stale only fires on a
    /// genuine feed outage. Default 90 = 60s timeout + 30s buffer.
    #[serde(default = "default_fv_stale_secs")]
    pub fv_stale_secs: u64,

    /// Maximum token exposure per outcome before stopping new orders on that side.
    pub max_position_tokens: Decimal,

    // ── Points farming metadata ───────────────────────────────────────────────
    /// Market's max-earning spread window (v in the scoring formula).
    /// `score_factor = ((v - spread_cents) / v)²`. Sides beyond this earn zero and
    /// are skipped entirely. Auto-filled by `predict-markets --write-configs`.
    #[serde(default = "default_spread_threshold")]
    pub spread_threshold_v: Decimal,

    /// Minimum qualifying order size (shares). Auto-filled by `predict-markets --write-configs`.
    #[serde(default = "default_min_shares")]
    pub min_shares_per_side: Decimal,
}

fn default_min_hold() -> u64 {
    5
}
fn default_touch_trigger_cents() -> Decimal {
    rust_decimal_macros::dec!(0.01)
}
fn default_touch_retreat_cents() -> Decimal {
    rust_decimal_macros::dec!(0.02)
}
fn default_fv_stale_secs() -> u64 {
    90
}
fn default_spread_threshold() -> Decimal {
    rust_decimal_macros::dec!(0.06)
}
fn default_min_shares() -> Decimal {
    rust_decimal_macros::dec!(100)
}
