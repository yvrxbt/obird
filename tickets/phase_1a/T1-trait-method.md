---
title: "[AGENT] Phase 1a T1: Add instruments() to ExchangeConnector trait"
labels: agent-task,phase-1a,difficulty-trivial,area-core
---

## Task
Add a new required method `instruments()` to the `ExchangeConnector` trait so connectors can declare the instruments they serve at wire time.

## Context
Maps to `PROJECT_PLAN.md` §1.8.1. Prerequisite for the engine's `HashMap<Exchange, Connector>` → `HashMap<InstrumentId, Connector>` refactor. Everything else in Phase 1a depends on this.

## Files to Touch
- `crates/core/src/traits/connector.rs`

## Cursor prompt

```
In crates/core/src/traits/connector.rs, add a new required method to the
ExchangeConnector trait (place it directly under `fn exchange(&self) -> Exchange;`):

    /// Instruments this connector can place and track orders for.
    ///
    /// Returned at wire time so the engine can build HashMap<InstrumentId, Connector>.
    /// A connector may serve many instruments (HL: all perps a client is built for)
    /// or exactly one (predict.fun: one CTFExchange per market).
    fn instruments(&self) -> Vec<InstrumentId>;

Do NOT provide a default implementation — every connector must declare.
Do not modify any other file.
```

## Acceptance Criteria
- [ ] `cargo check -p trading-core` passes
- [ ] `cargo check --workspace` fails with "not all trait items implemented" on every connector crate (expected — subsequent tickets fix those)

## Complexity
- [x] Small (<30 min)

## Blocks
T2, T3, T4, T6
