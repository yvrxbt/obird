# LLM Guide — Trading System

This file is for future LLM/code-agent contributors.

## 1) System shape and architectural decisions

- Workspace is split by concern: `core` (contracts), `engine` (runtime), `connectors` (exchange adapters), `strategies` (decision logic), `backtest` (simulation/replay), `cli` (entrypoint).
- Core abstraction boundaries:
  - `Strategy` trait: consumes `Event`, emits `Action`
  - `ExchangeConnector` trait: exchange-specific execution/state interface
- Engine uses in-process channels (`tokio::broadcast` + `mpsc`) on hot path.
- Fair value model is intended as separate process (`fair_value_service`) and should stay separate from strategies.

## 2) Invariants that must not be broken

1. Strategies must not import connector crates or perform network I/O.
2. Strategy output must remain `Action` only, never direct execution.
3. Strategy input must remain `Event` only.
4. `OrderRouter` is the only path from strategy action to exchange submission.
5. Money math uses `Decimal` wrappers (`Price`, `Quantity`), never float math for accounting.
6. Keep backtest/live behavior contractually equivalent at trait boundary.

## 3) Implementation reality (important)

The codebase is partially implemented.

- Implemented:
  - `core` contracts/types
  - `engine` skeleton (`runner`, `order_router`, `order_manager`, `market_data_bus`)
  - Hyperliquid connector (order APIs + state queries)
  - backtest primitives (`BacktestHarness`, `SimConnector`, replay feed, report)
- Not fully implemented:
  - CLI wiring for `backtest` and `record`
  - non-Hyperliquid connectors
  - strategy trading logic
  - full risk checks
  - fair value service runtime

Do not document or assume these TODOs are already production-ready.

## 4) Common pitfalls

- **Engine subscription bug/limitation:** `runner.rs` currently processes only `md_receivers.first_mut()` in `select!`; additional subscriptions are ignored.
- **Backtest async misuse risk:** `BacktestHarness::process_actions` calls `Handle::block_on` from async context. Refactor if you expand backtest concurrency.
- **Connector side preservation:** `SimConnector::modify_order` currently hardcodes `OrderSide::Buy` on cancel-replace.
- **Risk manager is permissive:** `UnifiedRiskManager::check` currently returns `Ok(())` with TODO logic.
- **CLI flags mismatch:** scripts reference `--config/--data/--output`, but CLI parser currently only dispatches by first positional command.

## 5) How to extend safely

### Add a new exchange connector

1. Create/complete `crates/connectors/<exchange>/src/client.rs` implementing `ExchangeConnector`.
2. Normalize exchange market data into `trading_core::Event`.
3. Map exchange order/fill semantics into `OrderRequest`/`OrderUpdate` consistently.
4. Register connector construction in live runtime wiring (currently minimal, in CLI/engine integration work).
5. Add unit tests for normalization and status mapping.

### Add a new strategy

1. Add crate under `crates/strategies/<strategy_name>`.
2. Implement `Strategy` trait only.
3. Keep all side effects outside strategy (no connector/network imports).
4. Add deterministic unit tests around `on_event` transitions.
5. Add replay tests through backtest harness where possible.

### Extend fair value integration

- Keep model code in `fair_value_service`.
- Publish `Event::FairValueUpdate` into engine ingestion path.
- Keep strategy crates model-agnostic.

## 6) Testing approach

Minimum expectation after changes:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

Recommended strategy-specific testing:
- Unit-test signal logic from synthetic `Event` streams.
- Integration-test strategy + `SimConnector` using recorded `.jsonl` playback.
- Regression-test risk policy when `UnifiedRiskManager` becomes strict.

## 7) Documentation sync rule

When behavior changes, update all three if impacted:
- `README.md` (human operational view)
- `ARCHITECTURE.md` (system-level design/implementation reality)
- this file (`.claude/LLM_GUIDE.md`) for agent-specific pitfalls/invariants
