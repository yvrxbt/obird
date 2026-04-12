//! Prediction market quoter — implements the Strategy trait.

use trading_core::traits::Strategy;
use trading_core::traits::strategy::StrategyState;
use trading_core::{Action, Event, InstrumentId};
use crate::params::QuoterParams;

pub struct PredictionQuoter {
    id: String,
    instruments: Vec<InstrumentId>,
    params: QuoterParams,
    // Internal state
    current_fair_value: Option<(trading_core::Price, f64)>, // (price, confidence)
    // TODO: position tracking, open order tracking
}

impl PredictionQuoter {
    pub fn new(id: String, instruments: Vec<InstrumentId>, params: QuoterParams) -> Self {
        Self {
            id,
            instruments,
            params,
            current_fair_value: None,
        }
    }
}

#[async_trait::async_trait]
impl Strategy for PredictionQuoter {
    fn id(&self) -> &str { &self.id }

    fn subscriptions(&self) -> Vec<InstrumentId> { self.instruments.clone() }

    async fn on_event(&mut self, event: &Event) -> Vec<Action> {
        match event {
            Event::FairValueUpdate { fair_value, confidence, .. } => {
                self.current_fair_value = Some((*fair_value, *confidence));
                // TODO: Recalculate quotes based on new fair value
                vec![]
            }
            Event::BookUpdate { .. } => {
                // TODO: Check if our quotes need updating
                vec![]
            }
            Event::Fill { fill, .. } => {
                // TODO: Update position, adjust quotes
                tracing::info!(order_id = %fill.order_id, "Fill received");
                vec![]
            }
            _ => vec![],
        }
    }

    async fn initialize(&mut self, _state: &StrategyState) -> Vec<Action> {
        tracing::info!(strategy = %self.id, "Prediction quoter initialized");
        vec![]
    }

    async fn shutdown(&mut self) -> Vec<Action> {
        // Cancel all outstanding orders
        self.instruments.iter().map(|inst| {
            Action::CancelAll { instrument: inst.clone() }
        }).collect()
    }
}
