---
title: "[AGENT] Phase 1d T4: Action idempotency layer in engine"
labels: agent-task,phase-1d,difficulty-medium,area-engine
---

## Task
Add an in-engine idempotency cache keyed by `action_id` so duplicate Actions (e.g., JetStream redelivery, retries) don't spam the exchange. Match the engine-controller contract from PRD §5.3.

## Context
Maps to `PROJECT_PLAN.md` §1.5.5. Without this, JetStream work-queue semantics could deliver the same Action twice under restart/network-blip conditions, and the engine would double-place.

## Files to Touch
- `crates/engine/src/idempotency.rs` (new)
- `crates/engine/src/order_router.rs`
- `crates/engine/src/runner.rs`

## Cursor prompt

```
Implement the PRD §5.3 order state machine + idempotency cache.

1. crates/engine/src/idempotency.rs:

    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use trading_core::types::order::OrderId;
    use uuid::Uuid;

    const MAX_CACHE_ENTRIES: usize = 10_000;

    /// Maps action_id → (OrderId, cached_at_ns) so repeated deliveries of the same
    /// Action return the prior result without re-hitting the exchange.
    pub struct IdempotencyCache {
        inner: Mutex<Inner>,
    }

    struct Inner {
        seen: HashMap<Uuid, (Option<OrderId>, u64)>,
        order: VecDeque<Uuid>,
    }

    impl IdempotencyCache {
        pub fn new() -> Self {
            Self { inner: Mutex::new(Inner { seen: HashMap::new(), order: VecDeque::new() }) }
        }

        /// Returns Some(cached_oid) if this action_id has been seen.
        pub fn check(&self, id: Uuid) -> Option<Option<OrderId>> {
            self.inner.lock().unwrap().seen.get(&id).map(|(oid, _)| oid.clone())
        }

        pub fn record(&self, id: Uuid, oid: Option<OrderId>) {
            let mut inner = self.inner.lock().unwrap();
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .unwrap().as_nanos() as u64;
            inner.seen.insert(id, (oid, ts));
            inner.order.push_back(id);
            while inner.order.len() > MAX_CACHE_ENTRIES {
                if let Some(old) = inner.order.pop_front() {
                    inner.seen.remove(&old);
                }
            }
        }
    }

2. Wire into OrderRouter:
   - Add Arc<IdempotencyCache> to the OrderRouter.
   - The incoming action channel already carries (strategy_id, Vec<Action>).
     When using NATS transport (T3), the wrapper ActionMsg includes action_id.
     Plumb action_id through into the batch as a per-action tag.
     Simplest change: augment the action channel message type to
     `(String, Vec<(Option<Uuid>, Action)>)`. In-process sender fills None (no
     idempotency check); NATS sender fills Some(env.payload.action_id).
   - In handle_batch:
     for each action with Some(action_id):
       if cache.check(action_id).is_some(): skip, emit "idempotent-hit" log.
   - After a place_batch resolves with OrderIds, call cache.record(action_id, Some(oid))
     for each action that had one.

3. Add tests:
   - Same action_id twice → second is a no-op.
   - Different action_id, same payload → both execute (idempotency is on id, not content).
   - Cache eviction at MAX_CACHE_ENTRIES.

4. Document the invariant in crates/engine/src/order_router.rs module docs.
```

## Acceptance Criteria
- [ ] `cargo test -p trading-engine` passes with new idempotency tests
- [ ] Duplicate action_id logs "idempotent-hit" and does not call the exchange
- [ ] In-process transport (action_id = None) behavior is unchanged

## Complexity
- [x] Medium (30-60 min)

## Blocked by
T3
