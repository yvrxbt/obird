---
title: "[AGENT] Phase 1c T1: UDS MarketData transport (publisher + subscriber)"
labels: agent-task,phase-1c,difficulty-medium,area-core
---

## Task
Implement a reusable UDS-based `MarketDataSink`: a server that accepts client connections and streams `Event`s over length-prefixed bincode, and a client that reads frames and republishes to an in-process `MarketDataBus` (or any `MarketDataSink`).

## Context
Maps to `PROJECT_PLAN.md` §1.3. The existing `MarketDataSink` trait (`crates/core/src/traits/market_data.rs`) is synchronous `publish(&self, instrument, event)`. That means the UDS writer needs a background flush task so the connector's hot path stays non-blocking.

Pattern mirrors the existing `FairValuePublisher` in `crates/fair_value_service/src/publisher.rs` (UDS, length-prefixed frames, multi-client broadcast).

## Files to Touch
- `crates/md-transport/` (new crate) — `Cargo.toml`, `src/lib.rs`, `src/publisher.rs`, `src/subscriber.rs`
- `Cargo.toml` (workspace) — add the new member

## Cursor prompt

```
Create a new crate `md-transport` implementing UDS-based MarketDataSink.

1. Create crates/md-transport/Cargo.toml:
   - package name = "md-transport"
   - edition = "2021"
   - deps: trading_core (workspace), tokio (workspace, features ["net","rt","macros","io-util","sync"]),
     bincode (workspace or "1.3"), serde (workspace), tracing (workspace), anyhow (workspace)

2. Add "crates/md-transport" to the workspace Cargo.toml members.

3. Define the wire frame in src/lib.rs:

    use serde::{Deserialize, Serialize};
    use trading_core::{Event, InstrumentId};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MdFrame {
        pub instrument: InstrumentId,
        pub event: Event,
    }

    pub mod publisher;
    pub mod subscriber;

4. In src/publisher.rs, implement UdsMarketDataPublisher:
   - new(socket_path) -> Self
   - run(self, mut rx: tokio::sync::broadcast::Receiver<MdFrame>) -> anyhow::Result<()>
   - Bind UnixListener, accept clients in background, maintain Vec<UnixStream> behind Mutex.
   - On MdFrame received on rx: bincode::serialize → [u32 LE length][payload bytes],
     write to all clients, drop disconnected ones.
   - Use bincode (not serde_json) for minimum overhead on the hot path.

   Also implement MarketDataSink for UdsMarketDataPublisherHandle where the handle
   forwards to the internal broadcast sender:

        pub struct UdsMarketDataPublisherHandle {
            tx: tokio::sync::broadcast::Sender<MdFrame>,
        }

        impl trading_core::MarketDataSink for UdsMarketDataPublisherHandle {
            fn publish(&self, instrument: &InstrumentId, event: Event) {
                let _ = self.tx.send(MdFrame { instrument: instrument.clone(), event });
            }
        }

   Provide a constructor that returns (UdsMarketDataPublisher, UdsMarketDataPublisherHandle)
   sharing the same broadcast sender.

5. In src/subscriber.rs, implement UdsMarketDataSubscriber:
   - new(socket_path, sink: Arc<dyn MarketDataSink>) -> Self
   - run(self) -> anyhow::Result<()>
   - Connect to UDS, loop: read 4-byte length, read payload, bincode::deserialize
     → MdFrame, call sink.publish(&frame.instrument, frame.event).
   - Reconnect with exponential backoff (1s → 2s → 4s → max 10s) on disconnect.

6. Run `cargo check --workspace` and `cargo test --workspace`. No tests required
   for this ticket — T5 validation covers end-to-end.
```

## Acceptance Criteria
- [ ] `cargo check -p md-transport` passes
- [ ] `UdsMarketDataPublisherHandle` implements `MarketDataSink`
- [ ] Publisher + subscriber handle multi-client + reconnect correctly

## Complexity
- [x] Medium (30-60 min)

## Blocks
T2, T3, T4
