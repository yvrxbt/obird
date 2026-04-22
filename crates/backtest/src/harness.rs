//! Backtest harness — drives a strategy with recorded market data.
//!
//! The harness replays events through the strategy, processing actions
//! through a SimConnector, and collects results for reporting.

use crate::report::BacktestReport;
use crate::sim_connector::SimConnector;
use crate::sim_market_data::SimMarketDataFeed;
use std::path::Path;
use trading_core::traits::strategy::StrategyState;
use trading_core::traits::{ExchangeConnector, Strategy};
use trading_core::types::instrument::Exchange;
use trading_core::{Action, Event};

pub struct BacktestHarness {
    strategy: Box<dyn Strategy>,
    connector: SimConnector,
    data_feed: SimMarketDataFeed,
}

impl BacktestHarness {
    pub fn new(
        strategy: Box<dyn Strategy>,
        exchange: Exchange,
        data_dir: impl AsRef<Path>,
    ) -> Self {
        Self {
            strategy,
            connector: SimConnector::new(exchange),
            data_feed: SimMarketDataFeed::new(data_dir),
        }
    }

    /// Run the backtest and return a report.
    pub async fn run(&mut self) -> anyhow::Result<BacktestReport> {
        let events = self.data_feed.load_events().await?;
        tracing::info!(events = events.len(), "Starting backtest");

        // Initialize strategy
        let state = StrategyState {
            positions: vec![],
            open_orders: vec![],
            decimal_precisions: std::collections::HashMap::new(), // backtest uses strategy defaults
        };
        let init_actions = self.strategy.initialize(&state).await;
        self.process_actions(init_actions);

        let mut total_actions = 0usize;

        // Replay events
        for event in &events {
            // Feed market data to matching engine (may trigger fills)
            if let Event::BookUpdate {
                instrument, book, ..
            } = event
            {
                if let Some(mid) = book.mid_price() {
                    self.connector.on_market_data(instrument, mid);
                }
            }

            // Feed event to strategy
            let actions = self.strategy.on_event(event).await;
            total_actions += actions.len();
            self.process_actions(actions);

            // Check for fills from matching engine
            // In a more complete implementation, we'd drain the
            // order_update_rx and feed those back to the strategy too.
        }

        // Shutdown
        let shutdown_actions = self.strategy.shutdown().await;
        self.process_actions(shutdown_actions);

        tracing::info!(
            events = events.len(),
            actions = total_actions,
            "Backtest complete"
        );

        // Build report
        let positions = self.connector.matching_engine().lock().unwrap().positions();
        Ok(BacktestReport::new(positions))
    }

    fn process_actions(&self, actions: Vec<Action>) {
        let rt = tokio::runtime::Handle::current();
        for action in actions {
            match &action {
                Action::PlaceOrder(req) => {
                    let connector = &self.connector;
                    // In a full implementation, use proper async handling
                    let _ = rt.block_on(connector.place_order(req));
                }
                Action::CancelOrder {
                    instrument,
                    order_id,
                } => {
                    let _ = rt.block_on(self.connector.cancel_order(instrument, order_id));
                }
                Action::CancelAll { instrument } => {
                    let _ = rt.block_on(self.connector.cancel_all(instrument));
                }
                Action::LogDecision { decision, .. } => {
                    tracing::debug!(decision, "Strategy decision");
                }
                _ => {}
            }
        }
    }
}
