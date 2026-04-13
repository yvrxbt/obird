# Trading System Architecture

> Last updated: 2026-04-13
> Status: HL MM live and trading. Binance connector next.

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

## 10. Live Market Making — HL Implementation

### Strategy: HlSpreadQuoter (`crates/strategies/hl_spread_quoter`)

Always-cancel-first pattern: every requote cycle emits `[CancelAll, PlaceOrder×N]` as a single
`Vec<Action>`. The router guarantees CancelAll completes before PlaceOrders are submitted.
HL executes the PlaceOrders as a single `BatchOrder` API call (one round-trip for N orders).

Drift is measured **order-price-based**: compare where quotes WOULD be now vs where they ARE.
This is correct even with position-skewed quotes; mid-to-mid drift is equivalent only for
symmetric fixed-spread strategies.

Config: `configs/quoter.toml`. Secrets in `.env` (HL_SECRET_KEY only).

Run: `RUST_LOG=quoter=info cargo run --bin trading-cli -- live --config configs/quoter.toml`

Logs: `logs/obird-YYYY-MM-DD.jsonl` — JSON lines, every mid price + drift level at DEBUG,
all state transitions at INFO. Full price trace always available for post-incident analysis.

### Cancel Latency

HL uses per-OID `BatchCancel` for cancel_all. OIDs are tracked in `HyperliquidClient::active_oids`
(Arc<Mutex<HashSet<u64>>>), populated by place_batch on Resting responses, cleared after a
successful cancel_all. ShutdownHandle shares the same Arc so Ctrl+C uses the same mechanism.

This works for all accounts. `scheduleCancel` (the previous approach) requires $1M+ traded volume
and is not suitable for new accounts.

Race window (unavoidable): fill can occur in the ~100-300ms between cancel being sent and
landing on HL. Cancelling a filled OID via BatchCancel is a safe no-op — HL returns a per-order
error inside the batch response which we ignore.

### Market Data

Subscribes to `AllMids` (sub-block, fires on any mid change) + `OrderUpdates` + `UserFills`.
`AllMids` gives a synthetic mid-only book — sufficient for fixed-spread quoting.
For BBO-aware strategies or pair trading: switch to `Subscription::L2Book` or `Subscription::Bbo`
per instrument.

Optimal deployment region: **Tokyo (ap-northeast-1)** per HL docs — lowest latency to validators.

## 11. Multi-Receiver Fix

`EngineRunner` now uses `futures::stream::select_all` to merge all instrument subscriptions
into a single fair stream. Both instruments of a pair-trade strategy are polled equally.
Previously only the first subscription receiver was polled (silent starvation on leg 2+).

## 12. Concurrent Cross-Exchange Dispatch

`OrderRouter` groups actions by exchange and submits place batches concurrently via `join_all`.
A strategy returning `[PlaceOrder(HL), PlaceOrder(Binance)]` fires both legs simultaneously —
total latency = max(HL_latency, Binance_latency) rather than sum.

## 13. Known Gaps / Risks

1. ~~`EngineRunner` only polls first subscription receiver~~ — fixed (select_all)
2. `UnifiedRiskManager::check` is not implemented — passes all orders.
3. `SimConnector::modify_order` does cancel-replace with hardcoded buy side.
4. Backtest harness uses `Handle::block_on` in action processing.
5. `PositionTracker::on_fill` is not implemented — strategies track position locally.
6. `scheduleCancel` cancels across all instruments — not safe for multi-strategy deployment.
7. Binance, Lighter, Polymarket, Predict.fun connectors are scaffolds only.
8. FairValueService is stubbed — needed for prediction market quoting.

## 14. Next Steps

- [ ] Binance connector: `BinanceClient` + `BinanceMarketDataFeed`
- [ ] FairValueService: subscribe to multi-exchange BookUpdate, publish FairValueUpdate
- [ ] PositionTracker: aggregate fills from all connectors, feed UnifiedRiskManager
- [ ] Per-OID cancel: track OIDs at connector level for faster targeted cancel
- [ ] BBO subscription: replace AllMids with Bbo for per-instrument latency
- [ ] Backtest wiring: connect CLI `backtest` command to BacktestHarness
- [ ] Multi-strategy safety: per-instrument scheduleCancel or OID tracking

## 15. ADRs

See `decisions/`:
- 001 single binary + order router
- 002 broadcast channels instead of NATS on hot path (MarketDataSink trait added for future distribution)
- 003 trait-based strategy abstraction
- 004 fair value as separate service
- 005 unified risk management
- 006 OTel telemetry
