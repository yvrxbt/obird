# predict.fun Points Farming — Session Notes
_Last updated: 2026-04-15_

Chronological record of bugs found, root causes, and fixes. Future LLMs: read top-to-bottom
to understand how the strategy reached its current design before making any changes.

---

## Build history (what we built / fixed, in order)

### 1. Bug: pre-existing orders misattributed on restart
**File:** `crates/connectors/predict_fun/src/client.rs`

**Problem:** On restart, `placed_instruments` and `active_orders` were empty. Old resting
orders from prior sessions got wallet-event fills defaulting to YES instrument — wrong
for NO fills. Position tracker accumulated incorrect inventory.

**Fix:** After auth, call `get_open_orders()` and seed both maps from existing open orders
before the client enters the engine. `cancel_all()` at startup can now actually cancel
pre-existing orders.

---

### 2. Bug: shutdown didn't cancel orders
**Files:** `crates/connectors/predict_fun/src/client.rs`, `crates/cli/src/live.rs`

**Problem:** `strategy.shutdown()` returns `Action::CancelAll` but the runner aborts
strategy tasks before processing it. Ctrl-C left orders resting on the book.

**Fix:** Added `PredictShutdownHandle` (mirrors HL pattern):
- `shutting_down: Arc<AtomicBool>` — blocks new places after flag set
- `shutdown_handle()` extracted before client moves into engine
- `cancel_all()` iterates `active_orders`, fires REST cancel, **awaits HTTP confirmation**

Shutdown sequence: `Ctrl-C → set flag → drain router → REST cancel → exit`.

---

### 3. Points not activating — order size too small
**Problem:** `order_size_usdt = 10` → ~28 YES shares at 0.35 price. Minimum qualifying
size is 100 shares. Points system ignored all orders.

**Sizing rule:** `order_size_usdt ≥ min_shares × max(yes_price, no_price)`.
At any mid ≤ 0.65: need `order_size_usdt ≥ 65`. Use 65–70 for headroom.

---

### 4. fill_pause_secs lesson
Maker fee = 0%. Every second off-book = zero points. `fill_pause_secs = 5` is correct.
30s pause we tried earlier was actively losing points.

---

### 5. Points metadata params added
Added `spread_threshold_v` and `min_shares_per_side` to `QuoterParams`.
These are NOT in the API — auto-filled by `predict-markets --write-configs` from
`GET /v1/markets/{id}` (which returns `spreadThreshold` and `shareThreshold`).

---

### 6. Points logging infrastructure
Added per-cycle tracking and `SESSION_SUMMARY` log on shutdown.
Appends JSON to `logs/data/points-sessions.jsonl` for points yield analysis.

Calibration: after weekly dashboard report, compute:
```
rate = dashboard_points / (est_yes_score_raw + est_no_score_raw)
```
Use rate to project future earnings and optimise spread/size.

---

### 7. Multi-market infrastructure
`scripts/farm.py`: reads all `configs/markets_poly/*.toml`, starts one process per market,
auto-restarts crashes. Ctrl-C triggers SIGTERM to all children → graceful cancel → exit.

`predict-markets --write-configs`: generates full TOML configs with auto-filled
`spread_threshold_v`, `min_shares_per_side`, `polymarket_yes_token_id`.

---

### 8. Bug: 1-cent BBO on precision=2 markets skipped all cycles
**Problem:** `decimalPrecision=2` markets (tick=0.01) with 1-cent BBO had no valid
interior price. Strategy logged "BBO < 2 ticks" and never quoted.

**Root cause:** Old pricing code assumed 2dp rounding globally. Market API returns
`decimalPrecision: 2 | 3` — must use the market's actual tick.

**Fix:** Fetch `decimalPrecision` from `GET /v1/markets/{id}` at connector startup.
Propagate through `StrategyState.decimal_precision` to strategy at `initialize()`.
`pricing::calculate` uses it for all rounding and crossing guards.

---

### 9. Bug: immediate fills — crossing guard clamped orders to mid
**Problem:** Old linked design `no_bid = 1 - yes_bid`. When `poly_mid - spread_cents ≤
market_bid`, crossing guard clamped YES to `best_bid + tick` ≈ mid. Both sides ended
up at mid. Any taker at fair value filled us instantly.

**Fix (first pass):** Decouple YES and NO pricing:
```
yes_bid = poly_mid - spread_cents          # independent, may rest below market bid
no_bid  = (1 - poly_mid) - spread_cents    # independent
```
YES below market bid = valid resting maker order. No crossing.
`yes_bid + no_bid = 1 - 2×spread_cents` (< 1.00). If both fill, combined position
is profitable by `2×spread_cents` regardless of outcome.

---

### 10. Bug: startup blindness — quoting before first Polymarket FV
**Commit:** `f44a8f8`

**Problem:** On startup, predict.fun WS connected before Polymarket WS. Strategy quoted
using `poly_fv=None fv_used=predict_mid` — exactly at predict.fun mid. Filled within
2–3 seconds by informed traders.

**Fix:** Hard gate in `effective_fv()`:
- If Polymarket FV is configured and unavailable: return `None` → strategy waits, no quotes.
- If FV is stale (> `fv_stale_secs` since last update): pause quoting.
- No fallback to predict.fun mid — blind quoting against poly-informed takers is always bad.

First log line after connect is now:
```
INFO quoter: Waiting for first Polymarket FV update before quoting
```
followed by `REQUOTE` only after the first poly `BookUpdate` arrives.

---

### 11. Polymarket WS feed — three critical bugs fixed
**File:** `crates/connectors/polymarket/src/market_data.rs`

**Bug A — Wrong PING protocol (critical)**:
Docs specify: client sends TEXT frame `"PING"`, server responds TEXT `"PONG"`.
Old code sent WebSocket protocol-level `Message::Ping(vec![])` — wrong layer.
Result: pings went nowhere, connection appeared alive but had no application heartbeat.

**Fix:** `Message::Text("PING".to_string())` every 10s. Handle `Message::Text("PONG")`.

**Bug B — Wrong subscription type (critical — caused missing price_change events)**:
Old: `{"type": "Market", "assets_ids": [...]}` — uppercase "Market".
Docs spec: `{"type": "market"}` — lowercase.
Result: server silently accepted the connection and sent the initial `book` snapshot
(because `initial_dump` defaults to true), but never delivered `price_change` events.
Looked like "quiet market" when it was actually a broken subscription.

**Fix:** Change to lowercase `"market"` in the subscribe message.

**Bug C — Wrong timestamp units**:
Old `parse_ts_secs` multiplied by `1_000_000_000` (treating input as seconds).
Polymarket timestamps are milliseconds — should multiply by `1_000_000`.

---

### 12. Polymarket FV staleness — false positives on quiet markets
**Problem:** `POLY_FV_STALE_SECS = 30` (hardcoded constant). Polymarket only sends
`price_change` events when the book changes — quiet prediction markets can be silent
for minutes. This triggered false "stale FV" pauses, killing quoting on valid sessions.

**Also**: `fv_stale_secs (30) < RECV_TIMEOUT_SECS (60)`. The WS reconnect (which
delivers a fresh `book` snapshot) fires at 60s, AFTER the stale threshold fires at 30s.

**Fix (two parts):**

**Part 1 — PONG heartbeat re-publish**: on every TEXT `"PONG"` response, re-publish
the last known book state for all subscribed tokens. This resets `polymarket_mid_ts`
in the strategy every 10 seconds, decoupling "feed is alive" from "book changed recently".
A market quiet for 6 hours still gets PONG heartbeats → FV never stale.

**Part 2 — `fv_stale_secs` as a configurable param** (default 90):
Must be > `RECV_TIMEOUT_SECS` (60). This ensures the WS reconnect (which delivers
a fresh snapshot) always fires before the stale threshold.
In TOML: `fv_stale_secs = 90  # must be > 60`.

---

### 13. Pricing: venues diverged → zero-score orders placed
**Problem:** Arsenal market: `poly_mid=0.545`, `predict_mid=0.635`, divergence=0.09.
With `yes_bid = poly_mid - 0.02 = 0.525`:
- `|0.525 - 0.635| = 0.11 ≥ spread_threshold_v(0.06)` → `score_factor=0`, earns nothing
- `no_bid = 0.435 > no_ask_est(0.37)` → crossing guard → skipped

Placed one useless zero-score YES order, held capital with full fill risk.

**Root cause:** using raw poly_mid as FV anchor without a scoring constraint.

**First attempted fix (`fv_clamp_cents`):** clamp poly_mid to within N cents of predict_mid.
**Problem with that fix:** when `effective_fv ≈ predict_mid - spread_cents`, derived NO bid
= NO predict mid = immediate fill risk. Solved scoring but created fill risk.

**Correct fix: conservative dual-FV pricing** (see next entry).

---

### 14. Core design upgrade: conservative dual-FV pricing (2026-04-15)
**File:** `crates/strategies/prediction_quoter/src/pricing.rs`

**Design:** for each side, use the **more conservative** of the two FV signals:

```
YES: use min(poly_mid, predict_mid) — the lower YES mid, furthest from both bids
     yes_bid = min(poly_mid, predict_mid) - spread_cents

NO:  use 1 - max(poly_mid, predict_mid) — the lower NO mid, furthest from both NO bids
     no_bid = (1 - max(poly_mid, predict_mid)) - spread_cents
```

**Why**: a bid above either venue's mid gives that venue's participants immediate edge.
`min()`/`(1-max())` guarantees we're below BOTH mids — neither poly-informed nor
predict.fun-informed traders can fill us without paying at least `spread_cents` in edge.

**Scoring window gate**: if `|bid - predict_mid| ≥ spread_threshold_v`, **skip that side**.
Zero-score orders lock up capital with nonzero fill risk. Not worth placing.

**Arsenal example** (poly=0.545, predict=0.635, spread=0.02, v=0.06):
```
YES: min(0.545, 0.635) - 0.02 = 0.525
     |0.525 - 0.635| = 0.11 ≥ 0.06 → SKIP (can't earn points safely)

NO:  (1 - max(0.545, 0.635)) - 0.02 = (1-0.635) - 0.02 = 0.345
     |0.345 - 0.365| = 0.025 < 0.06 → PLACED, score_factor ≈ 31%
     safe: 2.5 cents from NO predict mid, 0.345 < no_ask_est(0.37) ✓
```

**Removed params:** `join_cents` (obsolete), `fv_clamp_cents` (replaced by min/max logic).
**New signature:** `pricing::calculate(yes_book, poly_fv, predict_mid, spread_cents, spread_threshold_v, decimal_precision)`.

**API call site change:**
```rust
// Old: single effective_fv (clamped)
pricing::calculate(book, effective_fv, spread_cents, decimal_precision)

// New: separate poly and predict signals
pricing::calculate(book, poly_fv, predict_mid, spread_cents, spread_threshold_v, decimal_precision)
```

**Small/no divergence case**: `min(poly, predict) ≈ poly` → uses poly anchor exactly.
**Large divergence (poly << predict)**: YES skipped (unsafe), NO placed at predict anchor.
**Large divergence (poly >> predict)**: YES placed at predict anchor, NO skipped.

---

### 15. farm.py improvements
**File:** `scripts/farm.py`

- Canonical config dir is `configs/markets_poly/` (legacy `configs/markets/` removed to avoid split state)
- `--dir` argument for flexibility
- `--dry-run` flag to preview without starting
- Crash-loop protection: 3 restarts in 120s → 5-minute backoff
- Writes `logs/farm/farm.pids` with `market_id=pid` entries for external management
- Periodic status line every 60s: `[farm] status: N/M running, K in backoff`
- Stagger startup remains (0.5s between markets) to avoid JWT auth collisions

---

### 16. Strict poly-token market selection + liquidation helper (2026-04-15)

**Commits:** `4b62a68`, `049401c`

**A) `predict-markets` strict guard**
- Added `--fail-on-missing-poly-token`.
- In strict mode, markets missing `polymarket_yes_token_id` are skipped in config writes and command exits non-zero.
- Why: avoid accidentally running unanchored markets when the strategy assumes poly-aware logic.

**B) `predict-liquidate` CLI**
- Added passive unwind command:
  - `trading-cli predict-liquidate --dry-run --config ...`
  - `trading-cli predict-liquidate --config ...`
- Behavior:
  - reads positions for configured market
  - computes passive SELL limits from current book
  - cancels existing open orders first (no-op in dry-run)
- Why: UI path for selling was unreliable during live ops; needed deterministic emergency tool.

---

### 17. Touch-trigger v1 failure mode: top-of-book trigger caused thrash (2026-04-15)

**Commit:** `3315ce0`

Initial idea: trigger defensive requote when quote reaches top-of-book (`touch_trigger_cents=0`).

Observed outcome in live logs:
- repeated `Near touch detected` on alternating book updates
- immediate `CANCEL_ALL -> Place -> CANCEL_ALL` loops
- high roundtrip churn with minimal incremental information

Root cause:
- top-bid proximity is too noisy as a hit-risk proxy in thin/fast books
- combined with hold-bypass, this can self-trigger continuously

Lesson:
- trigger signal quality matters more than just adding debounce after the fact

---

### 18. Touch-trigger v2: ask-distance risk + latch (current) (2026-04-15)

**Commit:** `eff1367`

Refinement:
1. Trigger on **ask-distance** (actual hit-risk proxy), not top-bid proximity.
2. Keep poly as quote anchor; touch logic is a risk cap/trigger only.
3. Add **risk latch** so a risk-zone entry triggers once per regime, not every tick.

New knobs:
- `touch_trigger_cents` (default 0.01)
- `touch_retreat_cents` (default 0.02)

Design intent:
- protect against imminent lift risk
- preserve score-eligible quoting behavior
- avoid pathological cancel/replace loops

Operational note:
- still monitor for over-churn in low-depth regimes; this is materially better than v1 but not the final form.

---

## Current strategy state (as of 2026-04-15)

### Pricing logic summary
1. **FV gate**: require fresh Polymarket FV before any quotes. No fallback.
2. **Per-side conservative anchoring**: `yes = min(poly,predict) - spread`, `no = (1-max(poly,predict)) - spread`
3. **Scoring window gate**: skip if `|bid - predict_mid| ≥ spread_threshold_v`
4. **Crossing guards**: skip if YES target ≥ YES ask; skip if NO target ≥ NO ask estimate
5. **No clamping to mid**: a side is skipped, never moved to mid

### Tuning knobs (all in TOML `[strategies.params]`)
- `spread_cents` — fill-risk knob. Score: `((v-s)/v)²`. 0.02=44%, 0.03=25%. Start at 0.02.
- `drift_cents` — requote trigger. Set = spread_cents. Lower → more churn.
- `fill_pause_secs` — cooldown after fill. Keep at 5 (maker fee=0, every second matters).
- `fv_stale_secs` — staleness timeout. **Must be > 60.** Default 90.
- `max_position_tokens` — circuit breaker per outcome. 500 tokens ≈ $175 max exposure at mid.
- `min_quote_hold_secs` — prevents drift thrashing. 10s is safe.

### Key invariants
- `spread_cents < spread_threshold_v` (to earn any points at all)
- `fv_stale_secs > 60` (WS recv timeout)
- Both sides are always at least `spread_cents` from BOTH venues' mids when placed

---

## Key facts / scoring reference

### Scoring formula
```
score_factor = ((v - spread_from_predict_mid) / v)²
est_contribution = score_factor × shares × time_on_book_secs
```
- `v` = `spread_threshold_v` from API (typically 0.06 for ±6¢ markets)
- `spread_from_predict_mid` = `|bid - predict_mid|` at placement time
- Outside `v` = zero points, order is skipped by design

| spread_from_predict_mid | score_factor (v=0.06) |
|---|---|
| 0.00 | 1.000 (100%) |
| 0.01 | 0.694 (69%) |
| 0.02 | 0.444 (44%) |
| 0.03 | 0.250 (25%) |
| 0.04 | 0.111 (11%) |
| 0.06 | 0.000 (0%) — never placed |

### Polymarket WS protocol facts (critical for any future changes)
- **PING/PONG**: TEXT messages, not WebSocket frames. Send `"PING"`, receive `"PONG"`.
- **Subscription type**: must be lowercase `"market"` in the `type` field.
- **Timestamps**: milliseconds (multiply by 1,000,000 for nanoseconds).
- **Events**: `book` (snapshot on subscribe + after fills), `price_change` (incremental).
- **Quiet market**: book may not change for minutes — normal, not a dead feed.
- **Heartbeat interval**: send PING every 10s per docs. We re-publish book on PONG.

### Position risk
- `N YES + N NO` = costs $N, pays $N regardless of outcome → zero directional risk when balanced
- Imbalance risk = `|yes_tokens - no_tokens| × avg_price`
- `max_position_tokens` circuit breaker per side. At 500 tokens and ~$0.35 avg price ≈ $175 max exposure

### Points distribution cadence
Weekly (every 7 days). 2–3 day calculation period before distribution.

---

## File locations

| What | Where |
|---|---|
| Per-market configs (poly-linked) | `configs/markets_poly/<market_id>.toml` |
| Multi-market launcher | `scripts/farm.py` |
| Farm process PIDs | `logs/farm/farm.pids` |
| Per-market farm logs | `logs/farm/<market_id>.log` |
| Points sessions log | `logs/data/points-sessions.jsonl` |
| BBO tick data | `logs/data/bbo-YYYY-MM-DD.jsonl` |
| Fill data | `logs/data/fills-YYYY-MM-DD.jsonl` |
| Main tracing log | `logs/obird-YYYY-MM-DD.jsonl` |

---

## Run instructions

```bash
cd /home/ubuntu/.openclaw/workspace/obird
source .env

# Build (always after code changes)
cargo build --release --bin trading-cli

# Regenerate configs (do this when new boosted markets appear)
./target/release/trading-cli predict-markets \
    --all --write-configs --fail-on-missing-poly-token \
    --output-dir configs/markets_poly

# Run all markets
python3 scripts/farm.py

# Single market (debug)
RUST_LOG=quoter=info,connector_predict_fun=info,connector_polymarket=debug \
  ./target/release/trading-cli live --config configs/markets_poly/143028.toml
```

### What healthy startup looks like

```
INFO connector_polymarket: Polymarket CLOB WS connected and subscribed
INFO quoter: Waiting for first Polymarket FV update before quoting
  ... (a few seconds for first poly book event) ...
INFO quoter: REQUOTE predict_mid=0.635 poly_fv=Some(0.545)
             yes_bid=None no_bid=Some(0.345) yes_placed=false no_placed=true
             poly_divergence=Some(0.0900) score_factor_no=0.3086
```

Red flags in logs:
- `REQUOTE poly_fv=None` → started quoting before poly connected (should never happen post-fix)
- `score_factor_yes=0` AND `score_factor_no=0` → large divergence, both sides skipped
- `Polymarket FV stale` more than once → genuine feed outage (check Polymarket status)
- `PLACE_FAILED` → check USDT balance, API key, or order validation issue
- Market restarting repeatedly → check `logs/farm/<id>.log` for root cause
