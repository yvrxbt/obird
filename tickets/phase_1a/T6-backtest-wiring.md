---
title: "[AGENT] Phase 1a T6: Update backtest harness for InstrumentId keying"
labels: agent-task,phase-1a,difficulty-easy,area-backtest
---

## Task
Bring the `backtest` crate in line with the new `HashMap<InstrumentId, Arc<dyn ExchangeConnector>>` engine shape. No logic changes — just the collection type.

## Context
Maps to `PROJECT_PLAN.md` §1.8.2. Backtest is the regression net; skipping this is not optional.

## Files to Touch
- `crates/backtest/src/` — grep the whole crate for `HashMap<Exchange` and any place that mirrors the engine wiring
- Any test files under `crates/backtest/tests/` or `crates/*/tests/` that construct a backtest harness

## Cursor prompt

```
Update crates/backtest (and any dependent tests) for the new engine keying.

1. Grep: `rg 'HashMap<Exchange' crates/backtest/` and `rg 'Box<dyn ExchangeConnector>' crates/backtest/`.

2. For every occurrence: change the shape to
   HashMap<InstrumentId, Arc<dyn ExchangeConnector>> and use the
   same insert pattern as crates/cli/src/live.rs:

       let conn_arc: Arc<dyn ExchangeConnector> = Arc::new(sim_connector);
       for inst in conn_arc.instruments() {
           connectors.insert(inst, conn_arc.clone());
       }

3. Run `cargo test --workspace`. For each compile failure in test files:
   - If the test constructs a map by Exchange key, rewrite it to the new shape.
   - If the test asserts on router state by Exchange, update the assertion to
     key by InstrumentId.
   - Do not rewrite test intent — only the type shape.

4. Integration smoke test (existing in crates/backtest, if any): run it and
   verify P&L output matches expected numbers pre-refactor. If numbers drift,
   STOP and report the diff in the PR body.

Do not add new tests in this ticket — validation lives in T7.
Do not change MatchingEngine or SimMarketDataFeed internals.
```

## Acceptance Criteria
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` has no new warnings
- [ ] Any pre-existing backtest smoke test produces identical P&L (to 4 decimal places) as before the refactor

## Complexity
- [x] Small (<30 min) — probably, could slip to medium if tests are deep

## Blocked by
T1, T3
