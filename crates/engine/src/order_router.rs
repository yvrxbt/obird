//! OrderRouter — routes Actions from strategies to the correct ExchangeConnector.
//! Applies unified risk checks before routing.

use trading_core::{Action, Event};
use trading_core::types::instrument::Exchange;
use trading_core::error::RiskRejection;
use crate::order_manager::OrderManager;
use crate::risk::UnifiedRiskManager;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub type StrategyId = String;

pub struct OrderRouter {
    managers: HashMap<Exchange, OrderManager>,
    risk: UnifiedRiskManager,
    action_rx: mpsc::UnboundedReceiver<(StrategyId, Action)>,
    strategy_txs: HashMap<StrategyId, mpsc::UnboundedSender<Event>>,
}

impl OrderRouter {
    pub fn new(
        managers: HashMap<Exchange, OrderManager>,
        risk: UnifiedRiskManager,
        action_rx: mpsc::UnboundedReceiver<(StrategyId, Action)>,
        strategy_txs: HashMap<StrategyId, mpsc::UnboundedSender<Event>>,
    ) -> Self {
        Self { managers, risk, action_rx, strategy_txs }
    }

    pub async fn run(&mut self) {
        while let Some((strategy_id, action)) = self.action_rx.recv().await {
            self.handle_action(strategy_id, action).await;
        }
    }

    async fn handle_action(&mut self, strategy_id: StrategyId, action: Action) {
        // Log decisions don't go to exchanges
        if matches!(action, Action::LogDecision { .. }) {
            tracing::info!(?action, "Decision logged");
            return;
        }

        let exchange = match action.exchange() {
            Some(e) => e,
            None => return,
        };

        // Unified risk check
        let positions = self.risk.all_positions();
        if let Err(rejection) = self.risk.check(&action, &positions) {
            tracing::warn!(%strategy_id, ?rejection, "Risk rejection");
            return;
        }

        // Route to correct OrderManager
        if let Some(mgr) = self.managers.get_mut(&exchange) {
            if let Err(e) = mgr.submit(&action).await {
                tracing::error!(%strategy_id, ?e, "Order submission failed");
            }
        } else {
            tracing::error!(%strategy_id, ?exchange, "No OrderManager for exchange");
        }
    }
}
