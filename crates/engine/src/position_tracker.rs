//! Unified position tracking across all exchanges.

use trading_core::types::position::{Position, Fill};
use trading_core::InstrumentId;
use std::collections::HashMap;

pub struct PositionTracker {
    positions: HashMap<InstrumentId, Position>,
}

impl PositionTracker {
    pub fn new() -> Self { Self { positions: HashMap::new() } }

    pub fn on_fill(&mut self, fill: &Fill) {
        // TODO: Update position based on fill
        let _ = fill;
    }

    pub fn get(&self, instrument: &InstrumentId) -> Option<&Position> {
        self.positions.get(instrument)
    }

    pub fn all(&self) -> Vec<Position> {
        self.positions.values().cloned().collect()
    }
}
