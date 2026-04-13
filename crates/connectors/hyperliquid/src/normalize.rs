//! Convert hypersdk wire types → trading-core domain types.

use chrono::Utc;
use trading_core::{
    InstrumentId, Price, Quantity,
    types::{
        market_data::OrderbookSnapshot,
        order::{OrderId, OrderSide, OrderStatus, OrderUpdate},
        position::Fill,
    },
};

use hypersdk::hypercore::types::{
    BookLevel, Fill as HlFill, L2Book as HlL2Book, OrderStatus as HlOrderStatus,
    OrderUpdate as HlOrderUpdate, Side as HlSide, WsBasicOrder,
};

// ── Timestamps ────────────────────────────────────────────────────────────────

pub fn now_ns() -> u64 {
    Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_default()
        .max(0) as u64
}

// ── Order book ────────────────────────────────────────────────────────────────

/// Convert an L2Book snapshot to our domain type.
/// `exchange_ts_ns` comes from `book.time * 1_000_000` (HL sends milliseconds).
pub fn l2book_to_snapshot(book: &HlL2Book, exchange_ts_ns: u64) -> OrderbookSnapshot {
    let bids = book.bids().iter().map(level_to_pair).collect();
    let asks = book.asks().iter().map(level_to_pair).collect();
    OrderbookSnapshot { bids, asks, timestamp_ns: exchange_ts_ns }
}

fn level_to_pair(l: &BookLevel) -> (Price, Quantity) {
    (Price::new(l.px), Quantity::new(l.sz))
}

// ── Fills ─────────────────────────────────────────────────────────────────────

pub fn fill(instrument: &InstrumentId, hl: &HlFill) -> Fill {
    let side = if hl.side == HlSide::Bid { OrderSide::Buy } else { OrderSide::Sell };
    Fill {
        order_id: hl.oid.to_string(),
        instrument: instrument.clone(),
        side,
        price: Price::new(hl.px),
        quantity: Quantity::new(hl.sz),
        fee: Price::new(hl.fee),
        timestamp_ns: now_ns(),
    }
}

// ── Order updates ─────────────────────────────────────────────────────────────

pub fn order_update(instrument: &InstrumentId, hl: &HlOrderUpdate<WsBasicOrder>) -> OrderUpdate {
    let status = match hl.status {
        HlOrderStatus::Open => OrderStatus::Acknowledged,
        HlOrderStatus::Filled => OrderStatus::Filled,
        HlOrderStatus::Canceled
        | HlOrderStatus::MarginCanceled
        | HlOrderStatus::VaultWithdrawalCanceled
        | HlOrderStatus::OpenInterestCapCanceled
        | HlOrderStatus::SelfTradeCanceled
        | HlOrderStatus::ReduceOnlyCanceled
        | HlOrderStatus::SiblingFilledCanceled
        | HlOrderStatus::DelistedCanceled
        | HlOrderStatus::LiquidatedCanceled
        | HlOrderStatus::ScheduledCancel => OrderStatus::Cancelled,
        HlOrderStatus::Rejected
        | HlOrderStatus::TickRejected
        | HlOrderStatus::MinTradeNtlRejected
        | HlOrderStatus::PerpMarginRejected
        | HlOrderStatus::Triggered => OrderStatus::Rejected,
        _ => OrderStatus::Acknowledged,
    };

    let filled_qty = hl.order.orig_sz
        .checked_sub(hl.order.sz)
        .unwrap_or(rust_decimal::Decimal::ZERO);

    OrderUpdate {
        instrument: instrument.clone(),
        order_id: hl.order.oid.to_string(),
        status,
        filled_qty: Quantity::new(filled_qty),
        remaining_qty: Quantity::new(hl.order.sz),
        avg_fill_price: None,
        timestamp_ns: now_ns(),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn to_hl_side(side: OrderSide) -> bool {
    matches!(side, OrderSide::Buy)
}

pub fn order_id_from_oid(oid: u64) -> OrderId {
    oid.to_string()
}

pub fn oid_from_order_id(id: &OrderId) -> anyhow::Result<u64> {
    id.parse::<u64>().map_err(|e| anyhow::anyhow!("invalid oid '{}': {}", id, e))
}
