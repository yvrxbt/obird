---
title: "[AGENT] Phase 1c T2: Create md-ingest-{poly,predict,hl} binaries"
labels: agent-task,phase-1c,difficulty-easy,area-connectors
---

## Task
Create three thin standalone binaries that run a single venue's existing `XMarketDataFeed` and publish events via `UdsMarketDataPublisher` from Phase 1c T1.

## Context
Maps to `PROJECT_PLAN.md` §1.3.2, §1.3.3. Each binary is just: parse CLI flags → construct the existing feed type → publish via UDS.

## Files to Touch
- `crates/md-ingest/Cargo.toml` (new, one crate with three bins)
- `crates/md-ingest/src/bin/md-ingest-poly.rs`
- `crates/md-ingest/src/bin/md-ingest-predict.rs`
- `crates/md-ingest/src/bin/md-ingest-hl.rs`
- Workspace `Cargo.toml` — add member

## Cursor prompt

```
Create a new crate `md-ingest` hosting three venue-specific binaries.

1. crates/md-ingest/Cargo.toml:
   - package name = "md-ingest"
   - deps: trading_core, md-transport, tokio (full), anyhow, tracing, tracing-subscriber,
     clap (features derive), connector_polymarket, connector_predict_fun,
     connector_hyperliquid, dotenvy
   - Define three [[bin]] entries pointing at src/bin/md-ingest-{poly,predict,hl}.rs

2. Add "crates/md-ingest" to workspace members.

3. Common pattern for each binary. Start with md-ingest-poly.rs:

    use clap::Parser;
    use md_transport::publisher::UdsMarketDataPublisher;
    use std::sync::Arc;
    use trading_core::MarketDataSink;

    #[derive(Parser)]
    struct Args {
        /// Comma-separated token IDs to subscribe
        #[arg(long)]
        tokens: String,
        /// UDS path to bind the publisher on
        #[arg(long, default_value = "/tmp/md-ingest-poly.sock")]
        socket: String,
    }

    #[tokio::main]
    async fn main() -> anyhow::Result<()> {
        tracing_subscriber::fmt().with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()).init();
        let args = Args::parse();
        let _ = dotenvy::dotenv();

        let token_ids: Vec<String> = args.tokens.split(',').map(String::from).collect();

        let (publisher, handle) = UdsMarketDataPublisher::new(&args.socket);
        let handle_arc: Arc<dyn MarketDataSink> = Arc::new(handle);

        let publisher_task = tokio::spawn(async move {
            if let Err(e) = publisher.run().await {
                tracing::error!(error = %e, "UDS publisher exited");
            }
        });

        let feed = connector_polymarket::market_data::PolymarketMarketDataFeed::new(token_ids);
        feed.run(handle_arc).await;

        publisher_task.abort();
        Ok(())
    }

4. md-ingest-predict.rs: mirror, but use connector_predict_fun::PredictFunMarketDataFeed.
   CLI args will be market-config-shaped (accept a --config <toml> pointing at one
   of configs/markets_poly/). Parse just the predict.fun parts; ignore others.

5. md-ingest-hl.rs: mirror. Construct HlMarketDataFeed with the asset info.
   CLI args: --symbol (e.g. ETH), --testnet (bool), --socket.

6. Test each binary builds: `cargo build --release --bin md-ingest-poly` etc.

7. Do NOT yet switch live.rs to use UDS. That's T4.
```

## Acceptance Criteria
- [ ] All three binaries build in release mode
- [ ] Each binary's `--help` shows expected flags
- [ ] Running `md-ingest-poly --tokens <real_token>` connects and starts publishing (dry-run: kill after 10s and verify UDS socket was created)

## Complexity
- [x] Small (<30 min) per binary, ~1 hour total

## Blocked by
T1
