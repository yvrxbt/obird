---
title: "[AGENT] Phase 1c T4: Wire live.rs to consume external md-ingest over UDS"
labels: agent-task,phase-1c,difficulty-medium,area-cli
---

## Task
Add a `--external-feeds <socket>:<venue>:...` CLI flag to `trading-cli live` that, when present, spawns `UdsMarketDataSubscriber` tasks which re-publish to the engine's local `MarketDataBus`. The default (no flag) keeps the existing in-process feed spawns.

## Context
Maps to `PROJECT_PLAN.md` §1.3.1 engine side. This lets the operator choose: monolith mode (feeds inline) OR split mode (md-ingest binaries running separately). Rollout: default stays monolith during Phase 1c testing.

## Files to Touch
- `crates/cli/src/main.rs` — new CLI arg
- `crates/cli/src/live.rs`

## Cursor prompt

```
Add external-feed subscription support to the live runner.

1. In the Clap Live command, add:
     #[arg(long, num_args = 0.., value_delimiter = ',')]
     external_feeds: Vec<String>,

   Format per entry: "poly:/tmp/md-ingest-poly.sock" or "predict:/tmp/md-ingest-predict.sock".
   The venue prefix is advisory; the subscriber doesn't care about venue, just reads
   whatever MdFrames arrive.

2. In crates/cli/src/live.rs, when external_feeds is non-empty:
   - For each entry, spawn a md_transport::subscriber::UdsMarketDataSubscriber::new(path, md_bus.clone())
     .run().
   - SKIP the in-process feed spawn for the matching venue. (e.g., if "poly:..."
     is passed, don't also spawn PolymarketMarketDataFeed inline.)
   - The MarketDataBus is the same either way — subscribers see the same events
     regardless of which source published them.

3. Leave the default path (no --external-feeds) unchanged. This is a pure additive
   flag during Phase 1c rollout.

4. Test:
   # Terminal 1
   ./target/release/md-ingest-poly --tokens <token> --socket /tmp/md-poly.sock
   # Terminal 2
   ./target/release/trading-cli live \
     --config configs/markets_poly/21177.toml \
     --external-feeds poly:/tmp/md-poly.sock
   Verify strategy logs show FV updates driven by the external poly feed (bar
   any subtle staleness differences vs in-process).
```

## Acceptance Criteria
- [ ] `trading-cli live --help` documents `--external-feeds`
- [ ] With the flag: inline poly feed is skipped; UDS subscriber re-publishes
- [ ] Without the flag: identical behavior to today
- [ ] Engine doesn't double-subscribe (no duplicate BookUpdates)

## Complexity
- [x] Medium (30-60 min)

## Blocked by
T1, T2

## Blocks
T5
