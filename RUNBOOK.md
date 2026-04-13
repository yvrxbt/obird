# obird — Live Operations Runbook

> Reference for running, monitoring, and tuning the HL MM before Binance is wired in.
> Update this file as parameters are tuned and observations accumulate.

---

## Pre-flight Checklist

Before starting any live session:

- [ ] `.env` contains `HL_SECRET_KEY` (mainnet key, never commit)
- [ ] `configs/quoter.toml` has expected `instruments`, `order_size`, `max_position`
- [ ] Confirm you're on the Tokyo (ap-northeast-1) server — latency target is cancel_ms p95 < 50ms
- [ ] No other instance of `trading-cli` is running (`ps aux | grep trading-cli`)
- [ ] Previous session logs are in `logs/` and `logs/data/` before they get overwritten

---

## Starting the Bot

Always build release first. Dev binary has meaningful overhead on the hot path.

```bash
cd /path/to/obird
source .env
cargo build --release --bin trading-cli
```

Run under screen so it survives SSH disconnects:

```bash
screen -S obird
source .env && RUST_LOG=quoter=info,connector_hyperliquid=info,trading_engine=info \
  ./target/release/trading-cli live --config configs/quoter.toml
# Ctrl+A D  → detach
# screen -r obird  → reattach
```

**Stop gracefully with Ctrl+C.** The engine cancels all tracked OIDs before exit. Never `kill -9` — it leaves resting orders on the book.

---

## Log Files

| File | Contents | Use |
|---|---|---|
| `logs/obird-YYYY-MM-DD.jsonl` | All tracing events (debug+). Filter by `target` field. | Full audit trail, debugging |
| `logs/data/bbo-YYYY-MM-DD.jsonl` | BBO tick per L2Book update. `exchange_ts_ns`, `local_ts_ns`, `bid_px/sz`, `ask_px/sz` | Market data analysis, latency |
| `logs/data/fills-YYYY-MM-DD.jsonl` | One line per fill. `session_pnl`, `mark_pnl`, `net_pos` | P&L, adverse selection |

Useful `jq` one-liners:

```bash
# Watch fills in real time
tail -f logs/data/fills-$(date +%Y-%m-%d).jsonl | jq '{ts:.timestamp_ns,side:.side,price:.price,pnl:.session_pnl,pos:.net_pos}'

# Roundtrip latency summary from tracing log
jq 'select(.fields.message=="ROUNDTRIP") | {cancel_ms:.fields.cancel_ms|tonumber, place_ms:.fields.place_ms|tonumber}' \
  logs/obird-$(date +%Y-%m-%d).jsonl | jq -s '{n:length, cancel_p50:(map(.cancel_ms)|sort|.[length/2|floor]), place_p50:(map(.place_ms)|sort|.[length/2|floor])}'

# Count drift vs fill pulls
jq -r 'select(.fields.message=="PULL_QUOTES") | .fields.reason' logs/obird-$(date +%Y-%m-%d).jsonl | sort | uniq -c
```

---

## Health Indicators (Terminal Output)

### Good

```
REQUOTE cancel_all + batch_place  mid=3500 reservation=3498.25 skew_bps=-1.75 n_orders=4
ROUNDTRIP cancel_ms=22 place_ms=18 total_ms=40 n_orders=4
FILL side=Sell price=3503.5 net_pos=-0.01 session_pnl=0.0350 mark_pnl=0.0312
COOLDOWN_EXPIRED → REQUOTE
```

### Warning Signs

| Symptom | Likely cause | Action |
|---|---|---|
| `cancel_ms > 200` consistently | Network or HL degradation | Check ping to HL endpoint; may need to reconnect |
| `DRIFT` firing every cycle, zero fills | `drift_bps` too tight for current vol | Loosen: `drift_bps = 5` |
| `ORDER_REJECTED` appearing | Price below min tick or size too small | Check `RUST_LOG=debug` output for rejection reason |
| `net_pos` monotonically growing one direction | Skew not strong enough | Double `skew_factor_bps_per_unit` |
| `net_pos` pinned at `max_position` for > 10 min | Strong trending market | Expected behavior; bot stops adding. Consider halting session. |
| `ROUNDTRIP cancel_ms=0 place_ms=0` | Fill edge case with empty OID set | Note context; minor, doesn't affect correctness |
| WS reconnecting repeatedly | HL WS instability | Bot auto-reconnects; watch for orders left on book post-reconnect |

---

## Session Milestones

| Duration | What to check |
|---|---|
| First 30 min | Latency (cancel_ms), any crashes, ORDER_REJECTED. Manual watch. |
| 2–4 hours | Inventory excursion, skew effectiveness, P&L trend direction |
| 1–2 days | Adverse selection pattern, whether drift_bps needs tuning |
| 1 week | Meaningful Sharpe estimate, stable parameter set |

---

## Post-Session P&L Analysis

```python
import pandas as pd

fills = pd.read_json('logs/data/fills-YYYY-MM-DD.jsonl', lines=True)
fills['ts'] = pd.to_datetime(fills['timestamp_ns'], unit='ns')
fills.set_index('ts', inplace=True)

# Session summary
print(f"Fills: {len(fills)}")
print(f"Final cash P&L: {fills['session_pnl'].iloc[-1]:.4f}")
print(f"Final mark P&L: {fills['mark_pnl'].iloc[-1]:.4f}")
print(f"Net position at end: {fills['net_pos'].iloc[-1]:.4f}")
print(fills[['side','price','quantity','fee','session_pnl','net_pos']].tail(20))
```

### Adverse Selection Check

How far does mid move against you after a fill? Negative = getting picked off.

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

---

## Parameter Tuning Guide

Tune one parameter at a time. Wait at least one session between changes.

### `drift_bps` (current: 3)

- Getting pulled every few seconds, few fills → loosen to `5`
- Sitting with stale quotes, adverse selection after fills → tighten to `2`

### `skew_factor_bps_per_unit` (current: 50)

- `net_pos` keeps accumulating directionally → double it
- Fill rate on the mean-reverting side is too low → halve it
- Target: position should mean-revert within ~5 fills

### `fill_pause_secs` (current: 10)

- Missing re-entry opportunities after fills → bring to `5`
- Getting into back-to-back fills in trending conditions → bring to `15`

### `level_bps` (current: [5, 10])

- Adverse selection analysis shows systematic pick-off → widen L1 from 5 to 7
- Fill rate too low, P&L/hour insufficient → tighten L1 from 5 to 4 (only if latency is solid)

---

## Exit Criteria: Ready to Wire Binance

Hard requirements before adding the second exchange:

- [ ] Survives 4+ hour unattended run with no crashes
- [ ] `net_pos` stays bounded — never pins at `max_position` for more than 10 min continuously
- [ ] `session_pnl` trend is positive or breakeven across 2+ sessions
- [ ] Latency stable: `cancel_ms` p95 < 50ms in Tokyo (check with jq summary above)
- [ ] No `ORDER_REJECTED` during normal operation
- [ ] Adverse selection median at 5s is less than the L1 half-spread (5 bps) — i.e., not getting systematically picked off
- [ ] Skew is working: mean position mean-reverts within ~5 fills after hitting extreme

Once these pass, the HL leg is validated. Binance comes in as the reference price source
for quotes (not a second independent MM leg yet) — which is the key improvement that
kills adverse selection from stale self-quotes.

---

## Strategy Comparison Framework (Future)

When running multiple strategy variants for benchmarking, compare on:

| Metric | How to compute |
|---|---|
| Fill rate | `len(fills) / quoting_minutes` |
| Spread capture | `mean(fill_price - mid_at_fill) * side_sign` in bps |
| Inventory excursion | `max(abs(net_pos)) / max_position` |
| Mean reversion time | Mean time from fill to `abs(net_pos) < order_size` |
| Session Sharpe | `mean(fill_pnl_increments) / std(fill_pnl_increments)` |
| Adverse selection 5s | See script above — key quality metric |

Variants to compare:
1. **Symmetric** (`skew_factor=0`) — baseline, loses in trends
2. **Inventory-skewed** (current) — Avellaneda-Stoikov simplified
3. **Vol-adaptive spread** — scale `level_bps` by rolling realized vol EMA (next addition)
4. **Ref-based quoting** — quote around Binance mid (post-Binance, expected biggest improvement)
