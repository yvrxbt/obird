# Prediction Market Farming — predict.fun + Polymarket Hedge

> Canonical reference for points farming on predict.fun with a Polymarket hedge layer.
> Covers pricing strategy, decision tree, hedge logic, operations, and tuning.
> Companion docs: `DEX_CEX_MM.md` (HL MM), `PRD_FARMING_PLATFORM.md` (v2 target).

---

## 1. Overview

Two strategies run in the same engine process per predict.fun market:

1. **`PredictionQuoter`** — the quoting strategy. Places conservative dual-BUY quotes on predict.fun (YES and NO), priced against the *more conservative* of Polymarket and predict.fun mids. Optimized for points yield (quadratic scoring window).
2. **`PredictHedgeStrategy`** — the hedge strategy. Reacts to predict.fun fills by placing opposite-side taker orders on Polymarket. `predict YES fill → buy poly NO`, `predict NO fill → buy poly YES`. Keeps delta neutral at resolution.

**Core invariants:**
- Quote around Polymarket mid (deeper book); never blindly quote at predict.fun mid.
- If Polymarket FV is unavailable or stale, pause quoting entirely — no fallback.
- Each side is placed at `spread_cents` below the conservative anchor, or skipped. Never clamped to mid.
- `yes_bid + no_bid < 1.00` when both sides placed → profitable regardless of outcome if both fill.
- Hedge is purely reactive; farming continues even if hedge fails.

---

## 2. Pricing Strategy

### 2.1 Guiding principle

Polymarket is the deeper, more liquid venue for the same binary events. Using its mid as fair value (FV) means our quotes reflect true market consensus rather than a stale or wide predict.fun book. If the Polymarket feed is unavailable, the strategy pauses quoting rather than falling back to predict.fun mid — blind quoting against informed Polymarket participants is a reliable way to get filled at bad prices.

### 2.2 Conservative dual-FV anchoring

Each side uses the **more conservative** of the two FV signals:

```
YES anchor = min(poly_mid, predict_mid)          # lower YES mid, furthest from both bids
NO  anchor = 1 - max(poly_mid, predict_mid)      # lower NO mid, furthest from both NO bids

yes_bid = YES anchor - spread_cents
no_bid  = NO  anchor - spread_cents
```

**Why "conservative":** a bid above either venue's mid gives that venue's informed traders immediate edge. `min()` / `(1-max())` guarantees we're below BOTH mids on each side — neither a poly-informed nor a predict-informed trader can fill us without paying at least `spread_cents` in edge.

**Scoring-window skip:** if `|bid - predict_mid| ≥ spread_threshold_v`, skip that side. A zero-score order still locks up capital and carries fill risk.

### 2.3 Decision tree

Matches `crates/strategies/prediction_quoter/src/pricing.rs`.

```
INPUT: poly_fv, predict_mid, predict.fun YES BBO, decimal_precision

Step 0 — FV gate
  if poly_fv is None            → return (wait for first poly update)
  if poly_fv stale > fv_stale_secs → pull quotes, return

Step 1 — Compute YES bid
  yes_fv     = min(poly_fv, predict_mid)
  yes_target = yes_fv - spread_cents
  yes_target = min(yes_target, yes_ask_mkt - touch_retreat_cents)   # touch-retreat
  if |predict_mid - yes_target| >= spread_threshold_v:
      yes_target = predict_mid - (spread_threshold_v - tick)        # clamp inside window
  if yes_target >= yes_ask_mkt:
      yes_target = yes_ask_mkt - tick                               # crossing guard
  yes_target = round_down(yes_target, decimal_precision)
  yes_bid    = SKIP if yes_target >= yes_ask_mkt else clamp(yes_target)

Step 2 — Compute NO bid (symmetric)
  no_fv      = 1 - max(poly_fv, predict_mid)
  no_ask_est = 1 - yes_bid_mkt
  no_target  = no_fv - spread_cents
  # scoring-window clamp + crossing guard (no clamp-to-ask on NO, just SKIP)
  no_bid     = SKIP if would cross else clamp(round_down(no_target))

Step 3 — Position gate
  if yes_tokens >= max_position_tokens: yes_bid = SKIP
  if no_tokens  >= max_position_tokens: no_bid  = SKIP

Step 4 — Place
  if both sides SKIP: log, return no actions
  else: CancelAll + PlaceOrder for each surviving side
```

### 2.4 Arsenal divergence example (illustrative)

```
poly=0.545, predict=0.635, spread=0.02, v=0.06:

YES: min(0.545, 0.635) - 0.02 = 0.525
     |0.525 - 0.635| = 0.11 ≥ 0.06 → SKIP (outside scoring window)

NO:  (1 - max(0.545, 0.635)) - 0.02 = 0.345
     |0.345 - 0.365| = 0.025 < 0.06 → PLACED, score_factor ≈ 31%
     no_ask_est = 1 - 0.63 = 0.37 → 0.345 < 0.37 → safe ✓
```

YES skipped because earning points requires placing closer to predict_mid, which exposes us to poly-informed adverse selection. NO placed safely.

### 2.5 State machine

`Empty → Quoting → Cooldown(Instant) → Empty → …`

- Fill → `Cooldown(fill_pause_secs)`
- Drift on FV > `drift_cents` after `min_quote_hold_secs` → requote
- Touch trigger (resting bid within `touch_trigger_cents` of ask) → defensive requote, latched per risk-regime entry

### 2.6 Crossing guards summary

| Condition | Guard | Consequence if missed |
|---|---|---|
| `yes_bid >= yes_ask_mkt` (before round) | Clamp to `ask - tick` | Immediate fill vs resting YES seller |
| `yes_bid >= yes_ask_mkt` (after round) | Skip YES | Book too tight |
| `no_bid >= 1 - yes_bid_mkt` | Skip NO | Immediate fill vs resting NO seller |
| `|bid - predict_mid| >= spread_threshold_v` | Clamp inside window | Order earns 0 points |

**What is NOT a crossing condition (common mistake):** `yes_bid <= yes_bid_mkt`. Below-bid is a valid resting maker order — fills only if a taker walks the book down to us.

### 2.7 Tuning knobs (`[strategies.params]`)

| Knob | Default | Purpose |
|---|---|---|
| `spread_cents` | `"0.02"` | Distance from conservative FV anchor per side. Score factor `((v-s)/v)²`: `0.01→69%`, `0.02→44%`, `0.03→25%`. Must be `< spread_threshold_v`. |
| `order_size_usdt` | `"65"` | USDT per side. Must satisfy `order_size_usdt ≥ min_shares_per_side × max(yes_px, no_px)`. At mid ≤ 0.65, need `≥ 65`. |
| `drift_cents` | `"0.02"` | Requote when FV moves this much from last quote. Set `= spread_cents`. |
| `touch_trigger_cents` | `"0.01"` | Defensive requote when bid within this distance of ask. |
| `touch_retreat_cents` | `"0.02"` | On touch trigger, push bid back this much from ask. |
| `min_quote_hold_secs` | `10` | Min time on book before drift-triggered cancel. Prevents thrashing. |
| `fill_pause_secs` | `15` | Cooldown after any fill (anti-toxic-flow). Raised from 5 after live testing. |
| `fv_stale_secs` | `90` | **Must be > 60** (WS recv timeout). PONG every 10s resets this. |
| `max_position_tokens` | `"500.0"` | Circuit breaker per outcome (~$175 at mid 0.35). |
| `spread_threshold_v` | `"0.06"` | Auto-filled from predict.fun API (market's ±v scoring window). |
| `min_shares_per_side` | `"100"` | Auto-filled from API (minimum qualifying order size). |

**Removed params:** `join_cents` (replaced by skip-if-crossing), `fv_clamp_cents` (replaced by per-side min/max FV logic).

### 2.8 Scoring formula reference

predict.fun uses quadratic scoring (same as Polymarket):

```
score_contribution = score_factor × shares × time_on_book_secs
score_factor       = ((v - spread_from_predict_mid) / v)²
```

where `v = spread_threshold_v` (typically 0.06 for ±6¢ markets) and `spread_from_predict_mid = |bid - predict_mid|` at placement.

| `spread_from_predict_mid` | `score_factor` (v=0.06) |
|---|---|
| 0.00 | 1.000 (100%) |
| 0.01 | 0.694 (69%) |
| 0.02 | 0.444 (44%) |
| 0.03 | 0.250 (25%) |
| 0.04 | 0.111 (11%) |
| 0.06 | 0.000 (0%) — skipped |

Points distribution cadence: weekly, with 2–3 day calculation lag before distribution.

Per-cycle score estimates logged at `CYCLE_END`:
```
est_yes_score = score_factor_yes × yes_qty × on_book_secs
est_no_score  = score_factor_no  × no_qty  × on_book_secs
```

Calibrate rate after weekly dashboard report:
```
rate = dashboard_points / (est_yes_score_raw + est_no_score_raw)
```

---

## 3. Hedge Strategy

### 3.1 Objective

- Keep predict.fun farming active.
- Reduce directional risk by offsetting fills on Polymarket.
- Prefer hedge fills at neutral or better prices; allow near-taking when inventory risk is high.

Farming and hedging are **decoupled** — if hedge execution fails, farming continues unaffected.

### 3.2 Hedge identity (binary market)

```
predict_fill_YES + hedge_buy_NO = $1 certain payout at resolution
predict_fill_NO  + hedge_buy_YES = $1 certain payout at resolution
```

- `P_fill_yes + P_hedge_no < 1` → locked-in profit
- `= 1` → break-even (pure risk reduction)
- `> 1` → paying hedge cost (acceptable up to `max_slippage_cents`)

### 3.3 Topology

```
PredictFun WS feed
    │  Event::Fill (predict YES or NO)
    ▼
MarketDataBus  ←──────────────────────────────────┐
    │                                              │
    ├── PredictionQuoter (farming, unchanged)      │
    │                                              │
    └── PredictHedgeStrategy                       │
            │  checks poly_bbo cache               │
            ▼                                      │
     Action::PlaceOrder(Polymarket, opposite token)│
            │                                      │
     OrderRouter → PolymarketExecutionClient       │
                         │ POST /order (CLOB)      │
                         │ GTC limit at best_ask   │
                         ▼                         │
               Polymarket CLOB execution           │
                                                   │
PolymarketMarketDataFeed (1 WS, YES+NO tokens) ────┘
    Event::BookUpdate → poly_bbo cache in strategy
```

Both strategies are separate `StrategyInstance` entries in the same `EngineRunner`. Events are delivered via the engine's `select_all` merged stream over `MarketDataBus`.

### 3.4 Market mapping

Each predict.fun market with both poly token IDs configured gets a `MarketMapping`:

```rust
MarketMapping {
    predict_yes: InstrumentId(PredictFun, Binary, "<market>-Yes"),
    predict_no:  InstrumentId(PredictFun, Binary, "<market>-No"),
    poly_yes:    InstrumentId(Polymarket, Binary, "<yes_token_id>"),
    poly_no:     InstrumentId(Polymarket, Binary, "<no_token_id>"),
}
```

Internal hedge_map:
```
predict_yes → poly_no   (YES fill → buy NO)
predict_no  → poly_yes  (NO fill  → buy YES)
```

**Token ID convention:** Polymarket CLOB token IDs are large unsigned integers stored as decimal strings — used directly as `InstrumentId.symbol`. `U256::from_str` parses decimal (no `0x` prefix). Index 0 = YES, index 1 = NO in the Gamma API `clobTokenIds` array.

**To look up token IDs:**
```bash
curl "https://gamma-api.polymarket.com/markets?clob_token_ids=<yes_or_no_token_id>" \
  | python3 -c "import sys,json; m=json.load(sys.stdin)[0]; print(json.loads(m['clobTokenIds']))"
# Returns: ['<yes_token_id>', '<no_token_id>']
```

Or run `trading-cli predict-markets --write-configs` (auto-fills both).

### 3.5 Hedge decision logic (`try_hedge`)

Called after each predict fill and on urgency-check ticks.

```
1. params.enabled? No → return
2. poly_bbo available? No → log HEDGE_SKIP, return
3. hedge_notional = state.qty × poly_ask
4. urgent = first_unhedged_ts.elapsed() >= max_unhedged_duration_secs
5. hedge_notional < hedge_min_notional AND NOT urgent → log HEDGE_BATCH, return
6. poly_ask > 0.99 → log HEDGE_SKIP (market sanity), return
7. Slippage check (vs Polymarket spread, NOT vs predict fill price):
     spread_cross = poly_ask - poly_mid
     spread_cross > max_slippage_cents → HEDGE_SKIP "Polymarket spread too wide"
   NOTE: venue divergence (predict vs poly) does NOT block the hedge.
         We hedge for risk reduction. Cost is logged as HEDGE_COST_INFO.
8. qty_rounded = state.qty.round_dp(2); < 5 → return (Polymarket min)
9. price = poly_ask.round_dp(2)
10. Optimistic consume: state.consume_all(); pending_hedge.insert(...)
11. emit Action::PlaceOrder(poly_inst, Buy, price, qty, GTC)
```

**Optimistic position accounting:** unhedged qty is consumed when the `Action::PlaceOrder` is emitted (not on confirmation). Safe because GTC at `best_ask` is a taker order — matches immediately. On `Event::PlaceFailed`, qty is restored to retry next tick.

**Hedge log events:**
- `HEDGE_TRIGGER` — predict fill received, accumulation started
- `HEDGE_BATCH` — (debug) fill accumulated but below min_notional
- `HEDGE_COST_INFO` — combined predict+poly cost vs $1 payout (auditing)
- `HEDGE_PLAN` — hedge order about to be emitted
- `HEDGE_SKIP` — hedge skipped with reason
- `HEDGE_FILL` — Polymarket fill confirmed
- `HEDGE_REJECT` — `PlaceFailed`, qty restored
- `HEDGE_URGENT` — time threshold breached, escalating

### 3.6 HedgeParams defaults

Not yet loaded from TOML (Phase 3). Defaults in code:

```
hedge_min_notional        = 5 USDC    (batch small fills)
max_unhedged_notional     = 100 USDC  (future urgency-tier threshold)
max_unhedged_duration_secs = 60       (escalation timer)
max_slippage_cents        = 0.05      (max half-spread to cross on Polymarket)
enabled                   = true      (kill-switch)
```

### 3.7 Auth / execution

```rust
PolymarketExecutionClient::from_env("PREDICT_PRIVATE_KEY")
```

**Only `PREDICT_PRIVATE_KEY` is needed** — no separate `POLY_API_KEY` / `POLY_SECRET` / `POLY_PASSPHRASE`. The Polymarket SDK derives (or creates) the API key deterministically from the private key via `create_or_derive_api_key`, tied to the wallet address (e.g., `0xA27D22701Bf0f222467673F563e59aA0E38df847`).

Flow:
1. Parse `PREDICT_PRIVATE_KEY` as `PrivateKeySigner` (alloy secp256k1)
2. `.with_chain_id(Some(POLYGON))` — chain 137
3. `Client::new(CLOB_HOST, config).authentication_builder(&signer).authenticate().await`
4. Store signer directly in struct (`PrivateKeySigner` field)

**SDK:** `polymarket-client-sdk = "0.4.4"` with `features = ["clob"]`.

**Order placement:**
```
GTC limit order:
  token_id   = U256::from_str(instrument.symbol)   // decimal parse
  price      = req.price.inner()                   // Decimal, rounded to 0.01
  size       = req.quantity.inner()                // outcome tokens (e.g. 50.0)
  side       = Side::Buy                           // always, for hedge
  order_type = OrderType::GTC

Signing: client.sign(&signer, signable_order).await
  → fetches neg_risk from /neg-risk/{token_id} (cached)
  → EIP-712 typed-data on Polygon CTFExchange

Post:    client.post_order(signed_order).await
  → PostOrderResponse { success, order_id, status }
  → success=true → track order_id in active_orders
  → success=false → ConnectorError::OrderRejected
```

`decimal_precision()` returns `Some(2)` (0.01 tick). CLOB rejects prices outside `[0.01, 0.99]`.

`cancel_all` issues `DELETE /orders` with our tracked order IDs only (not account-wide).

### 3.8 Polymarket CLOB endpoints used

| Endpoint | Method | Usage |
|---|---|---|
| `/order` | POST | Place order |
| `/order` | DELETE | Cancel single |
| `/orders` | DELETE | Cancel multiple (shutdown) |
| `/neg-risk/{id}` | GET | Fetched internally by SDK before signing |

### 3.9 Polymarket WS protocol facts (critical for any future changes)

WebSocket: `wss://ws-subscriptions-clob.polymarket.com/ws/market`

Subscribe (BOTH tokens in one connection):
```json
{"type": "market", "assets_ids": ["<yes_token_id>", "<no_token_id>"]}
```

- **Subscription type MUST be lowercase `"market"`.** Uppercase is silently ignored by server (initial `book` dump arrives but no `price_change` events).
- **PING/PONG is TEXT messages, not WebSocket protocol frames.** Send `"PING"` every 10s, handle TEXT `"PONG"` response.
- **On PONG: re-publish last known book state** for all subscribed tokens. This decouples "feed is alive" from "book changed recently" — a quiet market gets PONG heartbeats every 10s, so FV never goes stale.
- **Timestamps are milliseconds** (multiply by 1,000,000 for nanoseconds).
- **Events:** `book` (snapshot on subscribe + after fills), `price_change` (incremental — `size="0"` = remove level), `PONG` (heartbeat).
- **Quiet market** may not change for minutes — normal, not a dead feed.
- **Reconnect:** exponential backoff (1s → 2s → 4s → max 30s). WS recv timeout is 60s — `fv_stale_secs` must exceed this.

---

## 4. Operations

### 4.1 Quick start

```bash
cd /home/ubuntu/.openclaw/workspace/obird
source .env   # needs PREDICT_API_KEY, PREDICT_PRIVATE_KEY
cargo build --release --bin trading-cli

# Run all poly-linked markets
python3 scripts/farm.py

# Or single market (for debugging)
RUST_LOG=quoter=info,connector_predict_fun=info,connector_polymarket=info \
  ./target/release/trading-cli live --config configs/markets_poly/21177.toml
```

Ctrl+C for graceful shutdown — cancels all resting orders before exit.

**Rebuild required after any code change:** `cargo build --release --bin trading-cli`

### 4.2 One-time setup (per wallet)

```bash
source .env && cargo run --bin trading-cli -- predict-approve --all --config configs/predict_quoter.toml
```

Contracts approved on the predict.fun wallet:
- Standard CTFExchange — ERC-1155 + USDT
- YieldBearing CTFExchange — ERC-1155 + USDT
- NegRisk CTFExchange — ERC-1155 + USDT + NegRiskAdapter
- YieldBearing NegRisk CTFExchange — ERC-1155 + USDT + NegRiskAdapter

**Polymarket hedge prerequisite:** deposit USDC on Polygon at the predict.fun wallet address (derived from `PREDICT_PRIVATE_KEY`). Without USDC, orders are accepted by the CLOB (200 OK) but don't settle on-chain.

### 4.3 CLI commands

```bash
# Smoke test — verify auth, WS, pricing for one market
source .env && RUST_LOG=info cargo run --bin trading-cli -- predict-check
source .env && PREDICT_MARKET_ID=21177 cargo run --bin trading-cli -- predict-check

# Discover boosted markets + auto-write TOML configs
source .env && cargo run --bin trading-cli -- predict-markets
source .env && cargo run --bin trading-cli -- predict-markets --all
source .env && cargo run --bin trading-cli -- predict-markets --write-configs
source .env && cargo run --bin trading-cli -- predict-markets \
    --all --write-configs --fail-on-missing-poly-token \
    --output-dir configs/markets_poly

# Passive position unwind helper (dry-run first)
source .env && cargo run --bin trading-cli -- predict-liquidate --dry-run --config configs/markets_poly/21177.toml

# On-chain approval setup (one-time per wallet)
source .env && cargo run --bin trading-cli -- predict-approve --all --config configs/predict_quoter.toml

# Live farming (single market)
source .env && RUST_LOG=quoter=info,connector_predict_fun=info \
  cargo run --release --bin trading-cli -- live --config configs/markets_poly/21177.toml
```

### 4.4 Multi-market farm

Each predict.fun market runs as a separate process (workaround until engine key changes to `(Exchange, market_id)`). `scripts/farm.py` manages the fleet:

```bash
python3 scripts/farm.py            # starts all configs/markets_poly/*.toml
python3 scripts/farm.py --dry-run  # preview without starting
python3 scripts/farm.py --dir ...  # alternate config dir
```

- One process per TOML
- Auto-restart on crash with exponential backoff
- **Crash-loop protection:** 3 restarts in 120s → 5-min backoff
- Staggered startup (0.5s between markets) to avoid JWT auth collisions
- **Graceful shutdown:** Ctrl-C → SIGTERM all children → wait 15s for cancel acks → SIGKILL stragglers
- Writes `logs/farm/farm.pids` (`market_id=pid` entries)
- Periodic status to stdout every 60s: `[farm] status: N/M running, K in backoff`

**Logs:**
```bash
tail -f logs/farm/21177.log               # one market
tail -f logs/farm/*.log                    # all markets interleaved
grep FILL logs/farm/*.log                  # all fills
grep "skipped" logs/farm/*.log             # scoring-window / crossing skips
grep -E "HEDGE_TRIGGER|HEDGE_PLAN|HEDGE_FILL|HEDGE_REJECT|HEDGE_SKIP" logs/farm/21177.log
```

**Add/remove a market:**
```bash
# Regenerate all poly-linked configs from the API
source .env && cargo run --release --bin trading-cli -- predict-markets \
    --all --write-configs --fail-on-missing-poly-token \
    --output-dir configs/markets_poly

# Restart farm to pick up changes
```

### 4.5 Log interpretation

**Healthy startup:**
```
INFO  PolymarketExecutionClient ready address=0xA27D...
INFO  Polymarket CLOB WS feed spawned yes_inst=... no_inst=...
INFO  PredictHedgeStrategy initialized id=predict_points_v1_hedge markets=1 enabled=true
INFO  quoter: Waiting for first Polymarket FV update before quoting
  ... (≤10s for first poly book event, triggered by PONG heartbeat) ...
INFO  quoter: REQUOTE predict_mid=0.635 poly_fv=Some(0.545) poly_divergence=Some(0.0900)
       yes_bid=None no_bid=Some(0.345) yes_placed=false no_placed=true
       score_factor_no=0.3086
```

**Normal requote (both sides, small divergence):**
```
INFO quoter: REQUOTE strategy=predict_points_v1
     predict_mid=0.600 poly_fv=Some(0.590) poly_divergence=Some(0.0100)
     yes_fv_used=0.590 no_fv_used=0.410
     yes_bid=Some(0.57) no_bid=Some(0.39)
     yes_placed=true no_placed=true n_orders=2
     score_factor_yes=0.25 score_factor_no=0.25
```

**Fill + cooldown:**
```
INFO quoter: FILL instrument=... side=Buy price=0.57 qty=114.03 fill_count=1
INFO quoter: PULL_QUOTES reason=fill pause_secs=15
INFO quoter: COOLDOWN_EXPIRED  → back to Empty, requotes on next tick
```

**Hedge cycle (normal):**
```
INFO quoter: HEDGE_TRIGGER predict_inst=PredictFun.Binary.<mkt>-Yes
             poly_inst=Polymarket.Binary.<no_token> fill_qty=50 fill_price=0.42
INFO quoter: HEDGE_PLAN  hedge_qty=50.00 hedge_price=0.58 hedge_notional=29.00 urgent=false
INFO quoter: POLY_PLACE  order_id=0xabc... side=Buy price=0.58 qty=50.00 status=matched
INFO quoter: HEDGE_FILL  confirmed filled_qty=50 fill_price=0.58
```

**Red flags:**
| Log | Meaning | Action |
|---|---|---|
| `REQUOTE poly_fv=None` after startup | FV staleness paused quoting | Check WS feed connection |
| `ROUNDTRIP` every <200ms | `drift_cents` or `min_quote_hold_secs` too low | Raise thresholds |
| `Polymarket FV stale` repeating | Genuine feed outage | Check Polymarket status page |
| `PLACE_FAILED` | USDT balance, API key, or order validation | Check balance + key |
| `ROUNDTRIP n_orders=0` at startup | Normal (init CancelAll, no resting) | Not an error |

**Hedge skip reasons:**
| Log | Cause | Action |
|---|---|---|
| `HEDGE_SKIP no poly book` | NO token WS not delivering | Check `Polymarket CLOB WS feed spawned` at startup; verify NO token ID |
| `HEDGE_SKIP Polymarket spread too wide` | Spread > `max_slippage_cents` | Unusual — Polymarket is normally tight; check liquidity |
| `HEDGE_SKIP poly ask above 0.99` | Near resolution or illiquid | Skip is correct |
| `HEDGE_SKIP qty < 5 shares` | Below Polymarket min | Normal batching |
| `HEDGE_BATCH not enough notional` | Below `hedge_min_notional` (5 USDC) | Normal; waits for more fills |
| `HEDGE_REJECT placement failed` | `POLY_PLACE` returned error | Check USDC balance, key validity |
| `HEDGE_URGENT time threshold breached` | >60s unhedged | Normal escalation; fires even below min_notional |

### 4.6 Disabling hedge at runtime

No runtime kill-switch yet (Phase 3). To disable:
1. Remove `polymarket_no_token_id` from the market config
2. Restart the bot

Or: unset `PREDICT_PRIVATE_KEY` temporarily — `PolymarketExecutionClient::from_env` will fail gracefully and hedge disables with a warning log.

### 4.7 Config reference

```toml
[engine]
tick_interval_ms = 200

[[exchanges]]
name           = "predict_fun"
api_key_env    = "PREDICT_API_KEY"
secret_key_env = "PREDICT_PRIVATE_KEY"
testnet        = false

[exchanges.params]
market_id               = 21177
yes_outcome_name        = "<outcome>"
yes_token_id            = "..."
no_outcome_name         = "<outcome>"
no_token_id             = "..."
is_neg_risk             = false
is_yield_bearing        = true
fee_rate_bps            = 200
polymarket_yes_token_id = "..."   # enables poly FV
polymarket_no_token_id  = "..."   # enables hedge

[[strategies]]
name           = "predict_points_v1"
strategy_type  = "prediction_quoter"
instruments    = ["PredictFun.Binary.21177-Yes", "PredictFun.Binary.21177-No"]

[strategies.params]
spread_cents         = "0.02"
order_size_usdt      = "65"
drift_cents          = "0.02"
touch_trigger_cents  = "0.01"
touch_retreat_cents  = "0.02"
min_quote_hold_secs  = 10
fill_pause_secs      = 15
fv_stale_secs        = 90      # must be > 60
max_position_tokens  = "500.0"
spread_threshold_v   = "0.06"  # auto-filled from API
min_shares_per_side  = "100"   # auto-filled from API
```

Both `polymarket_yes_token_id` and `polymarket_no_token_id` are auto-populated by `predict-markets --write-configs` when the Gamma API resolves both tokens.

**NegRisk markets** (multi-outcome, e.g., Liverpool/Draw/PSG): each outcome is a separate `market_id` with its own `[[exchanges]]` block. Set `is_neg_risk = true`. Approvals already cover negRisk contracts.

---

## 5. Engine Wiring

`crates/cli/src/live.rs :: run_predict` dispatches on `strategy_type = "prediction_quoter"`:

1. Always: build `PredictFunClient` + `PredictFunMarketDataFeed` + `PredictionQuoter`.
2. If `polymarket_yes_token_id` set: add YES token to `PolymarketMarketDataFeed`.
3. If BOTH `polymarket_yes_token_id` AND `polymarket_no_token_id` set:
   - Add NO token to the same WS connection (single feed, both tokens).
   - Build `PolymarketExecutionClient::from_env(&pf_cfg.secret_key_env)`.
   - Build `PredictHedgeStrategy` with `MarketMapping` for this market.
   - Register `Exchange::Polymarket` in the `connectors` HashMap.
   - Append hedger to the `strategies` Vec — both run in the same `EngineRunner`.
   - If `from_env` fails: log warning, continue in farming-only mode.
4. `EngineRunner::new(connectors, strategies, md_bus)` — both strategies share the bus.

Fill event routing from predict.fun WS → hedge strategy:
1. `PredictFunMarketDataFeed::run()` receives wallet event (order filled).
2. Looks up `instrument` in `placed_instruments` map (YES or NO).
3. Calls `sink.publish(instrument, Event::Fill { instrument, fill })`.
4. `MarketDataBus` broadcasts to subscribers of that instrument.
5. `PredictHedgeStrategy::subscriptions()` includes both predict instruments → receives the fill.
6. `on_event(Event::Fill) → on_predict_fill()` → `try_hedge(poly_inst)`.

---

## 6. Points Farming Meta

1. **Boosted markets are the priority** — up to 6 active at once, each for a few hours. Sports/esports: Champions League, NBA, UFC, CS2, LoL are common. Run `predict-markets` at session start.
2. **Both sides required** — YES + NO needed for full score. Single-sided earns ~1/3.
3. **Tight quotes score quadratically better** — 1¢ spread ≈ 2× a 2¢ spread. Don't go below the market's tick (0.01 for precision=2, 0.001 for precision=3 — auto-detected).
4. **Don't churn** — orders scored by random sampling every minute. Stable resting orders accumulate more samples than rapidly-cancelled ones.
5. **Fills are free (0 maker fee)** — accumulating tokens is the only risk. `max_position_tokens=500` caps exposure at ~$175/outcome. If a position builds above max, that side stops quoting automatically; use `predict-liquidate` or raise the cap.
6. **Position state at startup:** the quoter reads existing YES/NO balances from `positions()` on startup. Restarting after heavy fills may leave one side over max — only the other side will quote until you liquidate.
7. **Position risk reminder:** N YES + N NO = costs $N, pays $N → zero directional risk when balanced. Imbalance risk = `|yes_tokens − no_tokens| × avg_price`.

---

## 7. Implementation Status

| Component | Status |
|---|---|
| Dual-BUY quoter | ✅ Live |
| predict.fun connector (4 contract variants) | ✅ Live |
| Polymarket CLOB WS feed (text PING/PONG, multi-token) | ✅ Live |
| FV gate (no blind quoting) | ✅ Live |
| Conservative dual-FV pricing | ✅ Live |
| Scoring-window skip | ✅ Live |
| Per-side score tracking | ✅ Live |
| PONG heartbeat re-publish | ✅ Live |
| `fv_stale_secs` param | ✅ Live (default 90; must be > 60) |
| Poly-only market filter | ✅ Live (`--fail-on-missing-poly-token`) |
| Multi-market farm (separate processes) | ✅ Live |
| Per-instrument `decimal_precision` | ✅ Live |
| Polymarket hedge (taker) — Phase 2 | ✅ Live |
| `PolymarketExecutionClient` | ✅ Live |
| `PredictHedgeStrategy` | ✅ Live |
| Slippage guard + min-notional batching + urgency escalation | ✅ Live |
| Multi-market single-process | ❌ TODO — engine key `(Exchange, market_id)` |
| Auto-boost switching | ❌ TODO — poll `get_markets_filtered` every 5 min |
| HedgeParams TOML wiring | ❌ TODO — Phase 3 |
| Passive maker pricing (Tier A/B/C) | ❌ TODO — Phase 3 |
| Polymarket user WS (real-time fill confirmation) | ❌ TODO — Phase 3 |
| Hedge ledger (append-only JSONL + daily summary) | ❌ TODO — Phase 3 |
| Kill-switch wired to TOML | ❌ TODO — Phase 3 |

---

## 8. Phase 3 Roadmap

**Passive maker pricing:**
- Currently: buy at `best_ask` (taker, immediate fill).
- Phase 3: price tiers A (`best_bid + 1 tick`, passive maker), B (join), C (best_ask, current).
- Tier selection by `unhedged_notional / max_unhedged_notional` ratio and urgency.
- When Tier A fails to fill within timeout, escalate to B/C.

**HedgeParams TOML wiring:**
- Load `[hedge]` section: `enabled`, `hedge_min_notional`, `max_unhedged_notional`, `max_unhedged_duration_secs`, `max_slippage_cents`.

**Polymarket user WS feed:**
- Subscribe `wss://ws-subscriptions-clob.polymarket.com/ws/user` with API creds.
- Real-time `order` / `trade` events → replace optimistic tracking with confirmed fills.

**Hedge ledger:**
- Append-only `logs/data/hedges-YYYY-MM-DD.jsonl` — one record per hedge attempt.
- Fields: trigger reason, predict fill price, poly ask, slippage, outcome.
- Daily `HEDGE_SUMMARY`: predict exposure, hedged fraction, avg slippage, net MTM.

**Polymarket quoting (separate from hedging):**
- Same farming strategy on Polymarket for USDC rewards ($5M/month pool as of 2026-04).
- Architecture: Polymarket execution connector already built. Need `PolymarketQuoter` strategy + rewards API.
- Reward formula matches predict.fun (predict.fun copied Polymarket).

---

## 9. Files Reference

| What | Where |
|---|---|
| Per-market configs (poly-linked) | `configs/markets_poly/<market_id>.toml` |
| Predict quoter crate | `crates/strategies/prediction_quoter/` |
| Predict hedger crate | `crates/strategies/predict_hedger/` |
| predict.fun connector | `crates/connectors/predict_fun/` |
| Polymarket connector (MD + execution) | `crates/connectors/polymarket/` |
| Multi-market launcher | `scripts/farm.py` |
| Farm PIDs | `logs/farm/farm.pids` |
| Per-market farm logs | `logs/farm/<market_id>.log` |
| Points sessions log | `logs/data/points-sessions.jsonl` |
| BBO tick data | `logs/data/bbo-YYYY-MM-DD.jsonl` |
| Fill data | `logs/data/fills-YYYY-MM-DD.jsonl` |
| Main tracing log | `logs/obird-YYYY-MM-DD.jsonl` |

---

## 10. Known Issues

### Fills from cancelled orders occasionally mismatch instrument
Mitigated. `placed_instruments` map survives `cancel_all()`. Unknown-hash fills now warn and default to YES instrument.
**Future fix:** TTL-based cleanup of `placed_instruments` after 60s.

### Multi-market support in one process
`HashMap<Exchange, Box<dyn ExchangeConnector>>` uses `Exchange` enum as key. Two PredictFun markets can't coexist — second overwrites the first.
**Fix:** change key to `(Exchange, String)` in `crates/engine/src/order_router.rs` + `EngineRunner`. Enables multiple `[[exchanges]]` blocks with different `market_id`s, and lets one Polymarket WS serve all markets.
**Workaround:** separate processes per market (current `farm.py`).

### Auto-switch to boosted markets
Bot stays on its configured market even if a boost starts elsewhere.
**Fix:** polling task calling `get_markets_filtered("OPEN")` every 5 min, detecting `is_boosted=true && boostEndsAt > now`, triggering a graceful market switch (cancel-all → reconfigure → resubscribe).
**Workaround:** manual `predict-markets` run + config update + restart.

### Boost-aware market ranking
`predict-markets` shows depth but doesn't rank by expected PP yield.
**Enhancement:** score = `((v-spread)/v)² × depth` using `spreadThreshold` as proxy for `v`. Pre-rank to prioritize markets.

### NegRisk multi-outcome quoting
A 3-outcome negRisk market = 3 separate `market_id`s → 3 bot instances currently.
**Fix:** multi-market support (above) + NegRiskAdapter position conversion.
