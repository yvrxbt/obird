# Changelog

All notable iterations, experiments, and decisions are logged here.
This file is designed to give LLMs context on what has been tried and why.

## 2026-04-13 — Inventory Skew, P&L Tracking, DataRecorder

**What shipped:**

*Strategy improvements (`crates/strategies/hl_spread_quoter/`):*
- **Inventory skew**: quotes placed around `reservation_mid = mid - skew_factor_bps_per_unit * net_pos`. When long, asks shift cheaper to steer mean-reversion. `skew_factor_bps_per_unit=50` in config = 5 bps shift at max_position (0.1 ETH).
- **Session P&L tracking**: every FILL log now includes `session_pnl` (running cash flow) and `mark_pnl` (cash + position marked to current mid). SHUTDOWN log emits session summary.
- **Duplicate CancelAll fix**: `pull_quotes()` returns empty when already in Cooldown state — eliminates spurious zero-latency ROUNDTRIP logs from double-fill notifications.
- **`taker_fee_bps` param** added to config (default 0.2 = HL maker rebate) — used for P&L accuracy.
- `max_drift_bps()` uses raw mid (not reservation) so drift responds to market movement only, not inventory changes.

*Data capture (`crates/telemetry/src/recorder.rs` — new):*
- **`DataRecorder`**: background task subscribed to MarketDataBus. Writes clean JSONL for quant analysis:
  - `logs/data/bbo-YYYY-MM-DD.jsonl` — one line per BBO tick with `exchange_ts_ns`, `local_ts_ns`, bid/ask px+sz
  - `logs/data/fills-YYYY-MM-DD.jsonl` — one line per fill, flushed immediately
- Wired into `live.rs` as a separate subscriber — zero coupling to strategy path.
- BBO flush interval: every 500 records (bounded crash data loss). Fills always sync-flushed.

*Operations:*
- `RUNBOOK.md` added — full operational guide: pre-flight, monitoring, tuning, exit criteria.
- `cancel_all` per-OID `BatchCancel` confirmed correct approach. `scheduleCancel` removed (required $1M volume, cancelled all instruments — unsafe for multi-strategy). CLAUDE.md + docs updated to reflect this.
- ARCHITECTURE.md updated to current actual state.

**Config additions** (`configs/quoter.toml`):
```
skew_factor_bps_per_unit = "50"
taker_fee_bps            = "0.2"
```

**Observed behavior (US-East server, pre-Tokyo):**
- cancel_ms median ~398ms, place_ms ~345ms, total ~743ms
- Expected from Tokyo: ~40ms total
- Net position drifting short across sessions

**Next:** Deploy Tokyo → validate latency → run 4+ hour sessions → check RUNBOOK.md exit criteria → wire Binance as reference price source.

---

## 2026-04-13 — HL MM Live (Initial)

**What shipped:**
- `HlSpreadQuoter` strategy: two-level symmetric spread MM, order-price-based drift detection
- Always cancel-first: every requote is `[CancelAll, PlaceOrder×N]` as one atomic batch
- `cancel_all` uses per-OID `BatchCancel` — tracks OIDs from place_batch responses, works for all accounts regardless of traded volume
- `place_batch` uses HL `BatchOrder` — all 4 level orders in one REST call
- Startup loads real positions from HL (`clearinghouse_state`) — strategy survives restarts
- Graceful shutdown: `ShutdownHandle` does per-OID `BatchCancel` before process exits
- Persistent JSON line logging to `logs/obird-YYYY-MM-DD.jsonl` — every mid + drift at DEBUG
- `MarketDataSink` trait added to core — distributed transport seam without touching strategies
- `MarketDataBus` made `Arc`-safe — shared between feed task and engine runner
- `EngineRunner` uses `futures::stream::select_all` — all instruments polled fairly (pair-trade ready)
- Action channel changed to `Vec<Action>` batches — enables concurrent cross-exchange leg dispatch
- `OrderRouter` groups PlaceOrder by exchange, fires cross-exchange legs via `join_all`
- L2Book WS subscription for real BBO + exchange timestamps (replaced AllMids)

**Key bug fixes:**
- Price rounding: `tick.normalize().scale()` not `tick.scale()` (was returning 2 for 0.1 tick)
- Fill cancel: use per-OID `BatchCancel` (OIDs tracked via `active_oids` set populated by `place_batch`)
- Multi-receiver: `select_all` replaces first-only polling (silent starvation on second instrument)
- `scheduleCancel` removed: required $1M traded volume, broke on first run with stale orders

**Config:** `configs/quoter.toml` — `level_bps=[5,10]`, `drift_bps=3`, `drift_pause_secs=3`, `fill_pause_secs=10`, `order_size=0.01`, `max_position=0.1`

## 2026-04-11 — Initial Architecture

- Designed tiered messaging architecture (broadcast channels, not NATS on hot path)
- Chose single-binary multi-exchange design with OrderRouter
- Separated fair value service from strategy engine
- Defined core traits: Strategy, ExchangeConnector, RiskCheck
- Established workspace layout with 13 crates
- Created LLM-friendly documentation pattern (CONTEXT.md + ADRs)
