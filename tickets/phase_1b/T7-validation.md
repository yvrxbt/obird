---
title: "[AGENT] Phase 1b T7: Validate FV extraction matches pre-refactor behavior"
labels: phase-1b,difficulty-hard,area-ops,human-only
---

## Task
Run the refactored stack and verify that extracting FV into a separate service produced zero behavioral drift for the strategy. Compare quoting decisions, fills, and P&L against the monolith baseline.

## Context
Maps to `PROJECT_PLAN.md` §1.4 exit criteria. **Human-only**: requires watching live behavior and judging drift. Don't delegate.

## Files to Touch
- (Mostly observational — no code changes unless drift is found)
- `PROJECT_PLAN.md` §1.4 — tick deliverables
- `README.md` + `PREDICTION_MARKETS.md` — update architecture diagrams if FV extraction changes what the reader sees

## Validation steps

### Smoke compare: unit-test parity

```bash
# Run strategy tests before + after; diff the output traces if any
cargo test -p strategy_prediction_quoter -- --nocapture > /tmp/after.log
git stash && cargo test -p strategy_prediction_quoter -- --nocapture > /tmp/before.log && git stash pop
diff /tmp/before.log /tmp/after.log  # expect no difference in decisions
```

### Dry-run a live market

```bash
source .env
cargo build --release --bin trading-cli
./target/release/trading-cli live --config configs/markets_poly/21177.toml
# Kill after ~5 min once you've confirmed the startup and quoting logs below
```

## Checks — startup

- [ ] Logs show `FairValueService` task starting before strategy
- [ ] `FairValueService` subscribes to 3 instruments per tuple (poly_yes, predict_yes, predict_no)
- [ ] First FV publish happens within 2s of startup (needs both poly AND predict book ticks)
- [ ] Strategy logs "first FV received" (add a one-shot log if missing) before it starts quoting

## Checks — runtime

- [ ] `REQUOTE` decisions produce the same quoted prices as pre-refactor for the same (predict_mid, poly_mid) inputs. Sample 5 REQUOTE lines and verify against the pricing formula.
- [ ] Adverse-selection log still populates `poly_fv_at_fill` (via `latest_fv.sources`)
- [ ] P&L mark price uses poly mid from FV message (not None — if None, FV is stale)
- [ ] FV staleness log fires if poly feed drops for > `fv_stale_secs`

## Checks — shutdown

- [ ] Ctrl+C shuts down FairValueService task cleanly (no panic on channel close)
- [ ] Strategy shutdown is unaffected

## Follow-up tasks if validation passes

- [ ] Open Phase 1b.2: run `fair_value_service` as a separate binary communicating via UDS (reuses `FairValuePublisher` in `publisher.rs` — it's already written, just needs a matching client subscriber)
- [ ] Update `README.md` §3.3 "Fair value flow" — the diagram is now "FV service task → FairValueBus → Strategy" instead of "strategy reads poly BookUpdate directly"
- [ ] Tick `PROJECT_PLAN.md` §1.4 deliverables

## Acceptance Criteria
- [ ] All runtime checks pass
- [ ] No quoting-price drift on a representative REQUOTE sample
- [ ] Docs updated
- [ ] PR merged

## Complexity
- [x] Large (discuss first)

## Blocked by
T6
