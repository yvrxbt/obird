//! Instrument identification.
//! An InstrumentId uniquely identifies a tradeable across the entire system.
//! The Exchange field is critical — the same symbol on different exchanges
//! is a DIFFERENT instrument with different order books and positions.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Exchange {
    Hyperliquid,
    Lighter,
    Binance,
    Polymarket,
    PredictFun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InstrumentKind {
    Perpetual,
    Spot,
    /// Binary outcome (prediction market)
    Binary,
    /// Multi-outcome (prediction market)
    MultiOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstrumentId {
    pub exchange: Exchange,
    pub kind: InstrumentKind,
    pub symbol: String,
}

impl fmt::Display for InstrumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}.{:?}.{}", self.exchange, self.kind, self.symbol)
    }
}

impl InstrumentId {
    pub fn new(exchange: Exchange, kind: InstrumentKind, symbol: impl Into<String>) -> Self {
        Self {
            exchange,
            kind,
            symbol: symbol.into(),
        }
    }
}
