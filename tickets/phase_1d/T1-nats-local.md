---
title: "[AGENT] Phase 1d T1: Local NATS server + Rust client helper"
labels: agent-task,phase-1d,difficulty-easy,area-infra
---

## Task
Get a single-node NATS server (with JetStream enabled) running locally, and add a thin Rust helper crate that wraps connection + basic publish/subscribe/request-reply. Production clustering (3-node + multi-region) is Phase 2 infra.

## Context
Maps to `PROJECT_PLAN.md` §1.1 (scaled down to single-node localhost for dev/test). NATS itself is a prereq for every other 1d ticket.

## Files to Touch
- `infra/nats/docker-compose.yml` (new) — single-node NATS with JetStream
- `infra/nats/nats-server.conf` (new) — minimal config
- `crates/nats-transport/` (new crate) — Rust wrapper

## Cursor prompt

```
Set up a local NATS server and a Rust client helper.

1. Create infra/nats/docker-compose.yml:

    services:
      nats:
        image: nats:2.10-alpine
        command: ["-c", "/etc/nats/nats-server.conf"]
        ports: ["4222:4222", "8222:8222"]
        volumes:
          - ./nats-server.conf:/etc/nats/nats-server.conf
          - ./data:/data

2. Create infra/nats/nats-server.conf:

    port: 4222
    http_port: 8222

    jetstream {
      store_dir: /data
      max_memory_store: 256MB
      max_file_store: 10GB
    }

3. Create crates/nats-transport/Cargo.toml with deps:
   - async-nats = "0.35"
   - tokio (workspace, full)
   - serde, serde_json, bincode (workspace)
   - anyhow, tracing (workspace)

4. Create crates/nats-transport/src/lib.rs:

    use anyhow::Result;
    use async_nats::Client;

    pub async fn connect(url: &str) -> Result<Client> {
        let client = async_nats::ConnectOptions::new()
            .retry_on_initial_connect()
            .connect(url)
            .await?;
        tracing::info!(url, "connected to NATS");
        Ok(client)
    }

    /// JetStream handle for durable publish/consume.
    pub async fn jetstream(client: Client) -> async_nats::jetstream::Context {
        async_nats::jetstream::new(client)
    }

    pub mod subjects {
        pub fn md(venue: &str, instrument: &str) -> String {
            format!("md.{venue}.{}.book", instrument.replace('.', "_"))
        }
        pub fn fv(symbol: &str) -> String {
            format!("fv.{}", symbol.replace('.', "_"))
        }
        pub fn action(venue: &str, market: &str) -> String {
            format!("action.{venue}.{market}")
        }
        pub fn order(venue: &str, market: &str, oid: &str) -> String {
            format!("order.{venue}.{market}.{oid}")
        }
    }

5. Add "crates/nats-transport" to workspace members.

6. Smoke test:
   - `docker compose -f infra/nats/docker-compose.yml up -d`
   - `curl http://localhost:8222/jsz` returns JetStream info
   - Write a trivial integration test that connects and does pub/sub on a test subject
   - Stop with `docker compose down`
```

## Acceptance Criteria
- [ ] `docker compose up` starts NATS with JetStream enabled
- [ ] `crates/nats-transport::connect("nats://localhost:4222")` succeeds
- [ ] Integration test (pub on subject → sub receives) passes

## Complexity
- [x] Small (<30 min)

## Blocks
T2, T3, T4, T5, T6, T7
