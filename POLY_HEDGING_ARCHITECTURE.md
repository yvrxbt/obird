# Polymarket Hedging Architecture (Design v0)

> Date: 2026-04-15
> Scope: hedge predict.fun fill exposure on Polymarket while preserving points farming.
> Status: design-only, pre-implementation (API docs to be integrated next).

---

## 1) Objective

Primary goal:
- Keep predict.fun farming strategy active,
- Reduce directional risk by offsetting fills on Polymarket,
- Prefer hedge fills at neutral or better prices, but allow near-taking behavior when inventory risk is high.

Secondary goals:
- Preserve auditability of every hedge decision,
- Reuse existing obird architecture boundaries (`Strategy -> Action -> OrderRouter -> Connector`),
- Keep compatible with future pair-trader work (Hyperliquid/Binance).

---

## 2) Core Principle

Predict strategy remains the **signal source** for inventory changes.
Polymarket hedge is a **risk layer** that reacts to those inventory changes.

Do not hard-couple farming quote placement to hedge execution success in v1.
(If hedge fails, farming still runs, but risk limits tighten.)

---

## 3) High-level Topology

```text
PredictFun fills -> Event::Fill -> Position delta (YES/NO)
                           |
                           v
                HedgeController (new strategy/service layer)
                           |
                    HedgeIntent / HedgeAction
                           |
                     OrderRouter / Connector
                           |
                    Polymarket execution
```

Two viable integration modes:

1. **In-engine strategy mode (preferred)**
   - Add `PolymarketHedgeStrategy` implementing `Strategy` trait.
   - Subscribe to predict fill events + poly book updates.

2. **Sidecar risk daemon mode**
   - Separate runtime process consuming logs/events and issuing hedge orders.
   - Easier isolation, weaker determinism at first.

Recommend mode (1) first for consistency with existing architecture.

---

## 4) Data & State Model

### 4.1 Hedge state per market

- `predict_yes_pos`, `predict_no_pos`
- `target_net_delta` (usually near zero)
- `current_poly_hedge_pos_yes/no`
- `unhedged_delta_yes/no`
- `hedge_mode` (passive | aggressive)
- `last_hedge_ts`, `cooldown_until`

### 4.2 Hedge ledger (append-only)

Per hedge attempt record:
- trigger reason (`fill`, `risk_breach`, `rebalance`)
- source inventory snapshot
- intended hedge size/side
- selected price policy
- execution result (placed/partial/filled/cancelled/rejected)
- slippage vs reference

This gives auditable "why we did this" history.

---

## 5) Hedge Decision Tree (v1)

For each predict fill:

1. Compute unhedged exposure increment.
2. If exposure below `hedge_min_notional`, batch (do nothing yet).
3. Choose urgency tier:
   - **Tier A (low risk):** passive hedge (maker near touch)
   - **Tier B (medium):** join/inside 1 tick
   - **Tier C (high):** near-taking (cross up to `max_take_slippage_cents`)
4. Enforce guardrails:
   - max hedge clip size
   - max hedge frequency
   - max slippage
   - stale-book reject
5. Place hedge order(s).
6. If partial/not filled, retry policy based on urgency + cooldown.

---

## 6) Pricing Policy

Reference inputs:
- Polymarket BBO
- predict mid / poly mid divergence
- current unhedged delta

Candidate policy:
- passive baseline at best bid/ask +/- 1 tick
- urgency-adjusted price aggression
- hard cap: do not exceed configured slippage boundary

Invariants:
- risk-reducing hedges can be more aggressive than farming quotes,
- but every aggressive action must be explicitly bounded and logged.

---

## 7) Risk Controls

- `max_unhedged_notional`
- `max_unhedged_duration_secs`
- `max_hedge_slippage_cents`
- `max_hedge_orders_per_min`
- `hedge_kill_switch` (disable hedge without stopping farming)

Escalation behavior:
- If unhedged > threshold and hedge venue unavailable, throttle/disable new farming orders on the risky side.

---

## 8) Required Components

### Existing reusable pieces
- Event/Action bus
- OrderRouter
- predict connector fill stream
- polymarket market-data feed

### New components
- `connector_polymarket::execution` (if not already complete for order placement)
- `HedgePolicy` module
- `HedgeStateStore` + ledger writer
- Config schema additions for hedge knobs

---

## 9) Config Sketch

```toml
[hedge]
enabled = true
mode = "delta_neutral"
hedge_min_notional_usdt = "25"
max_unhedged_notional_usdt = "250"
max_unhedged_duration_secs = 20
max_hedge_slippage_cents = "0.02"
max_hedge_orders_per_min = 30
passive_join_ticks = 1
aggressive_take_ticks = 2
kill_switch = false
```

---

## 10) Telemetry & Audit Requirements

Add structured logs:
- `HEDGE_TRIGGER`
- `HEDGE_PLAN`
- `HEDGE_PLACE`
- `HEDGE_FILL`
- `HEDGE_REJECT`
- `HEDGE_SUMMARY`

Daily summary should include:
- gross predict exposure
- hedged fraction
- hedge slippage distribution
- net combined MTM (predict + hedge)

---

## 11) Implementation Phases

### Phase 0 — Design freeze
- Confirm API semantics and order constraints from Polymarket docs.
- Finalize hedge config schema.

### Phase 1 — Paper hedge (no order placement)
- Compute hedge intents from live fills.
- Log hypothetical fills/slippage only.

### Phase 2 — Passive-only hedge
- Place maker-only hedges with strict limits.
- Validate fill rates + exposure reduction.

### Phase 3 — Controlled aggressive hedge
- Enable near-taking for risk breaches.
- Add stronger alerts + guardrails.

### Phase 4 — Unify with pair-trader roadmap
- Shared risk core for predict/poly + HL/Binance pair systems.

---

## 12) Decision Patterns to Keep

- Prefer deterministic triggers over ad-hoc heuristics.
- Add knobs only when they map to a clear failure mode.
- Keep strategy logic explainable via logs; every hedge action must have a reason code.
- If behavior thrashes, improve trigger quality first, debounce second.
