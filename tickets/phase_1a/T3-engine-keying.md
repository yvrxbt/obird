---
title: "[AGENT] Phase 1a T3: Key engine by InstrumentId (HashMap refactor)"
labels: agent-task,phase-1a,difficulty-medium,area-engine
---

## Task
Refactor `EngineRunner` and `OrderRouter` to key connectors by `InstrumentId` instead of `Exchange`. Change `Box<dyn ExchangeConnector>` to `Arc<dyn ExchangeConnector>` so a connector serving many instruments can be shared across keys.

## Context
Maps to `PROJECT_PLAN.md` §1.8.1 + §1.8.2. Core of Phase 1a. Unblocks a single engine process serving N predict.fun markets.

Current code:
- `crates/engine/src/runner.rs` holds `connectors: HashMap<Exchange, Box<dyn ExchangeConnector>>`
- `crates/engine/src/order_router.rs` holds `managers: HashMap<Exchange, OrderManager>`
- `crates/engine/src/order_router.rs:62` groups `place_orders: HashMap<Exchange, Vec<OrderRequest>>`
- `crates/core/src/action.rs` has `Action::exchange()` helper but no `Action::instrument()`

## Files to Touch
- `crates/core/src/action.rs` — add `Action::instrument()` helper
- `crates/engine/src/runner.rs`
- `crates/engine/src/order_router.rs`
- `crates/engine/src/order_manager.rs` — likely needs `&self` instead of `&mut self` on `submit` (see below)

## Cursor prompt

```
Refactor the engine to key connectors by InstrumentId instead of Exchange.

1. In crates/core/src/action.rs, add a new helper mirroring exchange():

    impl Action {
        pub fn instrument(&self) -> Option<&InstrumentId> {
            match self {
                Action::PlaceOrder(req) => Some(&req.instrument),
                Action::CancelOrder { instrument, .. } => Some(instrument),
                Action::CancelAll { instrument } => Some(instrument),
                Action::ModifyOrder { instrument, .. } => Some(instrument),
                Action::LogDecision { .. } => None,
            }
        }
    }

2. In crates/engine/src/order_manager.rs:
   - Change `connector: Box<dyn ExchangeConnector>` field to
     `connector: Arc<dyn ExchangeConnector>`.
   - Change `pub async fn submit(&mut self, ...)` to `&self`. (NonceManager already
     uses AtomicU64, so it works behind an immutable reference.)
   - `new()` takes `Arc<dyn ExchangeConnector>` instead of Box.
   - The `place_batch` method is already `&self` — no change there.

3. In crates/engine/src/runner.rs:
   - Change `connectors: HashMap<Exchange, Box<dyn ExchangeConnector>>` to
     `connectors: HashMap<InstrumentId, Arc<dyn ExchangeConnector>>`.
   - Update `new()` signature identically.
   - `positions()` loop around line 79: this currently iterates (exchange, connector)
     pairs. Deduplicate by Arc identity so multi-instrument connectors don't get
     called N times. Use:

       let unique: Vec<Arc<dyn ExchangeConnector>> = {
           let mut seen = std::collections::HashSet::<*const dyn ExchangeConnector>::new();
           self.connectors.values()
               .filter(|c| seen.insert(Arc::as_ptr(c)))
               .cloned().collect()
       };

     Then iterate `unique` to call `.positions()`.
   - Decimal precision loop: replace `self.connectors.get(&inst.exchange)` with
     `self.connectors.get(&inst)`. Drop the `inst.exchange` field access.
   - The managers-build loop: change from one OrderManager per exchange to one
     Arc<OrderManager> per unique connector, then insert into
     `HashMap<InstrumentId, Arc<OrderManager>>` with one entry per instrument
     served by that connector.

       let mut unique_mgrs: HashMap<*const dyn ExchangeConnector, Arc<OrderManager>> = HashMap::new();
       let mut managers: HashMap<InstrumentId, Arc<OrderManager>> = HashMap::new();
       for (inst, conn) in self.connectors.drain() {
           let ptr = Arc::as_ptr(&conn);
           let mgr = unique_mgrs.entry(ptr).or_insert_with(|| {
               let uses_nonce = matches!(conn.exchange(), Exchange::Hyperliquid | Exchange::Lighter);
               Arc::new(OrderManager::new(conn.clone(), uses_nonce))
           }).clone();
           managers.insert(inst, mgr);
       }

4. In crates/engine/src/order_router.rs:
   - Change `managers: HashMap<Exchange, OrderManager>` to
     `managers: HashMap<InstrumentId, Arc<OrderManager>>`.
   - Update `new()` signature.
   - In `handle_batch`, change the `place_orders: HashMap<Exchange, Vec<OrderRequest>>`
     grouping to `HashMap<InstrumentId, Vec<OrderRequest>>`, keying by
     `req.instrument.clone()`.
   - The cancel-path: replace `action.exchange()` with `action.instrument()`.
     Then `self.managers.get(instrument)` instead of `.get(&exchange)`.
   - `submit` is `&self` now (after step 2), so drop `mut` on the match arm.
   - For the place-path: the futures::join_all block groups by InstrumentId, looks
     up `self.managers.get(&inst).cloned()` (Arc clone is cheap), and passes the
     Arc into the async move.

5. Run `cargo check --workspace` after each file. Leave crates/cli/src/live.rs
   broken — that's T4.
```

## Acceptance Criteria
- [ ] `cargo check -p trading-engine` passes
- [ ] `cargo check -p trading-core` passes
- [ ] `cargo check --workspace` fails only in `crates/cli` (expected)
- [ ] No new clippy warnings in `trading-engine` or `trading-core`

## Complexity
- [x] Medium (30-60 min)

## Blocked by
T1, T2

## Blocks
T4, T6
