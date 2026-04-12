//! ExchangeConnector trait.
//!
//! Each exchange crate implements this. The backtest SimConnector also implements it.
//! This is the key abstraction enabling identical strategy code across live and backtest.

use crate::error::ConnectorError;
use crate::types::decimal::{Price, Quantity};
use crate::types::instrument::{Exchange, InstrumentId};
use crate::types::order::{OpenOrder, OrderId, OrderRequest, OrderUpdate};
use crate::types::position::Position;
use tokio::sync::mpsc;

#[async_trait::async_trait]
pub trait ExchangeConnector: Send + Sync + 'static {
    fn exchange(&self) -> Exchange;

    async fn place_order(&self, req: &OrderRequest) -> Result<OrderId, ConnectorError>;

    /// Submit multiple orders in a single round-trip where the exchange supports it.
    ///
    /// Default: sequential loop over `place_order`.
    /// HL override: single `BatchOrder` API call.
    /// Pair-trade pattern: strategy returns `[CancelAll, PlaceOrder×N]` as one batch →
    /// router calls `place_batch` once per exchange → all legs in flight simultaneously.
    async fn place_batch(&self, reqs: &[OrderRequest]) -> Vec<Result<OrderId, ConnectorError>> {
        let mut results = Vec::with_capacity(reqs.len());
        for req in reqs {
            results.push(self.place_order(req).await);
        }
        results
    }

    async fn cancel_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
    ) -> Result<(), ConnectorError>;

    async fn cancel_all(&self, instrument: &InstrumentId) -> Result<(), ConnectorError>;

    async fn modify_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
        new_price: Price,
        new_qty: Quantity,
    ) -> Result<OrderId, ConnectorError>;

    async fn positions(&self) -> Result<Vec<Position>, ConnectorError>;

    async fn open_orders(
        &self,
        instrument: &InstrumentId,
    ) -> Result<Vec<OpenOrder>, ConnectorError>;

    fn order_update_rx(&mut self) -> &mut mpsc::UnboundedReceiver<OrderUpdate>;
}
