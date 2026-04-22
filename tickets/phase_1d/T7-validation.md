---
title: "[AGENT] Phase 1d T7: End-to-end NATS validation (5 processes)"
labels: phase-1d,difficulty-hard,area-ops,human-only
---

## Task
Run the full split-process stack over NATS: md-ingest × 2, fair-value-service, strategy-controller, obird-engine — five processes, all communicating over a single localhost NATS node. Validate against the monolith baseline.

## Context
Maps to `PROJECT_PLAN.md` §1.5.6 + §1.7.5. Final gate before Phase 1 is "done" on a single box.

## Validation steps

```bash
# 0. NATS up
docker compose -f infra/nats/docker-compose.yml up -d

# 1. Ensure JetStream streams
nats stream add actions_v1 --subjects 'action.>' --retention workqueue --max-age 1h
nats stream add orders_v1  --subjects 'order.>'  --retention limits    --max-age 7d

# 2. Processes (each in its own screen/tmux pane)
./target/release/md-ingest-poly    --transport nats --tokens <yes>,<no>
./target/release/md-ingest-predict --transport nats --config configs/markets_poly/21177.toml
./target/release/fair-value-service --transport nats --config ...      # may need a config file
./target/release/strategy-controller --transport nats --config configs/markets_poly/21177.toml
./target/release/trading-cli live    --transport nats --engine-only --config configs/markets_poly/21177.toml
```

## Checks

- [ ] All 5 processes start and connect to NATS (check `nats server report connections`)
- [ ] `nats sub 'md.>'` shows BookUpdates flowing
- [ ] `nats sub 'fv.>'` shows FairValues flowing
- [ ] `nats sub 'action.>'` shows Actions from strategy-controller
- [ ] `nats sub 'order.>'` shows OrderUpdates from engine
- [ ] Strategy receives FV within 2s of startup; first quote placed within 5s
- [ ] P&L + fills match monolith baseline for a 10-minute window
- [ ] Kill strategy-controller → engine sits idle (no actions). Restart → redelivery via JetStream; engine idempotency prevents double-place (check for "idempotent-hit" logs)
- [ ] Kill md-ingest-poly → strategy detects FV staleness within `fv_stale_secs`, pauses quoting
- [ ] Ctrl+C on engine cleans up OIDs
- [ ] Tier-0 NDJSON files still populated (fallback still works)

## Stress checks (optional but recommended)

- [ ] Under 100 actions/sec for 60s: no dropped orders, JetStream consumer keeps up
- [ ] NATS node restart: all 5 processes reconnect within 10s (retry_on_initial_connect)
- [ ] Idempotency: inject a duplicate action_id manually via `nats pub` — engine logs hit, no double-place

## Follow-ups

- [ ] Document the 5-process layout in `README.md` §3 as the new default
- [ ] Update `PROJECT_PLAN.md` — tick §1.1, §1.2, §1.5, §1.6 deliverables
- [ ] Open Phase 2 tickets (QuestDB + dashboard + cross-region NATS)

## Acceptance Criteria
- [ ] All baseline checks pass
- [ ] No P&L drift vs monolith
- [ ] Kill-restart scenarios work via JetStream redelivery
- [ ] Idempotency proven
- [ ] Docs + plan updated

## Complexity
- [x] Large (discuss first)

## Blocked by
T4, T5, T6
