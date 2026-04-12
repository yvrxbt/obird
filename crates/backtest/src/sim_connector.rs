//! Simulated ExchangeConnector for backtesting.
//!
//! Routes orders to a MatchingEngine instead of a real exchange.
//! The strategy sees exactly the same trait interface as in live trading.

use trading_core::traits::ExchangeConnector;
use trading_core::types::decimal::{Price, Quantity};
use trading_core::types::instrument::{Exchange, InstrumentId};
use trading_core::types::order::{
    OpenOrder, OrderId, OrderRequest, OrderSide, OrderStatus, OrderUpdate,
};
use trading_core::types::position::Position;
use trading_core::error::ConnectorError;
use crate::matching_engine::MatchingEngine;
use tokio::sync::mpsc;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub struct SimConnector {
    exchange: Exchange,
    matching_engine: Arc<Mutex<MatchingEngine>>,
    update_tx: mpsc::UnboundedSender<OrderUpdate>,
    update_rx: mpsc::UnboundedReceiver<OrderUpdate>,
}

impl SimConnector {
    pub fn new(exchange: Exchange) -> Self {
        let (update_tx, update_rx) = mpsc::unbounded_channel();
        Self {
            exchange,
            matching_engine: Arc::new(Mutex::new(MatchingEngine::new())),
            update_tx,
            update_rx,
        }
    }

    /// Feed a market data event into the matching engine.
    /// This may trigger fills on resting orders.
    pub fn on_market_data(&self, instrument: &InstrumentId, mid_price: Price) {
        let mut engine = self.matching_engine.lock().unwrap();
        let fills = engine.check_fills(instrument, mid_price);
        for update in fills {
            let _ = self.update_tx.send(update);
        }
    }

    pub fn matching_engine(&self) -> Arc<Mutex<MatchingEngine>> {
        self.matching_engine.clone()
    }
}

#[async_trait::async_trait]
impl ExchangeConnector for SimConnector {
    fn exchange(&self) -> Exchange {
        self.exchange
    }

    async fn place_order(&self, req: &OrderRequest) -> Result<OrderId, ConnectorError> {
        let order_id = Uuid::new_v4().to_string();
        let mut engine = self.matching_engine.lock().unwrap();
        engine.add_order(order_id.clone(), req.clone());

        // Send acknowledgment
        let _ = self.update_tx.send(OrderUpdate {
            instrument: req.instrument.clone(),
            order_id: order_id.clone(),
            status: OrderStatus::Acknowledged,
            filled_qty: Quantity::zero(),
            remaining_qty: req.quantity,
            avg_fill_price: None,
            timestamp_ns: 0, // TODO: use simulated clock
        });

        Ok(order_id)
    }

    async fn cancel_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
    ) -> Result<(), ConnectorError> {
        let mut engine = self.matching_engine.lock().unwrap();
        engine.cancel_order(order_id);

        let _ = self.update_tx.send(OrderUpdate {
            instrument: instrument.clone(),
            order_id: order_id.clone(),
            status: OrderStatus::Cancelled,
            filled_qty: Quantity::zero(),
            remaining_qty: Quantity::zero(),
            avg_fill_price: None,
            timestamp_ns: 0,
        });

        Ok(())
    }

    async fn cancel_all(&self, instrument: &InstrumentId) -> Result<(), ConnectorError> {
        let mut engine = self.matching_engine.lock().unwrap();
        let cancelled = engine.cancel_all_for(instrument);
        for order_id in cancelled {
            let _ = self.update_tx.send(OrderUpdate {
                instrument: instrument.clone(),
                order_id,
                status: OrderStatus::Cancelled,
                filled_qty: Quantity::zero(),
                remaining_qty: Quantity::zero(),
                avg_fill_price: None,
                timestamp_ns: 0,
            });
        }
        Ok(())
    }

    async fn modify_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
        new_price: Price,
        new_qty: Quantity,
    ) -> Result<OrderId, ConnectorError> {
        // Cancel-replace semantics
        self.cancel_order(instrument, order_id).await?;
        let new_req = OrderRequest {
            instrument: instrument.clone(),
            side: OrderSide::Buy, // TODO: preserve original side
            price: new_price,
            quantity: new_qty,
            tif: trading_core::types::order::TimeInForce::Gtc,
            client_order_id: None,
        };
        self.place_order(&new_req).await
    }

    async fn positions(&self) -> Result<Vec<Position>, ConnectorError> {
        let engine = self.matching_engine.lock().unwrap();
        Ok(engine.positions())
    }

    async fn open_orders(
        &self,
        instrument: &InstrumentId,
    ) -> Result<Vec<OpenOrder>, ConnectorError> {
        let engine = self.matching_engine.lock().unwrap();
        Ok(engine.open_orders_for(instrument))
    }

    fn order_update_rx(&mut self) -> &mut mpsc::UnboundedReceiver<OrderUpdate> {
        &mut self.update_rx
    }
}
