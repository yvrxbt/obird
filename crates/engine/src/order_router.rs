//! OrderRouter — routes action batches from strategies to exchange connectors.
//!
//! Receives Vec<Action> batches. Within a batch:
//!   1. Cancels execute sequentially (CancelAll before any PlaceOrder in the same batch)
//!   2. PlaceOrder actions are grouped by exchange and submitted concurrently
//!      — HL uses BatchOrder (single API call), cross-exchange legs fire in parallel
//!
//! This is the foundation for simultaneous pair-trade leg execution:
//! strategy returns [PlaceOrder(HL), PlaceOrder(Binance)] → both legs in-flight together.

use std::collections::HashMap;

use futures::future::join_all;
use trading_core::{
    Action, Event,
    types::instrument::Exchange,
    types::order::OrderRequest,
};

use crate::order_manager::OrderManager;
use crate::risk::UnifiedRiskManager;

pub type StrategyId = String;

pub struct OrderRouter {
    managers: HashMap<Exchange, OrderManager>,
    risk: UnifiedRiskManager,
    action_rx: tokio::sync::mpsc::UnboundedReceiver<(StrategyId, Vec<Action>)>,
    strategy_txs: HashMap<StrategyId, tokio::sync::mpsc::UnboundedSender<Event>>,
}

impl OrderRouter {
    pub fn new(
        managers: HashMap<Exchange, OrderManager>,
        risk: UnifiedRiskManager,
        action_rx: tokio::sync::mpsc::UnboundedReceiver<(StrategyId, Vec<Action>)>,
        strategy_txs: HashMap<StrategyId, tokio::sync::mpsc::UnboundedSender<Event>>,
    ) -> Self {
        Self { managers, risk, action_rx, strategy_txs }
    }

    pub async fn run(&mut self) {
        while let Some((strategy_id, actions)) = self.action_rx.recv().await {
            self.handle_batch(strategy_id, actions).await;
        }
    }

    async fn handle_batch(&mut self, strategy_id: StrategyId, actions: Vec<Action>) {
        if actions.is_empty() {
            return;
        }

        // Pass 1 — risk gate (stubbed, always passes)
        // TODO: implement per-strategy position limits + portfolio notional limits

        // Pass 2 — execute cancels first, sequentially
        // CancelAll / CancelOrder must land before any PlaceOrder in the same batch
        // to guarantee we're out of the way before re-quoting.
        let mut place_orders: HashMap<Exchange, Vec<OrderRequest>> = HashMap::new();
        let mut other_actions: Vec<Action> = Vec::new();

        for action in actions {
            match &action {
                Action::PlaceOrder(req) => {
                    place_orders
                        .entry(req.instrument.exchange)
                        .or_default()
                        .push(req.clone());
                }
                Action::LogDecision { decision, context, .. } => {
                    tracing::info!(decision, ?context, "Decision logged");
                }
                _ => {
                    // CancelAll, CancelOrder, ModifyOrder — execute now, sequentially
                    let exchange = action.exchange();
                    if let Some(exchange) = exchange {
                        if let Some(mgr) = self.managers.get_mut(&exchange) {
                            if let Err(e) = mgr.submit(&action).await {
                                tracing::error!(
                                    strategy = %strategy_id,
                                    ?action,
                                    error = %e,
                                    "Order action failed"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Pass 3 — submit place batches concurrently across exchanges
        // Each exchange gets one place_batch call (HL: single BatchOrder API call).
        // Multiple exchanges run in parallel via join_all.
        if !place_orders.is_empty() {
            let futures: Vec<_> = place_orders
                .into_iter()
                .filter_map(|(exchange, reqs)| {
                    self.managers.get(&exchange).map(|_| (exchange, reqs))
                })
                .map(|(exchange, reqs)| {
                    let mgr = self.managers.get(&exchange).unwrap();
                    let sid = strategy_id.clone();
                    async move {
                        let results = mgr.place_batch(reqs).await;
                        for (i, result) in results.iter().enumerate() {
                            if let Err(e) = result {
                                tracing::error!(
                                    strategy = %sid,
                                    exchange = ?exchange,
                                    order_idx = i,
                                    error = %e,
                                    "Batch place order failed"
                                );
                            }
                        }
                    }
                })
                .collect();

            join_all(futures).await;
        }
    }
}
