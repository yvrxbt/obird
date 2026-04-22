//! Engine runner — wires connectors, strategies, and routing together.
//!
//! Accepts an `Arc<MarketDataBus>` so the same bus can be shared with connector
//! feed tasks running outside the runner. This is the seam point for going
//! distributed: pass a different `MarketDataSink` impl and the runner is unchanged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::{collections::HashMap, sync::Arc};

use futures::StreamExt;
use rust_decimal_macros::dec;
use tokio::{signal, sync::mpsc};
use trading_core::{
    traits::strategy::StrategyState,
    traits::{ExchangeConnector, Strategy},
    types::instrument::Exchange,
    Action, Event,
};

use crate::{
    market_data_bus::MarketDataBus,
    order_manager::OrderManager,
    order_router::{OrderRouter, StrategyId},
    risk::UnifiedRiskManager,
};

pub struct StrategyInstance {
    pub strategy: Box<dyn Strategy>,
    pub id: StrategyId,
}

pub struct EngineRunner {
    connectors: HashMap<Exchange, Box<dyn ExchangeConnector>>,
    strategies: Vec<StrategyInstance>,
    md_bus: Arc<MarketDataBus>,
    risk: UnifiedRiskManager,
    /// Set before the action channel is drained so connectors skip new HTTP place calls.
    /// Shared with ShutdownHandle via Arc — live.rs sets this via shutdown_handle.set_shutting_down()
    /// before awaiting the router drain.
    shutdown_flag: Option<Arc<AtomicBool>>,
}

impl EngineRunner {
    pub fn new(
        connectors: HashMap<Exchange, Box<dyn ExchangeConnector>>,
        strategies: Vec<StrategyInstance>,
        md_bus: Arc<MarketDataBus>,
    ) -> Self {
        Self {
            connectors,
            strategies,
            md_bus,
            risk: UnifiedRiskManager::new(dec!(1_000_000), 0.10),
            shutdown_flag: None,
        }
    }

    /// Attach a shutdown flag that will be set to `true` when Ctrl+C fires,
    /// before the action queue is drained. Connectors check this to skip
    /// in-flight places that haven't hit the network yet.
    pub fn with_shutdown_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.shutdown_flag = Some(flag);
        self
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        tracing::info!("Engine starting with {} strategies", self.strategies.len());

        // Batched action channel: strategy responses travel as Vec<Action> so the router
        // can sequence cancels → concurrent place across exchanges atomically.
        let (action_tx, action_rx) = mpsc::unbounded_channel::<(StrategyId, Vec<Action>)>();
        let mut strategy_txs: HashMap<StrategyId, mpsc::UnboundedSender<Event>> = HashMap::new();
        let mut strategy_tasks = Vec::new();

        // Fetch real positions from every connector BEFORE spawning strategy tasks.
        // Connectors are still owned here — they haven't moved into OrderManager yet.
        // This ensures a restarted strategy knows its existing inventory.
        let mut initial_positions = Vec::new();
        for (exchange, connector) in &self.connectors {
            match connector.positions().await {
                Ok(pos) => {
                    tracing::info!(
                        ?exchange,
                        count = pos.len(),
                        "Loaded initial positions from exchange"
                    );
                    initial_positions.extend(pos);
                }
                Err(e) => {
                    tracing::warn!(?exchange, error = %e, "Failed to load initial positions — starting flat");
                }
            }
        }
        // Build per-instrument decimal precision map by asking each strategy's
        // subscribed instruments' connector for its precision.
        //
        // Previously this was a single scalar populated by iterating
        // `self.connectors.values()` in HashMap order and taking `.next()` — on
        // a multi-connector setup (predict.fun + Polymarket as FV feed) the
        // scan could hand back Polymarket's hard-coded 2 and clobber
        // predict.fun's actual per-market precision (commonly 3 for tight
        // markets). Keying by `InstrumentId` removes the ambiguity: every
        // strategy looks up the precision for *its own quoting instrument*.
        let mut decimal_precisions: HashMap<trading_core::InstrumentId, u32> = HashMap::new();
        for instance in &self.strategies {
            for inst in instance.strategy.subscriptions() {
                let Some(connector) = self.connectors.get(&inst.exchange) else {
                    continue;
                };
                let Some(prec) = connector.decimal_precision(&inst) else {
                    continue;
                };
                match decimal_precisions.get(&inst) {
                    Some(existing) if *existing != prec => {
                        tracing::warn!(
                            instrument = %inst,
                            existing = existing,
                            new = prec,
                            "decimal_precision conflict — keeping first value",
                        );
                    }
                    Some(_) => {}
                    None => {
                        tracing::info!(
                            instrument = %inst,
                            decimal_precision = prec,
                            "Registered decimal_precision for instrument",
                        );
                        decimal_precisions.insert(inst, prec);
                    }
                }
            }
        }

        let initial_state = StrategyState {
            positions: initial_positions,
            open_orders: vec![], // open orders fetched per-instrument by strategy if needed
            decimal_precisions,
        };

        for instance in self.strategies.drain(..) {
            let sid = instance.id.clone();
            let mut strategy = instance.strategy;

            // Subscribe to ALL instruments the strategy wants.
            // Merge them into a single stream so every instrument is polled fairly.
            // This fixes the first-receiver-only bug and enables multi-instrument strategies
            // (pair trader, spread quoter across two legs, etc.).
            let subscriptions = strategy.subscriptions();
            let receivers: Vec<_> = subscriptions
                .iter()
                .map(|inst| self.md_bus.subscribe(inst))
                .collect();

            // Convert each broadcast::Receiver into a stream, then merge all into one.
            // Box + pin each stream so select_all can hold them uniformly.
            let merged_md = futures::stream::select_all(receivers.into_iter().map(|rx| {
                Box::pin(futures::stream::unfold(rx, |mut rx| async move {
                    loop {
                        match rx.recv().await {
                            Ok(event) => return Some((event, rx)),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(lagged = n, "MD lagged — skipping stale");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                return None;
                            }
                        }
                    }
                }))
            }));

            let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();
            strategy_txs.insert(sid.clone(), event_tx);

            let action_tx_s = action_tx.clone();
            let sid_log = sid.clone();

            let init_actions = strategy.initialize(&initial_state).await;
            if !init_actions.is_empty() {
                let _ = action_tx_s.send((sid.clone(), init_actions));
            }

            let handle = tokio::spawn(async move {
                tracing::info!(strategy = %sid_log, "Strategy task started");
                tokio::pin!(merged_md);

                loop {
                    tokio::select! {
                        // Market data from any subscribed instrument
                        Some(event) = merged_md.next() => {
                            let actions = strategy.on_event(&event).await;
                            if !actions.is_empty() {
                                let _ = action_tx_s.send((sid_log.clone(), actions));
                            }
                        }
                        // Order/fill events routed back from OrderRouter
                        Some(event) = event_rx.recv() => {
                            let actions = strategy.on_event(&event).await;
                            if !actions.is_empty() {
                                let _ = action_tx_s.send((sid_log.clone(), actions));
                            }
                        }
                    }
                }
            });

            strategy_tasks.push((sid, handle));
        }

        let mut managers = HashMap::new();
        for (exchange, connector) in self.connectors.into_iter() {
            let uses_nonce = matches!(exchange, Exchange::Hyperliquid | Exchange::Lighter);
            managers.insert(exchange, OrderManager::new(connector, uses_nonce));
        }

        let mut router = OrderRouter::new(managers, self.risk, action_rx, strategy_txs);
        let router_handle = tokio::spawn(async move { router.run().await });

        tracing::info!("Engine running — Ctrl-C to shut down");
        signal::ctrl_c().await?;
        tracing::info!("Shutdown signal received");

        // Phase 1 — block new places.
        // Set the shutdown flag before touching anything else. Any place_batch call
        // that hasn't started its HTTP request yet will return immediately.
        // Calls already past the flag check will complete and record their OIDs.
        if let Some(flag) = &self.shutdown_flag {
            flag.store(true, Ordering::Release);
            tracing::info!("Shutdown flag set — new place requests will be rejected");
        }

        // Phase 2 — stop strategy tasks.
        // No new actions will be sent to the action channel after this.
        for (id, handle) in strategy_tasks {
            tracing::info!(strategy = %id, "Shutting down");
            handle.abort();
        }

        // Phase 3 — drain the router.
        // Drop action_tx so the router's recv() returns None once the queue is empty.
        // The router will finish processing any already-queued action batches (place calls
        // return immediately due to the shutdown flag), then exit its run loop naturally.
        // Crucially, if there's an in-flight join_all waiting on a place_batch HTTP response,
        // the router stays alive until that future resolves — populating active_oids with
        // the just-confirmed OIDs before we run cancel_all.
        drop(action_tx);
        tracing::info!("Draining router — waiting for in-flight orders to settle");
        if let Err(e) = router_handle.await {
            if !e.is_cancelled() {
                tracing::warn!("Router task error during drain: {e}");
            }
        }
        tracing::info!("Router drained — active_oids populated, ready for cancel");

        Ok(())
    }
}
