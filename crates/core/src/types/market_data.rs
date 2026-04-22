//! Market data types shared across all connectors.

use crate::types::decimal::{Price, Quantity};
use serde::{Deserialize, Serialize};

/// Level 2 orderbook snapshot.
/// Bids are sorted DESCENDING (best bid first).
/// Asks are sorted ASCENDING (best ask first).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderbookSnapshot {
    pub bids: Vec<(Price, Quantity)>,
    pub asks: Vec<(Price, Quantity)>,
    pub timestamp_ns: u64,
}

impl OrderbookSnapshot {
    pub fn best_bid(&self) -> Option<(Price, Quantity)> {
        self.bids.first().copied()
    }
    pub fn best_ask(&self) -> Option<(Price, Quantity)> {
        self.asks.first().copied()
    }
    pub fn mid_price(&self) -> Option<Price> {
        match (self.best_bid(), self.best_ask()) {
            (Some((b, _)), Some((a, _))) => {
                Some(Price::new((b.0 + a.0) / rust_decimal::Decimal::TWO))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub price: Price,
    pub quantity: Quantity,
    pub side: TradeSide,
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TradeSide {
    Buy,
    Sell,
}
