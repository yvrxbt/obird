//! Unified position tracking across all exchanges.

use std::collections::HashMap;
use trading_core::types::position::{Fill, Position};
use trading_core::InstrumentId;

pub struct PositionTracker {
    positions: HashMap<InstrumentId, Position>,
}

impl PositionTracker {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
        }
    }

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
