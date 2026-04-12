//! Hyperliquid exchange connector.
//!
//! Implements `ExchangeConnector` from trading-core.

pub mod client;
pub mod market_data;
pub mod normalize;

pub use client::HyperliquidClient;
