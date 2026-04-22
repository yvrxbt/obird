# DEX / CEX Market Making

> Regular perp/spot market making on exchanges with a continuous limit order book.
> Currently: Hyperliquid ETH perp spread MM (live mainnet).
> Next: Binance USD-M Futures as reference price source, then as a second MM leg.
>
> For prediction-market quoting (predict.fun / Polymarket), see `PREDICTION_MARKETS.md`.
> For the underlying engine/runtime, see `README.md`.

---

## 1. Overview

### 1.1 Status

| Venue | Connector | Strategy | Status |
|---|---|---|---|
| Hyperliquid (perp) | `connectors/hyperliquid` | `HlSpreadQuoter` | ✅ Live mainnet |
| Binance (USD-M futures) | `connectors/binance` | — | ⚠️ Connector built, not wired |
| Lighter | `connectors/lighter` | — | Scaffolding only |
| Pair trader | — | `strategies/pair_trader` | Stub |

### 1.2 Strategy catalog

**`HlSpreadQuoter` (live)**
- Two-level symmetric spread around an inventory-skewed reservation mid
- Always-cancel-first state machine (`Empty → Quoting → Cooldown`)
- ALO (post-only) orders only; drift-triggered and fill-triggered requotes
- Session P&L tracking (cash-flow basis + mark-to-market)

**`PairTrader` (stub)**
- Planned: HL–Binance basis trading once Binance is wired
- First milestone: Binance quotes become the reference for `HlSpreadQuoter` (quote around Binance mid instead of HL self-quote) — kills adverse selection from stale self-quotes
- Second milestone: independent MM on Binance side

---

## 2. HlSpreadQuoter — Strategy Mechanics

### 2.1 State machine

```
Empty ─────────► Quoting ─────────► Cooldown(until)
  ▲  (place batch)  │                    │
  │                 │ DRIFT / FILL       │
  │                 ▼                    │
  └──── cancel_all ─┴──── wait ──────────┘
```

- `Empty` — no orders resting. On next `Tick` or `BookUpdate` with fresh mid, emit `place_batch`.
- `Quoting` — 4 orders resting (2 levels × 2 sides). Monitor `BookUpdate` for drift; monitor `Fill` for any touch.
- `Cooldown(until)` — all orders cancelled, waiting until `Instant::now() >= until`. Then transitions to `Empty`.

Always cancels the full batch before requoting — never "modify in place". This is the correctness-first pattern: guaranteed consistent snapshot of live orders.

### 2.2 Reservation mid and inventory skew

```
reservation_mid = mid − (skew_factor_bps_per_unit × net_pos / 10_000) × mid
```

- Long inventory → reservation shifts **down** → ask becomes relatively cheaper → steers fills toward selling (reducing position)
- Short inventory → reservation shifts **up** → bid becomes relatively cheaper → steers fills toward buying
- Symmetric per level: `bid_L = reservation × (1 − level_bps/10_000)`, `ask_L = reservation × (1 + level_bps/10_000)`

Calibration target: at `max_position`, skew should ≈ one spread width. Config below: 50 bps/ETH × 0.1 ETH = 5 bps, which equals the L1 half-spread (when `level_bps[0]=5`).

### 2.3 Drift check — uses raw mid, not reservation

```
drift = |mid_now − last_quoted_mid| / last_quoted_mid × 10_000  (in bps)
if drift > drift_bps: cancel_all → Cooldown(drift_pause_secs)
```

Intentionally measures against **raw mid**, not reservation. The drift check is about market movement, not self-quote staleness from inventory changes. (Inventory changes trigger requotes via the skew term being baked into the next placement, not via drift.)

### 2.4 Post-fill cooldown

Any fill on any level → `cancel_all` → `Cooldown(fill_pause_secs)`. Prevents back-to-back fills in trending conditions from stacking the same-side inventory before the skew term can adjust.

### 2.5 Position cap

`max_position` is enforced at placement time: if `|net_pos + order_size|` would exceed it on the accumulating side, that side's orders are skipped for this batch. The opposite side continues to quote. This lets the position mean-revert without adding to the excursion.

---

## 3. Config Reference — `configs/quoter.toml`

Canonical live config (verified against code in `crates/strategies/hl_spread_quoter/src/params.rs`):

```toml
[engine]
tick_interval_ms = 100

[[exchanges]]
name = "hyperliquid"
api_key_env = ""                 # unused for HL (private key sufficient)
secret_key_env = "HL_SECRET_KEY"
testnet = false                  # mainnet

[[strategies]]
name = "hl_quoter_v1"
strategy_type = "hl_spread_quoter"
instruments = ["Hyperliquid.Perpetual.ETH"]

[strategies.params]
level_bps        = [50, 100]     # half-spreads per level (bps)
order_size       = "0.05"        # size per level per side (ETH)
drift_bps        = 15            # pull threshold on raw mid move
drift_pause_secs = 5             # cooldown after drift pull
fill_pause_secs  = 10            # cooldown after any fill
max_position     = "0.1"         # stop adding on accumulating side beyond this
skew_factor_bps_per_unit = "50"  # at 0.1 ETH long → 5 bps downshift
taker_fee_bps    = "0.2"         # HL maker rebate for P&L accuracy
```

**Why `level_bps = [50, 100]` not [5, 10]?** Current live config quotes 50/100 bps to survive HL's cumulative-volume rate limit (wider spreads → fewer requotes → lower request rate). Tighter `[5, 10]` is the target post-Binance-reference once self-quote churn is eliminated. The `params.rs` doc comments still use [5, 10] as the illustrative example.

**Why `drift_bps = 15` not 3?** Same reason — with 50 bps half-spreads, a 3 bps mid move barely degrades quote quality, so tightening drift just wastes requotes. 15 bps cuts request rate ~5× vs 3.

---

## 4. Operations

### 4.1 Pre-flight checklist

- [ ] `.env` contains `HL_SECRET_KEY` (mainnet key, never commit)
- [ ] `configs/quoter.toml` has expected `instruments`, `order_size`, `max_position`
- [ ] On Tokyo (ap-northeast-1) server — latency target is cancel_ms p95 < 50ms
- [ ] No other `trading-cli` is running (`ps aux | grep trading-cli`)
- [ ] Previous session logs in `logs/` and `logs/data/` are rotated/preserved

### 4.2 Start

Always build release first — dev binary has meaningful overhead on the hot path.

```bash
cd /path/to/obird
source .env
cargo build --release --bin trading-cli
```

Run under `screen` so it survives SSH disconnects:

```bash
screen -S obird
source .env && RUST_LOG=quoter=info,connector_hyperliquid=info,trading_engine=info \
  ./target/release/trading-cli live --config configs/quoter.toml
# Ctrl+A D  → detach
# screen -r obird  → reattach
```

### 4.3 Stop

**Ctrl+C only.** The engine's shutdown path runs `ShutdownHandle::cancel_all()` which fires per-OID `BatchCancel` on every tracked OID before exit. **Never `kill -9`** — it leaves resting orders on the book with no way to cancel them short of a manual REST call.

### 4.4 Logs

| File | Contents | Use |
|---|---|---|
| `logs/obird-YYYY-MM-DD.jsonl` | All tracing events (debug+). Filter by `fields.target`: `"quoter"` strategy, `"md"` market data. | Full audit trail, debugging |
| `logs/data/bbo-YYYY-MM-DD.jsonl` | BBO tick per L2Book update. `exchange_ts_ns`, `local_ts_ns`, `bid_px/sz`, `ask_px/sz`. | Market data analysis, latency |
| `logs/data/fills-YYYY-MM-DD.jsonl` | One line per fill. `session_pnl`, `mark_pnl`, `net_pos`. Flushed immediately. | P&L, adverse selection |

Useful `jq` one-liners:

```bash
# Watch fills in real time
tail -f logs/data/fills-$(date +%Y-%m-%d).jsonl \
  | jq '{ts:.timestamp_ns,side:.side,price:.price,pnl:.session_pnl,pos:.net_pos}'

# Roundtrip latency summary
jq 'select(.fields.message=="ROUNDTRIP") | {cancel_ms:.fields.cancel_ms|tonumber, place_ms:.fields.place_ms|tonumber}' \
  logs/obird-$(date +%Y-%m-%d).jsonl \
  | jq -s '{n:length, cancel_p50:(map(.cancel_ms)|sort|.[length/2|floor]), place_p50:(map(.place_ms)|sort|.[length/2|floor])}'

# Count drift vs fill pulls
jq -r 'select(.fields.message=="PULL_QUOTES") | .fields.reason' \
  logs/obird-$(date +%Y-%m-%d).jsonl | sort | uniq -c
```

### 4.5 Health indicators

Healthy terminal output:

```
REQUOTE cancel_all + batch_place  mid=3500 reservation=3498.25 skew_bps=-1.75 n_orders=4
ROUNDTRIP cancel_ms=22 place_ms=18 total_ms=40 n_orders=4
FILL side=Sell price=3503.5 net_pos=-0.01 session_pnl=0.0350 mark_pnl=0.0312
COOLDOWN_EXPIRED → REQUOTE
```

Warning signs:

| Symptom | Likely cause | Action |
|---|---|---|
| `cancel_ms > 200` consistently | Network or HL degradation | Check ping to HL endpoint; may need to reconnect |
| `DRIFT` firing every cycle, zero fills | `drift_bps` too tight for current vol | Loosen |
| `ORDER_REJECTED` appearing | Price below min tick or size too small | `RUST_LOG=debug` for reason |
| `net_pos` monotonically growing one direction | Skew not strong enough | Double `skew_factor_bps_per_unit` |
| `net_pos` pinned at `max_position` > 10 min | Strong trending market | Expected; bot stops adding. Consider halting. |
| `ROUNDTRIP cancel_ms=0 place_ms=0` | Fill edge case with empty OID set | Minor, no correctness impact |
| WS reconnecting repeatedly | HL WS instability | Auto-reconnects; watch for stranded orders post-reconnect |
| `PLACE_FAILED … Too many cumulative` | HL rate limit: requests vs cumulative traded volume | Strategy auto-backs off 5 min. Root fix: taker volume or implement `BatchModify`. |

### 4.6 Session milestones

| Duration | What to check |
|---|---|
| First 30 min | Latency, crashes, `ORDER_REJECTED` — manual watch |
| 2–4 hours | Inventory excursion, skew effectiveness, P&L trend |
| 1–2 days | Adverse selection pattern, `drift_bps` tuning |
| 1 week | Meaningful Sharpe estimate, stable parameter set |

---

## 5. P&L and Adverse Selection

### 5.1 Session summary

```python
import pandas as pd

fills = pd.read_json('logs/data/fills-YYYY-MM-DD.jsonl', lines=True)
fills['ts'] = pd.to_datetime(fills['timestamp_ns'], unit='ns')
fills.set_index('ts', inplace=True)

print(f"Fills: {len(fills)}")
print(f"Final cash P&L: {fills['session_pnl'].iloc[-1]:.4f}")
print(f"Final mark P&L: {fills['mark_pnl'].iloc[-1]:.4f}")
print(f"Net position at end: {fills['net_pos'].iloc[-1]:.4f}")
print(fills[['side','price','quantity','fee','session_pnl','net_pos']].tail(20))
```

### 5.2 Adverse selection check

How far does mid move **against** you after a fill? Negative = getting picked off.

```python
bbo = pd.read_json('logs/data/bbo-YYYY-MM-DD.jsonl', lines=True)
bbo['ts'] = pd.to_datetime(bbo['exchange_ts_ns'], unit='ns')
bbo['mid'] = (bbo['bid_px'] + bbo['ask_px']) / 2
bbo = bbo.set_index('ts').sort_index()

results = []
for ts, fill in fills.iterrows():
    mid_0  = bbo['mid'].asof(ts)
    mid_5s = bbo['mid'].asof(ts + pd.Timedelta('5s'))
    mid_30s = bbo['mid'].asof(ts + pd.Timedelta('30s'))
    sign = 1 if fill['side'] == 'Sell' else -1
    results.append({
        'adv_sel_5s':  sign * (mid_5s  - mid_0) / mid_0 * 10_000,
        'adv_sel_30s': sign * (mid_30s - mid_0) / mid_0 * 10_000,
    })

adv = pd.DataFrame(results)
print(adv.describe())
# If median adv_sel_5s < -(half_spread_bps), quotes are being systematically picked off.
```

### 5.3 Strategy comparison (future benchmarking)

| Metric | Computation |
|---|---|
| Fill rate | `len(fills) / quoting_minutes` |
| Spread capture | `mean(fill_price − mid_at_fill) × side_sign` in bps |
| Inventory excursion | `max(abs(net_pos)) / max_position` |
| Mean reversion time | Mean time from fill to `abs(net_pos) < order_size` |
| Session Sharpe | `mean(fill_pnl_increments) / std(fill_pnl_increments)` |
| Adverse selection 5s | See script — key quality metric |

Variants to compare:
1. **Symmetric** (`skew_factor=0`) — baseline, loses in trends
2. **Inventory-skewed** (current) — Avellaneda–Stoikov simplified
3. **Vol-adaptive spread** — scale `level_bps` by rolling realized-vol EMA
4. **Ref-based quoting** — quote around Binance mid (post-Binance; expected biggest win)

---

## 6. Parameter Tuning

Tune **one parameter at a time**. Wait at least one session between changes.

### `drift_bps` (current: 15)

- Getting pulled every few seconds, few fills → loosen
- Sitting with stale quotes, adverse selection after fills → tighten

### `skew_factor_bps_per_unit` (current: 50)

- `net_pos` keeps accumulating directionally → double it
- Fill rate on the mean-reverting side is too low → halve it
- Target: position should mean-revert within ~5 fills

### `fill_pause_secs` (current: 10)

- Missing re-entry after fills → bring to `5`
- Back-to-back fills in trending conditions → bring to `15`

### `level_bps` (current: [50, 100])

- Systematic pick-off at L1 → widen L1
- Fill rate too low, P&L/hour insufficient → tighten L1 (only if latency is solid)
- Post-Binance-ref: target is [5, 10] (current is conservative for rate limits + self-quote staleness)

---

## 7. HL Idiosyncrasies

### 7.1 Cancel mechanism — per-OID `BatchCancel`

Cancels only OIDs tracked from `place_batch` responses. Works for all accounts regardless of volume. Safe for multi-strategy.

**`scheduleCancel` (removed)** required $1M+ traded volume and cancelled ALL instruments on the account — unsafe when multiple strategies share a key.

### 7.2 Order placement — `BatchOrder`

All orders in one API call (not N sequential REST calls). Hot path: `place_batch` returns the OID list, which is what `cancel_all` uses.

### 7.3 Price rounding

**Always** use `PriceTick::tick_for(price).normalize().scale()`. Raw `.scale()` is wrong — e.g., returns 2 for 0.1.

### 7.4 Post-only orders — `HlTif::Alo`

Always use ALO TIF for maker orders. Prevents accidental taker fills if the quote would cross.

### 7.5 Symbol naming

- Perps: plain name — `"ETH"`, `"BTC"`
- Spot: `"@N"` format (e.g., `"@1"` for PURR/USDC)
- Auto-detected in `resolve_symbol()`

### 7.6 Deployment

Tokyo (`ap-northeast-1`). HL's infra is in Tokyo; other regions add 100–200ms RTT to the cancel path, which breaks the `cancel_ms p95 < 50ms` target.

---

## 8. Binance — Wiring Plan

### 8.1 What exists

`crates/connectors/binance/src/`:
- `client.rs` — HMAC-SHA256 signed REST; implements `ExchangeConnector`
- `market_data.rs` — WS BBO + fill feed, publishes via `MarketDataSink`
- Post-only orders use `timeInForce=GTX` (Binance rejects-if-cross semantic)
- Env: `BINANCE_API_KEY`, `BINANCE_SECRET`

### 8.2 What's missing

- Not registered in `live.rs` dispatch
- No `configs/binance_*.toml` template
- Not integrated into `UnifiedRiskManager`
- `PositionTracker` needed before cross-exchange risk gating is meaningful

### 8.3 Cutover plan

**Phase A — Binance as reference price for HL MM**
1. Wire `BinanceMarketDataFeed` in `live.rs` when HL config declares `binance_reference = true`
2. Add `binance_mid` to `HlSpreadQuoter` state; prefer it over self-BBO for `reservation_mid`
3. Fall back to HL mid if Binance feed is stale > N seconds
4. Expected impact: sharp drop in adverse selection (the #1 loss source in self-referential MM)

**Phase B — Independent MM leg on Binance**
1. Port `HlSpreadQuoter` pattern or introduce `BinanceSpreadQuoter` (likely just reuse with venue-parameterized connector)
2. Cross-venue position netting via `UnifiedRiskManager` + `PositionTracker`
3. Basis-trade strategy (`pair_trader`) goes live once both legs are validated independently

### 8.4 Exit criteria — ready to wire Binance

Hard requirements before adding the second exchange:

- [ ] HL survives 4+ hour unattended run with no crashes
- [ ] `net_pos` bounded — never pins `max_position` for > 10 min continuously
- [ ] `session_pnl` positive or breakeven across 2+ sessions
- [ ] `cancel_ms` p95 < 50ms in Tokyo
- [ ] No `ORDER_REJECTED` in normal operation
- [ ] Adverse selection median at 5s is less than L1 half-spread — not getting systematically picked off
- [ ] Skew works: position mean-reverts within ~5 fills after hitting an extreme

---

## 9. Known Gaps

| # | Gap | Severity | Fix |
|---|---|---|---|
| 1 | `UnifiedRiskManager::check` is a stub | High | Portfolio limits + drawdown constraints |
| 2 | Binance connector not wired to live runner | Medium | `live.rs` dispatch (Phase A above) |
| 3 | `PositionTracker` not implemented | Medium | Aggregate fills → feed risk manager |
| 4 | Backtest CLI not wired to harness | Low | Connect CLI `backtest` command |
| 5 | `SimConnector::modify_order` hardcodes buy side | Low | Trivial fix |
| 6 | HL `BatchModify` not implemented | Low | Would reduce rate-limit exposure vs `cancel + place` |

---

## 10. Files Reference

| Concern | Location |
|---|---|
| HL connector | `crates/connectors/hyperliquid/src/{client,market_data,normalize,lib}.rs` |
| HL spread quoter | `crates/strategies/hl_spread_quoter/src/{quoter,params,lib}.rs` |
| HL live config | `configs/quoter.toml` |
| Binance connector | `crates/connectors/binance/src/{client,market_data,normalize,lib}.rs` |
| Pair trader (stub) | `crates/strategies/pair_trader/src/lib.rs` |
| CLI dispatch | `crates/cli/src/live.rs` (HL path around lines 100–240) |
| Engine core | `crates/engine/src/{runner,order_router,order_manager,market_data_bus,risk}.rs` |
| ADRs | `decisions/001–006.md` |
