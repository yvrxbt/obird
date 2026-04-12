//! Data source ingestion for fair value computation.
//!
//! Aggregates price data from multiple exchanges to compute features
//! for the fair value model. Can also ingest external signals
//! (news sentiment, on-chain data, etc.)

use trading_core::types::decimal::Price;
use std::collections::HashMap;

/// Aggregates price data across exchanges.
pub struct PriceAggregator {
    /// Latest mid prices by (exchange, symbol) key
    prices: HashMap<String, Price>,
}

impl PriceAggregator {
    pub fn new() -> Self {
        Self {
            prices: HashMap::new(),
        }
    }

    pub fn update(&mut self, key: String, price: Price) {
        self.prices.insert(key, price);
    }

    pub fn get(&self, key: &str) -> Option<Price> {
        self.prices.get(key).copied()
    }

    /// Compute features for the fair value model.
    /// Returns a vector of feature values.
    pub fn compute_features(&self) -> Vec<f64> {
        // TODO: Implement feature computation
        // Examples:
        // - BTC mid price across exchanges
        // - BTC 5-min return
        // - Spread between exchanges
        // - Volume-weighted price
        // - On-chain flow signals
        vec![]
    }
}
