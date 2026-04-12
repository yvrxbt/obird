//! The Strategy trait — the central abstraction of the system.
//!
//! A strategy receives Events and emits Actions. It never touches I/O.
//! This contract enables live trading, backtesting, and paper trading
//! with IDENTICAL strategy code — no mode flags, no cfg gates.

use crate::{Action, Event, InstrumentId};
use crate::types::position::Position;
use crate::types::order::OpenOrder;

/// Initial state provided to a strategy at startup.
#[derive(Debug, Clone)]
pub struct StrategyState {
    pub positions: Vec<Position>,
    pub open_orders: Vec<OpenOrder>,
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
