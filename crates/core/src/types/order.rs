//! Order types.

use crate::types::decimal::{Price, Quantity};
use crate::types::instrument::InstrumentId;
use serde::{Deserialize, Serialize};

pub type OrderId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderSide { Buy, Sell }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce { Gtc, Ioc, PostOnly }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub instrument: InstrumentId,
    pub side: OrderSide,
    pub price: Price,
    pub quantity: Quantity,
    pub tif: TimeInForce,
    /// Client-assigned order ID for correlation
    pub client_order_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdate {
    pub instrument: InstrumentId,
    pub order_id: OrderId,
    pub status: OrderStatus,
    pub filled_qty: Quantity,
    pub remaining_qty: Quantity,
    pub avg_fill_price: Option<Price>,
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    Acknowledged,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenOrder {
    pub order_id: OrderId,
    pub instrument: InstrumentId,
    pub side: OrderSide,
    pub price: Price,
    pub quantity: Quantity,
    pub filled_qty: Quantity,
}
