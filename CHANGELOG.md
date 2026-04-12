# Changelog

All notable iterations, experiments, and decisions are logged here.
This file is designed to give LLMs context on what has been tried and why.

## 2026-04-13 — HL MM Live

**What shipped:**
- `HlSpreadQuoter` strategy: two-level symmetric spread MM, order-price-based drift detection
- Always cancel-first: every requote is `[CancelAll, PlaceOrder×N]` as one atomic batch
- `cancel_all` uses `scheduleCancel(now)` — single API call, no OID lookup, ~10× faster than previous N-cancel approach
- `place_batch` uses HL `BatchOrder` — all 4 level orders in one REST call
- Startup loads real positions from HL (`clearinghouse_state`) — strategy survives restarts
- Graceful shutdown: `ShutdownHandle` calls `scheduleCancel` before process exits
- Persistent JSON line logging to `logs/obird-YYYY-MM-DD.jsonl` — every mid + drift at DEBUG
- `MarketDataSink` trait added to core — distributed transport seam without touching strategies
- `MarketDataBus` made `Arc`-safe — shared between feed task and engine runner
- `EngineRunner` now uses `futures::stream::select_all` — all instruments polled fairly (pair-trade ready)
- Action channel changed to `Vec<Action>` batches — enables concurrent cross-exchange leg dispatch
- `OrderRouter` groups PlaceOrder by exchange, fires cross-exchange legs via `join_all`

**Key bug fixes:**
- Price rounding: `tick.normalize().scale()` not `tick.scale()` (was returning 2 for 0.1 tick)
- Fill cancel: use `CancelAll` (exchange-level) not per-slot cancel (OIDs not known when fill arrives)
- Multi-receiver: `select_all` replaces first-only polling (silent starvation on second instrument)

**Config:** `configs/quoter.toml` — `level_bps=[5,10]`, `drift_bps=3`, `drift_pause_secs=3`, `fill_pause_secs=10`

**Next:** Binance connector, FairValueService, PositionTracker, per-OID cancel for multi-strategy safety

## 2026-04-11 — Initial Architecture

- Designed tiered messaging architecture (broadcast channels, not NATS on hot path)
- Chose single-binary multi-exchange design with OrderRouter
- Separated fair value service from strategy engine
- Defined core traits: Strategy, ExchangeConnector, RiskCheck
- Established workspace layout with 13 crates
- Created LLM-friendly documentation pattern (CONTEXT.md + ADRs)
