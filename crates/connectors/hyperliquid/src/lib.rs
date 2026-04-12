//! Hyperliquid exchange connector.
//!
//! Two responsibilities:
//! - `HyperliquidClient` — order execution via REST (implements `ExchangeConnector`)
//! - `HlMarketDataFeed` — WebSocket market data publisher (run as a background task)
//!
//! These are intentionally separate: the feed runs independently and publishes to
//! whichever `MarketDataSink` it receives — enabling the distributed path without
//! changing this crate.

pub mod client;
pub mod market_data;
pub mod normalize;

pub use client::{HyperliquidClient, ResolvedMarket, ShutdownHandle, resolve_symbol};
pub use market_data::{AssetInfo, HlMarketDataFeed};
