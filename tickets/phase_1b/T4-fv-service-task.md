---
title: "[AGENT] Phase 1b T4: Wire FairValueService as a tokio task"
labels: agent-task,phase-1b,difficulty-medium,area-fair-value
---

## Task
Add a `FairValueService::run()` function that subscribes to market data for a configured set of (polymarket_yes, predict_yes, predict_no) tuples, computes FV via the model, and publishes to the `FairValueBus`. For Phase 1b, runs as an in-process tokio task inside the engine binary (not yet a separate process).

## Context
Maps to `PROJECT_PLAN.md` §1.4. This is the moment the computation physically leaves the strategy. Running in-process first lets us prove the boundary is right before adding UDS (phase 1b.2) or NATS (phase 1d).

## Files to Touch
- `crates/fair_value_service/src/lib.rs` (new — or promote main.rs-adjacent code into a lib)
- `crates/fair_value_service/Cargo.toml` — add library target if not present; add trading_core + trading_engine dev-deps if needed for in-process testing

## Cursor prompt

```
Add a runnable FairValueService that plumbs MarketDataBus → model → FairValueBus.

1. In crates/fair_value_service/, ensure the crate exposes a library (not just a
   binary). Cargo.toml should have `[lib] path = "src/lib.rs"` alongside the
   [[bin]] entry. If there's no lib yet, create crates/fair_value_service/src/lib.rs
   that re-exports publisher, model, data_source modules.

2. Create crates/fair_value_service/src/service.rs with:

    use std::sync::Arc;
    use rust_decimal::Decimal;
    use tokio::sync::broadcast;
    use tokio::time::{Duration, Instant};
    use trading_core::{
        types::instrument::InstrumentId,
        types::fair_value::FairValueMessage,
        traits::FairValueSink,
        Event,
    };
    use crate::model::{FairValueModel, FvInputs};

    /// A single FV tuple the service tracks: poly YES mid drives the external
    /// signal, predict YES mid drives the local venue signal; compute when
    /// either updates.
    pub struct FvTuple {
        pub polymarket_yes: InstrumentId,
        pub predict_yes: InstrumentId,
        pub predict_no: InstrumentId,
    }

    pub struct FairValueService {
        tuples: Vec<FvTuple>,
        model: Box<dyn FairValueModel>,
        sink: Arc<dyn FairValueSink>,
    }

    impl FairValueService {
        pub fn new(
            tuples: Vec<FvTuple>,
            model: Box<dyn FairValueModel>,
            sink: Arc<dyn FairValueSink>,
        ) -> Self { Self { tuples, model, sink } }

        /// Run loop: subscribe to both books for each tuple, compute FV on
        /// any BookUpdate, publish YES and NO messages.
        ///
        /// Takes an iterator of (InstrumentId, broadcast::Receiver<Event>) pairs —
        /// caller (live.rs / engine) is responsible for subscribing to the bus.
        /// This keeps FairValueService decoupled from MarketDataBus.
        pub async fn run(
            self,
            mut mds: Vec<(InstrumentId, broadcast::Receiver<Event>)>,
        ) -> anyhow::Result<()> {
            // Per-tuple state: latest poly_mid + predict_mid + their timestamps.
            struct State {
                poly_mid: Option<Decimal>,
                poly_ts: Option<Instant>,
                predict_mid: Option<Decimal>,
                predict_ts: Option<Instant>,
            }
            let mut states: Vec<State> = (0..self.tuples.len())
                .map(|_| State { poly_mid: None, poly_ts: None, predict_mid: None, predict_ts: None })
                .collect();

            // Index lookup: which tuple/side does this InstrumentId belong to?
            let mut idx_poly = std::collections::HashMap::new();
            let mut idx_predict = std::collections::HashMap::new();
            for (i, t) in self.tuples.iter().enumerate() {
                idx_poly.insert(t.polymarket_yes.clone(), i);
                idx_predict.insert(t.predict_yes.clone(), i);
            }

            let merged = futures::stream::select_all(mds.drain(..).map(|(_, rx)| {
                Box::pin(futures::stream::unfold(rx, |mut rx| async move {
                    loop {
                        match rx.recv().await {
                            Ok(e) => return Some((e, rx)),
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => return None,
                        }
                    }
                }))
            }));

            tokio::pin!(merged);
            use futures::StreamExt;
            while let Some(event) = merged.next().await {
                let Event::BookUpdate { instrument, bid_px, ask_px, .. } = event else { continue };
                let mid = (bid_px + ask_px) / Decimal::from(2);

                let (i, is_poly) = if let Some(&i) = idx_poly.get(&instrument) {
                    (i, true)
                } else if let Some(&i) = idx_predict.get(&instrument) {
                    (i, false)
                } else {
                    continue;
                };

                if is_poly {
                    states[i].poly_mid = Some(mid);
                    states[i].poly_ts = Some(Instant::now());
                } else {
                    states[i].predict_mid = Some(mid);
                    states[i].predict_ts = Some(Instant::now());
                }

                // Only publish when both sides are known.
                let (Some(poly), Some(predict), Some(poly_ts), Some(predict_ts)) =
                    (states[i].poly_mid, states[i].predict_mid, states[i].poly_ts, states[i].predict_ts)
                    else { continue };

                let now = Instant::now();
                let inputs = FvInputs {
                    poly_mid: Some(poly),
                    predict_mid: predict,
                    poly_staleness_ms: now.duration_since(poly_ts).as_millis() as u64,
                    predict_staleness_ms: now.duration_since(predict_ts).as_millis() as u64,
                    timestamp_ns: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64,
                };

                let t = &self.tuples[i];
                if let Some((yes, no)) = self.model.compute(&t.predict_yes, &t.predict_no, &inputs) {
                    self.sink.publish(yes).await;
                    self.sink.publish(no).await;
                }
            }
            Ok(())
        }
    }

3. Re-export from lib.rs: `pub mod service; pub use service::FairValueService;`.

4. Verify: `cargo check -p fair_value_service` passes. Do not wire into live.rs
   yet — that's T5.
```

## Acceptance Criteria
- [ ] `cargo check --workspace` passes
- [ ] `FairValueService::run` spawns a merged subscription stream and publishes FV on tick
- [ ] Unit test (add one if feasible): given fake BookUpdates, verifies correct FV published

## Complexity
- [x] Medium (30-60 min)

## Blocked by
T1, T2, T3
