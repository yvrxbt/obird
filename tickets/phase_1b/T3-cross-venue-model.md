---
title: "[AGENT] Phase 1b T3: Implement CrossVenueConservative FV model"
labels: agent-task,phase-1b,difficulty-easy,area-fair-value
---

## Task
Move the `min(poly_mid, predict_mid)` / `1 - max(poly_mid, predict_mid)` logic out of `crates/strategies/prediction_quoter/src/pricing.rs` and into a reusable `CrossVenueConservativeModel` inside `crates/fair_value_service/src/model.rs`.

## Context
Maps to `PROJECT_PLAN.md` §1.4.2. The current strategy hardcodes the FV computation. After extraction, the strategy subscribes to a pre-computed FV and the *model* is swappable (future: microprice, ML).

Current code: `crates/strategies/prediction_quoter/src/pricing.rs` has the formulas in doc comments (lines 7–57) and implements them inline. We lift that into the model layer but keep `pricing.rs` as the strategy-side pricing (using FV as an input, not computing it).

## Files to Touch
- `crates/fair_value_service/src/model.rs`
- `crates/strategies/prediction_quoter/src/pricing.rs` (reduce scope — remove FV computation, keep spread/skew/scoring-window logic)

## Cursor prompt

```
Move cross-venue conservative FV computation into the fair_value_service crate.

1. In crates/fair_value_service/src/model.rs, REPLACE the stub with:

    use std::collections::HashMap;
    use rust_decimal::Decimal;
    use trading_core::types::fair_value::{FairValueMessage, SourceSnapshot};
    use trading_core::types::instrument::InstrumentId;

    pub trait FairValueModel: Send + Sync {
        fn name(&self) -> &'static str;
        fn version(&self) -> &'static str;
        fn compute(
            &self,
            yes_inst: &InstrumentId,
            no_inst: &InstrumentId,
            inputs: &FvInputs,
        ) -> Option<(FairValueMessage, FairValueMessage)>;
    }

    pub struct FvInputs {
        pub poly_mid: Option<Decimal>,        // Polymarket YES mid
        pub predict_mid: Decimal,             // predict.fun YES mid
        pub poly_staleness_ms: u64,
        pub predict_staleness_ms: u64,
        pub timestamp_ns: u64,
    }

    pub struct CrossVenueConservativeModel;

    impl FairValueModel for CrossVenueConservativeModel {
        fn name(&self) -> &'static str { "cross_venue_conservative" }
        fn version(&self) -> &'static str { "v1" }

        fn compute(
            &self,
            yes_inst: &InstrumentId,
            no_inst: &InstrumentId,
            inputs: &FvInputs,
        ) -> Option<(FairValueMessage, FairValueMessage)> {
            // When poly is unavailable, the service emits nothing — the strategy's
            // poly-FV gate pauses quoting. (Matches current "Poly FV gate" behavior.)
            let poly = inputs.poly_mid?;
            let predict = inputs.predict_mid;

            let yes_fv = poly.min(predict);
            let no_fv = Decimal::ONE - poly.max(predict);

            let mut mids = HashMap::new();
            mids.insert("polymarket_yes_mid".to_string(), poly);
            mids.insert("predict_yes_mid".to_string(), predict);

            let mut staleness = HashMap::new();
            staleness.insert("polymarket".to_string(), inputs.poly_staleness_ms);
            staleness.insert("predict_fun".to_string(), inputs.predict_staleness_ms);

            let sources = SourceSnapshot { mids, staleness_ms: staleness };

            let yes_msg = FairValueMessage {
                instrument: yes_inst.clone(),
                fair_value: yes_fv,
                confidence: 1.0,
                model_name: self.name().into(),
                model_version: self.version().into(),
                timestamp_ns: inputs.timestamp_ns,
                features: HashMap::new(),
                sources: sources.clone(),
            };
            let no_msg = FairValueMessage {
                instrument: no_inst.clone(),
                fair_value: no_fv,
                confidence: 1.0,
                model_name: self.name().into(),
                model_version: self.version().into(),
                timestamp_ns: inputs.timestamp_ns,
                features: HashMap::new(),
                sources,
            };
            Some((yes_msg, no_msg))
        }
    }

2. Add unit tests covering the published pricing.rs doc-comment examples:
   - poly=0.50, predict=0.50 → yes_fv=0.50, no_fv=0.50
   - poly=0.60, predict=0.50 → yes_fv=0.50 (min), no_fv=0.40 (1 - max(0.60, 0.50))
   - poly=None → None returned

3. Do NOT yet change crates/strategies/prediction_quoter/src/pricing.rs — that's
   handled in T6. This ticket only extracts the model.
```

## Acceptance Criteria
- [ ] `cargo test -p fair_value_service` passes with new model tests
- [ ] `CrossVenueConservativeModel::compute` produces identical outputs to the current strategy for the test cases
- [ ] Model is a trait object (`Box<dyn FairValueModel>`) so Phase 3 can swap in ML models

## Complexity
- [x] Small (<30 min)

## Blocked by
T1
