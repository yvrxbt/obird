//! Strategy parameters for the prediction market quoter.

use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct QuoterParams {
    /// Base half-spread in basis points
    pub base_spread_bps: Decimal,
    /// Maximum position size (in outcome tokens)
    pub max_position: Decimal,
    /// How aggressively to skew quotes based on position
    /// Higher = more aggressive skew to reduce position
    pub skew_factor: Decimal,
    /// Minimum confidence from fair value model to quote
    pub min_confidence: f64,
    /// Order size per level
    pub order_size: Decimal,
    /// Number of levels to quote on each side
    pub num_levels: usize,
}
