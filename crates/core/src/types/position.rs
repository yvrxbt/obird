//! Position and fill types.

use crate::types::decimal::{Price, Quantity};
use crate::types::instrument::InstrumentId;
use crate::types::order::{OrderId, OrderSide};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub instrument: InstrumentId,
    /// Signed: positive = long, negative = short
    pub size: Quantity,
    pub avg_entry_price: Price,
    pub unrealized_pnl: Price,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub order_id: OrderId,
    pub instrument: InstrumentId,
    pub side: OrderSide,
    pub price: Price,
    pub quantity: Quantity,
    pub fee: Price,
    pub timestamp_ns: u64,
}
