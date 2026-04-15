# predict.fun Points Farming — Runbook

> Operations guide for the `prediction_quoter` strategy.
> Companion to `RUNBOOK.md` (HL MM). Last updated: 2026-04-15.
>
> Before modifying any pricing or strategy logic, read (in order):
> 1. `PREDICT_QUOTING_DESIGN.md` — decision tree, FV logic, all tuning knobs
> 2. `PREDICT_FARMING_NOTES.md` — full build history and why things are designed as they are
> 3. `POLY_HEDGING_ARCHITECTURE.md` — planned hedge layer and risk model direction
>
> **Current design summary**: conservative dual-FV pricing. YES anchored to `min(poly,predict)`,
> NO anchored to `1-max(poly,predict)`. Sides outside the scoring window are skipped.
> Polymarket FV is required — no fallback to predict.fun mid.

---

## Quick Start

```bash
cd /home/ubuntu/.openclaw/workspace/obird
source .env
cargo build --release --bin trading-cli

# Run all poly-linked markets
python3 scripts/farm.py

# Or single market (for debugging)
RUST_LOG=quoter=info,connector_predict_fun=info,connector_polymarket=info \
  ./target/release/trading-cli live --config configs/markets_poly/21177.toml
```

Ctrl+C for graceful shutdown — cancels all resting orders before exit.

**Rebuild required after any code change**: `cargo build --release --bin trading-cli`

---

## One-Time Setup (already done on this wallet)

```bash
# Set all 4 on-chain USDT + ERC-1155 approvals (covers all contract variants)
source .env && cargo run --bin trading-cli -- predict-approve --all --config configs/predict_quoter.toml
```

Contracts approved on `0xA27D22701Bf0f222467673F563e59aA0E38df847`:
- Standard CTFExchange (`0x8BC0...`) — ERC-1155 + USDT ✓
- YieldBearing CTFExchange (`0x6bEb...`) — ERC-1155 + USDT ✓
- NegRisk CTFExchange (`0x365f...`) — ERC-1155 + USDT + NegRiskAdapter ✓
- YieldBearing NegRisk CTFExchange (`0x8A28...`) — ERC-1155 + USDT + NegRiskAdapter ✓

**Only needs to be done once per wallet. BNB gas was consumed.**

---

## Strategy Params Reference

Config lives in each `configs/markets_poly/<market_id>.toml`. All fields with defaults
are optional — auto-filled by `predict-markets --write-configs`.

```toml
[strategies.params]
spread_cents         = "0.02"   # distance from conservative FV anchor per side
                                 # fills: 0.01→high, 0.02→moderate (default), 0.03→low
                                 # score: ((v-s)/v)² — 0.01→69%, 0.02→44%, 0.03→25%
order_size_usdt      = "65"     # USDT per side → shares = order_size_usdt / bid_price
                                 # must satisfy: shares ≥ min_shares_per_side
drift_cents          = "0.02"   # pull+requote if poly FV moves > this from last quoted
touch_trigger_cents  = "0.01"   # defensive requote when bid gets this close to ask
touch_retreat_cents  = "0.02"   # on touch trigger, push bid back by this much from ask
min_quote_hold_secs  = 10       # min seconds on book before drift-triggered cancel
fill_pause_secs      = 5        # cooldown after fill (maker fee=0, keep short)
fv_stale_secs        = 90       # MUST be > 60 (WS recv timeout). PONG every 10s resets.
max_position_tokens  = "500.0"  # circuit breaker per outcome (~$175 at mid=0.35)
spread_threshold_v   = "0.06"   # auto-filled — market's ±v scoring window
min_shares_per_side  = "100"    # auto-filled — minimum qualifying order size
```

**Auto-fetched at startup (not in TOML):**
- `decimal_precision` — from `GET /v1/markets/{id}`. Precision=2 → 0.01 tick, precision=3 → 0.001 tick.

**Removed params (no longer exist):**
- `join_cents` — was manual fallback join depth. Replaced by skip-if-crossing logic.
- `fv_clamp_cents` — was FV clamp. Replaced by per-side min/max FV logic.

**Scoring formula:** `score = ((v - |bid - predict_mid|) / v)² × shares × time_on_book`

---

## CLI Commands

```bash
# Smoke test — verify auth, WS, pricing for a given market
source .env && RUST_LOG=info cargo run --bin trading-cli -- predict-check
source .env && RUST_LOG=info PREDICT_MARKET_ID=21177 cargo run --bin trading-cli -- predict-check

# Discover currently boosted markets + auto-write TOML configs
source .env && cargo run --bin trading-cli -- predict-markets
source .env && cargo run --bin trading-cli -- predict-markets --all   # include non-boosted
source .env && cargo run --bin trading-cli -- predict-markets --write-configs
source .env && cargo run --bin trading-cli -- predict-markets --all --write-configs --fail-on-missing-poly-token

# Passive position unwind helper (dry-run first)
source .env && cargo run --bin trading-cli -- predict-liquidate --dry-run --config configs/markets_poly/143028.toml

# On-chain approval setup (one-time per wallet)
source .env && cargo run --bin trading-cli -- predict-approve --all --config configs/predict_quoter.toml

# Live farming
source .env && RUST_LOG=quoter=info,connector_predict_fun=info cargo run --release --bin trading-cli -- live --config configs/predict_quoter.toml
```

---

## Running All Markets (Multi-Market Farm)

### Start

```bash
cd /home/ubuntu/.openclaw/workspace/obird
source .env
cargo build --release --bin trading-cli   # ensure binary is current
python3 scripts/farm.py                   # starts all configs/markets_poly/*.toml
```

Each market gets its own process and log file. The script monitors children
and restarts on unexpected exits (with exponential backoff for crash loops).

### Status

```bash
# What's running (PIDs written at startup, updated on restart)
cat logs/farm/farm.pids

# Periodic status is printed to stdout every 60s:
#   [farm] status: 11/11 running, 0 in backoff
```

### Logs

```bash
# One market
tail -f logs/farm/143028.log

# All markets interleaved (shows market_id in each line via tracing)
tail -f logs/farm/*.log

# Filter for fills only
grep FILL logs/farm/*.log

# Filter for any skipped sides (scoring window or crossing)
grep "skipped" logs/farm/*.log
```

### Stop

```bash
Ctrl-C          # graceful: SIGTERM to all children, waits 15s for order cancels, then SIGKILL stragglers
kill -TERM <pid_of_farm.py>   # same behaviour from outside the terminal
```

### Add/remove a market

```bash
# Regenerate all poly-linked configs from the API
source .env
cargo run --release --bin trading-cli -- predict-markets \
    --all --write-configs --fail-on-missing-poly-token \
    --output-dir configs/markets_poly

# Restart farm to pick up changes
# (Ctrl-C the running farm first, then re-run farm.py)
```

### Crash-loop protection

If a market restarts more than 3 times within 120 seconds, the farm backs off
for 5 minutes before trying again. You'll see:

```
[farm] 143028 crash-looping (3 restarts in 120s) — backing off 300s
```

Check `logs/farm/143028.log` for the root cause (auth failure, API error, etc.)
before the backoff expires.

---

## Switching Markets

Run `predict-markets --write-configs` to find active boosts and write full TOML files automatically:

```bash
source .env && cargo run --bin trading-cli -- predict-markets --write-configs
```

Output example (when a boost is active):
```
=== CURRENTLY BOOSTED MARKETS (1) ===

  id=12345 "Arsenal vs Man City — Winner"
  boosted=true boost_ends=2026-04-15T21:00:00Z (~90m remaining)
  isNegRisk=false isYieldBearing=false feeRateBps=200 decimalPrecision=3
  book: bid=0.48 ask=0.52 bid_depth=5000 ask_depth=4800
  outcomes:
    [0] Yes → 12345...abcd
    [1] No  → 98765...efgh

  ── configs/markets/12345.toml ──────────────────────────────
  [exchanges.params]
  market_id        = 12345
  yes_outcome_name = "Yes"
  yes_token_id     = "12345...abcd"
  no_outcome_name  = "No"
  no_token_id      = "98765...efgh"
  is_neg_risk      = false
  is_yield_bearing = false
  fee_rate_bps     = 200

  instruments = ["PredictFun.Binary.12345-Yes", "PredictFun.Binary.12345-No"]

  [strategies.params]
  order_size_usdt     = "55"   # covers 100-share min at current mid
  spread_threshold_v  = "0.06" # auto-filled from API (no UI hover needed)
  min_shares_per_side = "100"  # auto-filled from API
  ────────────────────────────────────────────────────────────
```

**No more manual surgery.** `spread_threshold_v`, `min_shares_per_side`, and `polymarket_yes_token_id` (when resolvable) are auto-filled. `decimal_precision` and tick size are fetched at startup — not in TOML.

After updating config, restart the bot. The new market is live within seconds.

**NegRisk markets** (football multi-outcome, e.g., Liverpool/Draw/PSG): each outcome
is a separate `market_id` with its own `[[exchanges]]` block. Use the same config
format but set `is_neg_risk = true`. Approvals are already done for negRisk contracts.

---

## Log Interpretation

```
# Startup: wait for Polymarket FV before quoting
INFO quoter: Waiting for first Polymarket FV update before quoting

# Normal requote (both sides placed, small divergence)
INFO quoter: REQUOTE strategy=predict_points_v1
             predict_mid=0.600 poly_fv=Some(0.590) poly_divergence=Some(0.0100)
             yes_fv_used=0.590 no_fv_used=0.410
             yes_bid=Some(0.57) no_bid=Some(0.39)
             yes_placed=true no_placed=true n_orders=2
             score_factor_yes=0.25 score_factor_no=0.25

# Large divergence (Arsenal: poly=0.545, predict=0.635)
INFO quoter: REQUOTE predict_mid=0.635 poly_fv=Some(0.545) poly_divergence=Some(0.0900)
             yes_bid=None no_bid=Some(0.345)    ← YES skipped (outside scoring window)
             yes_placed=false no_placed=true n_orders=1

# Fill + cooldown
INFO quoter: FILL instrument=...-$60k side=Buy price=0.57 qty=114.03 fill_count=1
INFO quoter: PULL_QUOTES reason=fill pause_secs=5
INFO quoter: COOLDOWN_EXPIRED  → back to Empty, requotes on next tick

# Per-cycle scoring estimate (on each cancel or fill)
INFO quoter: CYCLE_END ended_by=fill on_book_secs=14.2
             yes_placed=true no_placed=true
             yes_spread=0.03 no_spread=0.03
             score_factor_yes=0.25 score_factor_no=0.25
             est_yes_score=485.4 est_no_score=321.6
```

**Key log fields:**
- `poly_fv=None` before first poly update → `Waiting for...` gate active (correct)
- `poly_fv=None` after startup → staleness paused quoting (check feed)
- `poly_divergence=Some(0.09)` → large divergence, expect one side skipped
- `yes_bid=None` or `no_bid=None` → skipped by scoring-window or crossing guard
- `score_factor_X=0` → bid outside `spread_threshold_v` (should never happen post-fix)
- `fv_clamped=true` → poly FV exceeded clamp (removed; clamp now implicit via min/max)

**Healthy behavior**: REQUOTE once per 10-30s, fills occasionally, ROUNDTRIP < 500ms.

**Red flags:**
- `REQUOTE poly_fv=None` → FV gate broken (should never happen; check quoter.rs)
- `ROUNDTRIP` every <200ms → drift_cents or min_quote_hold_secs too low
- `Polymarket FV stale` repeating → genuine feed outage (check Polymarket status page)
- `PLACE_FAILED` → USDT balance, API key, or order validation issue

---

## Points Farming Meta

Based on community research (X/Twitter, Grok research, April 2026):

1. **Boosted markets are the priority** — up to 6 active at once, each for a few hours.
   Sports/esports: Champions League, NBA, UFC, CS2, LoL are most common.
   Run `predict-markets` at session start and whenever a boost might have appeared.

2. **Both sides required** — YES + NO bids needed for full score. Single-sided earns 1/3.

3. **Tight quotes score better** (quadratic) — 1 cent spread beats 2 cent spread by ~2×.
   Don't go below the market's tick size (0.01 for precision=2, 0.001 for precision=3 — auto-detected).

4. **Don't churn** — orders scored by random sampling every minute. Stable resting orders
   accumulate more samples than rapidly-cancelled ones.

5. **min_quote_hold_secs = 10** keeps orders on the book long enough to score.

6. **Fills are free (0 maker fee)** — accumulating tokens is the only risk.
   With max_position_tokens=500, exposure is capped at ~$180/outcome.

---

## Known Issues / Next Steps

### Bug: Fills from cancelled orders occasionally mismatch instrument
**Status**: Mitigated. `placed_instruments` map survives `cancel_all()`.
Unknown-hash fills now warn and default to YES instrument. Should be rare.
**Future fix**: TTL-based cleanup of `placed_instruments` after 60s.

### Missing: Multi-market support in one process
**Problem**: `HashMap<Exchange, Box<dyn ExchangeConnector>>` uses `Exchange` enum as key.
Two PredictFun markets can't coexist — second would overwrite the first.
**Fix needed**: Change key to `(Exchange, String)` (exchange + market_id) in
`crates/engine/src/order_router.rs` and `EngineRunner`. Then the config can have
multiple `[[exchanges]]` blocks with `name = "predict_fun_21177"` etc.
**Workaround**: Run separate processes for separate markets (different `.env` or different ports).

### Missing: Auto-switch to boosted markets
**Problem**: Bot stays on configured market even if a boost starts elsewhere.
**Fix needed**: A polling task that calls `get_markets_filtered("OPEN")` every 5 min,
detects `is_boosted=true && boostEndsAt > now`, and triggers a graceful market switch
(cancel-all → reconfigure → re-subscribe).
**Workaround**: Run `predict-markets` manually, update config, restart bot.

### Missing: Boost-aware market ranking
**Problem**: `predict-markets` shows depth but doesn't rank by expected PP yield.
**Enhancement**: Use `spreadThreshold` as proxy for scoring window `v`. Score = `((v-spread)/v)² × depth`.
Pre-rank markets by expected score to tell you which boosted market to prioritize.

### Missing: NegRisk multi-outcome quoting
**Problem**: A 3-outcome negRisk market (Liverpool/Draw/PSG) is 3 separate `market_id`s.
Currently requires 3 separate bot instances.
**Fix needed**: Multi-market support (see above) + NegRiskAdapter position conversion logic.

### Enhancement: Polymarket integration
**Goal**: Same strategy on Polymarket for USDC rewards ($5M/month pool, April 2026).
**Architecture**: Copy `connector_predict_fun` pattern. Polymarket uses:
- CLOB API (off-chain order matching, similar to predict.fun)
- CTF contracts on Polygon
- Same EIP-712 order signing
- REST API: `https://clob.polymarket.com`
- WS feed for orderbook
**Reward formula**: Same quadratic scoring as predict.fun (predict.fun copied Polymarket).
**Min incentive size** and **max incentive spread** are per-market params from the CLOB API.
Priority: after multi-market is working on predict.fun.

---

## Architecture Notes

### Connector design (`crates/connectors/predict_fun/`)
- **One `PredictFunClient` per market** (covers YES + NO outcomes)
- Both outcomes are placed as `Side::Buy` — BUY YES at P, BUY NO at 1-P
- `cancel_all()` cancels all tracked OIDs across both outcomes in one REST call
- `placed_instruments: Arc<Mutex<HashMap<hash, InstrumentId>>>` — survives cancel_all
- `active_orders: Arc<Mutex<HashMap<hash, OrderEntry>>>` — cleared on cancel_all
- NegRisk handled transparently: `is_neg_risk` flag selects the correct EIP-712 contract

### Strategy design (`crates/strategies/prediction_quoter/`)
- State machine: `Empty → Quoting → Cooldown(Instant)` (mirrors HlSpreadQuoter)
- `pricing::calculate()` enforces 2dp price rounding + crossing guards
- Tracks `yes_tokens` and `no_tokens` separately (not net position)
- `min_quote_hold_secs` prevents thrashing on volatile books
- Fill pause bypasses hold time (fills always trigger immediate cancel)

### Price constraints
- `decimalPrecision` is a per-market property (either 2 or 3) returned by `GET /v1/markets/{id}`
- Fetched once at connector startup; precision=2 → 0.01 tick, precision=3 → 0.001 tick
- `pricing::calculate()` uses the market's tick for the narrow-spread guard, all clamps, and rounding
- `spread_cents` can be any value; the pricing function clamps to valid ticks automatically
- If BBO spread < 2 ticks (e.g. 1-cent spread on a precision=2 market), the cycle is skipped with a log message

### Engine wiring (`crates/cli/src/live.rs`)
- Dispatches on `strategy_type` field in config
- `prediction_quoter` → `PredictFunClient + PredictFunMarketDataFeed + PredictionQuoter`
- `hl_spread_quoter` → existing HL path (unchanged)
