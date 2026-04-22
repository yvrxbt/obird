---
title: "[AGENT] Phase 1a T7: Dry-run multi-market validation + farm.py retirement"
labels: phase-1a,difficulty-hard,area-ops,human-only
---

## Task
Run the new multi-market single-process engine against a testnet-equivalent environment (or mainnet with zero-size orders), validate behavior, then gate-retire `scripts/farm.py`.

## Context
Maps to `PROJECT_PLAN.md` §1.8.3 + §1.8.4. This is judgment-heavy: watching startup, fills, shutdown, and comparing against the monolith baseline. Not delegable to an autonomous agent — **you run this**.

## Files to Touch
- `scripts/farm.py` — add env-gated deprecation warning (not deletion)
- `PREDICTION_MARKETS.md` §4 — update farm instructions once validated
- `PROJECT_PLAN.md` §1.8 — check off deliverables

## Validation steps

```bash
cd /path/to/obird
source .env
cargo build --release --bin trading-cli

# Pick two active markets from configs/markets_poly/
trading-cli live \
  --configs configs/markets_poly/21177.toml configs/markets_poly/52261.toml
```

## Checks — startup

- [ ] Logs show 2 `PredictFunMarketDataFeed` tasks spawning
- [ ] Logs show exactly 1 `PolymarketMarketDataFeed` with the union of poly tokens (should be 4 tokens total — YES+NO per market)
- [ ] `StrategyInstance` count matches expectation (2 quoters + up to 2 hedgers = 2-4)
- [ ] `MarketDataBus` pre-registers senders for all predict.fun + poly instruments before feed tasks start (check for any "no sender for instrument" warnings)
- [ ] `ROUNDTRIP` log entries include the market ID / instrument (not just exchange) so you can tell markets apart
- [ ] Initial `positions()` fetch is called once per unique connector Arc (i.e., ≤ 3 times for 2 predict.fun + 1 poly), not once per instrument key

## Checks — runtime (let it quote for ~15 min)

- [ ] Both markets receive fresh FV from the shared Polymarket feed
- [ ] Drift/requote on market A does not touch resting orders on market B
- [ ] No cross-market contamination: fill on A triggers cooldown on A only
- [ ] Hedger on A routes to its poly NO instrument; hedger on B routes to its poly NO instrument; they don't collide

## Checks — shutdown

- [ ] Ctrl+C cleanly cancels all tracked OIDs across both markets
- [ ] No stranded orders: after shutdown, run `trading-cli predict-liquidate --dry-run --config configs/markets_poly/21177.toml` and `... --config configs/markets_poly/52261.toml` — both should report zero active

## farm.py retirement (only after all above pass)

- [ ] Add to `scripts/farm.py`:
      ```python
      import os, sys
      if not os.environ.get("LEGACY_FARM"):
          print("ERROR: scripts/farm.py is retired. Use 'trading-cli live --configs ...' instead.")
          print("Set LEGACY_FARM=1 to override for rollback.")
          sys.exit(2)
      ```
- [ ] Update `PREDICTION_MARKETS.md` §4 quick-start to use `--configs`
- [ ] Update `README.md` §2.2 quick-start similarly
- [ ] Tick `PROJECT_PLAN.md` §1.8.4

## Rollback plan

If validation finds a blocker:
1. Don't merge the PR
2. Set `LEGACY_FARM=1` and keep using `farm.py` on the monolith binary
3. Open a follow-up ticket describing what broke

## Acceptance Criteria
- [ ] All startup, runtime, and shutdown checks above pass
- [ ] farm.py deprecation gate is in place
- [ ] Docs updated
- [ ] PR merged

## Complexity
- [x] Large (discuss first)

## Blocked by
T5, T6
