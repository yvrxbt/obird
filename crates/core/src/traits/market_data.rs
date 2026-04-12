//! Market data transport abstraction.
//!
//! `MarketDataSink` decouples connectors from the in-process broadcast bus.
//! Swap the implementation to go distributed — NATS, Redis Streams, etc. —
//! without touching connector or strategy code.

use crate::{Event, InstrumentId};

/// Where market data events flow OUT of a connector.
///
/// In-process: [`MarketDataBus`](trading_engine::MarketDataBus) (tokio::broadcast).
/// Distributed: implement this over NATS JetStream, Redis Streams, Kafka, etc.
///
/// Connectors call `publish()` and are transport-agnostic.
pub trait MarketDataSink: Send + Sync + 'static {
    /// Publish a market data event for an instrument.
    /// If no subscribers exist, the event is silently dropped — never blocks.
    fn publish(&self, instrument: &InstrumentId, event: Event);
}
