---
title: "[AGENT] Phase 1d T3: NATS Action transport + OrderUpdate publish"
labels: agent-task,phase-1d,difficulty-medium,area-engine
---

## Task
Add a `NatsActionTransport` that pipes `(StrategyId, Vec<Action>)` batches into the engine's router via a JetStream work-queue consumer, and publishes `OrderUpdate`s back to `order.<venue>.<market>.<oid>`. Add a CLI flag `--transport nats|inprocess` to select at runtime. Default stays in-process.

## Context
Maps to `PROJECT_PLAN.md` §1.5. First actually-crossing-the-wire seam. Strategies can now live in a separate process.

## Files to Touch
- `crates/engine/src/transport.rs` (new) — trait + in-process impl
- `crates/nats-transport/src/action.rs` (new) — NATS impl
- `crates/engine/src/runner.rs` — accept a Box<dyn ActionTransport>
- `crates/cli/src/live.rs` + `main.rs` — CLI flag

## Cursor prompt

```
Split the engine's Action intake into a trait so NATS or in-process backends
can be swapped.

1. crates/engine/src/transport.rs:

    use tokio::sync::mpsc;
    use trading_core::{Action, Event};

    pub type ActionBatch = (String, Vec<Action>);  // (strategy_id, actions)

    #[async_trait::async_trait]
    pub trait ActionTransport: Send + Sync + 'static {
        /// Block until next batch. Returns None when shut down.
        async fn recv(&mut self) -> Option<ActionBatch>;
    }

    #[async_trait::async_trait]
    pub trait OrderUpdatePublisher: Send + Sync + 'static {
        async fn publish(&self, strategy_id: &str, event: Event);
    }

    // In-process impls wrap mpsc channels (matches current runner.rs wiring).
    pub struct InProcActionTransport(pub mpsc::UnboundedReceiver<ActionBatch>);
    #[async_trait::async_trait]
    impl ActionTransport for InProcActionTransport {
        async fn recv(&mut self) -> Option<ActionBatch> { self.0.recv().await }
    }
    // Similar wrapper for OrderUpdatePublisher.

2. crates/nats-transport/src/action.rs:

    pub struct NatsActionTransport {
        consumer: async_nats::jetstream::consumer::PullConsumer,
    }

    impl NatsActionTransport {
        pub async fn new(js: async_nats::jetstream::Context, stream: &str, consumer: &str)
            -> anyhow::Result<Self>
        {
            let stream = js.get_stream(stream).await?;
            let consumer = stream.get_consumer(consumer).await?;
            Ok(Self { consumer })
        }
    }

    #[async_trait::async_trait]
    impl trading_engine::transport::ActionTransport for NatsActionTransport {
        async fn recv(&mut self) -> Option<ActionBatch> {
            use futures::StreamExt;
            let mut msgs = self.consumer.messages().await.ok()?;
            let msg = msgs.next().await?.ok()?;
            let env: Envelope<ActionMsg> = crate::schemas::decode(&msg.payload).ok()?;
            let _ = msg.ack().await;
            Some((env.payload.strategy_id, vec![env.payload.action]))
        }
    }

    pub struct NatsOrderUpdatePublisher { client: async_nats::Client }
    #[async_trait::async_trait]
    impl trading_engine::transport::OrderUpdatePublisher for NatsOrderUpdatePublisher {
        async fn publish(&self, strategy_id: &str, event: Event) {
            // Subject: order.<venue>.<market>.<oid> — derive from event
            // Use async-nats client.publish with bincode-encoded Envelope
        }
    }

3. Update crates/engine/src/runner.rs:
   - EngineRunner::new takes Box<dyn ActionTransport> + Box<dyn OrderUpdatePublisher>
     instead of raw channels.
   - In-process wiring keeps working: pass InProc* wrappers around the existing
     mpsc channels.

4. In live.rs, add a `--transport` flag (default: in-process). When "nats":
   - Connect to NATS via nats-transport.
   - Ensure JetStream stream `actions_v1` exists (subject filter `action.>`,
     retention work-queue, max-age 1h).
   - Create a durable pull consumer per engine instance.
   - Build NatsActionTransport + NatsOrderUpdatePublisher and pass to EngineRunner.

5. Test: engine with --transport=nats and a tiny test publisher that shoves one
   Action onto `action.testnet.testmarket`. Engine should receive it through the
   new transport.

Do not yet run a full strategy separately — that's T4 (idempotency) + validation.
```

## Acceptance Criteria
- [ ] `cargo test --workspace` passes
- [ ] `--transport=inprocess` (default) behavior unchanged
- [ ] `--transport=nats` subscribes to `action.>` and processes an injected test Action
- [ ] OrderUpdate is published to `order.<venue>.<market>.<oid>` on every fill/place/cancel

## Complexity
- [x] Medium (30-60 min), likely closer to 2hr

## Blocked by
T1, T2
