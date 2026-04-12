//! MarketDataBus — manages broadcast channels for market data fan-out.
//!
//! Each instrument gets its own broadcast channel. Strategies subscribe
//! to the instruments they care about. If a strategy falls behind,
//! it automatically skips stale messages (broadcast lagging semantics).

use trading_core::{Event, InstrumentId};
use tokio::sync::broadcast;
use std::collections::HashMap;

/// Buffer size per instrument broadcast channel.
/// At 100 updates/sec, this buffers 640ms — more than enough.
const BROADCAST_BUFFER: usize = 64;

pub struct MarketDataBus {
    channels: HashMap<InstrumentId, broadcast::Sender<Event>>,
}

impl MarketDataBus {
    pub fn new() -> Self {
        Self { channels: HashMap::new() }
    }

    /// Get or create a broadcast sender for an instrument.
    pub fn sender(&mut self, instrument: &InstrumentId) -> broadcast::Sender<Event> {
        self.channels
            .entry(instrument.clone())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(BROADCAST_BUFFER);
                tx
            })
            .clone()
    }

    /// Subscribe to an instrument's market data.
    pub fn subscribe(&mut self, instrument: &InstrumentId) -> broadcast::Receiver<Event> {
        self.sender(instrument).subscribe()
    }

    /// Publish an event to the relevant instrument channel.
    pub fn publish(&self, instrument: &InstrumentId, event: Event) {
        if let Some(tx) = self.channels.get(instrument) {
            // Ignore send errors — means no active receivers
            let _ = tx.send(event);
        }
    }
}
