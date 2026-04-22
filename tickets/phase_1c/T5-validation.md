---
title: "[AGENT] Phase 1c T5: Validate md-ingest split-process mode"
labels: phase-1c,difficulty-hard,area-ops,human-only
---

## Task
Run the full stack in split-process mode: md-ingest binaries per venue + engine binary consuming feeds over UDS. Verify behavioral parity with monolith mode and that tier-0 NDJSON captures every event.

## Context
Maps to `PROJECT_PLAN.md` §1.3.5. Exit criterion for Phase 1c.

## Validation steps

```bash
# Build everything
cargo build --release

# Terminal 1 — md-ingest-poly
./target/release/md-ingest-poly \
  --tokens <yes_token>,<no_token> \
  --socket /tmp/md-poly.sock \
  --log-dir ./logs/md-ingest

# Terminal 2 — md-ingest-predict
./target/release/md-ingest-predict \
  --config configs/markets_poly/21177.toml \
  --socket /tmp/md-predict.sock \
  --log-dir ./logs/md-ingest

# Terminal 3 — engine
./target/release/trading-cli live \
  --config configs/markets_poly/21177.toml \
  --external-feeds poly:/tmp/md-poly.sock,predict:/tmp/md-predict.sock
```

## Checks

- [ ] Both md-ingest processes start, subscribe to their WS feeds, print "UDS listening"
- [ ] Tier-0 NDJSON files are created at `./logs/md-ingest/poly-YYYY-MM-DD.jsonl` etc.
- [ ] Each file grows by book-update events as quotes come in (verify `wc -l` increases)
- [ ] Engine subscriber connects to both sockets within 2s of startup
- [ ] Strategy receives BookUpdate events identical in rate and content to monolith mode
- [ ] Kill md-ingest-poly → engine logs disconnect, retries; restart md-ingest-poly → engine reconnects automatically (exponential backoff)
- [ ] Ctrl+C on engine cancels orders cleanly (unchanged by UDS split)
- [ ] Kill order: md-ingest last (engine first) → no data-loss panic

## Follow-ups if passed

- [ ] Document the split-process mode in `PREDICTION_MARKETS.md` §4 ops
- [ ] Add systemd unit files to `infra/systemd/` (deferred until Phase 2 infra work)
- [ ] Tick `PROJECT_PLAN.md` §1.3 deliverables

## Acceptance Criteria
- [ ] All 9 checks above pass
- [ ] No behavioral drift in a 5-minute quoting window
- [ ] Docs updated
- [ ] PR merged

## Complexity
- [x] Large (discuss first)

## Blocked by
T3, T4
