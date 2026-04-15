# predict.fun Points Farming — Runbook

> Operations guide for the `prediction_quoter` strategy.
> Companion to `RUNBOOK.md` (HL MM). Last updated: 2026-04-15.
>
> **Quoting strategy design (decision tree, Polymarket mid signal, join logic):**
> See `PREDICT_QUOTING_DESIGN.md` — read before modifying any pricing or strategy logic.

---

## Quick Start

```bash
cd /home/ubuntu/.openclaw/workspace/obird
source .env

# 1. Find active boosted markets (do this first every session)
cargo run --bin trading-cli -- predict-markets --write-configs

# 2. Pick the generated config in configs/markets/<id>.toml (includes Polymarket token ID when available)
# 3. Run the farming bot
RUST_LOG=quoter=info,connector_predict_fun=info cargo run --release --bin trading-cli -- live --config configs/markets/21177.toml
```

Ctrl+C for graceful shutdown — cancels all resting orders before exit.

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

## Current Config (`configs/predict_quoter.toml`)

**Market**: "Will Bitcoin hit $60k or $80k first?" (id=21177)
- Non-negRisk, yield-bearing, 200 bps fee
- Selected for: tightest spread + highest orderbook depth among non-negRisk open markets
- `$60k` = YES instrument, `$80k` = NO instrument

**Strategy params (per-market TOML):**
```
spread_cents        = 0.02   # place YES bid 2 cents below mid (score factor ~44% at v=0.06)
order_size_usdt     = 70.0   # $70 per outcome → ~200 YES shares, ~107 NO shares (≥100 min)
drift_cents         = 0.02   # requote when mid drifts > 2 cents
min_quote_hold_secs = 10     # hold quotes at least 10s before drift-triggered cancel
fill_pause_secs     = 5      # cooldown after any fill
max_position_tokens = 300.0  # max token exposure per outcome
spread_threshold_v  = 0.06   # from API (auto-filled by predict-markets CLI)
min_shares_per_side = 100    # from API (auto-filled by predict-markets CLI)
```

**Not in TOML (auto-fetched):**
- `decimal_precision` — fetched from `GET /v1/markets/{id}` at startup. Determines minimum tick (0.01 for precision=2, 0.001 for precision=3). Stored in connector, propagated to strategy via `StrategyState`.

**Points scoring context:**
- Polymarket-style quadratic scoring: `S = ((v - spread) / v)² × size`
- `v` = `spreadThreshold` per market (market 21177 has `spreadThreshold = 0.06`)
- At spread=0.01: `((0.06-0.01)/0.06)² = 0.69` (69% of max score)
- Two-sided (YES + NO) earns `min(Q_one, Q_two)` — required for full score outside [0.10, 0.90]
- **Makers pay ZERO fee** — fills are free, only directional exposure risk

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

# On-chain approval setup (one-time per wallet)
source .env && cargo run --bin trading-cli -- predict-approve --all --config configs/predict_quoter.toml

# Live farming
source .env && RUST_LOG=quoter=info,connector_predict_fun=info cargo run --release --bin trading-cli -- live --config configs/predict_quoter.toml
```

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
INIT existing YES position  yes_tokens=30.302     # position loaded from exchange on startup
REQUOTE  mid=0.355 yes_bid=0.35 no_bid=0.65       # placed 2 orders (cancel-all + place)
         yes_qty=28.57 no_qty=15.38 n_orders=2
ROUNDTRIP cancel_ms=111 place_ms=254 total_ms=365  # cycle latency (should be <500ms US-East)
FILL  instrument=...$60k price=0.35 qty=28.57      # YES order hit by taker (we get filled)
      yes_tokens=58.87 session_cost=19.99
PULL_QUOTES reason=fill pause_secs=5               # cooldown after fill
COOLDOWN_EXPIRED                                   # back to empty, will requote on next tick
```

**Healthy behavior**: REQUOTE once per 10-30s, FILL occasionally, ROUNDTRIP < 500ms.

**Unhealthy behavior**:
- `PLACE_FAILED` → check API key, USDT balance, or order precision issue
- `ROUNDTRIP` every 300ms → drift_cents or min_quote_hold_secs too low
- No `REQUOTE` for >60s → book may be empty or WS disconnected (will auto-reconnect)

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
