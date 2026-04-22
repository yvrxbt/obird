---
title: "[AGENT] Phase 1b T1: Finalize FairValueMessage wire format"
labels: agent-task,phase-1b,difficulty-trivial,area-core
---

## Task
Finalize the `FairValueMessage` struct in `crates/fair_value_service/src/publisher.rs` to carry the computed FV **plus** a source snapshot, so consumers (PredictionQuoter) can audit / log without subscribing to raw venue books themselves.

## Context
Maps to `PROJECT_PLAN.md` §1.4. Current `FairValueMessage` has `instrument`, `fair_value`, `confidence`, `model_version`, `timestamp_ns`, `features: HashMap<String, f64>`. We keep all of that but formalize the `source_snapshot` concept so the strategy can drop its inline `polymarket_mid` tracking without losing audit data.

## Files to Touch
- `crates/fair_value_service/src/publisher.rs`

## Cursor prompt

```
In crates/fair_value_service/src/publisher.rs:

1. Add a nested struct near FairValueMessage:

    /// Raw source prices used to compute the FV, for audit/logging.
    /// Keys are stable identifiers: "polymarket_mid", "predict_mid", "binance_mid", etc.
    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    pub struct SourceSnapshot {
        /// Per-source mid price at the time of FV computation.
        pub mids: HashMap<String, Decimal>,
        /// Per-source staleness in milliseconds (age of the book update used).
        pub staleness_ms: HashMap<String, u64>,
    }

2. Add `pub sources: SourceSnapshot` to FairValueMessage. Keep all existing fields.

3. Add a `pub model_name: String` field too (separate from `model_version` — name is
   e.g. "cross_venue_conservative", version is "v1"). Default to empty string if
   you need backward-compat during the transition.

4. Run `cargo check -p fair_value_service`.

Do not touch any other crate.
```

## Acceptance Criteria
- [ ] `cargo check -p fair_value_service` passes
- [ ] `FairValueMessage` now has `sources: SourceSnapshot` and `model_name: String` fields

## Complexity
- [x] Small (<30 min)

## Blocks
T3, T4, T6
