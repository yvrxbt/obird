---
title: "[AGENT] Phase 1b T6: Refactor PredictionQuoter to consume external FV"
labels: agent-task,phase-1b,difficulty-medium,area-strategies
---

## Task
Remove `polymarket_mid` + `polymarket_mid_ts` state from `PredictionQuoter`. Replace with a subscription to `FairValueBus`. Preserve all audit/log behavior by reading `source_snapshot.mids["polymarket_yes_mid"]` from the received `FairValueMessage`.

## Context
Maps to `PROJECT_PLAN.md` §1.4. This is the payoff — the strategy crate no longer knows about Polymarket at all. It subscribes to FV like any other external signal.

## Files to Touch
- `crates/strategies/prediction_quoter/src/quoter.rs`
- `crates/strategies/prediction_quoter/src/lib.rs` (exports)
- `crates/strategies/prediction_quoter/src/pricing.rs` (signature adjustment — takes `fv` as scalar input, not `poly_fv` + `predict_mid` separately)

## Cursor prompt

```
Refactor PredictionQuoter to consume FairValueMessage from FairValueBus instead of
computing FV inline from polymarket BookUpdates.

1. Add a new field to PredictionQuoter:
     fv_rx: Option<tokio::sync::broadcast::Receiver<FairValueMessage>>,

   And a field holding the latest received FV:
     latest_fv: Option<FairValueMessage>,
     // Drop polymarket_mid and polymarket_mid_ts entirely.

2. Change PredictionQuoter::new signature:

     pub fn new(
         strategy_id: String,
         yes_instrument: InstrumentId,
         no_instrument: InstrumentId,
         params: QuoterParams,
         fv_bus: Arc<trading_engine::fair_value_bus::FairValueBus>,
     ) -> Self

   In the constructor, subscribe to fv_bus for yes_instrument and keep the Receiver.

   (Alternative: add an `async fn wire_fv(&mut self, fv_bus: &Arc<FairValueBus>)`
   that's called from initialize(). This avoids making new() async. Implementer
   picks the cleaner path — whatever keeps the Strategy trait clean.)

3. In the event loop (on_event):
   - Drop the branch that handled BookUpdate for polymarket YES instrument.
     PredictionQuoter no longer subscribes to Polymarket at all.
   - Add a poll on fv_rx at the top of on_event (use try_recv loop to drain any
     pending FV messages). Update self.latest_fv with the newest one.
   - When the existing code reads self.polymarket_mid / self.polymarket_mid_ts,
     replace with self.latest_fv reads:
       poly_mid → self.latest_fv.as_ref().and_then(|m|
           m.sources.mids.get("polymarket_yes_mid").copied())
       poly_staleness_ms → self.latest_fv.as_ref().and_then(|m|
           m.sources.staleness_ms.get("polymarket").copied())
       fv (used for quoting) → self.latest_fv.as_ref().map(|m| m.fair_value)

4. In pricing.rs, change the quoting function to take the precomputed fv (scalar)
   as input instead of poly_fv + predict_mid. The min()/max() logic inside pricing.rs
   (that the doc comments describe) is now in the FV model — pricing.rs consumes
   the precomputed FV and applies spread/skew/scoring-window logic only.

   Keep the poly-FV-gate behavior: if self.latest_fv is None, pause quoting.
   That's equivalent to the old "Poly FV gate" — just now it's "FV gate".

5. Adjust PredictionQuoter::subscriptions():
   - Return only [yes_instrument, no_instrument]. REMOVE polymarket YES from
     subscriptions() — the strategy no longer cares about poly books.

6. Run `cargo test -p strategy_prediction_quoter`. Update tests: any that fed
   polymarket_mid directly now need to feed a FairValueMessage (build one with
   sources.mids populated for audit fields).

7. Run `cargo build --release`. Should now pass end-to-end.
```

## Acceptance Criteria
- [ ] `cargo test -p strategy_prediction_quoter` passes
- [ ] `cargo build --release` succeeds
- [ ] PredictionQuoter.subscriptions() no longer includes any Polymarket instrument
- [ ] `grep -n "polymarket_mid" crates/strategies/prediction_quoter/src/` returns zero matches
- [ ] Adverse-selection and mark-P&L logging still works (via `latest_fv.sources.mids["polymarket_yes_mid"]`)

## Complexity
- [x] Medium (30-60 min)

## Blocked by
T5
