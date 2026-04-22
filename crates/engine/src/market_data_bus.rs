//! MarketDataBus — per-instrument broadcast channels for market data fan-out.
//!
//! Implements [`MarketDataSink`] so connectors are transport-agnostic.
//! The default in-process implementation uses tokio::broadcast. To go distributed,
//! implement `MarketDataSink` over NATS / Redis Streams and pass that instead.
//!
//! Internally uses `RwLock<HashMap>` so it is safe behind an `Arc` and can be
//! shared between the connector feed task and the engine runner without cloning.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use tokio::sync::broadcast;
use trading_core::{Event, InstrumentId, MarketDataSink};

/// Buffer size per instrument channel.
/// At 200 book updates/sec, 64 entries = 320ms of buffering before a lagging
/// subscriber is dropped. Increase if strategies are slow consumers.
const BROADCAST_BUFFER: usize = 64;

/// In-process market data bus backed by per-instrument tokio::broadcast channels.
///
/// Wrap in `Arc<MarketDataBus>` and pass to both the connector feed and the engine runner.
pub struct MarketDataBus {
    channels: RwLock<HashMap<InstrumentId, broadcast::Sender<Event>>>,
}

impl MarketDataBus {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            channels: RwLock::new(HashMap::new()),
        })
    }

    /// Get or create a broadcast sender for an instrument.
    /// Called by the engine runner to pre-register instruments before the feed starts.
    pub fn sender(&self, instrument: &InstrumentId) -> broadcast::Sender<Event> {
        // Fast path: already exists
        if let Some(tx) = self.channels.read().unwrap().get(instrument) {
            return tx.clone();
        }
        // Slow path: create
        let mut map = self.channels.write().unwrap();
        map.entry(instrument.clone())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(BROADCAST_BUFFER);
                tx
            })
            .clone()
    }

    /// Subscribe to an instrument's market data stream.
    pub fn subscribe(&self, instrument: &InstrumentId) -> broadcast::Receiver<Event> {
        self.sender(instrument).subscribe()
    }
}

impl Default for MarketDataBus {
    fn default() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
        }
    }
}

/// The in-process sink implementation — events go into broadcast channels.
/// To go distributed, replace this with a NATS/Redis publisher that also implements `MarketDataSink`.
impl MarketDataSink for MarketDataBus {
    fn publish(&self, instrument: &InstrumentId, event: Event) {
        if let Some(tx) = self.channels.read().unwrap().get(instrument) {
            // Lagging receivers are automatically dropped by tokio::broadcast
            let _ = tx.send(event);
        }
        // If no channel exists yet, event is silently dropped (no subscribers).
        // This is intentional — the feed starts before the runner in some topologies.
    }
}
