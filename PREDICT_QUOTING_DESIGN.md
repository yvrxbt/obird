# predict.fun Quoting Strategy — Design Document

> Canonical reference for the order placement decision tree, FV logic, pricing knobs,
> and crossing-guard logic. Any session touching pricing, spread params, or fill-risk
> behaviour MUST read this first. Also read `PREDICT_FARMING_NOTES.md` for the full
> history of why things are designed the way they are.
> Last updated: 2026-04-15.

---

## Guiding Principle

**Quote around the Polymarket mid. Never place at the predict.fun mid.**

Polymarket is the deeper, more liquid venue for the same binary events. Using
its mid as fair value (FV) means:
- Our quotes reflect true market consensus, not a stale or wide predict.fun book.
- We're less likely to be adversely selected when predict.fun lags Polymarket.
- We control fill risk by choosing how far below FV to sit (`spread_cents`).

If the Polymarket feed is unavailable, the strategy **pauses quoting entirely**
rather than falling back to the predict.fun mid. Blind quoting against informed
Polymarket participants is a reliable way to get filled at bad prices.

## 2026-04-15 Ops refinement (important)

After live testing on 143028, touch-risk handling was updated:

1. **Trigger changed from top-bid proximity to ask-distance risk**
   - top-bid trigger caused self-induced cancel/replace loops in fast/tight books
   - current trigger: bid near ask (`touch_trigger_cents`)

2. **Defensive requote is latched per risk-regime**
   - one trigger on risk-zone entry, not one trigger per tick
   - avoids pathological thrash while preserving hit-risk protection

3. **Scoring-window behavior is farming-first**
   - bids outside `spread_threshold_v` are clamped just inside the scoring window
   - rationale: keep orders score-eligible while respecting safety guards

New knobs:
- `touch_trigger_cents`
- `touch_retreat_cents`

---

## Core Design: Independent Per-Side Pricing

YES and NO are priced **independently** from the Polymarket FV:

```
yes_bid = poly_mid - spread_cents
no_bid  = (1 - poly_mid) - spread_cents
```

Each side is placed at the target distance from FV, **or skipped entirely**.
There is no clamping to `best_bid + tick`. A side that cannot achieve `spread_cents`
distance without crossing its ask estimate is omitted from the batch.

### Why not `no_bid = 1 - yes_bid` (the old linked design)?

The old design forced both prices to sum to 1.00. When `yes_target = FV - spread_cents`
fell at or below the market YES bid, the crossing guard clamped YES up to
`best_bid + tick` (≈ mid), and since NO = 1 − YES, NO ended up near mid too.
Both orders rested at mid on tight books and were immediately filled by any taker
willing to trade at fair value.

**With independent pricing:**
- YES can rest *below* the YES market bid — there is no resting seller there,
  so no immediate fill. The "NO crossing" that previously forced YES upward only
  existed because NO was derived as `1 − yes_bid`. With independent NO, that
  constraint is gone.
- A side that truly cannot be placed safely is omitted. The other side still quotes.
- `yes_bid + no_bid = 1 − 2×spread_cents` (always < 1.00). If both sides fill,
  the combined position resolves profitably by exactly `2×spread_cents` regardless
  of outcome — the fill cost is bounded even in the worst case.

---

## Order Placement Decision Tree

```
INPUT: fv (Polymarket YES mid), spread_cents, predict.fun YES BBO, decimal_precision
```

### Step 0 — Conservative dual-FV pricing

Both YES and NO bids use the **more conservative** of the two FV signals (poly and predict),
independently for each side:

```
YES anchor = min(poly_mid, predict_mid)          # lower YES mid → furthest from both bids
NO  anchor = 1 - max(poly_mid, predict_mid)      # lower NO mid  → furthest from both NO bids

yes_bid = YES anchor - spread_cents
no_bid  = NO  anchor - spread_cents
```

**Why "conservative"**: placing a bid *above* either venue's mid would give that venue's
participants immediate edge. `min()` / `(1-max())` guarantees we're below BOTH mids on
each side — neither a poly-informed nor a predict.fun-informed trader can fill us without
paying at least `spread_cents` in edge.

**Scoring window skip**: if `|bid - predict_mid| ≥ spread_threshold_v`, skip that side
entirely. A zero-score order still locks up capital and carries fill risk. Not worth placing.

### Example: Arsenal (poly=0.545, predict=0.635, spread=0.02, v=0.06)

```
YES: min(0.545, 0.635) - 0.02 = 0.525
     |0.525 - 0.635| = 0.11 ≥ 0.06 → SKIP (outside scoring window, earns 0)

NO:  (1 - max(0.545, 0.635)) - 0.02 = (1 - 0.635) - 0.02 = 0.345
     |0.345 - 0.365| = 0.025 < 0.06 → PLACED, score_factor ≈ 31%
     no_ask_est = 1 - 0.63 = 0.37 → 0.345 < 0.37 → safe resting maker ✓
```

YES is skipped because earning points requires placing closer to predict_mid, which
would expose us to poly-informed adverse selection. NO is placed safely.

### Normal case (small divergence ≤ spread_cents)

When `|poly - predict| ≤ spread_cents`, `min(poly, predict) ≈ poly` and both sides
use poly as anchor — same as the pure poly-anchor design.

### Large upward divergence (poly >> predict)

```
poly=0.75, predict=0.60, spread=0.02:
YES: min(0.75, 0.60) - 0.02 = 0.58   → predict-anchored, |0.58-0.60|=0.02 → placed ✓
NO:  (1-max(0.75,0.60)) - 0.02 = 0.23 → |0.23-0.40|=0.17 ≥ 0.06 → SKIP
```

---

### Step 1 — Gate: wait for fresh Polymarket FV

```
if polymarket_fv_instrument configured AND polymarket_mid is None:
    log "Waiting for first Polymarket FV update"
    return (no actions)

if polymarket_mid is stale (> fv_stale_secs old):
    log "Polymarket FV stale — pausing"
    pull quotes if currently quoting
    return (no actions)
```

`fv_stale_secs` is a constant (30s) in `quoter.rs`. The Polymarket WS heartbeat
typically fires every 1–5s, so staleness means the feed is genuinely down.

### Step 2 — Compute YES bid (independent)

```
yes_target = poly_mid - spread_cents

if yes_target >= yes_ask_mkt:
    # poly FV >> predict.fun book — clamp just inside the ask
    yes_target = yes_ask_mkt - tick

yes_target = round_down(yes_target, decimal_precision)   # never round up

if yes_target >= yes_ask_mkt:
    # BBO is 1 tick wide and there's no room after clamp+round
    yes_bid = SKIP
else:
    yes_bid = yes_target   # valid resting maker; may be below market bid
```

**Key invariant**: `yes_bid < yes_ask_mkt`. It may be below the YES market bid —
that is intentional and safe. A below-bid order is a resting maker order.
It only fills if a taker walks the book down to our level.

### Step 3 — Compute NO bid (independent)

```
no_ask_est = 1 - yes_bid_mkt   # selling NO = buying YES at the bid
no_target = (1 - poly_mid) - spread_cents

no_target = round_down(no_target, decimal_precision)

if no_target >= no_ask_est:
    # NO would cross the NO ask — skip
    no_bid = SKIP
else:
    no_bid = no_target
```

**Key invariant**: `no_bid < 1 - yes_bid_mkt`. The NO bid never matches a
resting NO seller.

### Step 4 — Position gate

```
if yes_bid is not SKIP AND yes_tokens >= max_position_tokens:
    yes_bid = SKIP   (log "YES position limit reached")

if no_bid is not SKIP AND no_tokens >= max_position_tokens:
    no_bid = SKIP   (log "NO position limit reached")
```

### Step 5 — Place

If neither side survived the gates: log "both sides skipped", return no actions.

Otherwise: `CancelAll` + `PlaceOrder` for each surviving side.

Both orders are `strategy: "LIMIT"` (`tif: PostOnly` in the engine, mapped to
predict.fun's LIMIT strategy). predict.fun can match LIMITs immediately if a
counterparty rests at our price, but the crossing guards above ensure no resting
counterparty exists at our bid price.

---

## Crossing Conditions — What Each Guard Prevents

| Condition | Guard | Consequence if missed |
|---|---|---|
| `yes_bid >= yes_ask_mkt` | Clamp to `ask - tick` | Immediate fill against resting YES seller |
| `yes_bid` after clamp still `>= yes_ask_mkt` | Skip YES | Book too tight, no room |
| `no_bid >= 1 - yes_bid_mkt` | Skip NO | Immediate fill against resting NO seller |

**What is NOT a crossing condition with independent pricing:**
- `yes_bid <= yes_bid_mkt` (below market bid) — was a crossing condition in the
  old linked design but is irrelevant now. A resting maker bid below the best bid
  is perfectly valid.

---

## Tuning Knobs

All knobs live in `[strategies.params]` in the market TOML config.

### `spread_cents` — primary fill-risk knob

Distance from Polymarket FV placed per side:
```
yes_bid = poly_mid - spread_cents
no_bid  = (1 - poly_mid) - spread_cents
```

Score factor = `((v - spread_cents) / v)²` where `v = spread_threshold_v` (e.g. 0.06).

| `spread_cents` | score_factor (v=0.06) | Fill risk | Placement |
|---|---|---|---|
| 0.01 | 69% | High | 1 cent from FV |
| 0.02 | 44% | Moderate | **default** |
| 0.03 | 25% | Low | 3 cents from FV |
| 0.04 | 11% | Very low | 4 cents from FV |
| 0.06 | 0%  | Zero fills | Outside earning window |

**Rule of thumb**: start at 0.02. Increase to 0.03 if getting filled too often on
active sessions. Decrease to 0.01 only if fills are very rare and you want more points.

Setting `spread_cents >= spread_threshold_v` earns zero points — don't do this.

### `drift_cents` — requote trigger

Pull and requote when FV moves more than this from last quoted FV.
Set ≥ `spread_cents`. Setting it too small causes thrashing (constant cancel+replace).

**Recommended**: `drift_cents = spread_cents` (requote when market moves by 1 spread width).

### `fill_pause_secs` — cooldown after any fill

How long to stay off the book after a fill. Shorter = more points exposure.
`fill_pause_secs = 5` is the current setting. The 0-fee maker model means fills
are free; staying off the book longer than necessary costs points.

### `min_quote_hold_secs` — prevent drift thrashing

Minimum time orders must rest before a drift-triggered requote is allowed.
Fill-triggered cancels bypass this. Set to 5–15s to avoid thrashing on volatile books.

### `max_position_tokens` — directional exposure cap

Maximum tokens per outcome before stopping new orders on that side. At ~$0.35/YES
token and `max_position_tokens=500`, max directional risk = ~$175/outcome.

### `order_size_usdt` — USDT per order

Controls the share count per cycle: `shares = order_size_usdt / bid_price`.
Must satisfy: `order_size_usdt / max(yes_price, no_price) >= min_shares_per_side`.
At mid = 0.50: need `order_size_usdt >= 50`. At NO = 0.65: need `>= 65`. Use 70 for headroom.

### `spread_threshold_v` and `min_shares_per_side`

Read from the predict.fun API at startup and auto-filled by `predict-markets --write-configs`.
`spread_threshold_v` = the market's max-earning spread window (e.g. `0.06` for ±6¢).
`min_shares_per_side` = minimum qualifying order size (e.g. `100`).

---

## Score Accounting

The points formula at predict.fun is quadratic (same as Polymarket):
```
score_contribution = score_factor × shares × time_on_book_secs
score_factor = ((v - spread_from_predict_mid) / v)²
```

The `spread_from_predict_mid` used in scoring is measured against the **predict.fun
mid**, not the Polymarket FV. When Polymarket and predict.fun diverge, the effective
spread used by the exchange may differ from `spread_cents`. The `REQUOTE` log shows
both `yes_bid` and `no_bid` so you can compute the true spread at placement.

Per-cycle score estimates are logged at `CYCLE_END`:
```
est_yes_score = score_factor_yes × yes_qty × on_book_secs
est_no_score  = score_factor_no  × no_qty  × on_book_secs
```

After each weekly dashboard report, calibrate the points-per-score-second rate:
```
rate = dashboard_points / (est_yes_score_raw + est_no_score_raw)
```

---

## Config Reference

```toml
[strategies.params]
spread_cents         = "0.02"   # distance from conservative FV anchor per side
                                 # score_factor = ((v-spread)/v)²: 0.01→69%, 0.02→44%, 0.03→25%
                                 # increase to reduce fills; decrease for more points
order_size_usdt      = "65"     # USDT notional per side per cycle
max_position_tokens  = "500.0"  # max token exposure per outcome
drift_cents          = "0.02"   # pull+requote if poly FV moves > this from last quoted value
min_quote_hold_secs  = 10       # hold quotes ≥ this many secs before drift-triggered cancel
fill_pause_secs      = 5        # cooldown after any fill
fv_stale_secs        = 90       # secs since last Polymarket heartbeat before pausing
                                 # MUST be > 60 (WS recv-timeout). PONG every 10s resets this.
spread_threshold_v   = "0.06"   # auto-filled by predict-markets CLI (from API)
min_shares_per_side  = "100"    # auto-filled by predict-markets CLI (from API)
```

**Removed**: `join_cents` — was the fallback join depth when YES target crossed the
market bid. No longer needed. Independent pricing handles the tight-book case by
placing YES below the market bid as a valid resting maker order.

---

## Key Invariants (Do Not Break)

1. **Never place at mid.** No `best_bid + tick` clamping. A side that can't achieve
   `spread_cents` distance is skipped, not clamped to mid.

2. **`yes_bid + no_bid < 1.00` when both placed.** = `1 − 2×spread_cents` by
   construction. If both fill simultaneously, the combined position is profitable.

3. **`yes_bid < yes_ask_mkt`.** YES never crosses the YES ask.

4. **`no_bid < 1 − yes_bid_mkt`.** NO never crosses the NO ask estimate.

5. **No quoting without a fresh Polymarket FV.** If the feed is down or stale,
   the strategy pauses. There is no fallback to predict.fun mid.

6. **Use `poly_mid` as FV, not `predict_mid`.** predict.fun is the execution venue.
   Polymarket is the price oracle.

7. **YES below market bid is valid and intentional.** A resting bid below the best
   bid is a maker order. It earns points and is unlikely to fill unless a taker
   actively walks the book down to our level.

---

## Implementation Status

| Component | Status | Notes |
|---|---|---|
| Dual-BUY quoter | ✅ Done | `strategy_prediction_quoter` |
| predict.fun connector | ✅ Done | `connector_predict_fun` — all 4 contract variants |
| Polymarket CLOB WS feed | ✅ Done | text PING/PONG, multi-token, single connection |
| FV gate (no blind quoting) | ✅ Done | waits for first poly update; pauses on staleness |
| Conservative dual-FV pricing | ✅ Done | `min(poly,predict)` for YES, `1-max` for NO |
| Scoring-window skip | ✅ Done | zero-score orders never placed |
| Per-side score tracking | ✅ Done | per-cycle `yes_spread`, `no_spread`, score factors |
| PONG heartbeat re-publish | ✅ Done | FV freshness decoupled from book activity |
| `fv_stale_secs` param | ✅ Done | default 90; must be > 60 (WS recv timeout) |
| Poly-only market filter | ✅ Done | `--fail-on-missing-poly-token` flag |
| Multi-market farm | ✅ Done | `scripts/farm.py` — crash-loop protection |
| Multi-market single-process | ❌ TODO | one process per market; engine key needs `(Exchange, id)` |
| Auto-boost switching | ❌ TODO | poll `get_markets_filtered` every 5 min, graceful switch |
