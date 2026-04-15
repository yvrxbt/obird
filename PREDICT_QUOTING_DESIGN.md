# predict.fun Quoting Strategy — Design Document

> Canonical reference for the order placement decision tree.
> Any new session implementing predict.fun quoting changes MUST read this first.
> Last updated: 2026-04-15.

---

## Guiding Principle

**We quote around the Polymarket mid price, not the predict.fun mid.**

Predict.fun markets are often thin and wide. Polymarket is the deeper, more liquid venue for the same binary events. Using Polymarket mid as the fair-value signal means:
- Our quotes reflect true market consensus, not a stale or wide predict.fun book
- We're less likely to be adversely selected when predict.fun lags Polymarket

---

## Required Infrastructure (partially built, partially TODO)

### Done
- `connector_predict_fun`: places orders, tracks fills, cancel-all
- `strategy_prediction_quoter`: cancel-all + dual BUY (YES+NO) per tick
- `predict-markets` CLI: lists active boosted markets with copy-paste TOML

### TODO — needed to fully execute this design
1. **Polymarket market ID lookup**: given a predict.fun `market_id`, find the matching Polymarket condition ID / market slug. Likely via title string match against Polymarket REST API (`GET /markets?search=...`). Store mapping in config or auto-resolve at startup.
2. **Polymarket mid feed**: subscribe to Polymarket WS (`wss://ws-subscriptions-clob.polymarket.com/ws/market`) for the matching token. Extract YES best_bid / best_ask → mid. This is the fair-value signal `fv`.
3. **Wire `fv` into `PredictionQuoter`**: strategy currently uses predict.fun YES book mid. Replace with Polymarket mid when available; fall back to predict.fun mid if Polymarket feed is stale (>5s).

---

## Order Placement Decision Tree

```
INPUT: fv (Polymarket YES mid), spread config, predict.fun YES BBO
```

### Step 1 — Compute target YES bid

```
target_yes_bid = fv - spread_cents
```

where `spread_cents` comes from `[strategies.params] spread_cents` in the TOML.
This is the desired quote relative to fair value.

### Step 2 — Clamp YES bid to predict.fun book (crossing guard)

```
predict_yes_mid = (predict_yes_bid_mkt + predict_yes_ask_mkt) / 2

if target_yes_bid >= predict_yes_ask_mkt:
    # Would cross predict.fun ask → move inside
    target_yes_bid = predict_yes_ask_mkt - tick

if target_yes_bid <= predict_yes_bid_mkt:
    # Would be at or below predict.fun bid → join scenario (see Step 3)
    goto JOIN_LOGIC
```

### Step 3 — JOIN_LOGIC (when our bid would be at or behind predict.fun best bid)

This happens when either:
- fv is much lower than the predict.fun book (predict.fun is stale/wide)
- Our spread is large

Two sub-cases based on config:

#### 3a. Auto-join (no `join_cents` configured)
Place at the furthest distance from predict.fun mid that still earns farming points:
```
max_earning_spread = spread_threshold_v   # from UI tooltip, e.g. 0.06
join_yes_bid = predict_yes_mid - (max_earning_spread - tick)
```
This maximises score factor while staying within the earning window.

#### 3b. Manual join (TOML has `join_cents` set)
```
join_yes_bid = predict_yes_mid - join_cents
```
Place exactly at the configured join level regardless of score optimality.

In both cases: apply the tick-rounding and crossing guard from Step 2 to the join price.

### Step 4 — Derive NO bid

```
no_bid = 1 - yes_bid
```

**Tight-BBO special case** (predict.fun BBO spread < 2 × tick):
```
yes_bid = predict_yes_bid_mkt              # join YES natural bid
no_bid  = 1 - predict_yes_ask_mkt         # join NO natural bid (NOT 1 - yes_bid)
```
Using `1 - yes_bid_mkt` would place NO at exactly the NO ask → immediate taker fill.
`1 - yes_ask_mkt` places NO one tick below the NO ask → safe resting maker.
Note: YES + NO = 1 - tick (not 1.00) in this case; that is intentional and safe.

### Step 5 — Position-gate the order

```
if yes_tokens >= max_position_tokens:
    skip YES order (log "YES position limit reached")
if no_tokens >= max_position_tokens:
    skip NO order (log "NO position limit reached")
```

### Step 6 — Place

Both orders are `strategy: "LIMIT"` (predict.fun has no PostOnly type).
predict.fun can match LIMITs immediately if a counterparty rests at our price.
The crossing guards in Steps 2–4 minimise this risk but cannot eliminate it fully.

---

## Config Reference

```toml
[strategies.params]
spread_cents         = "0.02"   # target half-spread from fv (Polymarket mid)
join_cents           = "0.05"   # optional — if set, join at this fixed distance from predict mid
                                 # if omitted, auto-join at spread_threshold_v - tick
order_size_usdt      = "70.0"   # USDT per side per cycle
max_position_tokens  = "500.0"  # max directional exposure per outcome
drift_cents          = "0.02"   # pull+requote if fv moves > this from last quoted mid
min_quote_hold_secs  = 10       # don't pull on fast twitches
fill_pause_secs      = 5        # cooldown after any fill
spread_threshold_v   = "0.06"   # from predict.fun UI tooltip ("Max spread ±Nc")
min_shares_per_side  = "100"    # from predict.fun UI tooltip ("Min. shares: M")
```

Add `join_cents` only when you want a fixed join level. Omit it for auto-join.

---

## Implementation Status

| Component | Status | Notes |
|---|---|---|
| Dual-BUY cancel-all quoter | ✅ Done | `strategy_prediction_quoter` |
| predict.fun connector | ✅ Done | `connector_predict_fun` |
| Tight-BBO fill guard | ✅ Done | `no_bid = 1 - yes_ask_mkt` in pricing.rs |
| predict.fun mid as signal | ✅ Done | interim fallback only |
| Polymarket market ID lookup | ✅ Done | condition_id → Gamma API → YES/NO token IDs |
| Polymarket WS mid feed | ✅ Done | `wss://ws-subscriptions-clob.polymarket.com/ws/market` |
| fv signal wired to quoter | ✅ Done | Polymarket mid primary; predict.fun mid fallback on staleness |
| `join_cents` TOML param | ✅ Done | optional manual join depth parsed and enforced in pricing |
| Multi-market support | ❌ TODO | `HashMap<Exchange, Connector>` key → `(Exchange, String)` |
| Auto-boost switching | ❌ TODO | 5-min poll, detect boosted, graceful switch |

---

## Key Invariants (Do Not Break)

1. **Never send a taker fill intentionally.** Makers pay 0 fee; takers pay fee_rate_bps (200 bps on market 143028). Any fill costs the fee.
2. **Both sides must be placed for full score** on markets in [0.10, 0.90]. One-sided quoting halves expected points.
3. **YES + NO prices must not sum to > 1.00.** A YES BUY at 0.70 + NO BUY at 0.35 = 1.05 → losing $0.05/token if both fill simultaneously.
4. **Use Polymarket mid as fv, not predict.fun mid.** predict.fun is the execution venue, not the price oracle.
5. **Crossing guard runs after every clamp and rounding step.** Rounding DOWN on YES can reintroduce a NO crossing.
