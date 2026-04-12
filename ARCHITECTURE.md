# Trading System Architecture

> Last updated: 2026-04-12
> Status: Early implementation (foundation complete, many runtime features still TODO)

## 1. Purpose

Rust multi-crate HFT framework targeting two domains:
- Crypto/perp execution and pair trading
- Prediction-market quoting driven by external fair-value models

The architecture is designed to keep strategy logic portable across live trading and backtesting.

## 2. Core Contracts

Defined in `crates/core`:

- `Strategy` trait (`on_event`, `initialize`, `shutdown`)
- `ExchangeConnector` trait (place/cancel/modify, positions, open orders, update stream)
- `Event` enum (engine -> strategy)
- `Action` enum (strategy -> engine)

These contracts are the primary stability boundary.

## 3. Runtime Topology

### Data/control path

1. Exchange-specific connectors produce market/order updates
2. `MarketDataBus` fan-outs events by instrument using `tokio::broadcast`
3. Strategies subscribe to instruments and process events
4. Strategies emit `Action`s over mpsc
5. `OrderRouter` applies unified risk checks and routes by exchange
6. Per-exchange `OrderManager` executes via connector

### Components

- `engine::runner`: orchestrates task lifecycle and channel wiring
- `engine::market_data_bus`: per-instrument broadcast channels (buffer=64)
- `engine::order_router`: central action routing + risk gate
- `engine::order_manager`: per-exchange submission serialization layer
- `engine::risk`: portfolio-level risk manager (currently stubbed)

## 4. Backtesting Architecture

In `crates/backtest`:
- `BacktestHarness`: replay loop and strategy driving
- `SimConnector`: `ExchangeConnector` implementation backed by simulated matching engine
- `SimMarketDataFeed`: loads `.jsonl` events and reorders by timestamp
- `MatchingEngine`: simplified fill simulation
- `BacktestReport`: basic report object (PnL fields partially computed)
- `MarketDataRecorder`: records events as NDJSON

Key design rule: strategy code should be identical across live and backtest modes.

## 5. Exchange Connector Status

- Hyperliquid: implemented (`hyperliquid_sdk`-backed), supports order placement/cancel/modify plus positions/open orders.
- Binance, Lighter, Polymarket, Predict.fun: scaffold crates only.

## 6. Strategy Status

- `pair_trader`: skeleton strategy with parameters + spread model structure, logic mostly TODO.
- `prediction_quoter`: skeleton handling fair-value/book/fill events, quote logic TODO.

## 7. Fair Value Service

`crates/fair_value_service` exists as a separate binary boundary, but runtime implementation is TODO.
Intended contract is to publish fair values as `Event::FairValueUpdate` consumed by strategy engine.

## 8. Config and Operations

- `core::config::AppConfig` supports TOML config with engine/exchanges/strategies/telemetry blocks.
- Example config at `configs/example.toml`.
- Current CLI (`trading-cli`) behavior:
  - `live`: implemented as Hyperliquid test-order smoke function
  - `backtest`: command branch exists but harness wiring TODO
  - `record`: command branch exists but recorder wiring TODO

## 9. Architectural Invariants

1. Strategies do not call exchanges directly.
2. Strategies communicate intent only via `Action`.
3. Engine communicates state/events only via `Event`.
4. `OrderRouter` is the only path from action to connector execution.
5. Use decimal-safe monetary types (`Price`, `Quantity`) for state/accounting.
6. Hot path remains in-process channels, not external message bus.

## 10. Known Gaps / Risks

1. `EngineRunner` currently selects only the first market-data subscription receiver per strategy.
2. `UnifiedRiskManager::check` is not implemented.
3. `SimConnector::modify_order` does cancel-replace with hardcoded buy side.
4. Backtest harness uses `Handle::block_on` in action processing and should be refactored for cleaner async handling.
5. CLI argument parsing is minimal and not aligned with script flags yet.

## 11. ADRs

See `decisions/`:
- 001 single binary + order router
- 002 broadcast channels instead of NATS on hot path
- 003 trait-based strategy abstraction
- 004 fair value as separate service
- 005 unified risk management
- 006 OTel telemetry

The high-level ADR direction still matches implementation intent, but runtime completeness is not yet at production level.
