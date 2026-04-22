//! Core types, traits, and enums for the trading system.
//!
//! This crate defines the API contract that all other crates depend on.
//! It contains no I/O, no exchange-specific logic, and no strategy logic.

pub mod action;
pub mod config;
pub mod error;
pub mod event;
pub mod traits;
pub mod types;

pub use action::Action;
pub use event::Event;
pub use traits::market_data::MarketDataSink;
pub use types::decimal::{Price, Quantity};
pub use types::instrument::{Exchange, InstrumentId, InstrumentKind};
pub use types::market_data::{OrderbookSnapshot, Trade};
pub use types::order::{OrderId, OrderRequest, OrderSide, OrderUpdate, TimeInForce};
pub use types::position::{Fill, Position};
