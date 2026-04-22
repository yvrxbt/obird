//! Normalize predict-sdk types into trading-core types.
//!
//! - `orderbook_to_snapshot`: convert a WS `OrderbookData` into `OrderbookSnapshot`.
//! - `now_ns`: wall-clock nanoseconds for local_ts stamping.
//! - `from_wei` / `to_wei`: 18-decimal unit conversion helpers.
//!
//! Prices on predict.fun are in [0, 1] (probability space).
//! Quantities are in human-readable share units (not wei).

use predict_sdk::websocket::OrderbookData;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use trading_core::types::{
    decimal::{Price, Quantity},
    market_data::OrderbookSnapshot,
};

/// 1e18 — used for wei ↔ decimal conversions throughout.
const WEI_SCALE: Decimal = dec!(1_000_000_000_000_000_000);

/// Convert a WS orderbook payload into the canonical `OrderbookSnapshot`.
///
/// Bids are sorted descending (best bid first), asks ascending (best ask first).
/// predict.fun already sends them sorted — we sort anyway for safety.
pub fn orderbook_to_snapshot(book: &OrderbookData, local_ts_ns: u64) -> OrderbookSnapshot {
    let exchange_ts_ns = book.timestamp.unwrap_or(0).saturating_mul(1_000_000);

    let mut bids: Vec<(Price, Quantity)> = book
        .bids
        .iter()
        .map(|l| (Price::new(l.price), Quantity::new(l.size)))
        .collect();

    let mut asks: Vec<(Price, Quantity)> = book
        .asks
        .iter()
        .map(|l| (Price::new(l.price), Quantity::new(l.size)))
        .collect();

    // Ensure canonical sort order
    bids.sort_by(|a, b| b.0.cmp(&a.0)); // descending
    asks.sort_by(|a, b| a.0.cmp(&b.0)); // ascending

    // Use exchange timestamp if available, fall back to local
    let ts = if exchange_ts_ns > 0 {
        exchange_ts_ns
    } else {
        local_ts_ns
    };

    OrderbookSnapshot {
        bids,
        asks,
        timestamp_ns: ts,
    }
}

/// Current wall clock in nanoseconds.
pub fn now_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Convert wei amount string (18 decimals) to human-readable decimal.
/// E.g. "500000000000000000" → 0.5
pub fn from_wei(wei: &str) -> Decimal {
    let n: Decimal = wei.parse().unwrap_or(Decimal::ZERO);
    n.checked_div(WEI_SCALE).unwrap_or(Decimal::ZERO)
}

/// Convert human-readable decimal to wei integer string.
/// E.g. 0.5 → "500000000000000000"
pub fn to_wei(val: Decimal) -> String {
    (val * WEI_SCALE).trunc().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_to_wei_round_trip() {
        let price = dec!(0.5);
        let wei = to_wei(price);
        assert_eq!(wei, "500000000000000000");
        assert_eq!(from_wei(&wei), price);
    }

    #[test]
    fn test_from_wei_ten_shares() {
        assert_eq!(from_wei("10000000000000000000"), dec!(10));
    }

    #[test]
    fn test_from_wei_zero() {
        assert_eq!(from_wei("0"), Decimal::ZERO);
        assert_eq!(from_wei("invalid"), Decimal::ZERO);
    }
}
