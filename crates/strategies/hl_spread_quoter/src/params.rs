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

    /// Inventory skew — bps of reservation-mid shift per unit of net_position.
    ///
    /// When long N units, the mid reference shifts down by (N * skew_factor_bps_per_unit) bps,
    /// making the ask relatively cheaper and the bid relatively more expensive. This steers
    /// fills toward mean-reverting the position without explicitly widening spreads.
    ///
    /// Set to 0 to disable (pure symmetric quoting). Reasonable start: 1–5 bps per unit,
    /// calibrated so that at max_position the shift equals roughly one spread width.
    ///
    /// Example: order_size=0.01, max_position=0.1, level_bps=[5,10]
    ///   skew_factor_bps_per_unit=50 → at max long (0.1 ETH), reservation shifts 5 bps down.
    #[serde(default)]
    pub skew_factor_bps_per_unit: Decimal,

    /// Exchange taker fee in bps — used for P&L reporting only, does not affect quoting.
    /// HL mainnet taker fee is ~2 bps for most accounts. Set accurately for correct P&L.
    #[serde(default = "default_taker_fee_bps")]
    pub taker_fee_bps: Decimal,
}

fn default_taker_fee_bps() -> Decimal { Decimal::new(2, 1) } // 0.2 bps default (HL maker rebate)

impl QuoterParams {
    pub fn drift_ratio(&self) -> Decimal {
        Decimal::from(self.drift_bps) / Decimal::from(10_000)
    }

    pub fn level_ratio(&self, level_idx: usize) -> Decimal {
        Decimal::from(self.level_bps[level_idx]) / Decimal::from(10_000)
    }
}
