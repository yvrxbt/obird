//! Per-exchange OrderManager — delegates to ExchangeConnector.

use std::sync::atomic::{AtomicU64, Ordering};
use trading_core::error::ConnectorError;
use trading_core::traits::ExchangeConnector;
use trading_core::types::order::{OrderId, OrderRequest};
use trading_core::Action;

pub struct NonceManager {
    current: AtomicU64,
}

impl NonceManager {
    pub fn new(initial: u64) -> Self {
        Self {
            current: AtomicU64::new(initial),
        }
    }
    pub fn next(&self) -> u64 {
        self.current.fetch_add(1, Ordering::SeqCst)
    }
}

pub struct OrderManager {
    pub(crate) connector: Box<dyn ExchangeConnector>,
    nonce: Option<NonceManager>,
}

impl OrderManager {
    pub fn new(connector: Box<dyn ExchangeConnector>, use_nonce: bool) -> Self {
        let nonce = if use_nonce {
            Some(NonceManager::new(0))
        } else {
            None
        };
        Self { connector, nonce }
    }

    /// Submit a single non-place action (cancel, modify). Awaited by the router
    /// before any place_batch call in the same strategy batch.
    pub async fn submit(&mut self, action: &Action) -> Result<(), ConnectorError> {
        match action {
            Action::PlaceOrder(req) => {
                self.connector.place_order(req).await?;
            }
            Action::CancelOrder {
                instrument,
                order_id,
            } => {
                self.connector.cancel_order(instrument, order_id).await?;
            }
            Action::CancelAll { instrument } => {
                self.connector.cancel_all(instrument).await?;
            }
            Action::ModifyOrder {
                instrument,
                order_id,
                new_price,
                new_qty,
            } => {
                self.connector
                    .modify_order(instrument, order_id, *new_price, *new_qty)
                    .await?;
            }
            Action::LogDecision { .. } => {}
        }
        Ok(())
    }

    /// Batch-place using the connector's native batch API where available.
    /// For HL: single BatchOrder call. Default: sequential.
    pub async fn place_batch(
        &self,
        reqs: Vec<OrderRequest>,
    ) -> Vec<Result<OrderId, ConnectorError>> {
        self.connector.place_batch(&reqs).await
    }
}
