//! Polymarket exchange connector.
//!
//! ## What this crate provides
//!
//! 1. **Gamma API client** (`client`): Given a Polymarket condition ID, resolves
//!    the YES and NO CLOB token IDs needed for WebSocket subscription.
//!    Called by `predict-markets` CLI and at live startup when `polymarket_yes_token_id`
//!    is configured.
//!
//! 2. **CLOB WebSocket feed** (`market_data`): A single multiplexed connection to
//!    `wss://ws-subscriptions-clob.polymarket.com/ws/market` that subscribes to one
//!    or more YES token IDs. Publishes `BookUpdate` events to `MarketDataBus`.
//!    Designed to handle all quoting markets on a single WS connection — critical
//!    when running 20+ predict.fun markets simultaneously.
//!
//! ## Architecture alignment
//!
//! Follows the `XClient` + `XMarketDataFeed` split mandated by CLAUDE.md:
//! - `client.rs` — REST lookups (Gamma API, no order execution yet)
//! - `market_data.rs` — WS background feed task
//! - `normalize.rs` — WS message parsing + book state management

pub mod client;
pub mod execution;
pub mod market_data;
pub mod normalize;
