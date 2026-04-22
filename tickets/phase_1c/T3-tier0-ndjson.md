---
title: "[AGENT] Phase 1c T3: Tier-0 NDJSON safety-net writer in md-ingest"
labels: agent-task,phase-1c,difficulty-easy,area-connectors
---

## Task
Add an always-on NDJSON writer inside each md-ingest binary: every published event is also serialized to a daily-rotated NDJSON file on local SSD. This is the ultimate safety net — if NATS/UDS ever fails, we still have the raw data.

## Context
Maps to `PROJECT_PLAN.md` §8.2 (PRD) and §1.3.1 deliverables. Matches existing `logs/data/bbo-YYYY-MM-DD.jsonl` pattern; extends it to be a formal tier-0.

## Files to Touch
- `crates/md-ingest/src/ndjson_writer.rs` (new)
- All three `crates/md-ingest/src/bin/*.rs`

## Cursor prompt

```
Add an NDJSON tier-0 writer that mirrors every published event to a local file.

1. Create crates/md-ingest/src/ndjson_writer.rs:

    use std::path::PathBuf;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::mpsc;
    use trading_core::{Event, InstrumentId, MarketDataSink};

    pub struct NdjsonTier0 {
        dir: PathBuf,
        venue: String,
        tx: mpsc::UnboundedSender<(InstrumentId, Event)>,
    }

    impl NdjsonTier0 {
        pub fn spawn(dir: PathBuf, venue: &str) -> Self {
            tokio::fs::create_dir_all(&dir).await.ok();  // wrap properly — this won't compile verbatim
            let (tx, mut rx) = mpsc::unbounded_channel();
            let dir_cloned = dir.clone();
            let venue_cloned = venue.to_string();
            tokio::spawn(async move {
                let mut current_day: Option<String> = None;
                let mut file: Option<tokio::fs::File> = None;
                while let Some((inst, event)) = rx.recv().await {
                    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                    if current_day.as_deref() != Some(&today) {
                        let path = dir_cloned.join(format!("{}-{}.jsonl", venue_cloned, today));
                        file = Some(tokio::fs::OpenOptions::new()
                            .create(true).append(true).open(&path).await
                            .expect("open tier-0 ndjson"));
                        current_day = Some(today);
                    }
                    if let Some(f) = file.as_mut() {
                        let line = serde_json::json!({
                            "ts_ns": std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64,
                            "instrument": inst.to_string(),
                            "event": event,
                        });
                        if let Ok(mut bytes) = serde_json::to_vec(&line) {
                            bytes.push(b'\n');
                            let _ = f.write_all(&bytes).await;
                        }
                    }
                }
            });
            Self { dir, venue: venue.to_string(), tx }
        }
    }

    impl MarketDataSink for NdjsonTier0 {
        fn publish(&self, inst: &InstrumentId, event: Event) {
            let _ = self.tx.send((inst.clone(), event));
        }
    }

   (Fix the compile errors — the snippet above has one async-in-sync bug. Resolve
   with a lazy-init pattern: create the dir synchronously via std::fs, or defer the
   create to the spawned task.)

2. In each md-ingest binary, compose the two sinks with a fan-out wrapper:

    struct FanoutSink(Vec<Arc<dyn MarketDataSink>>);
    impl MarketDataSink for FanoutSink {
        fn publish(&self, inst: &InstrumentId, event: Event) {
            for s in &self.0 {
                s.publish(inst, event.clone());
            }
        }
    }

   Construct:
      let ndjson = Arc::new(NdjsonTier0::spawn("/var/log/md-ingest".into(), "poly"));
      let uds = Arc::new(handle);
      let sink: Arc<dyn MarketDataSink> = Arc::new(FanoutSink(vec![ndjson, uds]));
      feed.run(sink).await;

3. Add a --log-dir CLI arg (default: /var/log/md-ingest — or ./logs/md-ingest if
   /var/log is not writable for dev).

4. Test: run md-ingest-poly briefly, verify a file like
   `./logs/md-ingest/poly-2026-04-22.jsonl` appears and contains lines.
```

## Acceptance Criteria
- [ ] Each md-ingest binary writes to a daily NDJSON file
- [ ] File rolls on UTC day change
- [ ] UDS publish still works (both sinks fan out)
- [ ] Missing log directory is auto-created

## Complexity
- [x] Small (<30 min)

## Blocked by
T2
