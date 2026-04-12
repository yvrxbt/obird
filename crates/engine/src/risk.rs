//! Unified risk management across all exchanges.

use trading_core::Action;
use trading_core::error::RiskRejection;
use trading_core::types::position::Position;
use rust_decimal::Decimal;
use std::collections::HashMap;
use trading_core::InstrumentId;

pub struct UnifiedRiskManager {
    positions: HashMap<InstrumentId, Position>,
    max_total_notional: Decimal,
    max_drawdown_pct: f64,
}

impl UnifiedRiskManager {
    pub fn new(max_total_notional: Decimal, max_drawdown_pct: f64) -> Self {
        Self {
            positions: HashMap::new(),
            max_total_notional,
            max_drawdown_pct,
        }
    }

    pub fn check(&self, action: &Action, positions: &[Position]) -> Result<(), RiskRejection> {
        // TODO: Implement risk checks
        // 1. Per-strategy position limits
        // 2. Portfolio-level notional limits
        // 3. Correlated exposure limits (e.g., total BTC exposure)
        // 4. Drawdown check
        let _ = (action, positions);
        Ok(())
    }

    pub fn all_positions(&self) -> Vec<Position> {
        self.positions.values().cloned().collect()
    }

    pub fn update_position(&mut self, instrument: InstrumentId, position: Position) {
        self.positions.insert(instrument, position);
    }
}
