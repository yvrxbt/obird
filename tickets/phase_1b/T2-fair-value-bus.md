---
title: "[AGENT] Phase 1b T2: Add FairValueBus (in-process broadcast)"
labels: agent-task,phase-1b,difficulty-easy,area-engine
---

## Task
Add a `FairValueBus` type in `crates/engine` analogous to `MarketDataBus` — per-instrument broadcast channels for `FairValueMessage`s. This is the in-process seam that later gets swapped for UDS (phase 1b.2) or NATS (phase 1d).

## Context
Maps to `PROJECT_PLAN.md` §1.4.4. Mirrors the `MarketDataSink`/`MarketDataBus` pattern (ADR-002). Strategies don't care whether FV comes from a local task or a remote service — they just subscribe.

## Files to Touch
- `crates/engine/src/fair_value_bus.rs` (new)
- `crates/engine/src/lib.rs` (export the new module)
- `crates/core/src/traits/mod.rs` — add a `FairValueSink` trait (mirrors `MarketDataSink`)

## Cursor prompt

```
Implement the in-process FairValueBus mirroring the existing MarketDataBus pattern.

1. In crates/core/src/traits/, create fair_value_sink.rs (or append to mod.rs):

    use crate::types::instrument::InstrumentId;
    use fair_value_service::publisher::FairValueMessage;
    // If fair_value_service is not already a dep of trading_core, pull FairValueMessage
    // up into trading_core instead — put it in crates/core/src/types/fair_value.rs and
    // re-export from fair_value_service. Strategies and engine should depend on
    // trading_core, not fair_value_service.

    #[async_trait::async_trait]
    pub trait FairValueSink: Send + Sync + 'static {
        async fn publish(&self, msg: FairValueMessage);
    }

   Decision point: I recommend moving FairValueMessage + SourceSnapshot into
   trading_core (crates/core/src/types/fair_value.rs). The fair_value_service crate
   re-exports for backward compat. This avoids trading_core → fair_value_service
   dependency cycles.

2. Create crates/engine/src/fair_value_bus.rs:

    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{broadcast, Mutex};
    use trading_core::{types::instrument::InstrumentId, traits::FairValueSink, types::fair_value::FairValueMessage};

    const CHANNEL_CAPACITY: usize = 64;

    pub struct FairValueBus {
        senders: Mutex<HashMap<InstrumentId, broadcast::Sender<FairValueMessage>>>,
    }

    impl FairValueBus {
        pub fn new() -> Arc<Self> {
            Arc::new(Self { senders: Mutex::new(HashMap::new()) })
        }

        pub async fn sender(&self, inst: &InstrumentId) -> broadcast::Sender<FairValueMessage> {
            let mut map = self.senders.lock().await;
            map.entry(inst.clone())
                .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
                .clone()
        }

        pub async fn subscribe(&self, inst: &InstrumentId) -> broadcast::Receiver<FairValueMessage> {
            self.sender(inst).await.subscribe()
        }
    }

    #[async_trait::async_trait]
    impl FairValueSink for Arc<FairValueBus> {
        async fn publish(&self, msg: FairValueMessage) {
            let sender = self.sender(&msg.instrument).await;
            let _ = sender.send(msg); // drop on no subscribers, same as MarketDataBus
        }
    }

3. Export in crates/engine/src/lib.rs:

    pub mod fair_value_bus;

4. Run `cargo check --workspace`. Mirror the MarketDataBus design as closely as
   possible — consumers should be able to use the two interchangeably conceptually.
```

## Acceptance Criteria
- [ ] `cargo check --workspace` passes
- [ ] `FairValueBus::new()`, `sender()`, `subscribe()` work with the same semantics as `MarketDataBus`
- [ ] `Arc<FairValueBus>` implements `FairValueSink`

## Complexity
- [x] Small (<30 min)

## Blocked by
T1
