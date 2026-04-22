//! Normalize Polymarket WS events to trading-core types.
//!
//! ## Message format
//!
//! Events arrive as TEXT frames containing either a JSON object (incremental)
//! or a JSON array (initial snapshot — one entry per subscribed token).
//!
//! ## Book state
//!
//! `BookState` maintains an incremental BTreeMap for both sides. This is correct
//! for `price_change` events (size=0 means remove the level) and for `book`
//! snapshots (clears and re-populates). The BBO is derived as max bid / min ask.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::Deserialize;
use trading_core::types::{
    decimal::{Price, Quantity},
    market_data::OrderbookSnapshot,
};

// ── WS message types ──────────────────────────────────────────────────────────

/// A single bid or ask price level from a `book` snapshot event.
#[derive(Debug, Deserialize)]
pub struct PriceLevel {
    pub price: String,
    pub size: String,
}

/// A single change entry from a `price_change` incremental event.
#[derive(Debug, Deserialize)]
pub struct PriceChangeEntry {
    pub price: String,
    pub size: String,
    /// `"BUY"` for bid side, `"SELL"` for ask side.
    pub side: String,
}

/// Parsed Polymarket CLOB WebSocket event.
///
/// The `event_type` field is used as the serde tag.
#[derive(Debug, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum PolymarketEvent {
    /// Full orderbook snapshot — sent on initial subscription and after fills.
    Book {
        asset_id: String,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        /// Unix timestamp in **milliseconds** (per Polymarket API docs).
        timestamp: Option<String>,
    },
    /// Incremental price level update. `size = "0"` means remove that level.
    PriceChange {
        asset_id: String,
        changes: Vec<PriceChangeEntry>,
        /// Unix timestamp in **milliseconds** (per Polymarket API docs).
        timestamp: Option<String>,
    },
}

// ── Book state ────────────────────────────────────────────────────────────────

/// Incremental orderbook for a single Polymarket token.
///
/// Bids stored ascending by price (BTreeMap default); best bid = last entry.
/// Asks stored ascending by price; best ask = first entry.
#[derive(Debug, Default)]
pub struct BookState {
    pub bids: BTreeMap<Decimal, Decimal>, // price → size
    pub asks: BTreeMap<Decimal, Decimal>, // price → size
}

impl BookState {
    /// Replace book with a full snapshot from a `book` event.
    pub fn apply_snapshot(&mut self, bids: &[PriceLevel], asks: &[PriceLevel]) {
        self.bids.clear();
        self.asks.clear();
        for l in bids {
            if let (Ok(p), Ok(s)) = (l.price.parse::<Decimal>(), l.size.parse::<Decimal>()) {
                if s > Decimal::ZERO {
                    self.bids.insert(p, s);
                }
            }
        }
        for l in asks {
            if let (Ok(p), Ok(s)) = (l.price.parse::<Decimal>(), l.size.parse::<Decimal>()) {
                if s > Decimal::ZERO {
                    self.asks.insert(p, s);
                }
            }
        }
    }

    /// Apply incremental changes from a `price_change` event.
    pub fn apply_changes(&mut self, changes: &[PriceChangeEntry]) {
        for c in changes {
            let Ok(price) = c.price.parse::<Decimal>() else {
                continue;
            };
            let Ok(size) = c.size.parse::<Decimal>() else {
                continue;
            };
            match c.side.as_str() {
                "BUY" => {
                    if size.is_zero() {
                        self.bids.remove(&price);
                    } else {
                        self.bids.insert(price, size);
                    }
                }
                "SELL" => {
                    if size.is_zero() {
                        self.asks.remove(&price);
                    } else {
                        self.asks.insert(price, size);
                    }
                }
                _ => {}
            }
        }
    }

    /// Best bid price (highest bid), or `None` if empty.
    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.iter().next_back().map(|(p, _)| *p)
    }

    /// Best ask price (lowest ask), or `None` if empty.
    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.iter().next().map(|(p, _)| *p)
    }

    /// Produce a minimal `OrderbookSnapshot` containing only the BBO.
    ///
    /// Returns `None` if either side is empty (no tradeable market).
    pub fn to_snapshot(&self, ts_ns: u64) -> Option<OrderbookSnapshot> {
        let bb = self.best_bid()?;
        let ba = self.best_ask()?;
        Some(OrderbookSnapshot {
            bids: vec![(Price::new(bb), Quantity::new(self.bids[&bb]))],
            asks: vec![(Price::new(ba), Quantity::new(self.asks[&ba]))],
            timestamp_ns: ts_ns,
        })
    }
}

// ── Message parsing ───────────────────────────────────────────────────────────

/// Parse a single Polymarket WS TEXT frame into zero or more events.
///
/// The initial subscription response is a JSON **array** (one `book` event per
/// subscribed token). All subsequent events are single JSON **objects**.
/// Both formats are handled here.
pub fn parse_message(text: &str) -> Vec<PolymarketEvent> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| serde_json::from_value::<PolymarketEvent>(v).ok())
            .collect(),
        Ok(v) => serde_json::from_value::<PolymarketEvent>(v)
            .ok()
            .into_iter()
            .collect(),
        Err(_) => vec![],
    }
}

/// Current wall clock in nanoseconds (for local timestamp stamping).
pub fn now_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
