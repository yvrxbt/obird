---
title: "[AGENT] Phase 1b T5: Wire FairValueService + FairValueBus in live.rs"
labels: agent-task,phase-1b,difficulty-medium,area-cli
---

## Task
In `crates/cli/src/live.rs`, spawn a `FairValueService` task alongside the strategy tasks. Give strategies a handle to `FairValueBus` so they can subscribe.

## Context
Maps to `PROJECT_PLAN.md` §1.4. The wiring point for the extraction. Strategy still runs in the same process — only the computation moved.

## Files to Touch
- `crates/cli/src/live.rs` (both `run_predict` and the new `run_multi` from Phase 1a T5 if it exists)
- `crates/engine/src/runner.rs` — add `Arc<FairValueBus>` to `EngineRunner`
- `crates/core/src/traits/strategy.rs` — `StrategyState` gets a `fair_value_subscribers: HashMap<InstrumentId, broadcast::Receiver<FairValueMessage>>` field OR strategies get an `fv_bus: Arc<FairValueBus>` handle at construction

## Cursor prompt

```
Wire the FairValueService into the live engine.

Decision: how does PredictionQuoter get its FairValueMessage stream?
 Option A: include an fv_bus: Option<Arc<FairValueBus>> in StrategyInstance so
 the engine subscribes on the strategy's behalf.
 Option B: pass Arc<FairValueBus> into PredictionQuoter::new at construction,
 let the strategy subscribe itself during initialize().
 Option B is cleaner — the strategy knows which instruments it wants to watch.

Proceed with Option B.

1. In crates/engine/src/runner.rs:
   - No change required — engine doesn't need to route FV itself. Just document
     that the FV bus is owned by the wiring layer (live.rs).

2. In crates/cli/src/live.rs, inside run_predict (and run_multi if it exists):

   a) Build the FairValueBus:
        use trading_engine::fair_value_bus::FairValueBus;
        let fv_bus = FairValueBus::new();

   b) Collect the FV tuples from loaded configs. For each predict.fun market
      with a polymarket_yes_token_id, build one FvTuple { polymarket_yes,
      predict_yes, predict_no }.

   c) Subscribe the FV service to the MarketDataBus for every instrument in those
      tuples (poly YES, predict YES, predict NO):
        let mut fv_mds = Vec::new();
        for t in &tuples {
            for inst in [&t.polymarket_yes, &t.predict_yes, &t.predict_no] {
                fv_mds.push((inst.clone(), md_bus.subscribe(inst)));
            }
        }

   d) Spawn the service:
        let fv_sink: Arc<dyn FairValueSink> = fv_bus.clone();
        let service = FairValueService::new(
            tuples,
            Box::new(CrossVenueConservativeModel),
            fv_sink,
        );
        tokio::spawn(async move {
            if let Err(e) = service.run(fv_mds).await {
                tracing::error!(error = %e, "FairValueService exited");
            }
        });

   e) Pass fv_bus.clone() into PredictionQuoter::new (signature change happens in T6).

3. Add imports at top of live.rs:
   use fair_value_service::{service::{FairValueService, FvTuple}, model::CrossVenueConservativeModel};
   use trading_core::traits::FairValueSink;
   use trading_engine::fair_value_bus::FairValueBus;

4. Run `cargo build --release --bin trading-cli`. It will fail on the
   PredictionQuoter::new signature until T6 — that's expected.
```

## Acceptance Criteria
- [ ] `cargo build --release` passes after T6 lands
- [ ] `cargo check -p trading-cli` shows the FV service is wired and spawned
- [ ] `FairValueService` receives subscriptions to all tuple instruments
- [ ] `FairValueBus` is passed to strategy construction (pending T6)

## Complexity
- [x] Medium (30-60 min)

## Blocked by
T2, T4
