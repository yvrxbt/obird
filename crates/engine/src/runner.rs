//! Main engine runner — spawns strategy tasks and wires everything together.
//!
//! This is the top-level orchestrator. It:
//! 1. Loads config and initializes ExchangeConnectors
//! 2. Creates the MarketDataBus (broadcast channels)
//! 3. Creates the OrderRouter with per-exchange OrderManagers
//! 4. Spawns each strategy as a tokio task
//! 5. Runs until shutdown signal

use crate::market_data_bus::MarketDataBus;
use crate::order_router::{OrderRouter, StrategyId};
use crate::order_manager::OrderManager;
use crate::risk::UnifiedRiskManager;
use trading_core::traits::connector::ExchangeConnector;
use trading_core::traits::strategy::{Strategy, StrategyState};
use trading_core::types::instrument::Exchange;
use trading_core::{Action, Event};
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};
use tokio::signal;
use rust_decimal_macros::dec;

/// Configuration for a strategy instance within the engine.
pub struct StrategyInstance {
    pub strategy: Box<dyn Strategy>,
    pub id: StrategyId,
}

/// The engine runner. Owns all connectors, strategies, and the event loop.
pub struct EngineRunner {
    connectors: HashMap<Exchange, Box<dyn ExchangeConnector>>,
    strategies: Vec<StrategyInstance>,
    md_bus: MarketDataBus,
    risk: UnifiedRiskManager,
}

impl EngineRunner {
    pub fn new(
        connectors: HashMap<Exchange, Box<dyn ExchangeConnector>>,
        strategies: Vec<StrategyInstance>,
    ) -> Self {
        Self {
            connectors,
            strategies,
            md_bus: MarketDataBus::new(),
            risk: UnifiedRiskManager::new(dec!(1_000_000), 0.10),
        }
    }

    /// Run the engine until shutdown.
    pub async fn run(mut self) -> anyhow::Result<()> {
        tracing::info!("Engine starting with {} strategies", self.strategies.len());

        // 1. Create the shared action channel (all strategies → OrderRouter)
        let (action_tx, action_rx) = mpsc::unbounded_channel::<(StrategyId, Action)>();

        // 2. Create per-strategy event channels (OrderRouter → each strategy)
        let mut strategy_txs: HashMap<StrategyId, mpsc::UnboundedSender<Event>> = HashMap::new();
        let mut strategy_tasks = Vec::new();

        // 3. For each strategy, set up channels and spawn task
        for instance in self.strategies.drain(..) {
            let strategy_id = instance.id.clone();
            let mut strategy = instance.strategy;

            // Market data: subscribe to each instrument the strategy needs
            let subscriptions = strategy.subscriptions();
            let mut md_receivers: Vec<broadcast::Receiver<Event>> = subscriptions
                .iter()
                .map(|inst| self.md_bus.subscribe(inst))
                .collect();

            // Action channel: strategy → OrderRouter
            let action_tx = action_tx.clone();
            let sid = strategy_id.clone();

            // Event channel: OrderRouter → strategy (fills, order updates)
            let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();
            strategy_txs.insert(strategy_id.clone(), event_tx);

            // Initialize strategy with current state
            let state = StrategyState {
                positions: vec![],
                open_orders: vec![],
            };
            let init_actions = strategy.initialize(&state).await;
            for action in init_actions {
                let _ = action_tx.send((sid.clone(), action));
            }

            // Spawn strategy task
            let handle = tokio::spawn(async move {
                tracing::info!(strategy = %sid, "Strategy task started");

                loop {
                    tokio::select! {
                        // Market data from broadcast channels
                        // We select on the first receiver; in production, use a
                        // merged stream or select! macro over all receivers
                        event = async {
                            if let Some(rx) = md_receivers.first_mut() {
                                match rx.recv().await {
                                    Ok(event) => Some(event),
                                    Err(broadcast::error::RecvError::Lagged(n)) => {
                                        tracing::warn!(strategy = %sid, lagged = n, "MD lagged, skipping stale");
                                        None
                                    }
                                    Err(broadcast::error::RecvError::Closed) => {
                                        tracing::warn!(strategy = %sid, "MD channel closed");
                                        None
                                    }
                                }
                            } else {
                                // No subscriptions — just wait
                                tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
                                None
                            }
                        } => {
                            if let Some(event) = event {
                                let actions = strategy.on_event(&event).await;
                                for action in actions {
                                    let _ = action_tx.send((sid.clone(), action));
                                }
                            }
                        }

                        // Events from OrderRouter (fills, order updates)
                        Some(event) = event_rx.recv() => {
                            let actions = strategy.on_event(&event).await;
                            for action in actions {
                                let _ = action_tx.send((sid.clone(), action));
                            }
                        }
                    }
                }
            });

            strategy_tasks.push((strategy_id, handle));
        }

        // 4. Build OrderManagers per exchange
        let mut managers = HashMap::new();
        for (exchange, connector) in self.connectors.into_iter() {
            let uses_nonce = matches!(
                exchange,
                Exchange::Hyperliquid | Exchange::Lighter
            );
            managers.insert(exchange, OrderManager::new(connector, uses_nonce));
        }

        // 5. Build and spawn OrderRouter
        let mut router = OrderRouter::new(managers, self.risk, action_rx, strategy_txs);
        let router_handle = tokio::spawn(async move {
            router.run().await;
        });

        // 6. Wait for shutdown signal
        tracing::info!("Engine running. Press Ctrl+C to shut down.");
        signal::ctrl_c().await?;
        tracing::info!("Shutdown signal received");

        // 7. Cancel all tasks (in production, call strategy.shutdown() first)
        router_handle.abort();
        for (id, handle) in strategy_tasks {
            tracing::info!(strategy = %id, "Shutting down strategy");
            handle.abort();
        }

        tracing::info!("Engine shut down");
        Ok(())
    }
}
