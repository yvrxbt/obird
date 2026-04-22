# PRD — Farming & Market-Making Platform (obird v2)

> Draft: 2026-04-21
> Authors: Z (CTO) + Jarvis (agent)
> Status: Review with partner → iterate → commit
> Target: 6-figure/month farming + cross-venue MM on 1M AUM at 2-person shop

---

## 0. TL;DR

Split the current monolith into five cleanly-composed services that can run colocated or centralized as needed:

```
[MD Ingest] → [Quant Tap → QuestDB/S3]     (slow path, fire-and-forget)
     │
     └─→ [Fair Value Service] → [Strategy/Controller] → [obird Engine] → [Venue]
            (FV fan-out)         (instructions/actions)   (OMS + execution)
```

- **obird becomes a pure per-venue OMS/execution engine** — accepts `Action` messages, owns order lifecycle, idempotent, colocated with each exchange region.
- **Fair Value Service** and **Strategy/Controller** are new separate binaries — strategy decides *what* to do, obird is *how* to do it on the wire.
- **NATS JetStream** is the single messaging substrate for everything cross-process. In-process broadcast stays on the hot path per ADR-002.
- **QuestDB + S3 Parquet** is the quant storage stack. Separate write path from live trading so quant ingestion can't back-pressure quoting.
- **AWS in 3 regions**: ap-northeast-1 (Tokyo, for HL), eu-west-2 (London, for Polymarket + predict.fun), us-east-1 (central services + future Kalshi/Lighter).
- **Budget**: ~$600-900/mo MVP, ~$1.5-2.5k/mo at full scale (3 regions, 5 venues, quant lake live).

---

## 0.1 Current State → Target State

Single source of truth for what exists today vs what this PRD proposes. v1 details are in `README.md`, `PREDICTION_MARKETS.md`, `DEX_CEX_MM.md`; this table is the jump-off.

| Concern | v1 (today) | v2 (this PRD) |
|---|---|---|
| Process model | Single `trading-cli live` binary per market; `scripts/farm.py` spawns N processes for N predict.fun markets | One `obird-engine` per venue, one `md-ingest-<venue>`, one `fair-value-service`, one `strategy-controller` — all separate binaries |
| Engine routing key | `HashMap<Exchange, Connector>` — forces one-process-per-market for predict.fun | `HashMap<InstrumentId, Connector>` — one process serves all markets of an exchange |
| Messaging substrate | `tokio::broadcast` in-process per instrument (ADR-002) | NATS JetStream cross-process; in-process broadcast retained on the co-located hot path |
| Fair value | Inline in `PredictionQuoter` (`min(poly_mid, predict_mid)`); `fair_value_service` crate is a stub | `fair-value-service` binary, pluggable models, publishes on `fv.<symbol>` |
| Market data | WS feeds embedded in connector crates, publish via `MarketDataSink` to in-proc bus | Extracted `md-ingest-<venue>` binaries, fan out via NATS; tier-0 NDJSON to local SSD as safety net |
| Quant storage | `logs/*.jsonl` on the trading host (same disk as the hot path) | QuestDB hot 90d + S3 Parquet cold 2y; write path decoupled from live quoting via JetStream consumer |
| Risk / PnL | `UnifiedRiskManager::check` is a stub; `PositionTracker` not implemented | Risk gate in engine (hot-reloadable limits from Postgres); central `position-service` + PnL aggregator |
| Strategies live | `HlSpreadQuoter`, `PredictionQuoter`, `PredictHedgeStrategy` — all co-located in the engine | Same strategies, promoted to run either co-located *or* in network mode (same crate, different transport) |
| Deployment | `screen` + `cargo build --release` on a single Tokyo box; secrets in `.env` | Terraform + systemd + SSM on EC2 in 3 regions (ap-northeast-1, eu-west-2, us-east-1); AWS Secrets Manager |
| Observability | Tracing to `logs/obird-*.jsonl`, manual `jq`; `crates/telemetry` present but not wired to Prom | Prometheus + Grafana + Loki + Tempo (OTLP); PagerDuty-grade alerts; OTel trace on Action→Order→Fill |
| Control plane | CLI only (`predict-markets`, `predict-liquidate`, etc.) | Next.js 15 + tRPC dashboard: fleet health, positions, kill switches, hot-reload strategy TOML |
| Backtest | Primitives in `crates/backtest`; CLI dispatch is a stub | CLI wired to harness; CI gate replays recorded day and asserts P&L tolerance |
| Binance / Lighter / Kalshi | Binance connector built not wired; Lighter scaffolding; Kalshi absent | Binance live (Phase A ref-price, Phase B second MM leg); Lighter + Kalshi in Phase 3 |

**What's not v2-delta** — stays the same:
- The `Strategy` / `ExchangeConnector` / `Action` / `Event` / `MarketDataSink` trait contracts (ADR-002, ADR-003). These are the single most valuable asset in the codebase and the PRD explicitly preserves them.

---

## 1. Goals

1. **Scale farming to $100k/mo in incentives** across Polymarket + predict.fun + Lighter + HL + Binance.
2. **Cross-venue MM**: use external fair-value signals to quote; do not rely on a single venue's mid.
3. **Quant research loop**: L2 tick data flows into a research-queryable store without ever touching the live quoting hot path.
4. **Controller/engine separation**: strategy logic can be iterated and swapped without redeploying the execution engine, and colocated with either.
5. **Reasonable ops burden for 2 people**: minimal bespoke infra, no Kubernetes, no multi-year tuning projects.
6. **Latency target**: 5–20 ms tick-to-order for farming; clean upgrade path to 1–5 ms by instance-class swap and connector co-location, not by rewrite.

## 2. Non-Goals (explicit)

- Sub-ms HFT. Not in scope on AWS at 1M AUM. Revisit if we grow to 10M+.
- Multi-tenant / external customers — this is an internal platform.
- Strategy backtesting at scale — already stubbed in `crates/backtest`, phase 3.
- Replacing mission-control for portfolio-level reporting — that stays separate.
- Cross-cloud portability. AWS-only for MVP.

---

## 3. High-Level Architecture

### 3.1 Topology

```
┌──────────────────────────────────────────────────────────────────────────┐
│  AWS eu-west-2 (London)          ← colocates Polymarket + predict.fun    │
│                                                                          │
│  ┌─────────────┐  ┌─────────────┐  ┌────────────┐                        │
│  │ md-poly     │  │ md-predict  │  │ obird-poly │◄─ Actions ──┐          │
│  │ md-kalshi*  │  │             │  │ obird-pred │             │          │
│  └──────┬──────┘  └──────┬──────┘  └─────┬──────┘             │          │
│         │                │               │                    │          │
│         └────────┬───────┘               │                    │          │
│                  ▼                       ▼                    │          │
│           NATS (local cluster) ◄──────── order updates        │          │
│                  │                                            │          │
└──────────────────┼────────────────────────────────────────────┼──────────┘
                   │                                            │
                   │ NATS Supercluster Gateway (TLS)            │
                   ▼                                            ▼
┌──────────────────────────────┐      ┌────────────────────────────────────┐
│  AWS ap-northeast-1 (Tokyo)  │      │  AWS us-east-1 (Central)           │
│                              │      │                                    │
│  ┌──────────┐  ┌──────────┐  │      │  ┌─────────────┐  ┌─────────────┐  │
│  │ md-hl    │  │ obird-hl │  │      │  │ FairValueSvc│  │ Controller  │  │
│  └─────┬────┘  └─────┬────┘  │      │  └──────┬──────┘  └──────┬──────┘  │
│        │             │       │      │         │                │         │
│        └─────┬───────┘       │      │         └────────┬───────┘         │
│              ▼               │      │                  ▼                 │
│       NATS (local)           │      │           NATS (hub cluster)       │
└──────────────────────────────┘      │                  │                 │
                                      │                  ▼                 │
                                      │  ┌──────────┐  ┌──────────────┐    │
                                      │  │ Quant Tap│  │ Control Plane│    │
                                      │  │  consumer│  │ (Next.js +   │    │
                                      │  └────┬─────┘  │  Postgres)   │    │
                                      │       │        └──────────────┘    │
                                      │       ▼                            │
                                      │  ┌──────────┐  ┌──────────────┐    │
                                      │  │ QuestDB  │→ │ S3 Parquet   │    │
                                      │  │ (hot 90d)│  │ (cold 2y)    │    │
                                      │  └──────────┘  └──────────────┘    │
                                      └────────────────────────────────────┘
```

### 3.2 Flow narrative

1. **Market data ingest** — One `md-<venue>` daemon per venue per region. Connects to exchange WS, normalizes events, publishes to local NATS subjects `md.<venue>.<instrument>.book` / `.trade` / `.fill`.
2. **Fan-out, not serial-fan-through** — NATS publishes go to (a) Fair Value Service (hot-path, latest-value semantics), (b) Quant Tap JetStream consumer (persistent, durable, replayable). These are independent. The Quant Tap can lag without affecting FV.
3. **Fair Value Service** — Subscribes to whatever markets the active strategies need. Emits normalized FV on `fv.<symbol>` at its own cadence (on-change or on-tick; configurable). Stateless w.r.t. orders.
4. **Strategy / Controller** — Subscribes to FV + position/fill stream. Decides whether to `Place`, `Cancel`, or `Replace`. Emits `Action` messages on `action.<venue>.<market>`. Idempotent: if an order is already resting at the target price/size, issues no-op.
5. **obird engine (per venue, colocated)** — Consumes `action.<venue>.*`. Translates Actions into connector calls. Tracks open-order state. Deduplicates redundant places. Returns acks + order updates on `order.<venue>.<market>`. Single-threaded per market for ordering, multi-market per process.
6. **Central services** — Control plane (dashboard), Postgres (config, audit), QuestDB (tick DB), S3 (cold), Grafana (observability).

---

## 4. Component Catalog

| Component                  | Language/Runtime    | Deployment                | Exists? | Phase |
|----------------------------|--------------------|---------------------------|---------|-------|
| obird-engine (per venue)   | Rust (existing)    | EC2 colocated per region  | ✅ refactor | 1 |
| md-ingest (per venue)      | Rust               | EC2 colocated per region  | Partial (in-process) | 1 |
| fair-value-service         | Rust               | EC2 central or colo       | Skeleton | 1 |
| strategy-controller        | Rust (or Python)   | EC2 central or colo       | Embedded | 1 |
| NATS cluster (local + hub) | NATS server        | 3x t4g.small per region   | ❌ new   | 1 |
| quant-tap (consumer)       | Vector or Rust     | EC2 central               | ❌ new   | 1 |
| QuestDB                    | Java/C++ (self-host) | EC2 r7g.xlarge + gp3 1TB | ❌ new   | 1 |
| S3 Parquet archive         | AWS S3             | s3://obird-quant-cold     | ❌ new   | 1 |
| control-plane dashboard    | Next.js 15 + tRPC  | Vercel or EC2             | ❌ new   | 2 |
| RDS Postgres (control)     | AWS RDS db.t4g.small | us-east-1               | ❌ new   | 2 |
| risk / PnL aggregator      | Rust               | EC2 central               | Stub    | 2 |
| Grafana + Prom + Loki      | Grafana stack      | EC2 t4g.small or free tier | ❌ new | 2 |

---

## 5. obird Refined: OMS/Execution Engine

### 5.1 Responsibility boundary

obird **owns**: order lifecycle, exchange connector state, idempotent order state machine, cancel/replace sequencing, rate-limit compliance, dry-run/kill-switch.

obird **does not own**: pricing, sizing, fair value, strategy decisions (those now live outside).

### 5.2 External contract (new)

**Input subjects** (consumed):
- `action.<venue>.<market>` — `Action` message. Schema:
  ```json
  {
    "action_id": "uuid",           // idempotency key
    "cmd": "place" | "cancel" | "replace",
    "order": {                     // present for place/replace
      "side": "buy" | "sell",
      "price": "0.43",
      "size": "100.0",
      "tif": "gtc" | "alo" | "ioc",
      "client_tag": "prediction_quoter:yes:bid"
    },
    "target_order_id": "...",      // present for cancel/replace
    "constraints": {               // engine-enforced
      "max_slippage_cents": "0.02",
      "not_before": 1700000000000
    }
  }
  ```
- `fv.<symbol>` — FairValue (advisory only; obird does not use this for pricing, but can surface it in audit logs).

**Output subjects** (published):
- `order.<venue>.<market>.<order_id>` — OrderUpdate (placed, acked, partial_fill, fill, canceled, rejected). At-least-once via JetStream durable consumer.
- `engine.<venue>.<market>.health` — heartbeat every 1s (connector state, WS status, open-order count).

### 5.3 Idempotency — the "don't spam-place" guarantee

Current obird implements this via `placed_instruments` map + per-strategy state machine. Formalize into an engine-level primitive:

```
OrderStateMachine per (venue, market, client_tag):
  Empty → PlacePending(action_id) → Resting(order_id)
  Resting(order_id) → CancelPending(action_id) → Empty
  * → Rejected → Empty  (with backoff before next Action accepted)
```

Engine rules:
1. **Duplicate place**: if state is `PlacePending` or `Resting` at the *same price/size*, no-op with ack.
2. **Replace**: cancel current, place new. If cancel fails, retry cancel with exponential backoff; do NOT place the new order until cancel acked.
3. **Duplicate cancel**: if state is `CancelPending` or `Empty`, no-op with ack.
4. **Stuck cancel**: if cancel has been retrying for > `cancel_timeout_secs` (configurable, default 10s), emit `CancelStuck` event. Strategy can decide to escalate (e.g., reduce size via opposite-side limit).

This maps 1:1 to the "engine-controller pattern" user described: engine owns the FSM, controller only emits desired state.

### 5.4 Colocation rules

- `obird-hl` → ap-northeast-1 (Tokyo), c7g.large or c7gn.large for enhanced networking.
- `obird-poly`, `obird-predict` → eu-west-2 (London), c7g.large.
- `obird-binance` → ap-northeast-1 or ap-southeast-1 (Singapore), depends on user base.
- `obird-kalshi` (future) → us-east-2 (Ohio, closest to Chicago) or non-AWS Equinix Chicago VPS.
- `obird-lighter` (future) → us-east-1 until sequencer location confirmed.

Instance upgrade path: c7g.large (free-tier-ish) → c7gn.large (ENA Express) → c7in.large (Intel Ice Lake, DPDK-capable). Zero code changes needed.

### 5.5 Single-process multi-market

Resolve gap #2 in `README.md §12`: change engine key from `HashMap<Exchange, Connector>` to `HashMap<InstrumentId, Connector>` (finer than `(Exchange, MarketId)` and maps cleanly to the existing `InstrumentId { exchange, kind, symbol }` type in `crates/core`). This lets one obird process serve all Polymarket markets through one WS connection (the `PolymarketMarketDataFeed` already supports multi-token) and all predict.fun markets through one engine, and kills the `scripts/farm.py` crash-loop orchestration.

Keep `farm.py` as the fallback for the multi-market-multi-process regression case during rollout.

---

## 6. Fair Value Service

### 6.1 Purpose

Decouple the FV computation from both the strategy and the connectors. Today, `PredictionQuoter` has a hardcoded "min(poly_mid, predict_mid)" inside the strategy — this is a special case. Generalize.

### 6.2 FV Models (pluggable)

| Model | Description | Use case |
|-------|-------------|----------|
| `mid` | Simple mid of BBO for a symbol | Single-venue baseline |
| `cross_venue_conservative` | Current PredictionQuoter logic: `min(a, b)` on bid, `max(a, b)` on ask | Farming (stay conservative) |
| `microprice` | `(bid*ask_size + ask*bid_size)/(bid_size+ask_size)` | CEX MM on deeper books |
| `vwap_depth` | Depth-weighted VWAP over top N levels | Prediction markets with thin books |
| `ml_ensemble` | Predicted fair value from model server | Phase 3 |

### 6.3 Wire format

Already defined as skeleton in `crates/fair_value_service/src/publisher.rs`. Finalize as:

```json
{
  "symbol": "polymarket:21177:yes",
  "fv": "0.437",
  "confidence": "0.92",           // 0-1, model-dependent
  "sources": ["polymarket", "predict_fun"],
  "staleness_ms": 45,
  "model": "cross_venue_conservative",
  "model_version": "v1",
  "ts_ns": 1700000000123456789
}
```

### 6.4 Transport

Publishes to NATS subject `fv.<symbol>`. NATS Core (not JetStream) — latest-value semantics, slow consumers skip, 100-200μs cross-cluster. Quant Tap separately JetStreams the FV stream for historical research.

### 6.5 Colocation

FV service can run:
- **Central** (us-east-1) for cross-venue strategies where the FV depends on multiple regions.
- **Colocated with the execution venue** when FV is single-venue-derived (e.g., a Binance microprice FV for Binance MM).

Keep the decision per-symbol in config. Default: central for v1.

---

## 7. Strategy / Controller Service

### 7.1 Responsibility

Consume FV + own-order state + fills. Emit Actions. Stateless re-entrant logic where possible; persistent state (position, inventory) in a sidecar store (Redis or embedded sled, decision below).

### 7.2 Two modes

- **Co-located mode**: Strategy runs in-process inside obird-engine as a crate (existing pattern). Hot-path, sub-ms Action dispatch. Use when latency-critical (HL MM on size).
- **Network mode**: Strategy runs as a separate binary, consumes NATS, emits Actions over NATS. Use when strategy is cross-venue or compute-heavy (FV depends on multiple regions). Adds ~1-3ms RTT.

This is the "optional colocation" the user asked for. Same code, different deployment. The seam is the `Action` channel — in-process it's a tokio mpsc, cross-process it's a NATS subject. Wrap behind a trait so the strategy crate doesn't care.

### 7.3 Language choice

- **Rust** for strategies that must be co-located with obird (share the type system, compile in same binary). Current `hl_spread_quoter`, `prediction_quoter`, `predict_hedger` stay here.
- **Python** permitted for cross-venue strategies in network mode — use a `nats-py` client and share the `Action` schema via protobuf or msgpack. Lets quants iterate faster. The Rust bar is a tax for co-located strategies, not a mandate for all.

### 7.4 Position state

Single source of truth = `position-service` (Rust, central). Aggregates fills from all `order.*` subjects, maintains per-(venue, market) position + P&L. Exposes via NATS request/reply for strategy queries and via HTTP for dashboard.

Don't replicate position state in every strategy. Query on init, subscribe to deltas.

---

## 8. Market Data Ingest + Quant Tap (the dual-path question)

This is one of the user's explicit design concerns: live MD path must not be slowed by quant persistence.

### 8.1 The split

**Inside each `md-<venue>` process**:
```
     WS feed ─┐
              ├─→ MarketDataSink (trait, in-process)
              │      │
              │      ├─→ local broadcast (obird, FV, etc — in-process subscribers)
              │      └─→ NATS publish (cross-process)
              │
              └─→ Raw NDJSON line-writer (always-on, local SSD)  ← quant tap tier 0
```

**Outside the process**:
```
  NATS subject md.<venue>.<instrument>.book
        │
        ├─→ JetStream stream "md_archive" (retention 7d, 4 replicas)
        │       │
        │       └─→ Durable consumer "quant-tap"
        │              │
        │              ├─→ QuestDB (via ILP socket, sub-ms writes)
        │              └─→ S3 Parquet writer (batches, writes every 5min)
        │
        └─→ Latest-value subscribers (FV, Strategy) — no JetStream, NATS Core
```

The live path uses NATS Core (fire-and-forget, latest-value). The quant path uses JetStream (durable, at-least-once). **A backpressure on QuestDB or S3 cannot propagate to the live path** — separate subscribers, separate semantics.

### 8.2 Tier-0 local NDJSON

Every md-ingest writes a raw NDJSON log to local SSD unconditionally (one file per day per instrument). This is the ultimate safety net: if NATS/JetStream is down entirely, we still have the data. Rotated to S3 daily via a cron-triggered `aws s3 cp`.

Cost: 10-50 GB/day per venue @ gp3 → ~$5/mo per region for the buffer.

### 8.3 Schema

Normalized schema (applied at md-ingest):

```sql
-- QuestDB table
CREATE TABLE book_updates (
  ts_ns TIMESTAMP,
  exchange_ts_ns TIMESTAMP,
  venue SYMBOL CAPACITY 16,
  instrument SYMBOL CAPACITY 10000 INDEX,
  side SYMBOL CAPACITY 2,
  level INT,
  price DOUBLE,
  size DOUBLE,
  seq LONG
) TIMESTAMP(ts_ns) PARTITION BY DAY WAL;

CREATE TABLE trades (ts_ns, venue, instrument, price, size, side, exchange_trade_id);
CREATE TABLE fills (ts_ns, venue, instrument, side, price, size, fee, order_id, strategy);
CREATE TABLE fv_snapshots (ts_ns, symbol, fv, confidence, model);
```

Partitioned daily; WAL enabled for safety. Retention: 90d hot on QuestDB, then compacted to daily Parquet on S3 (zstd level 9), kept 2y.

### 8.4 Why QuestDB

- **Built for market data**: `LATEST ON` finds the latest book state per instrument in 1-2ms vs ClickHouse's 30-60ms.
- **ASOF JOIN / SAMPLE BY / WINDOW JOIN** are native — microstructure research without query contortion.
- **ILP (InfluxDB Line Protocol) ingest** — sub-ms inserts, ~1M rows/sec on a c7g.xlarge.
- **SQL Postgres wire** — use any BI/notebook tool.
- **Open source community edition** — no license cost.

Where ClickHouse wins: heavy multi-day range aggregations. For those, we query the S3 Parquet archive from DuckDB (embedded, free, fast). Best of both worlds.

---

## 9. Messaging Substrate (NATS decision)

### 9.1 Decision: NATS JetStream as the sole cross-process substrate

| Need | NATS subject style | Delivery |
|------|-------------------|----------|
| Market data fan-out (FV, strategy) | `md.>` | NATS Core (latest-value, slow-consumer drops) |
| Market data persistence (quant tap) | Same subjects, JetStream stream `md_archive` | Durable, at-least-once |
| Fair value fan-out | `fv.>` | NATS Core |
| Actions (strategy → engine) | `action.<venue>.<market>` | JetStream with work-queue consumer, exactly-once via `action_id` idempotency |
| Order updates (engine → all) | `order.>` | JetStream, durable consumers per interested service |
| Heartbeats / health | `engine.>`, `strategy.>` | NATS Core |
| RPC (position query etc) | NATS req/reply | Core |

One server, one set of ops runbooks.

### 9.2 Why not Kafka / Redpanda

- Redpanda wins on raw p99 throughput but the cost is Kafka-protocol operational surface: ACL management, Schema Registry, MirrorMaker for cross-region. 2-person shop doesn't want that.
- NATS JetStream gives "good enough" latency (sub-ms in-memory, 1-5ms persisted) for farming-grade 5-20ms targets. For sub-ms strategies, they run co-located and use in-process channels anyway — Kafka vs NATS is irrelevant on the hot path.
- NATS Supercluster (gateways between regions) is a single config change. Redpanda cross-region is heavier.

### 9.3 Why not gRPC for actions

- Actions are naturally pub/sub: the Controller emits, obird consumes. gRPC forces a point-to-point coupling and re-solves service discovery.
- JetStream's work-queue semantics + the `action_id` idempotency give us at-most-once *effective* delivery with the pub/sub shape.
- Keeps everything on NATS. Fewer moving parts.

### 9.4 NATS deployment

- **Per-region cluster**: 3x t4g.small in each region (ap-northeast-1, eu-west-2, us-east-1). ~$20/mo/region.
- **Supercluster gateways** between regions — cross-region egress only for subjects that actually need to cross (use `queue groups` + `sourcing` rules). Don't ship Tokyo L2 book data to London.
- **JetStream**: streams with replica=3 within region, `sources` to pull selected subjects to the central hub for persistence.
- **mTLS + auth** via NATS JWT + nkeys. Vault-style key management: static short-lived JWTs, rotated via CI.

### 9.5 When to reconsider

- If a single strategy needs > 500k messages/sec (current obird is < 10k/sec): Redpanda.
- If we need replay beyond 7 days in-memory / 90 days on disk: move archive to Kafka-compatible store.
- If cross-region egress cost breaks $500/mo: pin more services to a single region.

---

## 10. Storage / Data Lake

### 10.1 Tiering

| Tier | Store | Retention | Query path |
|------|-------|-----------|------------|
| T0 (in-process) | Rust `broadcast` | <1s | Strategy only |
| T1 (hot) | QuestDB on EC2 | 90d | SQL via Postgres wire, Grafana, notebooks |
| T2 (cold) | S3 Parquet + DuckDB | 2y | DuckDB embedded (run anywhere) |
| T3 (archive) | S3 Glacier Deep Archive | 5y+ | Only for regulatory/forensic |

### 10.2 S3 layout

```
s3://obird-quant-hot/
  venue=<venue>/
    table=<book_updates|trades|fills|fv>/
      date=YYYY-MM-DD/
        instrument=<instrument>/
          part-00.parquet
```

Hive-style partitioning. DuckDB reads this directly with `read_parquet('s3://...**/*.parquet')`.

### 10.3 Compaction job

Daily cron on the central host:
1. Dump previous day from QuestDB to staging Parquet (per table per instrument)
2. zstd compress, write to S3 hot bucket
3. Verify row count = source row count
4. Drop partition from QuestDB if > 90d

Written in Python with `pyarrow` + `questdb-client`. ~50 lines.

### 10.4 Research access pattern

- **Interactive**: Jupyter notebooks on central host. `questdb-connect` or raw psycopg for hot; `duckdb` for cold.
- **Reproducibility**: all notebooks live in `obird/research/`, committed.
- **Feature store (phase 3)**: Featherlite store in Postgres for features used by ML FV models.

---

## 11. Control Plane / Dashboard

### 11.1 Stack (new Next.js app)

- **Frontend**: Next.js 15 (App Router), React 19, TailwindCSS, shadcn/ui.
- **API**: tRPC or Hono.
- **DB**: AWS RDS Postgres db.t4g.small.
- **Realtime**: WS subscription to NATS (via `nats.ws` client) + server-sent events for metrics.
- **Auth**: Clerk or Auth.js with GitHub OAuth — 2 users, nothing fancy.
- **Hosting**: Vercel (free tier covers 2 users) or EC2 with Caddy.

### 11.2 Surface

- **Fleet health**: live status per obird, md-ingest, FV, strategy. Pulled from `*.health` NATS subjects.
- **Positions + PnL**: live cross-venue, 1s refresh.
- **Order book**: resting orders per market, with one-click flatten.
- **Strategy params**: hot-reload TOML, stored in Postgres, pushed to services via NATS config subject.
- **Kill switches**: global, per-venue, per-market.
- **Quant queries**: saved query runner against QuestDB (Metabase or simple built-in SQL pane).
- **Audit log**: every action + decision, from NATS JetStream replay into Postgres.

### 11.3 Why separate from mission-control

Mission-control is your portfolio/fund dashboard (holdings, fund perf). This is an ops console — different audience, different data freshness, different auth scope. They can share Clerk SSO and cross-link, but don't share DB or deployment.

---

## 12. Risk & PnL

### 12.1 Risk gate (currently stub)

Lives inside `order-router` per engine. Checks pre-action:
- Per-market position limit
- Per-venue exposure limit
- Global $ notional at risk
- Daily drawdown limit (kill switch at -X%)
- Rate limits (max orders/sec per connector)

Implementation: pull limits from Postgres at engine start, subscribe to `risk.limits.changed` for hot-reload. Fast check (all in-memory).

### 12.2 PnL service (new)

- Subscribes to `order.*` and `fill.*` across all venues.
- Marks to market using latest `fv.*` or md.
- Publishes `pnl.<venue>.<market>` + `pnl.portfolio` at 1Hz.
- Persists snapshot to Postgres every minute.

Unified risk (ADR-005) stays in spec; this is the phase-2 implementation.

---

## 13. Deployment / Infra

### 13.1 AWS regions

| Region | Services | Rationale |
|--------|----------|-----------|
| `us-east-1` | Central services: FV svc, Controller, QuestDB, Postgres, NATS hub, dashboard, Grafana | Cheapest, closest to team, Polymarket tolerable at 130ms |
| `eu-west-2` | md-poly, md-predict, obird-poly, obird-predict, NATS | Polymarket colo |
| `ap-northeast-1` | md-hl, obird-hl, md-binance (future), NATS | HL colo, Tokyo is HL's validator region |

**Future regions** (phase 3):
- `us-east-2` (Ohio) or Equinix Chicago non-AWS: md-kalshi, obird-kalshi
- `ap-southeast-1` (Singapore): alt for Binance

### 13.2 Networking

- **VPC per region**, private subnets only for engines (no public IPs).
- **VPC peering** between the three regions (cheap, ~$0.02/GB intra-AWS).
- **NATS Supercluster gateways** between regional clusters — mTLS over TCP 4222. Gateways traverse peering.
- **Tailscale mesh** overlay for ops access (SSH, QuestDB Postgres port, dashboards). Tailscale free tier covers 2 users.
- **No public exchange connectivity** beyond what each venue requires (all outbound from engines; no inbound except through bastion).

### 13.3 Compute

| Role | Instance | Monthly est. |
|------|----------|--------------|
| obird engine x3 (poly, predict, hl) | c7g.large (2 vCPU, 4GB) | $60/ea = $180 |
| md-ingest x3 | c7g.medium | $30/ea = $90 |
| FV service | c7g.large | $60 |
| Strategy/Controller | c7g.large | $60 |
| NATS cluster: 3x3 = 9 nodes | t4g.small | $15/ea = $135 |
| QuestDB | r7g.xlarge (4 vCPU, 32GB, 1TB gp3) | $250 |
| Postgres (RDS) | db.t4g.small | $25 |
| Dashboard | Vercel free / EC2 t4g.small | $0-15 |
| Grafana + Prom + Loki | t4g.medium | $30 |
| **Compute subtotal** | | **~$840** |

### 13.4 Storage & egress

| Item | Cost |
|------|------|
| gp3 EBS for QuestDB + md buffers | ~$100 |
| S3 Parquet hot (90d active) | $15-30 |
| S3 Parquet cold (2y archive) | $20-50 |
| Cross-region data transfer | $80-200 depending on MD volume |
| NAT gateway (per region) | 3x $32 = $96 |
| **Storage + egress subtotal** | **~$300-500** |

**Grand total: $1.1–1.4k/mo steady-state.** Starts lower in phase 1 (~$600/mo — no QuestDB, no cross-region supercluster, single venue pair). Can trim by ~$150 by replacing NAT gateways with NAT instances (t4g.nano, acceptable for outbound-only).

### 13.5 Deploy pipeline

- **Infra**: Terraform in `obird/infra/`. One module per region.
- **Build**: GitHub Actions on push to main → cargo build in container → push to ECR (one repo per binary).
- **Deploy**: systemd on EC2, unit files managed by Ansible playbook OR SSM Run Command that pulls the latest ECR tag and restarts the service.
- **No Kubernetes**. EC2 + systemd + Terraform is 10x less operational overhead for this scale.
- **Secrets**: AWS Secrets Manager, pulled at service start by systemd via `systemd-creds`. No secrets in TOML.

### 13.6 CI/CD gates

- `cargo clippy --workspace -- -D warnings` and `cargo test --workspace` must pass.
- Integration smoke test: spin up sim-connector + FV svc + strategy, feed recorded day, assert P&L within tolerance.
- Deploy is one-by-one per service per region. Canary: deploy to eu-west-2 obird-poly first, 30 min soak, then roll.
- Rollback: `systemctl start obird@<old-tag>` — keep 3 tags on each host.

---

## 14. Observability

- **Metrics**: Prometheus scrape of each service (already wired in `crates/telemetry`). Node exporter on every host. Central Prom in us-east-1.
- **Logs**: structured JSON to `/var/log/obird/*.jsonl` → promtail → Loki. Retention 30d.
- **Traces**: OpenTelemetry OTLP to central collector → Tempo. Trace every Action → Order → Fill as one span tree.
- **Dashboards**: Grafana in central region. Pre-built: fleet overview, per-venue latency, FV freshness, connector health, P&L.
- **Alerts**: Grafana alerting → PagerDuty free tier (or Opsgenie / plain SNS → SMS). Core alerts:
  - FV stale > 2 min on any active market
  - obird engine unhealthy > 30s
  - Cross-region NATS gateway down
  - Position limit breach
  - Drawdown beyond daily threshold

---

## 15. Tech Stack Summary

| Layer | Choice | Why |
|-------|--------|-----|
| Engine / MD / FV / Strategy | Rust, tokio | Existing, performance, type safety |
| Messaging | NATS JetStream | Unified substrate, right semantics, 2-person ops |
| Hot DB | QuestDB | Built for tick data, SQL-compatible, free |
| Cold DB | S3 Parquet + DuckDB | Cheap, queryable anywhere |
| Control DB | Postgres (RDS) | Boring, reliable, already know it |
| Dashboard | Next.js 15 + tRPC + Tailwind | Fast to build, already in stack |
| Auth | Clerk or Auth.js | 2 users, don't overthink |
| IaC | Terraform | Industry standard |
| Orchestration | systemd on EC2 | Not Kubernetes |
| CI | GitHub Actions | Already using |
| Secrets | AWS Secrets Manager | Native |
| Observability | Grafana + Prom + Loki + Tempo | Self-hosted, cheap |
| VPN | Tailscale | Free tier, zero-config |

---

## 16. Phased Rollout

### Phase 1 — "Split the monolith" (4-6 weeks)

**Goal**: obird becomes a pure engine; FV + Strategy run as separate processes on the same box; NATS is wired but only one region.

- [ ] Add NATS subject contract for Actions + OrderUpdates (single-region first)
- [ ] Refactor current in-process Strategy→Engine handoff to go through NATS (same host first, keep hot path on localhost)
- [ ] Extract `PredictionQuoter`'s poly-mid FV computation into `fair-value-service` binary
- [ ] Extract `md-ingest` per venue into its own binary
- [ ] Single-region deploy (eu-west-2) for Poly + predict.fun
- [ ] Keep HL running as monolith for now
- [ ] **Exit criteria**: all current predict.fun farming migrated, same P&L, no new bugs from split

### Phase 2 — "Quant lake + dashboard" (3-4 weeks)

- [ ] QuestDB on EC2 + ILP ingest from md-ingest services
- [ ] S3 Parquet compaction cron
- [ ] Control plane dashboard MVP: fleet, positions, kill switches
- [ ] Cross-region NATS (add ap-northeast-1 for HL, set up supercluster)
- [ ] Migrate HL engine off monolith
- [ ] **Exit criteria**: 7 days of L2 data queryable in QuestDB; one-click kill switch per venue

### Phase 3 — "Scale venues" (ongoing)

- [ ] Binance connector wired (already built, just wire to live runner)
- [ ] Multi-market-single-process fix (engine key change)
- [ ] Lighter + Kalshi connectors
- [ ] ML FV model (research, then productionize)
- [ ] Full PnL + risk gate
- [ ] Backtest harness wired to CLI (`README.md §12` gap #6)

---

## 17. Key Decisions for Partner Review

These are where I'd value an explicit call-out before commit:

1. **NATS vs Redpanda** — I chose NATS. If your partner has Kafka ops background, Redpanda might actually be cheaper in person-hours. Willing to revisit.
2. **QuestDB vs ClickHouse** — QuestDB is the clear quant choice. Only risk is community edition support + we may grow out of it at 10M+ msg/sec.
3. **Strategy language**: Rust-only vs Rust+Python. I'd open the door to Python for cross-venue network-mode strategies. Keep HFT strategies in Rust. Question: is this worth the schema-sharing tax?
4. **Dashboard hosting**: Vercel vs self-host EC2. Vercel is fastest; EC2 keeps everything in one AWS account and avoids a new bill. Lean slightly Vercel.
5. **Kalshi infra**: not AWS-native. Either accept us-east-2 (~20ms to Chicago) or add Equinix Chicago VPS as a one-off. I'd defer until Kalshi's actually on the farming sheet.
6. **Secrets**: AWS Secrets Manager vs SOPS-in-git. SOPS is faster but audit trail is weaker. AWS SM wins for 2-person shop.
7. **In-process vs network strategy mode default**: I'd default strategies to network mode (separate process) for the clean seam, and only force in-process for HL MM on size. Partner opinion?

---

## 18. Open Questions / Risks

- **predict.fun server location unknown** — pragmatically colocating in London but should verify. If their WS is US-East, restructure.
- **Lighter sequencer location unknown** — may push us to a different region later.
- **NATS JetStream at > 500k msg/sec per subject** — untested at that scale on our target instances. Will benchmark during phase 1.
- **QuestDB backup story** — native backup is OK but replica-less. Consider RDS-style multi-AZ if we become dependent on hot queries.
- **Reg risk on Kalshi** — US-regulated contract market. Requires legal review we haven't done. Keep out of MVP.
- **Key material**: `PREDICT_PRIVATE_KEY` doubles as Polymarket signing key per current arch. Need to revisit custody model before we grow — consider per-venue HSM-backed keys (AWS KMS or Fireblocks) at phase 3.

---

## 19. Appendix A — Venue Location Reference

| Venue | Infra | AWS Region | Our Engine Region | RTT from our engine |
|-------|-------|------------|-------------------|---------------------|
| Hyperliquid | AWS Tokyo validators | ap-northeast-1 | ap-northeast-1 | ~2-3ms |
| Polymarket CLOB | AWS London | eu-west-2 | eu-west-2 | <1ms |
| predict.fun | BNB chain + WS (unknown) | assume EU | eu-west-2 | TBD, verify |
| Binance | Distributed (Tokyo + Singapore + Frankfurt) | multiple | ap-northeast-1 | 1-5ms |
| Kalshi | Chicago Equinix (likely) | non-AWS | us-east-2 | ~15-20ms |
| Lighter | Unknown | unknown | us-east-1 (placeholder) | TBD |

## 20. Appendix B — Action/Event Schemas (abbreviated)

See `crates/core/src/action.rs` and `crates/core/src/event.rs` for canonical defs. Network schemas are a msgpack serialization of these with a stable `schema_version` field.

Breaking changes to `Action` or `Event` require schema version bump + dual-version support on engine for one release window.

---

## 21. Appendix C — What's reused vs rebuilt

**Reused as-is** from current obird:
- `crates/core` (Strategy, ExchangeConnector, Action, Event, MarketDataSink traits) — the contract layer is the single most important asset; do not churn.
- All connector crates (hyperliquid, polymarket, predict_fun, binance) — promote them behind a stable API surface.
- Strategy crates — `hl_spread_quoter`, `prediction_quoter`, and `predict_hedger` all stay (predict_hedger is the poly-NO delta-neutral hedge on predict YES fills, live 2026-04-16). Only the transport seam changes (in-proc mpsc → NATS for network mode).
- Order state machine inside `engine` — formalize as documented above.
- Pricing logic in `prediction_quoter` — lift into FV service + simplified strategy.

**Rebuilt or extracted**:
- `fair_value_service` (skeleton → full service with NATS publisher)
- `md-ingest-<venue>` binaries (currently embedded in connectors, extract)
- `strategy-controller` (currently embedded, extract for network mode)
- Control plane dashboard (new)
- Quant tap pipeline (new)
- NATS cluster + JetStream streams (new)
- Risk + PnL service (currently stubs)

**Retired**:
- `scripts/farm.py` (replaced by multi-market-in-process obird once engine key change lands)
- Per-strategy hardcoded FV logic (moves to FV service)

---

**End of PRD**. Ready for partner review.
