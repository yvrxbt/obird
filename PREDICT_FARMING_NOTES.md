# predict.fun Points Farming — Session Notes
_Last updated: 2026-04-15_

---

## What we built / fixed (chronological)

### 1. Bug: pre-existing orders misattributed on restart
**File:** `crates/connectors/predict_fun/src/client.rs` — `PredictFunClient::from_env()`

**Problem:** On restart, `placed_instruments` and `active_orders` were empty. Old resting orders from prior sessions (still on the book) got wallet-event fills whose hashes weren't in our maps → defaulted to YES instrument. Position tracker credited them as YES fills even when they were NO orders (e.g. fill at price 0.640 logged as YES — impossible, our YES bid was 0.35).

**Fix:** After auth, call `get_open_orders()` and seed both `active_orders` and `placed_instruments` from existing open orders before moving the client into the engine. This also means `cancel_all()` at startup can actually cancel those old orders (it iterates `active_orders` for predict_ids).

---

### 2. Bug: shutdown didn't cancel orders (predict path)
**Files:** `crates/connectors/predict_fun/src/client.rs`, `crates/cli/src/live.rs`

**Problem:** `strategy.shutdown()` returns `Action::CancelAll` but the runner aborts strategy tasks before processing it — it was never called. After Ctrl-C the predict path just logged "Engine stopped" with orders still resting.

**Fix:** Added `PredictShutdownHandle` (mirrors `HyperliquidClient::ShutdownHandle`):
- `shutting_down: Arc<AtomicBool>` — checked at top of `place_order()`, blocks new places after flag is set
- `shutdown_handle()` method — extracted BEFORE client moves into engine runner
- `cancel_all()` — iterates `active_orders` for predict_ids, fires REST cancel, **awaits HTTP confirmation**, logs ack

**Shutdown sequence now (identical to HL):**
```
Ctrl-C / SIGTERM
  → EngineRunner sets shutting_down flag     (blocks new place_order calls)
  → strategy tasks aborted
  → router drains                            (any in-flight place completes, hash recorded)
  → shutdown.cancel_all()                    (REST cancel all active_orders, awaits ack)
  → logs "cancel ack received"
  → process exits
```

`live.rs` `run_predict()` now mirrors `run_hl()` exactly.

---

### 3. Points not activating — order size too small
**File:** `configs/predict_quoter.toml` (and `configs/markets/21177.toml`)

**Problem:** `order_size_usdt = 10`:
- YES at ~0.35: buys **28.6 shares** (minimum is 100)
- NO at ~0.65: buys **15.4 shares** (minimum is 100)
Neither side qualified. Points system ignored all our orders.

**Fix:** `order_size_usdt = 70`:
- YES at ~0.35: buys **~200 shares** ✓
- NO at ~0.65: buys **~107 shares** ✓

**Sizing rule:** binding constraint is the higher-priced side (fewer tokens per dollar).
`order_size_usdt ≥ min_shares × max(yes_price, no_price)`
At mid = 0.50: need $50. At NO = 0.65: need $65. Use $70 for headroom.

---

### 4. fill_pause_secs logic
**Insight:** Maker fee = 0%. Fills cost nothing. Every second off-book = zero points.
`fill_pause_secs = 5` (kept short). The 30s pause we briefly tried was actively hurting.

---

### 5. Points metadata params added
**File:** `crates/strategies/prediction_quoter/src/params.rs`

Added two new fields to `QuoterParams` (with defaults so old configs still work):
- `spread_threshold_v: Decimal` — the `v` in `score_factor = ((v - spread) / v)²`
- `min_shares_per_side: Decimal` — minimum qualifying order size

**These are NOT in the predict.fun API.** Read them manually by hovering the
"Activate Points" / "Points Active" badge in the orderbook UI. The tooltip shows:
```
Max spread ±6¢ | Min. shares: 100
```
→ `spread_threshold_v = 0.06`, `min_shares_per_side = 100`

---

### 6. Points logging infrastructure
**File:** `crates/strategies/prediction_quoter/src/quoter.rs`

**Per-cycle tracking:** When orders are placed, we record:
- `cycle_placed_at: Instant` (wall clock)
- `cycle_yes/no_qty` (shares placed)
- `cycle_yes/no_bid` (prices)
- `cycle_spread_from_mid` (actual distance from mid)

**On cancel/fill → `CYCLE_END` log:**
```
on_book_secs=45.1  yes_qty=200  no_qty=107  spread_from_mid=0.005
score_factor=0.6944  est_yes_score=6249  est_no_score=3340  ended_by=drift
```

**REQUOTE log now includes:**
`score_factor`, `qualifies_yes`, `qualifies_no`, `min_shares`, `spread_threshold_v`

**On shutdown → `SESSION_SUMMARY` log + file write:**
```
runtime_secs=3600  cycles=48  on_book_secs=3240  pct_on_book=90%
est_yes_score_raw=123456  est_no_score_raw=66789
```
Also appends one JSON line to **`logs/data/points-sessions.jsonl`** with full cycle array.

**Correlating with dashboard:**
When Z reports N points from the dashboard:
```
points_per_score_second = N / (est_yes_score_raw + est_no_score_raw)
```
Use that rate to project future earnings and optimize spread/size tradeoffs.

---

### 7. Multi-market infrastructure
**Files:** `configs/markets/TEMPLATE.toml`, `configs/markets/21177.toml`, `scripts/farm.py`

**`farm.py`** launcher:
- Reads all `configs/markets/*.toml` (skips `TEMPLATE.toml`)
- Starts one `trading-cli live` process per config
- Logs each to `logs/farm/<market_id>.log`
- Ctrl-C sends SIGTERM to all → each process runs its shutdown cancel sequence
- Auto-restarts any process that crashes

**`predict-markets` CLI** now:
- Prints per-market config blocks with auto-resolved Polymarket token IDs when available
- Estimates `order_size_usdt` to meet minimum-share thresholds
- Auto-fills `spread_threshold_v` and `min_shares_per_side` from the API
- Can write full TOML files directly via `--write-configs` (no manual editing)

---

### 8. Bug: 1-cent BBO spread caused all cycles to be skipped
**Files:** `crates/strategies/prediction_quoter/src/pricing.rs`, `configs/markets/*.toml`

**Problem:** Both active markets (21177 BTC $60k/$80k, 143028 Arsenal) had 1-cent BBO spreads (0.36/0.37 and 0.65/0.66). The pricing code had two bugs:
1. Early-exit check: `spread < join_cents × 2` → `0.01 < 0.02` → always skipped
2. Even if that passed, 2dp rounding left no valid price strictly between a 1-cent-wide bid and ask

**Root cause:** `pricing::calculate` hardcoded 2dp rounding but the predict.fun API actually exposes `decimalPrecision: 2 | 3` per market. Precision=3 markets (0.001 tick) have 9 valid placements inside a 1-cent BBO spread (0.651 to 0.659).

**Fix (two-part):**

**Part A — Automated precision detection:**
- `get_market_by_id(id)` called once at connector startup, fetches `decimalPrecision` (and also `spreadThreshold`, `shareThreshold`)
- `decimal_precision: u32` stored in `PredictFunClient`, exposed via `ExchangeConnector::decimal_precision()`
- Engine runner propagates it through `StrategyState.decimal_precision` to the strategy at `initialize()`
- `PredictionQuoter` reads it in `initialize()`, passes it to `pricing::calculate()` every tick

**Part B — Pricing logic now uses market tick:**
- `pricing::calculate` signature: `(book, fair_value, spread_cents, join_cents?, decimal_precision)`
- Tick = `match decimal_precision { 2 => 0.01, _ => 0.001 }`
- All clamps and rounding use the fetched tick
- `join_cents` is optional: when set, join path targets `predict_mid - join_cents`; otherwise auto-join remains

**`predict-markets` CLI also updated:**
- Calls `get_market_by_id` in parallel with orderbook fetch
- Auto-fills `spread_threshold_v` and `min_shares_per_side` from the API
- Resolves `polymarket_yes_token_id` from `polymarketConditionIds` via Gamma API
- Supports `--write-configs` to emit ready-to-run files in `configs/markets/`
- Supports `--fail-on-missing-poly-token` to hard-fail if any selected market lacks Polymarket linkage

**Verified with tests:** 10 pricing tests including `one_cent_spread_precision3_market_quotes` (regression), `one_cent_spread_precision2_market_returns_none`, `two_cent_spread_precision2_market_quotes`.

---

## Key facts / strategy notes

### Scoring formula
```
score_factor = ((v - spread_from_mid) / v)²
est_points_contribution = score_factor × shares × time_on_book
```
- `v` = market's `spread_threshold_v` (e.g. 0.06 for ±6¢ market)
- `spread_from_mid` = |yes_bid - mid| at time of placement
- Orders beyond `v` earn **zero** points
- Closer to mid = quadratically more points

| spread_from_mid | score_factor (v=0.06) |
|---|---|
| 0.00 (at mid) | 1.000 (100%) |
| 0.01 | 0.694 (69%) |
| 0.02 | 0.444 (44%) |
| 0.03 | 0.250 (25%) |
| 0.06 | 0.000 (0%) |

### Binary market constraint
`no_bid = 1 - yes_bid` (hard identity). We can't post `yes_bid < BBO_bid` without `no_bid` crossing the NO ask. On a tight-BBO market, the quote clamps to `best_bid + tick` regardless of `spread_cents`. To genuinely sit deeper, pick a market with BBO spread ≥ `spread_cents`.

**Tick size**: fetched automatically from the market API at startup. `decimalPrecision=2` → 0.01 tick, `decimalPrecision=3` → 0.001 tick. Most active markets are precision=3. A 1-cent BBO spread on a precision=3 market has 9 valid placements (0.001 to 0.009 inside). A 1-cent BBO spread on a precision=2 market has NO valid placements — the strategy will log "BBO spread < 2 ticks" and skip until the spread widens.

### Position risk
`N YES + N NO tokens` = costs $N, pays $N regardless of outcome → **zero P&L risk when balanced**.
Imbalance risk = `|yes_tokens - no_tokens| × avg_price`.
`max_position_tokens` is the circuit breaker per side.

### Both-sides requirement
BUY YES = bid side. BUY NO = ask-equivalent side (since `no_bid = 1 - yes_bid`). Our dual-BUY strategy satisfies the "qualifying orders on both bid and ask sides" requirement.

### Points distribution cadence
Weekly (every 7 days). 2-3 day calculation period before distribution.

---

## File locations

| What | Where |
|---|---|
| Active single-market config | `configs/predict_quoter.toml` |
| Per-market configs | `configs/markets/<market_id>.toml` |
| Market config template | `configs/markets/TEMPLATE.toml` |
| Multi-market launcher | `scripts/farm.py` |
| Points sessions log | `logs/data/points-sessions.jsonl` |
| Per-market farm logs | `logs/farm/<market_id>.log` |
| BBO tick data | `logs/data/bbo-YYYY-MM-DD.jsonl` |
| Fill data | `logs/data/fills-YYYY-MM-DD.jsonl` |
| Main tracing log | `logs/obird-YYYY-MM-DD.jsonl` |

---

## Run instructions

### Prerequisites
```bash
cd /home/ubuntu/.openclaw/workspace/obird
source .env            # loads PREDICT_API_KEY, PREDICT_PRIVATE_KEY
```

### Build (first time or after code changes)
```bash
cargo build --release -p trading-cli
```

### Discover markets
```bash
./target/release/trading-cli predict-markets
# --all to include non-boosted markets
```
Prints full TOML config blocks with estimated order sizes. Copy into `configs/markets/<id>.toml`, fill in `spread_threshold_v` and `min_shares_per_side` from the UI.

### Run single market
```bash
./target/release/trading-cli live --config configs/markets/21177.toml
# or use the root config:
./target/release/trading-cli live --config configs/predict_quoter.toml
```

### Run all markets (recommended)
```bash
python3 scripts/farm.py
```
Ctrl-C shuts down all cleanly (cancels all orders on each market before exit).

### Add a new market
1. Run `predict-markets` — it now auto-fills `spread_threshold_v`, `min_shares_per_side`, and `order_size_usdt` from the API
2. Copy the printed TOML block into a new file: `configs/markets/<id>.toml`
3. Assign a unique `metrics_port` (9091, 9092, 9093, …) to avoid port conflicts when running multiple markets
4. `farm.py` picks it up automatically on next run
5. `decimal_precision` and tick size are fetched automatically at startup — nothing to configure

### Check points log after a session
```bash
# Pretty-print the last session summary
tail -1 logs/data/points-sessions.jsonl | python3 -m json.tool

# All sessions (one per line)
cat logs/data/points-sessions.jsonl | python3 -m json.tool --no-ensure-ascii
```

### When dashboard shows N points (weekly)
```bash
# Quick correlation calc
python3 -c "
import json, sys
sessions = [json.loads(l) for l in open('logs/data/points-sessions.jsonl')]
total_raw = sum(s['totals']['est_yes_score_raw'] + s['totals']['est_no_score_raw'] for s in sessions)
reported = float(sys.argv[1])
print(f'Reported points: {reported}')
print(f'Total score·secs: {total_raw:.1f}')
print(f'Rate: {reported/total_raw:.6f} points per score·sec')
" <N_FROM_DASHBOARD>
```
