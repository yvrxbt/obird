//! Actions flow OUT of strategies. They represent intentions.
//! The OrderRouter validates and routes them. Strategies never execute directly.

use crate::types::decimal::{Price, Quantity};
use crate::types::instrument::InstrumentId;
use crate::types::order::{OrderId, OrderRequest};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    PlaceOrder(OrderRequest),
    CancelOrder { instrument: InstrumentId, order_id: OrderId },
    CancelAll { instrument: InstrumentId },
    ModifyOrder {
        instrument: InstrumentId,
        order_id: OrderId,
        new_price: Price,
        new_qty: Quantity,
    },
    LogDecision {
        strategy_id: String,
        decision: String,
        context: serde_json::Value,
    },
}

impl Action {
    /// Extract the exchange from this action's instrument.
    pub fn exchange(&self) -> Option<crate::types::instrument::Exchange> {
        match self {
            Action::PlaceOrder(req) => Some(req.instrument.exchange),
            Action::CancelOrder { instrument, .. } => Some(instrument.exchange),
            Action::CancelAll { instrument } => Some(instrument.exchange),
            Action::ModifyOrder { instrument, .. } => Some(instrument.exchange),
            Action::LogDecision { .. } => None,
        }
    }
}
