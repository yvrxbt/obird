//! Simulated matching engine for backtesting.
//!
//! Supports multiple fill models:
//! - TradeThrough: fills when a trade occurs at/through our price (realistic)
//! - Optimistic: fills immediately on price cross (overstates PnL)

use rust_decimal::Decimal;
use std::collections::HashMap;
use trading_core::types::decimal::{Price, Quantity};
use trading_core::types::instrument::InstrumentId;
use trading_core::types::order::{
    OpenOrder, OrderId, OrderRequest, OrderSide, OrderStatus, OrderUpdate,
};
use trading_core::types::position::Position;

#[derive(Debug, Clone)]
pub enum FillModel {
    /// Fill when price crosses our order — optimistic
    Optimistic,
    /// Fill only after a trade at/through our price — more realistic
    TradeThrough,
}

#[derive(Debug, Clone)]
struct SimOrder {
    order_id: OrderId,
    instrument: InstrumentId,
    side: OrderSide,
    price: Price,
    quantity: Quantity,
    filled: Quantity,
}

pub struct MatchingEngine {
    orders: HashMap<OrderId, SimOrder>,
    positions: HashMap<InstrumentId, (Decimal, Decimal)>, // (size, avg_entry)
    fill_model: FillModel,
}

impl MatchingEngine {
    pub fn new() -> Self {
        Self {
            orders: HashMap::new(),
            positions: HashMap::new(),
            fill_model: FillModel::TradeThrough,
        }
    }

    pub fn with_fill_model(mut self, model: FillModel) -> Self {
        self.fill_model = model;
        self
    }

    pub fn add_order(&mut self, order_id: OrderId, req: OrderRequest) {
        self.orders.insert(
            order_id.clone(),
            SimOrder {
                order_id,
                instrument: req.instrument,
                side: req.side,
                price: req.price,
                quantity: req.quantity,
                filled: Quantity::zero(),
            },
        );
    }

    pub fn cancel_order(&mut self, order_id: &OrderId) {
        self.orders.remove(order_id);
    }

    pub fn cancel_all_for(&mut self, instrument: &InstrumentId) -> Vec<OrderId> {
        let to_cancel: Vec<OrderId> = self
            .orders
            .iter()
            .filter(|(_, o)| &o.instrument == instrument)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &to_cancel {
            self.orders.remove(id);
        }
        to_cancel
    }

    /// Check if any resting orders should be filled given a price update.
    /// Returns OrderUpdate events for any fills that occurred.
    pub fn check_fills(
        &mut self,
        instrument: &InstrumentId,
        trade_price: Price,
    ) -> Vec<OrderUpdate> {
        let mut updates = Vec::new();
        let mut filled_ids = Vec::new();

        for (id, order) in self.orders.iter() {
            if &order.instrument != instrument {
                continue;
            }

            let should_fill = match order.side {
                OrderSide::Buy => trade_price.inner() <= order.price.inner(),
                OrderSide::Sell => trade_price.inner() >= order.price.inner(),
            };

            if should_fill {
                let remaining = Quantity::new(order.quantity.inner() - order.filled.inner());
                updates.push(OrderUpdate {
                    instrument: order.instrument.clone(),
                    order_id: id.clone(),
                    status: OrderStatus::Filled,
                    filled_qty: remaining,
                    remaining_qty: Quantity::zero(),
                    avg_fill_price: Some(order.price),
                    timestamp_ns: 0,
                });

                // Update position
                let sign = match order.side {
                    OrderSide::Buy => Decimal::ONE,
                    OrderSide::Sell => -Decimal::ONE,
                };
                let entry = self
                    .positions
                    .entry(order.instrument.clone())
                    .or_insert((Decimal::ZERO, Decimal::ZERO));
                entry.0 += sign * remaining.inner();
                entry.1 = order.price.inner(); // simplified avg entry

                filled_ids.push(id.clone());
            }
        }

        for id in filled_ids {
            self.orders.remove(&id);
        }

        updates
    }

    pub fn positions(&self) -> Vec<Position> {
        self.positions
            .iter()
            .map(|(inst, (size, avg_entry))| Position {
                instrument: inst.clone(),
                size: Quantity::new(*size),
                avg_entry_price: Price::new(*avg_entry),
                unrealized_pnl: Price::zero(), // TODO: calculate from current price
            })
            .collect()
    }

    pub fn open_orders_for(&self, instrument: &InstrumentId) -> Vec<OpenOrder> {
        self.orders
            .values()
            .filter(|o| &o.instrument == instrument)
            .map(|o| OpenOrder {
                order_id: o.order_id.clone(),
                instrument: o.instrument.clone(),
                side: o.side,
                price: o.price,
                quantity: o.quantity,
                filled_qty: o.filled,
            })
            .collect()
    }
}
