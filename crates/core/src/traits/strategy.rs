//! The Strategy trait — the central abstraction of the system.
//!
//! A strategy receives Events and emits Actions. It never touches I/O.
//! This contract enables live trading, backtesting, and paper trading
//! with IDENTICAL strategy code — no mode flags, no cfg gates.

use std::collections::HashMap;

use crate::types::order::OpenOrder;
use crate::types::position::Position;
use crate::{Action, Event, InstrumentId};

/// Initial state provided to a strategy at startup.
#[derive(Debug, Clone)]
pub struct StrategyState {
    pub positions: Vec<Position>,
    pub open_orders: Vec<OpenOrder>,
    /// Price tick precision per instrument, populated by the engine from each
    /// connector's `ExchangeConnector::decimal_precision(instrument)` call for
    /// every instrument this strategy subscribes to.
    ///
    /// Value is `n` where `10^-n` is the minimum price increment (e.g. `2` → 0.01).
    /// Missing entries mean the connector returned `None` (e.g. Hyperliquid uses
    /// sig-fig rounding, computed per-price via `PriceTick`).
    ///
    /// ## Why per-instrument (not per-exchange or a single scalar)
    ///
    /// - The same exchange can host instruments with different tick sizes
    ///   (Binance ETH vs BTC; Polymarket CLOB markets vary by market; Kalshi similar).
    /// - A strategy quoting on venue A while reading FV from venue B must never
    ///   pick up B's precision — the lookup key `self.quoting_instrument` makes
    ///   that structurally impossible.
    pub decimal_precisions: HashMap<InstrumentId, u32>,
}

#[async_trait::async_trait]
pub trait Strategy: Send + Sync + 'static {
    /// Unique identifier for this strategy instance.
    fn id(&self) -> &str;

    /// Which instruments does this strategy need market data for?
    fn subscriptions(&self) -> Vec<InstrumentId>;

    /// Process an event and return zero or more actions.
    /// This is the hot path — keep it fast and non-blocking.
    async fn on_event(&mut self, event: &Event) -> Vec<Action>;

    /// Called once at startup with initial positions and open orders.
    async fn initialize(&mut self, state: &StrategyState) -> Vec<Action>;

    /// Called on graceful shutdown. Return actions to cancel/flatten.
    async fn shutdown(&mut self) -> Vec<Action>;
}
