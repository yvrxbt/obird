# predict.fun Points Farming — Runbook

> Operations guide for the `prediction_quoter` + `predict_hedger` strategies.
> Companion to `RUNBOOK.md` (HL MM). Last updated: 2026-04-16.
>
> Before modifying any pricing or strategy logic, read (in order):
> 1. `PREDICT_QUOTING_DESIGN.md` — decision tree, FV logic, all tuning knobs
> 2. `PREDICT_FARMING_NOTES.md` — full build history and why things are designed as they are
> 3. `POLY_HEDGING_ARCHITECTURE.md` — hedge layer design, implementation, and Phase 3 roadmap
>
> **Current design summary**: conservative dual-FV pricing. YES anchored to `min(poly,predict)`,
> NO anchored to `1-max(poly,predict)`. Sides outside the scoring window are skipped.
> Polymarket FV is required — no fallback to predict.fun mid.
> Hedge strategy runs alongside quoter: predict fills → Polymarket opposite-side orders.

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

**Polymarket hedge prerequisite** (NOT yet confirmed done — check before running):
- Deposit USDC on Polygon at address `0xA27D22701Bf0f222467673F563e59aA0E38df847` (derived from `PREDICT_PRIVATE_KEY`)
- The SDK signs orders with this key — the on-chain maker address must hold USDC
- Without USDC balance, orders will be accepted by the CLOB (200 OK) but won't actually settle on-chain
- Auth uses `PREDICT_PRIVATE_KEY` only — no separate `POLY_API_KEY` / `POLY_SECRET` / `POLY_PASSPHRASE` needed

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

  ── configs/markets_poly/12345.toml ─────────────────────────
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

**Startup timing note**: The Polymarket WS initial book dump often arrives before the strategy task subscribes. You'll see `Waiting for first Polymarket FV update before quoting` on the first few predict.fun ticks. The PONG heartbeat (every 10s) re-publishes the last known book state — quoting starts within ~10s. This is normal.

**Red flags:**
- `REQUOTE poly_fv=None` → FV gate broken (should never happen; check quoter.rs)
- `ROUNDTRIP` every <200ms → drift_cents or min_quote_hold_secs too low
- `Polymarket FV stale` repeating → genuine feed outage (check Polymarket status page)
- `PLACE_FAILED` → USDT balance, API key, or order validation issue
- `ROUNDTRIP n_orders=0` at startup → normal (init CancelAll with no resting orders), not an error

---

## Hedge Operations (PredictHedgeStrategy)

### How the hedge activates

The hedge strategy co-runs with `PredictionQuoter` whenever both `polymarket_yes_token_id` AND `polymarket_no_token_id` are set in `[exchanges.params]` AND the three `POLY_*` env vars are exported. No new CLI flags needed.

Startup log sequence (healthy):
```
INFO  PolymarketExecutionClient ready address=0xA27D...  ← signing key loaded
INFO  Polymarket CLOB WS feed spawned yes_inst=... no_inst=...  ← WS subscribes both tokens
INFO  PredictHedgeStrategy initialized id=predict_points_v1_hedge markets=2 enabled=true
```

### Hedge log sequence (normal fill → hedge cycle)

```
# Predict fill triggers hedge accumulation
INFO quoter: HEDGE_TRIGGER predict_inst=PredictFun.Binary.143028-Yes
             poly_inst=Polymarket.Binary.2527312... fill_qty=50 fill_price=0.42

# Hedge plan computed (notional=50*0.58=29 USDC > min 5)
INFO quoter: HEDGE_PLAN poly_inst=Polymarket.Binary.2527312...
             hedge_qty=50.00 hedge_price=0.58 hedge_notional=29.00 urgent=false

# Order lands on Polymarket CLOB
INFO quoter: POLY_PLACE instrument=Polymarket.Binary.2527312...
             order_id=0xabc... side=Buy price=0.58 qty=50.00 status=matched

# Fill confirmed (Polymarket WS fill or PlaceFailed — see below)
INFO quoter: HEDGE_FILL confirmed poly_inst=... filled_qty=50 fill_price=0.58
```

### Hedge skip reasons and actions

| Log | Cause | Action |
|-----|-------|--------|
| `HEDGE_SKIP no poly book` | NO token WS feed not delivering updates | Check `Polymarket CLOB WS feed spawned` at startup; verify NO token ID is correct |
| `HEDGE_SKIP Polymarket spread too wide` | poly bid-ask spread wider than `max_slippage_cents` (unusual — Polymarket is normally tight) | Check poly market liquidity; `max_slippage_cents=0.05` allows 5 ticks of spread, should never trigger normally |
| `HEDGE_SKIP poly ask above 0.99` | Market near resolution or illiquid | Nothing; skip is correct |
| `HEDGE_SKIP qty < 1 share` | Too small to place | Normal batching; next fill will accumulate |
| `HEDGE_BATCH not enough notional yet` | Below `hedge_min_notional` (5 USDC) | Normal; waiting for more fills |
| `HEDGE_REJECT placement failed` | `POLY_PLACE` returned error | Check `POLY_*` env vars, USDC balance, API key validity |
| `HEDGE_URGENT time threshold breached` | >60s of unhedged position | Normal escalation; hedge should fire even if below min_notional |

### Disabling hedge at runtime

No runtime kill-switch is wired yet (Phase 3). To disable:
1. Remove `polymarket_no_token_id` from the market config
2. Restart the bot

Or: unset `POLY_API_KEY` from env — the `PolymarketExecutionClient::from_env` will fail gracefully and hedge will be disabled with a warning log.

### Checking hedge effectiveness

```bash
# All hedge actions in a session
grep -E "HEDGE_TRIGGER|HEDGE_PLAN|HEDGE_FILL|HEDGE_REJECT|HEDGE_SKIP" logs/farm/143028.log

# Hedge fill rate (plan vs reject)
grep HEDGE_PLAN logs/farm/143028.log | wc -l
grep HEDGE_REJECT logs/farm/143028.log | wc -l

# What slippage we're seeing
grep HEDGE_PLAN logs/farm/143028.log | grep -o "hedge_price=[0-9.]*"

# Poly orders placed
grep POLY_PLACE logs/farm/143028.log
```

### Token IDs for market 143028

```
predict YES: 88632176792205708175552212115019750624663026701991425037794038217700051469304
predict NO:  100760647293882693638751365626138407657360538060017390664563598193145574423450
poly YES:    8501497159083948713316135768103773293754490207922884688769443031624417212426
poly NO:     2527312495175492857904889758552137141356236738032676480522356889996545113869
```

Hedge mapping for this market:
```
predict YES fill → buy poly NO  (token: 2527312...)
predict NO fill  → buy poly YES (token: 8501497...)
```

### Adding hedge to a new market

1. Run `predict-markets --write-configs` — now writes both `polymarket_yes_token_id` AND `polymarket_no_token_id` if the Gamma API resolves the market
2. Ensure `POLY_API_KEY`, `POLY_SECRET`, `POLY_PASSPHRASE` are in `.env`
3. Start the bot — hedge auto-enables

If `predict-markets` can't resolve the poly token IDs:
```bash
# Manual: get both token IDs from any known token ID
curl "https://gamma-api.polymarket.com/markets?clob_token_ids=<yes_or_no_token_id>" \
  | python3 -c "import sys,json; m=json.load(sys.stdin)[0]; print(json.loads(m['clobTokenIds']))"
# Returns: ['<yes_token_id>', '<no_token_id>']
```

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
   If a position builds above max, that side stops quoting automatically.
   Use `predict-liquidate` to sell down, or raise `max_position_tokens` in config.

7. **Position state at startup**: The quoter reads existing YES/NO token balances from `positions()` on startup. If you restarted after heavy fills, your position will already be partially maxed. Market 143028 example: YES=724 tokens at start of 2026-04-16 session — only NO bids placed.

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

### Hedge Phase 3 items

**Passive maker pricing** (not yet built):
- Currently: buy at `best_ask` (taker, immediate fill, guaranteed execution)
- Phase 3: buy at `best_bid + 1 tick` (passive maker, lower cost, may not fill)
- Tier selection: based on `unhedged_notional / max_unhedged_notional` ratio
- When Tier A (passive) fails to fill within timeout, escalate to Tier B/C

**HedgeParams TOML wiring** (not yet built):
- Params are currently hardcoded defaults in `HedgeParams::default()`
- Phase 3: add `[hedge]` section to market TOML and deserialize via `strategy_cfg.params`
- Fields: `enabled`, `hedge_min_notional`, `max_unhedged_notional`, `max_unhedged_duration_secs`, `max_slippage_cents`

**Polymarket user WS feed** (not yet built):
- Currently: optimistic position tracking (assume fill on placement)
- Phase 3: subscribe to `wss://ws-subscriptions-clob.polymarket.com/ws/user` with API credentials
- Receives real-time `order` and `trade` events for authenticated user
- Would enable real-time hedge confirmation instead of optimistic accounting

**Hedge ledger** (not yet built):
- Append-only JSONL at `logs/data/hedges-YYYY-MM-DD.jsonl`
- One record per hedge attempt: trigger reason, predict fill price, poly ask price, slippage, outcome
- Daily `HEDGE_SUMMARY` log with: predict exposure, hedged fraction, avg slippage, net MTM

**Kill-switch** (not yet wired to TOML):
- `HedgeParams.enabled = false` exists in code
- Phase 3: wire to `[hedge] enabled = false` in TOML; read at startup like other params
- Workaround: remove `polymarket_no_token_id` from config and restart

### Enhancement: Polymarket quoting (separate from hedging)
**Goal**: Same farming strategy on Polymarket for USDC rewards ($5M/month pool, April 2026).
**Architecture**: Polymarket execution connector is now built (as hedge layer). Adapting it for full quoting requires:
- `PolymarketQuoter` strategy (copy `PredictionQuoter` pattern, wire Polymarket book as FV)
- Position tracking via user WS feed
- Polymarket rewards API to verify scoring eligibility
**Reward formula**: Same quadratic scoring as predict.fun (predict.fun copied Polymarket).
Priority: after multi-market + hedge ledger are working.

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

Dispatches on `strategy_type` field in config.

`prediction_quoter` path (updated 2026-04-16):
1. Always: `PredictFunClient + PredictFunMarketDataFeed + PredictionQuoter`
2. If `polymarket_yes_token_id` set: adds YES token to `PolymarketMarketDataFeed`
3. If BOTH poly token IDs set:
   - Adds NO token to same WS connection (single feed handles both)
   - Builds `PolymarketExecutionClient::from_env("POLY_API_KEY", "POLY_SECRET", "POLY_PASSPHRASE", secret_key_env)`
   - Builds `PredictHedgeStrategy` with `MarketMapping` for this market
   - Registers `Exchange::Polymarket` in `connectors` HashMap
   - Appends hedger to `strategies` Vec → both run in same `EngineRunner`
   - If `from_env` fails (missing env var): logs warning, continues farming-only
4. `EngineRunner::new(connectors, strategies, md_bus)` — both strategies share the bus

`hl_spread_quoter` → existing HL path (unchanged)
