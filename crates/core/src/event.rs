//! Events flow INTO strategies. They represent things that happened.

use crate::types::decimal::Price;
use crate::types::instrument::InstrumentId;
use crate::types::market_data::{OrderbookSnapshot, Trade};
use crate::types::order::OrderUpdate;
use crate::types::position::{Fill, Position};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    BookUpdate {
        instrument: InstrumentId,
        book: OrderbookSnapshot,
        exchange_ts_ns: u64,
        local_ts_ns: u64,
    },
    MarketTrade {
        instrument: InstrumentId,
        trade: Trade,
    },
    Fill {
        instrument: InstrumentId,
        fill: Fill,
    },
    OrderUpdate {
        instrument: InstrumentId,
        update: OrderUpdate,
    },
    Tick {
        timestamp_ns: u64,
    },
    FairValueUpdate {
        instrument: InstrumentId,
        fair_value: Price,
        confidence: f64,
        model_version: String,
    },
    PositionSnapshot {
        positions: Vec<Position>,
    },
}
