//! ExchangeConnector trait — abstraction over exchange-specific order management.
//!
//! Each exchange crate implements this. The backtest crate also implements it
//! with a simulated matching engine. This is the key abstraction that enables
//! backtesting with identical strategy code.

use crate::types::decimal::{Price, Quantity};
use crate::types::instrument::{Exchange, InstrumentId};
use crate::types::order::{OrderId, OrderRequest, OrderUpdate, OpenOrder};
use crate::types::position::Position;
use crate::error::ConnectorError;
use tokio::sync::mpsc;

#[async_trait::async_trait]
pub trait ExchangeConnector: Send + Sync + 'static {
    /// Which exchange this connector handles.
    fn exchange(&self) -> Exchange;

    async fn place_order(&self, req: &OrderRequest) -> Result<OrderId, ConnectorError>;
    async fn cancel_order(&self, instrument: &InstrumentId, order_id: &OrderId) -> Result<(), ConnectorError>;
    async fn cancel_all(&self, instrument: &InstrumentId) -> Result<(), ConnectorError>;
    async fn modify_order(
        &self, instrument: &InstrumentId, order_id: &OrderId,
        new_price: Price, new_qty: Quantity,
    ) -> Result<OrderId, ConnectorError>;

    async fn positions(&self) -> Result<Vec<Position>, ConnectorError>;
    async fn open_orders(&self, instrument: &InstrumentId) -> Result<Vec<OpenOrder>, ConnectorError>;

    /// Receive order updates (fills, acks, rejects) from this exchange.
    fn order_update_rx(&mut self) -> &mut mpsc::UnboundedReceiver<OrderUpdate>;
}
