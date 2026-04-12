use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct PairTraderParams {
    pub entry_zscore: f64,
    pub exit_zscore: f64,
    pub stop_zscore: f64,
    pub lookback_periods: usize,
    pub max_position_notional: Decimal,
    pub order_size: Decimal,
}
