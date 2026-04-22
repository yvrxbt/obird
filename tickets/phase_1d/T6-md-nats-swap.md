---
title: "[AGENT] Phase 1d T6: Swap MD transport from UDS → NATS"
labels: agent-task,phase-1d,difficulty-easy,area-connectors
---

## Task
Add a NATS-based `MarketDataSink` for md-ingest binaries and a NATS subscriber for the engine. Keep UDS path as the single-box fallback.

## Context
Maps to `PROJECT_PLAN.md` §1.3.2 (publish to NATS subjects). Phase 1c made md-ingest a separate binary; this ticket swaps its transport.

## Files to Touch
- `crates/nats-transport/src/market_data.rs` (new)
- `crates/md-ingest/src/bin/*.rs` — add `--transport nats|uds` flag
- `crates/cli/src/live.rs` — accept NATS as external-feeds source

## Cursor prompt

```
Add NATS MarketDataSink and subscriber.

1. crates/nats-transport/src/market_data.rs:

    use async_nats::Client;
    use trading_core::{Event, InstrumentId, MarketDataSink};
    use crate::schemas::Envelope;
    use crate::subjects;

    pub struct NatsMdSink {
        client: Client,
        venue: String,  // for subject construction
    }

    impl NatsMdSink {
        pub fn new(client: Client, venue: impl Into<String>) -> Self {
            Self { client, venue: venue.into() }
        }
    }

    impl MarketDataSink for NatsMdSink {
        fn publish(&self, instrument: &InstrumentId, event: Event) {
            let subject = subjects::md(&self.venue, &instrument.symbol);
            let frame = md_transport::MdFrame {
                instrument: instrument.clone(),
                event,
            };
            let env = Envelope::new(frame);
            let bytes = crate::schemas::encode(&env);
            let client = self.client.clone();
            tokio::spawn(async move {
                let _ = client.publish(subject, bytes.into()).await;
            });
        }
    }

    pub struct NatsMdSubscriber {
        client: Client,
        sink: std::sync::Arc<dyn MarketDataSink>,
    }

    impl NatsMdSubscriber {
        pub async fn run(self, subject_pattern: &str) -> anyhow::Result<()> {
            use futures::StreamExt;
            let mut sub = self.client.subscribe(subject_pattern.to_string()).await?;
            while let Some(msg) = sub.next().await {
                let env: Envelope<md_transport::MdFrame> =
                    crate::schemas::decode(&msg.payload)?;
                self.sink.publish(&env.payload.instrument, env.payload.event);
            }
            Ok(())
        }
    }

2. In each md-ingest binary, add a `--transport nats|uds` flag (default uds
   during rollout). For nats mode: replace UdsMarketDataPublisherHandle with
   NatsMdSink wrapped in FanoutSink alongside NdjsonTier0.

3. In live.rs, extend --external-feeds to accept NATS subject patterns:
   --external-feeds nats:md.polymarket.*,nats:md.predict_fun.*
   For each nats: entry, spawn NatsMdSubscriber with pattern and the engine's
   MarketDataBus as sink.

4. Test end-to-end:
   - md-ingest-poly --transport nats
   - nats sub 'md.>' (verify subjects populated)
   - trading-cli live --external-feeds nats:md.polymarket.*
```

## Acceptance Criteria
- [ ] `md-ingest-poly --transport nats` publishes to `md.polymarket.*`
- [ ] Engine consumes from NATS MD subjects identically to UDS path
- [ ] UDS path still works for single-box dev

## Complexity
- [x] Small (<30 min) per side, ~1hr total

## Blocked by
T1, T2, Phase 1c T2

## Blocks
T7
