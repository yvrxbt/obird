---
title: "[AGENT] Phase 1a T2: Implement instruments() on all connectors"
labels: agent-task,phase-1a,difficulty-easy,area-connectors
---

## Task
Implement the `instruments()` method on every `ExchangeConnector` impl: Hyperliquid, Polymarket, predict.fun, Binance, and SimConnector.

## Context
Maps to `PROJECT_PLAN.md` §1.8.1. Follows T1 (which adds the trait method). Each connector declares which `InstrumentId`s it is configured to serve.

## Files to Touch
- `crates/connectors/hyperliquid/src/client.rs` — `HyperliquidClient`
- `crates/connectors/polymarket/src/execution.rs` — `PolymarketExecutionClient`
- `crates/connectors/predict_fun/src/client.rs` — `PredictFunClient`
- `crates/connectors/binance/src/client.rs` — `BinanceClient`
- `crates/backtest/src/sim_connector.rs` — `SimConnector`

## Cursor prompt

```
For each file below, find the `impl ExchangeConnector for <ClientType>` block and
add an `instruments()` method returning the InstrumentId(s) this client serves.

1. crates/connectors/hyperliquid/src/client.rs
   HyperliquidClient has `self.instrument()`. Add:
       fn instruments(&self) -> Vec<InstrumentId> {
           vec![self.instrument().clone()]
       }

2. crates/connectors/polymarket/src/execution.rs
   PolymarketExecutionClient tracks its instruments. Find the struct field
   holding them (grep for `InstrumentId` in the file). If a field exists,
   return a clone. If not, add a `instruments: Vec<InstrumentId>` field to
   the struct, populate it in the constructor(s), then return it.

3. crates/connectors/predict_fun/src/client.rs
   PredictFunClient is built per market and serves both YES and NO outcomes.
   Find the fields holding those InstrumentIds (likely `yes_instrument` /
   `no_instrument` or similar). Return:
       fn instruments(&self) -> Vec<InstrumentId> {
           vec![self.yes_instrument.clone(), self.no_instrument.clone()]
       }
   Adjust names if the fields are called differently.

4. crates/connectors/binance/src/client.rs
   Mirror the HL pattern:
       fn instruments(&self) -> Vec<InstrumentId> {
           vec![self.instrument().clone()]
       }

5. crates/backtest/src/sim_connector.rs
   SimConnector is configured with one or more instruments. Return all of them
   (grep the file for where InstrumentId is stored on the struct).

Run `cargo check -p <crate>` after each edit. Do not touch anything else.
Do not change construction logic.
```

## Acceptance Criteria
- [ ] `cargo check --workspace` passes (trait now has all impls)
- [ ] No clippy warnings introduced

## Complexity
- [x] Small (<30 min)

## Blocked by
T1
