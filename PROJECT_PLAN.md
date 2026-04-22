# obird v2 — Project Plan & Task Tracking

> **Status**: Planning → Ready for execution
> **Last updated**: 2026-04-22
> **Owner**: Z
> **Timeline**: Phase 1 (6 weeks) → Phase 2 (4 weeks) → Phase 3 (ongoing)
> **Budget**: ~$600/mo MVP → ~$1.5k/mo at scale

---

## Executive Summary

Transform obird from a monolith into a cleanly-composed multi-service platform:
- **obird** → pure per-venue OMS/execution engine
- **Fair Value Service** + **Strategy Controller** → separate binaries
- **NATS JetStream** → cross-process messaging
- **QuestDB + S3** → quant data lake
- **3-region AWS** → Tokyo (HL), London (Poly/predict), US-East (central services)

**Target**: Scale farming to $100k/mo across 5+ venues with 2-person operational overhead.

**Reference**: `PRD_FARMING_PLATFORM.md` for full architecture.

---

## Phase Milestones

| Phase | Goal | Duration | Dependencies | Exit Criteria |
|---|---|---|---|---|
| **Phase 1** | Split monolith, NATS wired, single-region | 4-6 weeks | None | All predict.fun farming migrated, same P&L, no new bugs |
| **Phase 2** | Quant lake + dashboard + multi-region | 3-4 weeks | Phase 1 complete | 7d L2 data in QuestDB, one-click kill switches, HL migrated off monolith |
| **Phase 3** | Scale venues (Binance/Lighter/Kalshi) | Ongoing | Phase 2 complete | New venues live, ML FV models in prod, backtest CI gate |

---

## Phase 1: Split the Monolith (4-6 weeks)

**Goal**: Decouple engine/strategy/FV with NATS as the transport, prove it on one region.

### 1.1 NATS Infrastructure Setup (Week 1)

**Owner**: Z  
**Blockers**: None  
**Deliverables**:
- [ ] 1.1.1 Deploy 3x t4g.small NATS cluster in eu-west-2 (Terraform module)
- [ ] 1.1.2 Configure JetStream streams: `md_archive`, `actions`, `order_updates`
- [ ] 1.1.3 Wire mTLS + nkey auth (AWS Secrets Manager for keys)
- [ ] 1.1.4 Test latency: localhost Core publish/subscribe < 200μs
- [ ] 1.1.5 Test JetStream work-queue consumer (action idempotency)

**Acceptance**:
- NATS cluster healthy, 3 replicas, <1ms cross-node latency
- Test harness proves idempotent delivery on `action.*` subjects

**Open Questions**:
- JetStream retention on `md_archive`: start with 7d, OK?
- Auth: static JWTs rotated monthly or Vault integration? (Lean static for MVP)

**GitHub Label**: `phase-1` `infra` `nats`

---

### 1.2 NATS Subject Contract + Schemas (Week 1)

**Owner**: Z  
**Blockers**: None (can run parallel with 1.1)  
**Deliverables**:
- [ ] 1.2.1 Document NATS subject hierarchy in `docs/NATS_SUBJECTS.md`:
  - `md.<venue>.<instrument>.book` / `.trade` / `.fill`
  - `fv.<symbol>`
  - `action.<venue>.<market>`
  - `order.<venue>.<market>.<order_id>`
  - `engine.<venue>.health`
- [ ] 1.2.2 Define wire schemas (msgpack or protobuf?) for:
  - `Action` (place/cancel/replace)
  - `OrderUpdate` (placed/acked/fill/rejected)
  - `FairValue` (fv/confidence/sources/model)
- [ ] 1.2.3 Add `schema_version` field to each message type
- [ ] 1.2.4 Write schema evolution policy (dual-version support for 1 release)

**Acceptance**:
- All subject patterns documented
- Schema definition file committed (`schemas/action.proto` or `.msgpack.json`)

**Open Questions**:
- **msgpack vs protobuf?** (Lean msgpack: simpler, Rust `serde` support, no codegen)
- Schema registry needed or just versioned files in git?

**GitHub Label**: `phase-1` `contract` `schema`

---

### 1.3 Extract Market Data Ingest Binaries (Week 2)

**Owner**: Z  
**Blockers**: 1.2 (needs subject contract)  
**Deliverables**:
- [ ] 1.3.1 Create `crates/md-ingest/` with common binary scaffold
- [ ] 1.3.2 Extract Polymarket feed: `md-ingest-poly` binary
  - Consumes `PolymarketMarketDataFeed` (existing crate)
  - Publishes to `md.polymarket.<token_id>.book` via NATS Core
  - Publishes to JetStream `md_archive` stream
  - Tier-0 NDJSON to `/var/log/md-ingest/poly-YYYY-MM-DD.jsonl`
- [ ] 1.3.3 Extract predict.fun feed: `md-ingest-predict`
- [ ] 1.3.4 Add systemd unit files for both
- [ ] 1.3.5 Test: verify NATS subjects populated, NDJSON rotates daily

**Acceptance**:
- Both feeds run as separate processes, publish to NATS
- Tier-0 NDJSON logs written to local SSD, rotated by date
- In-process broadcast still works (for co-located strategies during transition)

**Open Questions**:
- Should md-ingest also expose HTTP health endpoint? (Yes, /health + Prometheus /metrics)

**GitHub Label**: `phase-1` `md-ingest` `extraction`

---

### 1.4 Fair Value Service Extraction (Week 2-3)

**Owner**: Z  
**Blockers**: 1.2 (needs NATS contract), 1.3 (needs MD ingest)  
**Deliverables**:
- [ ] 1.4.1 Promote `crates/fair_value_service` from stub to full binary
- [ ] 1.4.2 Implement pluggable FV models:
  - `mid` (single venue BBO)
  - `cross_venue_conservative` (current `PredictionQuoter` logic)
  - `microprice` (depth-weighted)
- [ ] 1.4.3 Subscribe to `md.<venue>.<instrument>.book` via NATS
- [ ] 1.4.4 Publish to `fv.<symbol>` (NATS Core, latest-value)
- [ ] 1.4.5 Config file: map symbols to models + source venues
- [ ] 1.4.6 Add staleness monitoring: emit warning if no FV update in >2s

**Acceptance**:
- FV service runs standalone, publishes Poly + predict FV
- `PredictionQuoter` can subscribe to `fv.*` instead of computing inline
- Config-driven model selection (no hardcoded venue pairs)

**Open Questions**:
- **Colocation**: run FV service in us-east-1 (central) or eu-west-2 (colocated with poly)? (Start central, measure latency)
- Store FV model state (e.g., EMA) in Redis or in-memory only? (In-memory for MVP)

**GitHub Label**: `phase-1` `fv-service` `extraction`

---

### 1.5 Refactor Engine: NATS Action/Event Transport (Week 3-4)

**Owner**: Z  
**Blockers**: 1.2 (needs contract), 1.4 (needs FV service)  
**Deliverables**:
- [ ] 1.5.1 Add `ActionTransport` trait: `in_process(mpsc)` vs `nats(JetStream)`
- [ ] 1.5.2 Add `EventTransport` trait: `in_process(broadcast)` vs `nats(Core)`
- [ ] 1.5.3 Wire `OrderRouter` to consume `action.<venue>.<market>` via NATS
- [ ] 1.5.4 Wire `OrderManager` to publish `order.<venue>.<market>.<oid>` via NATS
- [ ] 1.5.5 Add idempotency layer in engine: track `action_id` → `order_id` map
  - If duplicate place with same price/size → no-op + ack
  - If duplicate cancel → no-op + ack
- [ ] 1.5.6 Test: run strategy in separate process, prove Actions round-trip

**Acceptance**:
- Engine can run with `--transport=nats` flag
- Strategies can run co-located (in-process) OR network mode (separate binary)
- Idempotency proven: duplicate Actions don't spam exchange

**Open Questions**:
- **Backwards compat**: keep in-process mode as default during rollout? (Yes, feature-flag it)
- Action timeout: if no ack in Xs, re-publish or dead-letter? (Dead-letter after 30s)

**GitHub Label**: `phase-1` `engine` `nats` `refactor`

---

### 1.6 Extract Strategy Controller Binary (Week 4)

**Owner**: Z  
**Blockers**: 1.5 (needs Action transport)  
**Deliverables**:
- [ ] 1.6.1 Create `crates/strategy-controller/` binary scaffold
- [ ] 1.6.2 Move `PredictionQuoter` logic into controller:
  - Subscribe to `fv.<symbol>` (from FV service)
  - Subscribe to `order.<venue>.<market>.*` (for own-order state)
  - Emit `Action` to `action.<venue>.<market>`
- [ ] 1.6.3 Add config hot-reload: watch TOML, re-parse on change
- [ ] 1.6.4 Add kill-switch subscription: `control.kill_switch.<venue>`
- [ ] 1.6.5 Prove strategy runs in network mode (separate process from engine)

**Acceptance**:
- `strategy-controller` binary runs standalone, quotes predict.fun
- Engine receives Actions over NATS, executes, returns Events
- No inline FV computation in strategy (all via `fv.*` subscription)

**Open Questions**:
- **Language**: keep Rust or allow Python for network-mode strategies? (Rust for now, revisit in Phase 3)
- How to handle position state? Query `position-service` or track locally? (Track locally for now, Phase 2 adds position-service)

**GitHub Label**: `phase-1` `strategy` `controller` `extraction`

---

### 1.7 Single-Region Deploy + Migration (Week 5-6)

**Owner**: Z  
**Blockers**: 1.3, 1.4, 1.5, 1.6 (all components ready)  
**Deliverables**:
- [ ] 1.7.1 Deploy to eu-west-2 (London):
  - 3x t4g.small NATS cluster
  - 1x c7g.large `obird-engine` (Poly + predict)
  - 1x c7g.medium `md-ingest-poly`
  - 1x c7g.medium `md-ingest-predict`
  - 1x c7g.large `fair-value-service`
  - 1x c7g.large `strategy-controller`
- [ ] 1.7.2 Terraform module: `infra/phase1-single-region/`
- [ ] 1.7.3 Systemd units for all services
- [ ] 1.7.4 Secrets via AWS Secrets Manager (no .env in systemd)
- [ ] 1.7.5 Test: run one predict.fun market end-to-end
- [ ] 1.7.6 Migrate all predict.fun farming from monolith
- [ ] 1.7.7 Compare P&L: 7-day window before/after migration

**Acceptance**:
- All predict.fun markets running via new architecture
- P&L delta < 2% (allowing for market noise)
- No new bugs/crashes vs monolith
- `scripts/farm.py` deprecated (multi-market-single-process working)

**Open Questions**:
- Rollback plan if migration fails? (Keep monolith build tagged, can revert in <5 min)
- Monitoring: Grafana dashboard ready or just NATS metrics? (Basic NATS dashboard + engine health checks, full Grafana in Phase 2)

**GitHub Label**: `phase-1` `deploy` `migration`

---

### 1.8 Multi-Market Single-Process Fix (Week 5)

**Owner**: Z  
**Blockers**: None (can run parallel with 1.7)  
**Deliverables**:
- [ ] 1.8.1 Change engine key from `HashMap<Exchange, Connector>` to `HashMap<InstrumentId, Connector>`
- [ ] 1.8.2 Update `OrderRouter` and `EngineRunner` to support multi-market per exchange
- [ ] 1.8.3 Test: one `obird-engine` process quotes 3+ predict.fun markets simultaneously
- [ ] 1.8.4 Retire `scripts/farm.py` crash-loop orchestration

**Acceptance**:
- Single engine process serves all predict.fun markets
- One Polymarket WS connection serves all FV subscriptions
- Process count drops from N (markets) to 1 per venue

**Open Questions**:
- Does this break HL? (No, HL is already single-instrument)

**GitHub Label**: `phase-1` `engine` `multi-market`

---

## Phase 2: Quant Lake + Dashboard + Multi-Region (3-4 weeks)

**Goal**: Add persistent data storage, control plane UI, and cross-region NATS for HL.

### 2.1 QuestDB Deployment + Ingestion (Week 7-8)

**Owner**: Z  
**Blockers**: Phase 1 complete (needs md-ingest publishing to NATS)  
**Deliverables**:
- [ ] 2.1.1 Deploy QuestDB on r7g.xlarge in us-east-1:
  - 1TB gp3 EBS
  - Postgres wire + ILP socket exposed
- [ ] 2.1.2 Create `quant-tap` consumer (Rust or Vector):
  - Subscribe to JetStream `md_archive` stream
  - Write to QuestDB via ILP (sub-ms writes)
- [ ] 2.1.3 Define QuestDB schemas (PRD Appendix §8.3):
  - `book_updates`, `trades`, `fills`, `fv_snapshots`
- [ ] 2.1.4 Test: verify 90d retention, query latency <50ms for BBO reconstruction
- [ ] 2.1.5 Add Grafana data source + sample dashboard (book depth viz)

**Acceptance**:
- QuestDB ingesting live MD from all venues
- Query: "Latest BBO per instrument" returns in <50ms
- 7 days of tick data queryable

**Open Questions**:
- **Partitioning**: daily WAL partitions OK or need hourly? (Daily OK for MVP)
- Backup strategy: QuestDB snapshots to S3 daily? (Yes, cron at 02:00 UTC)

**GitHub Label**: `phase-2` `questdb` `quant`

---

### 2.2 S3 Parquet Archive (Week 8)

**Owner**: Z  
**Blockers**: 2.1 (needs QuestDB live)  
**Deliverables**:
- [ ] 2.2.1 Create S3 bucket: `s3://obird-quant-hot/`
- [ ] 2.2.2 Write compaction script (Python + `pyarrow`):
  - Dump prior day from QuestDB per table per instrument
  - zstd compress, write to S3 Hive-partitioned layout
  - Verify row count matches source
  - Drop QuestDB partition if > 90d
- [ ] 2.2.3 Deploy as daily cron (02:00 UTC on central host)
- [ ] 2.2.4 Test: query S3 Parquet with DuckDB embedded

**Acceptance**:
- Compaction runs successfully for 1 day
- S3 bucket contains Parquet files, queryable via DuckDB
- QuestDB disk usage stays <100GB (90d hot retention enforced)

**Open Questions**:
- Glacier Deep Archive after 2y? (Yes, lifecycle policy auto-transitions)

**GitHub Label**: `phase-2` `s3` `parquet` `archive`

---

### 2.3 Control Plane Dashboard (Week 8-9)

**Owner**: Z (or delegate to subagent for Next.js scaffold)  
**Blockers**: 2.1 (needs live data)  
**Deliverables**:
- [ ] 2.3.1 Scaffold Next.js 15 app in `obird/dashboard/`:
  - TailwindCSS + shadcn/ui
  - Clerk or Auth.js for GitHub OAuth (2 users)
- [ ] 2.3.2 Deploy RDS Postgres db.t4g.small in us-east-1 (control DB)
- [ ] 2.3.3 Wire tRPC or Hono API layer
- [ ] 2.3.4 Build pages:
  - **Fleet Health**: live status per service (poll `*.health` NATS subjects)
  - **Positions**: query `position-service` (stub for now, show open orders)
  - **Kill Switches**: publish to `control.kill_switch.<venue>`
  - **Strategy Params**: hot-reload TOML (store in Postgres, push via NATS)
- [ ] 2.3.5 Add WS subscription to NATS for realtime updates
- [ ] 2.3.6 Deploy to Vercel free tier or EC2 + Caddy

**Acceptance**:
- Dashboard accessible at `https://obird-dash.<domain>`
- Can view live fleet health (engine status, connector WS status)
- Can toggle kill switch per venue, see effect in <2s

**Open Questions**:
- **Hosting**: Vercel vs self-host? (Lean Vercel for speed, can migrate later)
- Auth: Clerk ($25/mo) vs Auth.js free? (Auth.js for 2 users)

**GitHub Label**: `phase-2` `dashboard` `ui` `control-plane`

---

### 2.4 Cross-Region NATS + HL Migration (Week 9-10)

**Owner**: Z  
**Blockers**: Phase 1 complete, 2.1 live (ensures single-region stable)  
**Deliverables**:
- [ ] 2.4.1 Deploy NATS cluster in ap-northeast-1 (Tokyo):
  - 3x t4g.small
  - Configure supercluster gateway to eu-west-2 + us-east-1
- [ ] 2.4.2 Deploy md-ingest-hl + obird-hl in Tokyo:
  - c7g.large instances
  - Extract `HlSpreadQuoter` into strategy-controller (or keep co-located)
- [ ] 2.4.3 Wire NATS subject routing:
  - `md.hyperliquid.*` stays local to Tokyo
  - `fv.HL-*` published from Tokyo, consumed in central FV service
  - `action.hyperliquid.*` published from central, consumed in Tokyo engine
- [ ] 2.4.4 Test: measure cross-region latency (Tokyo ↔ us-east-1)
- [ ] 2.4.5 Migrate HL trading from monolith to new architecture

**Acceptance**:
- HL spread MM running via new arch
- Cross-region Action → Order → Fill latency < 20ms (p95)
- No P&L degradation vs monolith

**Open Questions**:
- **HlSpreadQuoter**: keep co-located (in-process) or promote to network mode? (Co-located for latency, revisit if >10ms overhead)
- VPC peering vs public IPs for NATS gateways? (VPC peering, cheaper + secure)

**GitHub Label**: `phase-2` `multi-region` `hl` `migration`

---

### 2.5 Risk Gate + Position Service (Week 10)

**Owner**: Z  
**Blockers**: 2.3 (needs dashboard for config management)  
**Deliverables**:
- [ ] 2.5.1 Promote `UnifiedRiskManager` from stub to full implementation:
  - Check per-market position limit before place
  - Check per-venue exposure limit
  - Check global $ notional at risk
  - Check daily drawdown limit (emit kill switch if breached)
- [ ] 2.5.2 Create `position-service` (Rust):
  - Subscribe to `order.*` fills from all venues
  - Maintain per-(venue, market) position + unrealized PnL
  - Expose via NATS request/reply + HTTP for dashboard
- [ ] 2.5.3 Persist position snapshots to Postgres every 1min
- [ ] 2.5.4 Wire dashboard to show live positions + PnL

**Acceptance**:
- Risk gate blocks orders that violate limits
- Position service correctly aggregates fills from all venues
- Dashboard shows live PnL, updates every 1s

**Open Questions**:
- Limit source: Postgres or hot-reloadable TOML? (TOML for fast iteration, Postgres for audit trail)
- Drawdown kill switch: auto-resume or require manual override? (Manual override via dashboard)

**GitHub Label**: `phase-2` `risk` `position` `pnl`

---

### 2.6 Observability Stack (Week 10)

**Owner**: Z  
**Blockers**: None (can run parallel)  
**Deliverables**:
- [ ] 2.6.1 Deploy Grafana + Prometheus + Loki + Tempo on t4g.medium in us-east-1
- [ ] 2.6.2 Wire Prometheus scrape for all services (already in `crates/telemetry`)
- [ ] 2.6.3 Wire Loki for structured logs (`/var/log/obird/*.jsonl` → promtail)
- [ ] 2.6.4 Wire Tempo for OTLP traces (Action → Order → Fill span tree)
- [ ] 2.6.5 Build Grafana dashboards:
  - Fleet overview (CPU/mem/network per service)
  - Per-venue latency (tick→order, order→ack, order→fill)
  - FV freshness (staleness per symbol)
  - Connector health (WS status, error rate)
- [ ] 2.6.6 Configure alerts → PagerDuty or SNS:
  - FV stale > 2min
  - Engine unhealthy > 30s
  - NATS gateway down
  - Position limit breach

**Acceptance**:
- Grafana accessible at `https://grafana.<domain>`
- Can trace an Action from strategy → engine → exchange → fill
- Alerts fire when FV goes stale (tested via kill md-ingest)

**Open Questions**:
- Retention: Prometheus (30d), Loki (30d), Tempo (7d)? (Yes, OK for MVP)
- PagerDuty vs Opsgenie vs SNS+SMS? (SNS+SMS for free tier)

**GitHub Label**: `phase-2` `observability` `grafana` `prometheus`

---

## Phase 3: Scale Venues (Ongoing)

**Goal**: Add Binance, Lighter, Kalshi; ML FV models; backtest CI gate.

### 3.1 Binance Connector Wiring (Week 11-12)

**Owner**: Z  
**Blockers**: Phase 2 complete  
**Deliverables**:
- [ ] 3.1.1 Wire `BinanceConnector` (already built) into live runner
- [ ] 3.1.2 Create `md-ingest-binance` binary
- [ ] 3.1.3 Deploy in ap-northeast-1 (Tokyo) or ap-southeast-1 (Singapore)
- [ ] 3.1.4 Add Binance to FV service as ref-price source
- [ ] 3.1.5 Test: Phase A ref-price only (no live quoting)
- [ ] 3.1.6 Phase B: second MM leg (pair-trade or spread arb)

**Acceptance**:
- Binance MD flowing into FV service
- Can quote HL using Binance microprice as FV anchor
- Phase B: live Binance MM running

**Open Questions**:
- Binance API rate limits: need VIP tier? (Monitor in Phase A, upgrade if needed)

**GitHub Label**: `phase-3` `binance` `venue`

---

### 3.2 Lighter + Kalshi Connectors (Week 13+)

**Owner**: Z  
**Blockers**: 3.1 (proves multi-venue scaling pattern)  
**Deliverables**:
- [ ] 3.2.1 Build `LighterConnector` (scaffolding exists)
- [ ] 3.2.2 Build `KalshiConnector` (new)
- [ ] 3.2.3 Deploy Kalshi in us-east-2 (Ohio) or Equinix Chicago
- [ ] 3.2.4 Wire into farming rotation

**Acceptance**:
- Lighter + Kalshi live, farming incentives

**Open Questions**:
- Kalshi regulatory risk? (Legal review needed before Phase 3 starts)
- Lighter sequencer location unknown — defer to Phase 3

**GitHub Label**: `phase-3` `lighter` `kalshi` `venue`

---

### 3.3 ML Fair Value Models (Week 14+)

**Owner**: Z + quant partner  
**Blockers**: 2.1 (needs QuestDB for feature extraction)  
**Deliverables**:
- [ ] 3.3.1 Extract features from QuestDB (book imbalance, volatility, spread)
- [ ] 3.3.2 Train ML model (XGBoost or simple regression)
- [ ] 3.3.3 Add `ml_ensemble` model to FV service
- [ ] 3.3.4 Backtest vs simple `mid` / `microprice` models
- [ ] 3.3.5 Deploy to production if >10bps edge

**Acceptance**:
- ML FV model live, outperforms baseline

**Open Questions**:
- Feature store: Postgres or separate service? (Postgres for MVP)
- Model update cadence: daily retrain or weekly? (Weekly initially)

**GitHub Label**: `phase-3` `ml` `fv` `quant`

---

### 3.4 Backtest CI Gate (Week 15+)

**Owner**: Z  
**Blockers**: None (can run parallel)  
**Deliverables**:
- [ ] 3.4.1 Wire `trading-cli backtest` to harness (currently stub)
- [ ] 3.4.2 Record 1 day of live MD as test fixture
- [ ] 3.4.3 Add CI job: replay recorded day, assert P&L within tolerance
- [ ] 3.4.4 Block PR merge if backtest fails

**Acceptance**:
- CI runs backtest on every PR
- Can catch regressions before deploy

**Open Questions**:
- P&L tolerance: ±5% or ±10%? (±10% allowing for sim noise)

**GitHub Label**: `phase-3` `backtest` `ci`

---

## Open Questions & Decisions Needed

### High Priority (blocks Phase 1)

1. **Schema format**: msgpack vs protobuf for NATS messages?
   - **Recommendation**: msgpack (simpler, Rust `serde` native, no codegen)
   
2. **FV service colocation**: central (us-east-1) vs colocated (eu-west-2)?
   - **Recommendation**: start central, measure latency, move if >5ms overhead
   
3. **In-process vs network mode default**: force network or allow in-process?
   - **Recommendation**: allow both, default network mode, opt-in co-located for latency-critical

4. **Auth for NATS**: static JWTs vs Vault?
   - **Recommendation**: static JWTs rotated monthly (simpler ops)

### Medium Priority (blocks Phase 2)

5. **Dashboard hosting**: Vercel vs self-host EC2?
   - **Recommendation**: Vercel free tier for speed (can migrate later)

6. **HlSpreadQuoter**: co-located or network mode?
   - **Recommendation**: co-located in-process (latency-critical), revisit if overhead <10ms

7. **Position state**: track locally in strategy or query position-service?
   - **Recommendation**: track locally in Phase 1, migrate to position-service in Phase 2

### Low Priority (defer to Phase 3)

8. **Python strategies**: allow Python for network-mode controllers?
   - **Recommendation**: Rust-only for Phase 1-2, revisit in Phase 3 if quants request

9. **Binance VIP tier**: needed for API rate limits?
   - **Recommendation**: monitor in Phase 3 A, upgrade if hit limits

10. **Kalshi legal review**: regulatory risk?
    - **Recommendation**: legal consult before Phase 3 execution

---

## Risk Register

| Risk | Impact | Probability | Mitigation |
|---|---|---|---|
| NATS JetStream at >500k msg/sec untested | High | Medium | Benchmark in Phase 1, fallback to Redpanda |
| QuestDB backup story weak | Medium | Low | Add daily snapshots to S3 in Phase 2 |
| predict.fun server location unknown | Low | Medium | Measure latency in Phase 1, relocate if >50ms |
| Cross-region egress cost >$500/mo | Medium | Low | Monitor daily, optimize subject routing |
| Key material custody (PREDICT_PRIVATE_KEY shared) | High | Medium | Migrate to per-venue KMS keys in Phase 3 |
| Rollback during migration fails | High | Low | Tag monolith build, test rollback procedure before Phase 1 deploy |

---

## Dependencies Graph (Critical Path)

```
Phase 1:
  1.1 NATS Setup
    ↓
  1.2 Subject Contract ──→ 1.3 MD Ingest ──→ 1.4 FV Service
    ↓                                              ↓
  1.5 Engine Refactor ──────────────────────────→ 1.6 Strategy Controller
    ↓                                              ↓
  1.7 Single-Region Deploy ←───────────────────────┘
    ↓
  1.8 Multi-Market Fix (parallel)

Phase 2:
  1.7 Complete
    ↓
  2.1 QuestDB ──→ 2.2 S3 Archive
    ↓               ↓
  2.3 Dashboard ←──┘
    ↓
  2.4 Multi-Region NATS + HL Migration
    ↓
  2.5 Risk + Position Service
    ↓
  2.6 Observability (parallel)

Phase 3:
  2.4, 2.5 Complete
    ↓
  3.1 Binance ──→ 3.2 Lighter/Kalshi
    ↓
  3.3 ML FV (parallel with 2.1)
    ↓
  3.4 Backtest CI (parallel)
```

**Critical path**: 1.1 → 1.2 → 1.5 → 1.6 → 1.7 → 2.1 → 2.4 → 3.1

**Estimated duration**: 4 weeks (Phase 1) + 3 weeks (Phase 2) + 4 weeks (Phase 3 core) = **11 weeks to full platform**

---

## Tracking & Cadence

### Daily
- Commit + push at EOD (even if WIP)
- Update task status in this file or GitHub issues

### Weekly
- Review: what's done, what's blocked, what's slipping
- Adjust estimates if >20% variance
- Document decisions in `decisions/` if architectural

### Phase Gates
- Phase 1 exit: code review + migration validation + 7d P&L comparison
- Phase 2 exit: QuestDB 7d retention proven + dashboard live + multi-region stable
- Phase 3 exit: per-venue (continuous delivery, no hard gate)

---

## GitHub Issue Labels

Suggested label taxonomy for issue tracking:

- **Phase**: `phase-1`, `phase-2`, `phase-3`
- **Component**: `nats`, `engine`, `fv-service`, `strategy`, `md-ingest`, `dashboard`, `questdb`, `observability`
- **Type**: `infra`, `feature`, `refactor`, `bug`, `docs`
- **Priority**: `p0-critical`, `p1-high`, `p2-medium`, `p3-low`
- **Status**: `blocked`, `in-progress`, `review`, `done`

---

## Next Steps

1. **Review this plan** — flag anything that looks wrong or risky
2. **Clarify open questions** (marked above in each task section)
3. **Create GitHub issues** from this plan (can auto-generate or manual)
4. **Start Phase 1 Week 1**: NATS setup + subject contract (tasks 1.1, 1.2)

---

**End of Project Plan**
