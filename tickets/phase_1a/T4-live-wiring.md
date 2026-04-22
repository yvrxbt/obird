---
title: "[AGENT] Phase 1a T4: Update live.rs wiring for InstrumentId keying"
labels: agent-task,phase-1a,difficulty-medium,area-cli
---

## Task
Update `crates/cli/src/live.rs` to build the new `HashMap<InstrumentId, Arc<dyn ExchangeConnector>>` shape. No functional change to strategies — only the collection-assembly sites.

## Context
Maps to `PROJECT_PLAN.md` §1.8.2. There are two collection-assembly sites in `live.rs`: around line 100 (HL path) and around line 298 (predict.fun path).

## Files to Touch
- `crates/cli/src/live.rs`

## Cursor prompt

```
Update crates/cli/src/live.rs for the new engine key shape.

Two places build the connectors map. Change both to use
HashMap<InstrumentId, Arc<dyn ExchangeConnector>>, inserting one entry per
instrument the connector serves (from `connector.instruments()`).

Also update imports at the top of the file:
  - Remove `types::instrument::Exchange` from the use-list if no longer used.
  - Add `std::sync::Arc` if not present.
  - Keep `InstrumentId` and `InstrumentKind` imports.

1. Around line 100 (run_hl):
   Current:
       let mut connectors: HashMap<Exchange, Box<dyn trading_core::traits::ExchangeConnector>> =
           HashMap::new();
       connectors.insert(Exchange::Hyperliquid, Box::new(connector));

   Change to:
       let mut connectors: HashMap<InstrumentId, Arc<dyn trading_core::traits::ExchangeConnector>> =
           HashMap::new();
       let hl_arc: Arc<dyn trading_core::traits::ExchangeConnector> = Arc::new(connector);
       for inst in hl_arc.instruments() {
           connectors.insert(inst, hl_arc.clone());
       }

2. Around line 298 (run_predict):
   Current builds with Box and Exchange keys. Do the same Arc/instruments pattern:

       let mut connectors: HashMap<InstrumentId, Arc<dyn trading_core::traits::ExchangeConnector>> =
           HashMap::new();

       let predict_arc: Arc<dyn trading_core::traits::ExchangeConnector> = Arc::new(client);
       for inst in predict_arc.instruments() {
           connectors.insert(inst, predict_arc.clone());
       }

       if let Some(poly_client) = poly_connector {
           let poly_arc: Arc<dyn trading_core::traits::ExchangeConnector> = Arc::new(poly_client);
           for inst in poly_arc.instruments() {
               connectors.insert(inst, poly_arc.clone());
           }
       }

   (The old `connectors.insert(Exchange::PredictFun, Box::new(client))` and
   `connectors.insert(Exchange::Polymarket, Box::new(poly_client))` calls go away.)

3. Run `cargo build --release --bin trading-cli`. Fix any remaining compile errors.
   Do not change strategy, shutdown-handle, or connector-construction logic.
```

## Acceptance Criteria
- [ ] `cargo build --release --bin trading-cli` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` passes (or at least no new warnings)
- [ ] Existing `trading-cli live --config configs/quoter.toml` command still works (dry compile test — do not run live)

## Complexity
- [x] Medium (30-60 min)

## Blocked by
T3
