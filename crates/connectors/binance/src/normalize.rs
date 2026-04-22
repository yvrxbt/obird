//! Normalize Binance USD-M Futures wire types → trading-core domain types.

use chrono::Utc;
use rust_decimal::Decimal;
use serde::Deserialize;
use trading_core::{
    types::{
        market_data::OrderbookSnapshot,
        order::{OpenOrder, OrderSide, OrderStatus, OrderUpdate},
        position::{Fill, Position},
    },
    InstrumentId, Price, Quantity,
};

pub fn now_ns() -> u64 {
    Utc::now().timestamp_nanos_opt().unwrap_or_default().max(0) as u64
}

// ── WebSocket message types ───────────────────────────────────────────────────

/// Individual bookTicker stream message (wss://fstream.binance.com/ws/<symbol>@bookTicker).
/// Fires on any BBO change — sub-millisecond update frequency on liquid perps.
#[derive(Debug, Deserialize)]
pub struct BookTickerMsg {
    /// Best bid price
    #[serde(rename = "b")]
    pub best_bid: Decimal,
    /// Best bid quantity
    #[serde(rename = "B")]
    pub best_bid_qty: Decimal,
    /// Best ask price
    #[serde(rename = "a")]
    pub best_ask: Decimal,
    /// Best ask quantity
    #[serde(rename = "A")]
    pub best_ask_qty: Decimal,
    /// Transaction time (milliseconds, Binance exchange clock)
    #[serde(rename = "T")]
    pub trade_time: u64,
    /// Event time (milliseconds)
    #[serde(rename = "E")]
    pub event_time: u64,
}

/// Envelope for user data stream events — discriminate by `e` field.
#[derive(Debug, Deserialize)]
pub struct UserDataEnvelope {
    /// Event type: "ORDER_TRADE_UPDATE", "ACCOUNT_UPDATE", etc.
    #[serde(rename = "e")]
    pub event_type: String,
}

/// ORDER_TRADE_UPDATE event — fires on every order status change and fill.
#[derive(Debug, Deserialize)]
pub struct OrderTradeUpdate {
    /// Transaction time (ms)
    #[serde(rename = "T")]
    pub transaction_time: u64,
    #[serde(rename = "o")]
    pub order: OrderUpdateInner,
}

#[derive(Debug, Deserialize)]
pub struct OrderUpdateInner {
    /// Symbol, e.g. "ETHUSDT"
    #[serde(rename = "s")]
    pub symbol: String,
    /// Binance order ID
    #[serde(rename = "i")]
    pub order_id: u64,
    /// Side: "BUY" or "SELL"
    #[serde(rename = "S")]
    pub side: String,
    /// Current order status: NEW, CANCELED, FILLED, PARTIALLY_FILLED, REJECTED, EXPIRED
    #[serde(rename = "X")]
    pub order_status: String,
    /// Last executed quantity (this fill leg)
    #[serde(rename = "l")]
    pub last_qty: Decimal,
    /// Last filled price (this fill leg)
    #[serde(rename = "L")]
    pub last_price: Decimal,
    /// Cumulative filled quantity across all legs
    #[serde(rename = "z")]
    pub cum_qty: Decimal,
    /// Average fill price (0 until at least one fill)
    #[serde(rename = "ap")]
    pub avg_price: Decimal,
    /// Original order quantity
    #[serde(rename = "q")]
    pub orig_qty: Decimal,
    /// Commission (fee) for this fill leg
    #[serde(rename = "n")]
    pub commission: Decimal,
}

// ── REST response types ───────────────────────────────────────────────────────

/// Response from POST /fapi/v1/order (single placement).
#[derive(Debug, Deserialize)]
pub struct PlaceOrderResponse {
    #[serde(rename = "orderId")]
    pub order_id: u64,
    #[serde(rename = "clientOrderId")]
    pub client_order_id: String,
    /// "NEW", "FILLED", "REJECTED", etc.
    pub status: String,
    #[serde(rename = "avgPrice")]
    pub avg_price: Decimal,
    #[serde(rename = "executedQty")]
    pub executed_qty: Decimal,
    #[serde(rename = "origQty")]
    pub orig_qty: Decimal,
}

/// One element from POST /fapi/v1/batchOrders response array.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum BatchOrderResult {
    Ok(PlaceOrderResponse),
    Err { code: i64, msg: String },
}

/// One element from GET /fapi/v1/openOrders response array.
#[derive(Debug, Deserialize)]
pub struct OpenOrderResponse {
    #[serde(rename = "orderId")]
    pub order_id: u64,
    pub symbol: String,
    pub side: String,
    pub price: Decimal,
    #[serde(rename = "origQty")]
    pub orig_qty: Decimal,
    #[serde(rename = "executedQty")]
    pub executed_qty: Decimal,
}

/// One element from GET /fapi/v2/positionRisk response array.
#[derive(Debug, Deserialize)]
pub struct PositionRiskResponse {
    pub symbol: String,
    /// Signed: positive = long, negative = short
    #[serde(rename = "positionAmt")]
    pub position_amt: Decimal,
    #[serde(rename = "entryPrice")]
    pub entry_price: Decimal,
    #[serde(rename = "unrealizedProfit")]
    pub unrealized_profit: Decimal,
}

/// Response from POST /fapi/v1/listenKey.
#[derive(Debug, Deserialize)]
pub struct ListenKeyResponse {
    #[serde(rename = "listenKey")]
    pub listen_key: String,
}

// ── Conversion functions ──────────────────────────────────────────────────────

/// Convert a bookTicker WS message to an OrderbookSnapshot + exchange timestamp.
pub fn book_ticker_to_snapshot(msg: &BookTickerMsg) -> (OrderbookSnapshot, u64) {
    let exchange_ts_ns = msg.trade_time * 1_000_000;
    let snap = OrderbookSnapshot {
        bids: vec![(Price::new(msg.best_bid), Quantity::new(msg.best_bid_qty))],
        asks: vec![(Price::new(msg.best_ask), Quantity::new(msg.best_ask_qty))],
        timestamp_ns: exchange_ts_ns,
    };
    (snap, exchange_ts_ns)
}

pub fn order_status(s: &str) -> OrderStatus {
    match s {
        "NEW" => OrderStatus::Acknowledged,
        "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
        "FILLED" => OrderStatus::Filled,
        "CANCELED" | "EXPIRED" | "EXPIRED_IN_MATCH" => OrderStatus::Cancelled,
        "REJECTED" => OrderStatus::Rejected,
        _ => OrderStatus::Acknowledged,
    }
}

pub fn order_side(s: &str) -> OrderSide {
    if s == "BUY" {
        OrderSide::Buy
    } else {
        OrderSide::Sell
    }
}

/// Convert a REST place response to an OrderUpdate for the strategy update channel.
pub fn place_to_update(instrument: &InstrumentId, resp: &PlaceOrderResponse) -> OrderUpdate {
    let remaining = (resp.orig_qty - resp.executed_qty).max(Decimal::ZERO);
    OrderUpdate {
        instrument: instrument.clone(),
        order_id: resp.order_id.to_string(),
        status: order_status(&resp.status),
        filled_qty: Quantity::new(resp.executed_qty),
        remaining_qty: Quantity::new(remaining),
        avg_fill_price: if resp.avg_price > Decimal::ZERO {
            Some(Price::new(resp.avg_price))
        } else {
            None
        },
        timestamp_ns: now_ns(),
    }
}

/// Convert an ORDER_TRADE_UPDATE inner to an OrderUpdate.
pub fn ws_order_update(instrument: &InstrumentId, inner: &OrderUpdateInner) -> OrderUpdate {
    let remaining = (inner.orig_qty - inner.cum_qty).max(Decimal::ZERO);
    OrderUpdate {
        instrument: instrument.clone(),
        order_id: inner.order_id.to_string(),
        status: order_status(&inner.order_status),
        filled_qty: Quantity::new(inner.cum_qty),
        remaining_qty: Quantity::new(remaining),
        avg_fill_price: if inner.avg_price > Decimal::ZERO {
            Some(Price::new(inner.avg_price))
        } else {
            None
        },
        timestamp_ns: now_ns(),
    }
}

/// Extract a Fill from an ORDER_TRADE_UPDATE when last_qty > 0 (actual fill leg).
pub fn ws_fill(instrument: &InstrumentId, inner: &OrderUpdateInner) -> Fill {
    Fill {
        order_id: inner.order_id.to_string(),
        instrument: instrument.clone(),
        side: order_side(&inner.side),
        price: Price::new(inner.last_price),
        quantity: Quantity::new(inner.last_qty),
        fee: Price::new(inner.commission),
        timestamp_ns: now_ns(),
    }
}

pub fn position_from_risk(
    instrument: &InstrumentId,
    resp: &PositionRiskResponse,
) -> Option<Position> {
    if resp.position_amt.is_zero() {
        return None;
    }
    Some(Position {
        instrument: instrument.clone(),
        size: Quantity::new(resp.position_amt),
        avg_entry_price: Price::new(resp.entry_price),
        unrealized_pnl: Price::new(resp.unrealized_profit),
    })
}

pub fn open_order_from_rest(instrument: &InstrumentId, resp: &OpenOrderResponse) -> OpenOrder {
    OpenOrder {
        order_id: resp.order_id.to_string(),
        instrument: instrument.clone(),
        side: order_side(&resp.side),
        price: Price::new(resp.price),
        quantity: Quantity::new(resp.orig_qty),
        filled_qty: Quantity::new(resp.executed_qty),
    }
}
