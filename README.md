# obird — Rust HFT Framework

A multi-crate Rust workspace for low-latency trading execution across two verticals in a single binary:

1. **Crypto perp market-making** — Hyperliquid ETH spread MM (live mainnet)
2. **Prediction market quoting + hedging** — predict.fun points farming with Polymarket fair-value anchor and delta-neutral hedging (live mainnet)

This document is the **engineering overview**: architecture, data flow, code flow. For domain-specific operations and design see:

| Doc | Covers |
|---|---|
| [`PREDICTION_MARKETS.md`](PREDICTION_MARKETS.md) | predict.fun farming + Polymarket hedging: pricing, ops, config |
| [`DEX_CEX_MM.md`](DEX_CEX_MM.md) | Hyperliquid spread MM + Binance wiring plan |
| [`PRD_FARMING_PLATFORM.md`](PRD_FARMING_PLATFORM.md) | v2 platform roadmap (NATS, QuestDB, 3-region AWS, FV service) |
| [`CHANGELOG.md`](CHANGELOG.md) | Historical evolution of strategies and connectors |
| [`decisions/`](decisions/) | ADRs 001–006 |

---

## 1. What's live

| Component | Status | Notes |
|---|---|---|
| `HlSpreadQuoter` | ✅ Live mainnet | 2-level symmetric MM, inventory skew, session P&L |
| `PredictionQuoter` | ✅ Live mainnet | Conservative dual-FV pricing, poly FV gate |
| `PredictHedgeStrategy` | ✅ Live mainnet | Polymarket NO-side hedge on predict.fun YES fills |
| Hyperliquid connector | ✅ Full | place/cancel/modify, per-OID BatchCancel, L2Book WS |
| Polymarket connector | ✅ Full | CLOB WS feed + execution (EIP-712), text PING/PONG |
| predict.fun connector | ✅ Full | EIP-712 signing, yield-bearing + negRisk contracts |
| `predict-markets` CLI | ✅ Full | Discovers markets, writes configs with poly token IDs |
| `predict-liquidate` CLI | ✅ Full | Passive limit-only position unwind helper |
| Multi-market farm | ✅ Full | `scripts/farm.py` — N markets, crash-loop protection |
| Binance connector | ⚠️ Built | Not wired into live runner — see `DEX_CEX_MM.md` §8 |
| Backtest harness | ⚠️ Partial | Primitives in `crates/backtest`, CLI not wired |
| Risk manager | ⚠️ Stub | Passes all orders, no portfolio limits yet |
| `fair_value_service` | ⚠️ Stub | Binary scaffolded; FV still inline in strategies |

---

## 2. Quick start

### 2.1 HL spread MM

```bash
source .env   # needs HL_SECRET_KEY
cargo build --release --bin trading-cli
RUST_LOG=quoter=info,connector_hyperliquid=info \
  ./target/release/trading-cli live --config configs/quoter.toml
```

Full ops guide → `DEX_CEX_MM.md` §4.

### 2.2 predict.fun multi-market farm

```bash
source .env   # needs PREDICT_API_KEY, PREDICT_PRIVATE_KEY
cargo build --release --bin trading-cli

# Discover live boosted markets, write poly-linked configs only
./target/release/trading-cli predict-markets \
  --all --write-configs --fail-on-missing-poly-token \
  --output-dir configs/markets_poly

# Optional: passive unwind preview
./target/release/trading-cli predict-liquidate --dry-run \
  --config configs/markets_poly/21177.toml

# Start all markets
python3 scripts/farm.py
```

Full pricing + ops guide → `PREDICTION_MARKETS.md`.

---

## 3. Runtime Architecture

### 3.1 Data and control flow

```
Exchange WS feeds  (HL L2Book, Polymarket CLOB, predict.fun)
       │  BookUpdate / Fill / OrderUpdate / PlaceFailed events
       ▼
  MarketDataBus   (tokio::broadcast, per-instrument, buffer=64)
       │          merged across subscriptions via futures::select_all
       ▼
  Strategy.on_event(Event) → Vec<Action>    (pure logic, no exchange imports)
       │
       ▼
  OrderRouter   (risk gate → UnifiedRiskManager; groups by exchange; join_all)
       │
       ▼
  OrderManager  (per-exchange submission serialisation)
       │
       ▼
  ExchangeConnector   (REST + WS per venue)
```

Strategies express intent exclusively as `Action` (place / cancel / cancel_all). The engine responds exclusively as `Event` (BookUpdate, Fill, OrderUpdate, PlaceFailed, Tick).

### 3.2 Engine components

- `engine::runner` — orchestrates task lifecycle, channel wiring, shutdown
- `engine::market_data_bus` — per-instrument broadcast channels; `select_all` merge avoids leg-2+ starvation
- `engine::order_router` — central routing + risk gate; `join_all` for cross-exchange concurrent dispatch
- `engine::order_manager` — per-exchange submission serialisation (one outstanding request per venue)
- `engine::risk` — portfolio risk gate (currently stub: passes all orders)

### 3.3 Fair value flow

For predict.fun quoting, FV is currently **inline in the strategy** (not a separate service):

1. `PolymarketMarketDataFeed` publishes `BookUpdate` for the poly YES token
2. `PredictionQuoter` subscribes to **both** predict.fun and Polymarket instruments
3. On poly `BookUpdate`: strategy stores `polymarket_mid + timestamp`
4. On predict.fun `BookUpdate`: compute conservative prices using `min(poly_mid, predict_mid)` and `max(...)` for NO side

The `fair_value_service` crate exists as scaffolding for the v2 architecture (see `PRD_FARMING_PLATFORM.md`) — separate binary, UDS bincode publisher — but it's a stub today.

---

## 4. Core Contracts

Defined in `crates/core`. These are the stability boundary that lets strategies run unchanged in live + backtest.

| Trait / Type | Role |
|---|---|
| `Strategy` | `on_event(Event) → Vec<Action>`, `initialize`, `shutdown` |
| `ExchangeConnector` | place/cancel/modify, positions, open orders, update stream |
| `MarketDataSink` | Decouples feed publishing from `MarketDataBus` (future distribution seam) |
| `Event` enum | Engine → strategy communication (BookUpdate, Fill, OrderUpdate, PlaceFailed, Tick) |
| `Action` enum | Strategy → engine communication (PlaceOrder, CancelAll, ModifyOrder, ...) |
| `RiskCheck` | Portfolio-level gate invoked by `OrderRouter` |

**Connector split** (enforced convention): each exchange crate has a client (`XClient`, implements `ExchangeConnector`) plus a market-data feed (`XMarketDataFeed`, runs as background task, publishes via `MarketDataSink`).

---

## 5. Strategy catalog

See domain docs for full design and operations.

| Strategy | Crate | Doc | Status |
|---|---|---|---|
| `HlSpreadQuoter` | `strategies/hl_spread_quoter` | `DEX_CEX_MM.md` §2 | ✅ Live |
| `PredictionQuoter` | `strategies/prediction_quoter` | `PREDICTION_MARKETS.md` §2 | ✅ Live |
| `PredictHedgeStrategy` | `strategies/predict_hedger` | `PREDICTION_MARKETS.md` §3 | ✅ Live |
| `PairTrader` (stub) | `strategies/pair_trader` | `DEX_CEX_MM.md` §1.2 | Planned (post-Binance) |

---

## 6. Workspace layout

```
crates/
├── core/                    # Traits, Event/Action enums, shared types
├── engine/                  # Runner, router, risk, MarketDataBus
├── backtest/                # SimConnector, SimMarketDataFeed, MatchingEngine
├── cli/                     # trading-cli entrypoint (live / predict-* / backtest)
├── fair_value_service/      # Stub — separate FV binary (PRD v2)
├── connectors/
│   ├── hyperliquid/         # ✅ Full — client + L2Book WS
│   ├── polymarket/          # ✅ Full — CLOB WS feed + execution (EIP-712)
│   ├── predict_fun/         # ✅ Full — EIP-712 signing, all CTFExchange variants
│   ├── binance/             # ⚠️  Built, not wired
│   └── lighter/             # Scaffolding only
└── strategies/
    ├── hl_spread_quoter/    # ✅ Live — 2-level MM, inventory skew
    ├── prediction_quoter/   # ✅ Live — dual-FV conservative pricing
    ├── predict_hedger/      # ✅ Live — poly NO hedge on predict YES fill
    └── pair_trader/         # Stub
configs/
├── quoter.toml              # HL MM config
└── markets_poly/            # Auto-generated predict.fun configs (one per market)
scripts/
└── farm.py                  # Multi-market launcher with crash-loop protection
decisions/                   # ADRs (001–006)
graphify-out/                # Generated codebase graph (GRAPH_REPORT.md)
```

### 6.1 Why one process per predict.fun market (today)

The engine key is `HashMap<Exchange, Connector>` — two predict.fun markets can't share an engine instance because they'd collide on the `PredictFun` key. Workaround: `scripts/farm.py` runs one process per TOML in `configs/markets_poly/`, with exponential-backoff restart and 3-crashes-in-120s → 5-min backoff.

**Planned fix**: change the key to `(Exchange, market_id)` in `OrderRouter` and `EngineRunner`. Then a single process quotes all markets, and the single Polymarket WS connection can serve all FV subscriptions (the `PolymarketMarketDataFeed` already handles many tokens per connection). See `PRD_FARMING_PLATFORM.md` for the v2 design.

---

## 7. Build and test

```bash
cargo build --workspace                     # debug
cargo build --release --bin trading-cli     # release (always use for live)
cargo test --workspace
cargo clippy --workspace
```

Backtesting primitives live in `crates/backtest`:
- `BacktestHarness` — replay loop
- `SimConnector` / `SimMarketDataFeed` — simulated venue + feed
- `MatchingEngine` — simplified fill simulation
- `MarketDataRecorder` — records live events as NDJSON for later replay

**Status**: the primitives work but the `trading-cli backtest` dispatch path is a stub.

---

## 8. Environment variables

| Variable | Used by | Required for |
|---|---|---|
| `HL_SECRET_KEY` | Hyperliquid connector | HL MM |
| `HL_SYMBOL` | CLI (validation) | HL MM |
| `PREDICT_API_KEY` | predict.fun connector | Farming |
| `PREDICT_PRIVATE_KEY` | predict.fun + Polymarket execution | Farming + hedging (doubles as poly EIP-712 signing key; poly SDK derives its API key from this) |
| `BINANCE_API_KEY`, `BINANCE_SECRET` | Binance connector | Not yet used at runtime |

Secrets live in `.env` (git-ignored). `source .env` before running `cargo run` / `trading-cli`.

---

## 9. Architectural Invariants

These are the rules that keep strategies portable across live + backtest and prevent venue-specific logic from leaking. Enforced by convention (not the type system in every case):

1. Strategies NEVER import connector crates or call exchange APIs
2. `Strategy` trait is the ONLY interface between strategy logic and engine
3. `Action` enum is the ONLY way strategies express intent
4. `Event` enum is the ONLY way the engine communicates with strategies
5. `OrderRouter` is the single point of routing; `OrderManager` per exchange serialises submission
6. Market data flows via `MarketDataSink` trait (in-process default: `Arc<MarketDataBus>` backed by `tokio::broadcast`) — connector feeds call `sink.publish()`, never reference broadcast directly. This seam enables NATS/Redis distribution without touching strategies.
7. Fair value model lives in `fair_value_service` (or inline in the strategy for now, for predict.fun) — **never** in a general-purpose strategy crate
8. Connector crates are split: `XClient` (order execution, implements `ExchangeConnector`) + `XMarketDataFeed` (WS feed, background task, publishes to `MarketDataSink`)

### 9.1 Code standards

- `cargo clippy --workspace` and `cargo test --workspace` before calling work done
- `rust_decimal::Decimal` for all prices/quantities — **never** `f64`
- `thiserror` for error types — no string errors
- `tokio` runtime exclusively
- Doc comments on all public items, explaining *why* not *what*

---

## 10. Graceful shutdown

Ctrl+C triggers a deterministic shutdown path across all running strategies and connectors:

1. Engine stops accepting new `Action`s
2. Each `ShutdownHandle` flips its `AtomicBool` → connector blocks new `place_batch` calls
3. `cancel_all()` fires on tracked OIDs (per-OID `BatchCancel` on HL; tracked-OID cancel-all on predict.fun; tracked-order cancel on Polymarket)
4. Engine awaits cancel acks, then exits

**Never `kill -9`** — it leaves resting orders on venues with no local state to recover them.

---

## 11. ADRs

Decision records live in `decisions/`:

- **001** — single binary + `OrderRouter`
- **002** — `tokio::broadcast` on the hot path (not NATS) + `MarketDataSink` trait for future distribution
- **003** — trait-based strategy abstraction
- **004** — fair value as separate service (future binary, currently inline)
- **005** — unified risk management (currently stub)
- **006** — OpenTelemetry for tracing

See `decisions/template.md` for new ADRs — number sequentially.

---

## 12. Known gaps (cross-cutting)

| # | Gap | Severity | Reference |
|---|---|---|---|
| 1 | `UnifiedRiskManager::check` is a stub | High | README §3.2 |
| 2 | One-process-per-market (predict.fun) | Medium | §6.1 + `PRD_FARMING_PLATFORM.md` |
| 3 | Binance connector not wired | Medium | `DEX_CEX_MM.md` §8 |
| 4 | `PositionTracker` not implemented | Medium | `DEX_CEX_MM.md` §9 |
| 5 | `fair_value_service` is a stub | Medium | `PRD_FARMING_PLATFORM.md` |
| 6 | Backtest CLI not wired to harness | Low | §7 |
| 7 | Auto-market-switch on boost detection | Low | `PREDICTION_MARKETS.md` |

Domain-specific gaps are tracked in the domain docs.
