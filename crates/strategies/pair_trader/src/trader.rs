//! Pair trading strategy — implements the Strategy trait.

use trading_core::traits::Strategy;
use trading_core::traits::strategy::StrategyState;
use trading_core::{Action, Event, InstrumentId};
use crate::params::PairTraderParams;
use crate::spread_model::SpreadModel;

pub struct PairTrader {
    id: String,
    leg_a: InstrumentId,
    leg_b: InstrumentId,
    params: PairTraderParams,
    spread_model: SpreadModel,
}

impl PairTrader {
    pub fn new(
        id: String, leg_a: InstrumentId, leg_b: InstrumentId,
        params: PairTraderParams,
    ) -> Self {
        let spread_model = SpreadModel::new(params.lookback_periods);
        Self { id, leg_a, leg_b, params, spread_model }
    }
}

#[async_trait::async_trait]
impl Strategy for PairTrader {
    fn id(&self) -> &str { &self.id }

    fn subscriptions(&self) -> Vec<InstrumentId> {
        vec![self.leg_a.clone(), self.leg_b.clone()]
    }

    async fn on_event(&mut self, event: &Event) -> Vec<Action> {
        match event {
            Event::BookUpdate { instrument, book, .. } => {
                // TODO: Update spread model, check for entry/exit signals
                let _ = (instrument, book);
                vec![]
            }
            Event::Fill { fill, .. } => {
                tracing::info!(order_id = %fill.order_id, "Pair trader fill");
                vec![]
            }
            _ => vec![],
        }
    }

    async fn initialize(&mut self, _state: &StrategyState) -> Vec<Action> {
        tracing::info!(strategy = %self.id, "Pair trader initialized");
        vec![]
    }

    async fn shutdown(&mut self) -> Vec<Action> {
        vec![
            Action::CancelAll { instrument: self.leg_a.clone() },
            Action::CancelAll { instrument: self.leg_b.clone() },
        ]
    }
}
