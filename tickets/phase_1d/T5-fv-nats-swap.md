---
title: "[AGENT] Phase 1d T5: Swap FV transport from FairValueBus → NATS"
labels: agent-task,phase-1d,difficulty-easy,area-fair-value
---

## Task
Make `fair-value-service` publish to NATS subject `fv.<symbol>` and have `PredictionQuoter` subscribe via NATS when `--transport=nats` is set. Keep `FairValueBus` path for in-process mode.

## Context
Maps to `PROJECT_PLAN.md` §1.4.4. This is the Phase 1b.2 swap promised in 1b's scoping. The FairValueSink trait from 1b T2 already abstracts this — new NATS impl slots in.

## Files to Touch
- `crates/nats-transport/src/fair_value.rs` (new)
- `crates/cli/src/live.rs` — select sink based on `--transport` flag

## Cursor prompt

```
Add a NATS FairValueSink implementation.

1. crates/nats-transport/src/fair_value.rs:

    use async_nats::Client;
    use std::sync::Arc;
    use trading_core::{types::fair_value::FairValueMessage, traits::FairValueSink};
    use crate::schemas::Envelope;
    use crate::subjects;

    pub struct NatsFvSink {
        client: Client,
    }

    impl NatsFvSink {
        pub fn new(client: Client) -> Arc<Self> { Arc::new(Self { client }) }
    }

    #[async_trait::async_trait]
    impl FairValueSink for NatsFvSink {
        async fn publish(&self, msg: FairValueMessage) {
            let subject = subjects::fv(&msg.instrument.to_string());
            let env = Envelope::new(msg);
            let bytes = crate::schemas::encode(&env);
            let _ = self.client.publish(subject, bytes.into()).await;
        }
    }

    pub async fn subscribe_fv(
        client: &Client,
        instrument: &trading_core::InstrumentId,
    ) -> anyhow::Result<async_nats::Subscriber> {
        let subject = subjects::fv(&instrument.to_string());
        Ok(client.subscribe(subject).await?)
    }

2. In live.rs, when --transport=nats:
   - Connect NATS client.
   - Pass NatsFvSink as the sink argument to FairValueService::new.
   - Strategy subscribes via nats_transport::fair_value::subscribe_fv and reads
     Envelope<FairValueMessage> → feeds into its existing FV handling.

   The PredictionQuoter constructor (changed in 1b T6 to take Arc<FairValueBus>)
   needs a third option: an async_nats::Subscriber. Simplest fix: introduce a
   trait FvSubscriber with two impls (in-process Receiver + NATS Subscriber).
   Strategy takes Box<dyn FvSubscriber>.

3. Test:
   - Start NATS (T1)
   - Run fair-value-service with NATS sink
   - Run `nats sub 'fv.>'` CLI — should see FV messages
   - Run strategy consuming NATS FV — same behavior as FairValueBus path
```

## Acceptance Criteria
- [ ] `--transport=nats` publishes FV to `fv.<symbol>`
- [ ] Strategy consumes via NATS subscriber; behavior identical to in-process
- [ ] `--transport=inprocess` (default) unchanged

## Complexity
- [x] Small (<30 min)

## Blocked by
T1, T2, Phase 1b T6

## Blocks
T7
