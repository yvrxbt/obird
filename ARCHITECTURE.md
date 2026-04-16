# Trading System Architecture

> Last updated: 2026-04-16
> Status: HL MM + predict.fun farming + Polymarket hedge all live on mainnet.

---

## 1. Purpose

Rust multi-crate HFT framework targeting two domains:
- **Crypto/perp MM**: Hyperliquid ETH spread market-making with inventory skew
- **Prediction market quoting**: predict.fun points-farming with Polymarket FV anchor

The architecture enforces strategy portability across live trading and backtesting.

---

## 2. Core Contracts

Defined in `crates/core`:

- `Strategy` trait — `on_event(Event) → Vec<Action>`, `initialize`, `shutdown`
- `ExchangeConnector` trait — place/cancel/modify, positions, open orders, update stream
- `Event` enum — engine → strategy communication
- `Action` enum — strategy → engine communication
- `MarketDataSink` trait — decouples feed publishing from `MarketDataBus` (enables future NATS/Redis distribution without touching strategies)

These are the primary stability boundary. **Strategies must never import connector crates.**

---

## 3. Runtime Topology

### Data/control path

```
Exchange WS feeds
    │  BookUpdate / Fill / OrderUpdate events
    ▼
MarketDataBus (tokio::broadcast, per instrument, buffer=64)
    │
    ▼
Strategy.on_event(Event) → Vec<Action>
    │
    ▼
OrderRouter (risk gate, groups by exchange, concurrent dispatch via join_all)
    │
    ▼
OrderManager (per-exchange serialisation)
    │
    ▼
ExchangeConnector (REST + WS per venue)
```

### Components

- `engine::runner` — orchestrates task lifecycle, channel wiring, shutdown
- `engine::market_data_bus` — per-instrument broadcast channels; uses `futures::select_all` to merge all subscriptions (no starvation on leg 2+)
- `engine::order_router` — central action routing + risk gate; `join_all` for cross-exchange concurrent dispatch
- `engine::order_manager` — per-exchange submission serialisation
- `engine::risk` — portfolio-level risk (stub; passes all orders)

---

## 4. Backtesting Architecture

`crates/backtest`:
- `BacktestHarness` — replay loop and strategy driving
- `SimConnector` — `ExchangeConnector` backed by simulated matching engine
- `SimMarketDataFeed` — loads `.jsonl` events and replays by timestamp
- `MatchingEngine` — simplified fill simulation
- `MarketDataRecorder` — records live events as NDJSON for future replay

**Note**: `trading-cli backtest` CLI path exists but harness wiring is TODO.

---

## 5. Exchange Connector Status

### Hyperliquid (`crates/connectors/hyperliquid`) ✅ Full

- Order placement via `BatchOrder` API (all levels in one call)
- Cancel via per-OID `BatchCancel` (tracks OIDs from `place_batch` responses)
- L2Book WS subscription for BBO
- `AllMids` subscription for mid-only strategies
- `ShutdownHandle` with `AtomicBool` — blocks new places, fires cancel on shutdown
- Optimal deployment: **Tokyo (ap-northeast-1)**

### Polymarket CLOB (`crates/connectors/polymarket`) ✅ Full (market data + execution)

**Two purposes**: (1) external fair-value signal for predict.fun quoting, (2) execution venue for hedge orders.

**`market_data.rs` — `PolymarketMarketDataFeed`**
- Single WS connection handles all subscribed tokens (YES + NO for each market)
- Subscribes with `{"type": "market", "assets_ids": [...]}` — type must be **lowercase**
- Application-level PING/PONG: sends TEXT `"PING"` every 10s, handles TEXT `"PONG"`
- On PONG: re-publishes last known book state → strategy FV stays fresh on quiet markets
- Reconnects with exponential backoff (1s → 2s → 4s → max 30s)
- Timestamps are milliseconds (not seconds)

**`client.rs` — `PolymarketGammaClient`**
- REST client for condition ID → token ID resolution
- `GET /markets?clob_token_ids={token}` also works to get both tokens given either one

**`execution.rs` — `PolymarketExecutionClient`** (added 2026-04-16)
- Implements `ExchangeConnector` for `Exchange::Polymarket`
- Auth: pre-existing `POLY_API_KEY`/`POLY_SECRET`/`POLY_PASSPHRASE` via HMAC per request
- Order signing: EIP-712 per order using `PREDICT_PRIVATE_KEY` (shared with predict.fun)
- SDK: `polymarket-client-sdk = "0.4.4"` with `features = ["clob"]`
- `place_order`: GTC limit order → SDK builder → `client.sign(&signer, order)` → `client.post_order(signed)`
- `cancel_all`: cancels only tracked active order IDs (not account-wide)
- Token IDs stored as `instrument.symbol` (large decimal string, `U256::from_str` parses decimal)
- `decimal_precision()` returns `Some(2)` (0.01 tick standard on Polymarket CLOB)

**Critical bugs fixed (2026-04-15)**:
1. PING was WS protocol-level frames — Polymarket uses TEXT "PING"/"PONG"
2. Subscription type was "Market" (uppercase) — server silently ignored it, no price_change events
3. Timestamps treated as seconds — they are milliseconds

### predict.fun (`crates/connectors/predict_fun`) ✅ Full

- `PredictFunClient` — EIP-712 order signing for all contract variants:
  - Standard CTFExchange, YieldBearing CTFExchange, NegRisk CTFExchange, YieldBearing NegRisk
- `PredictFunMarketDataFeed` — WS market data feed
- Both outcomes placed as `Side::Buy` (BUY YES at P, BUY NO at Q)
- `cancel_all()` cancels all tracked OIDs across both outcomes in one REST call
- Pre-populates order maps from existing open orders at startup (prevents misattribution on restart)
- `PredictShutdownHandle` mirrors HL shutdown pattern: blocks new places, awaits cancel ack

### Binance (`crates/connectors/binance`) ⚠️ Built, not wired

REST + WS scaffolding built. Not connected to live runner.

---

## 6. Strategy Status

### HlSpreadQuoter (`crates/strategies/hl_spread_quoter`) ✅ Live

- 2-level symmetric spread MM with inventory skew
- State machine: `Empty → Quoting → Cooldown(Instant)` (always-cancel-first pattern)
- Inventory skew: `reservation_mid = mid - skew_factor_bps × net_pos / 10_000 × mid`
- Drift measured against resting prices (not mid-to-mid) — correct with skewed quotes
- Session P&L tracking (cash-flow basis + mark-to-market)
- `PlaceFailed` event handling — prevents "ghost quoting" when batch order fails

### PredictionQuoter (`crates/strategies/prediction_quoter`) ✅ Live

**Core design: conservative dual-FV pricing.**

For each side, uses the more conservative of Polymarket and predict.fun mids:
```
yes_bid = min(poly_mid, predict_mid) - spread_cents
no_bid  = (1 - max(poly_mid, predict_mid)) - spread_cents
```

This guarantees bids are below BOTH venues' mids simultaneously — neither poly-informed
nor predict.fun-informed traders have immediate edge.

**Scoring window gate**: if a computed bid would be `>= spread_threshold_v` from predict_mid,
clamp it just inside the window. Farming strategy prioritizes staying score-eligible.

**Touch-risk gate**: if a resting bid gets too close to ask (`touch_trigger_cents`),
requote defensively (`touch_retreat_cents`) while keeping poly-anchored pricing.
Trigger is latched per risk-regime entry to avoid tick-by-tick retrigger churn.

**Poly FV gate**: if Polymarket FV is configured but unavailable/stale, pauses quoting
entirely. No fallback to predict.fun mid (prevents blind quoting against poly-informed takers).

State machine: `Empty → Quoting → Cooldown(Instant)`

Key parameters:
- `spread_cents` — distance from conservative FV per side (fill-risk knob)
- `spread_threshold_v` — scoring window from predict.fun API
- `fv_stale_secs` — must be > 60 (WS recv timeout); PONG heartbeats keep FV fresh
- `drift_cents` — pull+requote threshold

See `PREDICT_QUOTING_DESIGN.md` for full decision tree.

### PredictHedgeStrategy (`crates/strategies/predict_hedger`) ✅ Live (added 2026-04-16)

**Core design**: parallel risk-reduction strategy running alongside `PredictionQuoter` in the same engine instance. Reacts to predict.fun fills by placing opposite-side orders on Polymarket.

**Hedge identity** (binary market invariant):
```
long YES on predict + long NO on Polymarket = $1 certain at resolution (delta neutral)
long NO on predict  + long YES on Polymarket = $1 certain at resolution (delta neutral)
```

**hedge_map** (built at construction from `MarketMapping`):
```
predict_yes_instrument → poly_no_instrument   (YES fill → buy poly NO)
predict_no_instrument  → poly_yes_instrument  (NO fill  → buy poly YES)
```

**Trigger flow**:
1. `Event::Fill` from `Exchange::PredictFun` → `on_predict_fill` → accumulate `UnhedgedState`
2. `try_hedge(poly_inst)` checks: notional threshold, poly BBO available, slippage guard, min qty
3. On approval: optimistically `consume_all()`, emit `Action::PlaceOrder(poly_inst, Buy, best_ask, qty, GTC)`
4. `Event::PlaceFailed` from `Exchange::Polymarket` → `restore(qty)` to retry next tick
5. `Event::Tick` → `check_urgency()` → bypasses min_notional if `first_unhedged_ts` age > `max_unhedged_duration_secs`

**Optimistic position tracking**: unhedged qty is consumed when the `Action::PlaceOrder` is emitted (not on confirmation). Safe because GTC at best_ask = taker order = immediate fill. Failure path is `PlaceFailed` → restore.

**Key design invariant**: both `PredictionQuoter` and `PredictHedgeStrategy` are separate `StrategyInstance` entries — they are entirely independent and do not share state. The engine delivers fills and book updates to both via the `MarketDataBus` `select_all` merge stream.

**Subscriptions** (returned by `subscriptions()`):
- All predict YES instruments (to receive fills)
- All predict NO instruments (to receive fills)
- All poly YES token instruments (for book updates → BBO cache)
- All poly NO token instruments (for book updates → BBO cache + hedge target)

**HedgeParams defaults** (TOML loading not yet wired — Phase 3):
```
hedge_min_notional      = 5 USDC    (batch small fills)
max_unhedged_notional   = 100 USDC  (future: urgency tier threshold)
max_unhedged_duration   = 60 sec    (urgency escalation timer)
max_slippage_cents      = 0.05      (max poly ask above reference price)
enabled                 = true      (kill-switch)
```

See `POLY_HEDGING_ARCHITECTURE.md` for full design rationale, token ID lookup procedure, and Phase 3 roadmap.

---

## 7. Fair Value Architecture

The `fair_value_service` crate exists as a future separate binary, but for predict.fun
quoting the FV is implemented directly in the strategy + connector:

1. `PolymarketMarketDataFeed` publishes `BookUpdate` for Polymarket YES token
2. Strategy subscribes to both predict.fun AND Polymarket instruments
3. On Polymarket `BookUpdate`: store `polymarket_mid` + timestamp (used as FV signal)
4. On predict.fun `BookUpdate`: compute prices using `min(poly_mid, predict_mid)`

This sidesteps the `FairValueService` boundary for now — acceptable since predict.fun quoting
is the only current FV consumer. Revisit when pair_trader needs cross-exchange FV.

---

## 8. Config and CLI

```
trading-cli live             --config <market.toml>     # single market (HL or predict.fun + hedge)
trading-cli predict-markets  [--all] [--write-configs]  # discover + generate configs (writes both poly token IDs)
trading-cli predict-check                               # smoke-test auth + pricing
trading-cli predict-approve  --all                      # on-chain ERC-1155 + USDT approvals (one-time)
trading-cli predict-liquidate --dry-run --config ...    # passive SELL unwind preview
trading-cli backtest         (stub)
trading-cli record           (stub)
```

Config format: `configs/markets_poly/<market_id>.toml` (auto-generated by `predict-markets`).
Secrets loaded from env (set via `source .env` before running).

**Env vars required for hedge** (in addition to existing predict.fun vars):
```
POLY_API_KEY=<uuid>       # Polymarket CLOB API key
POLY_SECRET=<string>      # HMAC signing secret
POLY_PASSPHRASE=<string>  # API key passphrase
```
`PREDICT_PRIVATE_KEY` doubles as the Polymarket EIP-712 signing key.

**live.rs dispatch (`prediction_quoter` type)**:
1. Builds `PredictFunClient` + `PredictionQuoter` (always)
2. If `polymarket_yes_token_id` set: spins up `PolymarketMarketDataFeed` (YES token for FV)
3. If BOTH `polymarket_yes_token_id` AND `polymarket_no_token_id` set:
   - Adds NO token to same WS feed subscription
   - Builds `PolymarketExecutionClient` (graceful fallback on auth failure)
   - Builds `PredictHedgeStrategy` as second `StrategyInstance`
   - Registers `Exchange::Polymarket` connector in engine
4. Engine runs both strategies concurrently against shared `MarketDataBus`

---

## 9. Architectural Invariants (from CLAUDE.md)

1. Strategies NEVER import connector crates or call exchange APIs
2. `Strategy` trait is the ONLY interface between strategy logic and engine
3. `Action` enum is the ONLY way strategies express intent
4. `Event` enum is the ONLY way the engine communicates with strategies
5. `OrderRouter` is the single point of routing
6. Market data flows via `MarketDataSink` trait (default: `Arc<MarketDataBus>`)
7. Fair value model lives in `fair_value_service` (or inline strategy for now), NOT in strategy crates
8. Connector crates split: `XClient` (order execution) + `XMarketDataFeed` (WS, background task)

---

## 10. Live HL Implementation Notes

### Cancel mechanism
Per-OID `BatchCancel` — tracks OIDs from `place_batch` responses, cancels only those.
Works for all accounts. `scheduleCancel` (removed) required $1M+ volume and cancelled
ALL instruments — unsafe for multi-strategy.

### Price rounding
Use `PriceTick::tick_for(price).normalize().scale()` — raw `.scale()` is wrong for prices like 0.1.

### ALO orders
Always use `HlTif::Alo` for maker orders — prevents crossing the spread.

---

## 11. Multi-Market Farm

Each predict.fun market runs as a **separate process** (one-process-per-market).
This is the current workaround for the engine's `HashMap<Exchange, Connector>` key
which prevents two PredictFun markets from sharing an engine.

**Future fix**: change engine key to `(Exchange, market_id)` in `OrderRouter` and `EngineRunner`.
Then a single process can quote all markets. Also enables the Polymarket feed to serve all
subscriptions over a single WS connection (it's already designed for this: `PolymarketMarketDataFeed::new(vec![...many tokens...])`.

`scripts/farm.py` manages the process fleet:
- One process per TOML in `configs/markets_poly/`
- Restarts on crash with exponential backoff
- Crash-loop protection (3 crashes in 120s → 5-min backoff)
- Graceful shutdown: SIGTERM → wait 15s for cancel acks → SIGKILL

---

## 12. Known Gaps / Risks

| # | Gap | Severity | Fix |
|---|---|---|---|
| 1 | `UnifiedRiskManager::check` is stub | High | Portfolio limits + drawdown constraints |
| 2 | No multi-market single-process support | Medium | Change engine key to `(Exchange, String)` |
| 3 | Binance connector not wired to live runner | Medium | Wire in `live.rs` |
| 4 | `PositionTracker` not implemented | Medium | Aggregate fills, feed risk manager |
| 5 | Backtest CLI not wired to harness | Low | Connect CLI `backtest` command |
| 6 | `SimConnector::modify_order` hardcodes buy side | Low | Trivial fix |
| 7 | Auto-market-switch on boost detection | Low | Poll `get_markets_filtered` every 5 min |
| 8 | Predict→Polymarket hedge path not implemented | High | Implement `PolymarketHedgeStrategy` (see `POLY_HEDGING_ARCHITECTURE.md`) |

---

## 13. ADRs

See `decisions/`:
- 001 — single binary + order router
- 002 — broadcast channels (not NATS) on hot path; `MarketDataSink` trait for future distribution
- 003 — trait-based strategy abstraction
- 004 — fair value as separate service
- 005 — unified risk management
- 006 — OTel telemetry
