//! Strategy parameters for HlSpreadQuoter — deserializable from [strategies.params] in TOML.

use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct QuoterParams {
    /// Half-spread in bps for each quoting level.
    /// e.g. [5, 10] → quotes at mid±5bps and mid±10bps simultaneously.
    pub level_bps: Vec<u32>,

    /// Order size per side per level, in asset units.
    pub order_size: Decimal,

    /// Pull all quotes if mid moves more than this many bps from last-quoted mid.
    pub drift_bps: u32,

    /// Seconds to wait after a drift pull before requoting.
    pub drift_pause_secs: u64,

    /// Seconds to wait after any fill before requoting.
    pub fill_pause_secs: u64,

    /// Max absolute net position. Stop adding to the accumulating side beyond this.
    pub max_position: Decimal,
}

impl QuoterParams {
    pub fn drift_ratio(&self) -> Decimal {
        Decimal::from(self.drift_bps) / Decimal::from(10_000)
    }

    pub fn level_ratio(&self, level_idx: usize) -> Decimal {
        Decimal::from(self.level_bps[level_idx]) / Decimal::from(10_000)
    }
}
