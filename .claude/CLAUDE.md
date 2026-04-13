# Trading System — Claude Code Instructions

## Project Overview

Rust HFT trading system. Two verticals: prediction market quoting + crypto pair trading.
Single binary connects to 5 exchanges. Unified risk management.

## Before Modifying Any Crate

1. Read `ARCHITECTURE.md` at workspace root
2. Read the `CONTEXT.md` in the crate you're modifying
3. Check `decisions/` for relevant ADRs

## Code Standards

- `cargo clippy --workspace` and `cargo test --workspace` before done
- `rust_decimal::Decimal` for all prices/quantities — NEVER `f64`
- `thiserror` for error types, no string errors
- `tokio` runtime exclusively
- Doc comments on all public items, explain *why* not *what*

## Architecture Invariants (Do NOT Break)

1. Strategies NEVER import connector crates or call exchange APIs
2. `Strategy` trait is the ONLY interface between strategy logic and engine
3. `Action` enum is the ONLY way strategies express intent
4. `Event` enum is the ONLY way the engine communicates with strategies
5. `OrderRouter` is the single point of routing; `OrderManager` per exchange serializes submission
6. Market data flows via `MarketDataSink` trait (in-process default: `Arc<MarketDataBus>` backed by `tokio::broadcast`). Connector feeds call `sink.publish()` — never reference broadcast directly. This seam enables distributed deployment without touching strategies.
7. Fair value model lives in `fair_value_service`, NOT in strategy crates
8. Connector crates are split: `XClient` (order execution, implements `ExchangeConnector`) + `XMarketDataFeed` (WS feed, runs as background task, publishes to `MarketDataSink`)

## Live Run

```bash
source .env && RUST_LOG=quoter=info,connector_hyperliquid=info cargo run --bin trading-cli -- live --config configs/quoter.toml
```
Always use `--release` in prod. Ctrl+C: engine stops → `ShutdownHandle::cancel_all()` fires `BatchCancel` on tracked OIDs before exit. Never `kill -9`.

Logs:
- `logs/obird-YYYY-MM-DD.jsonl` — all tracing events (debug+). Filter on `fields.target`: `"quoter"` for strategy, `"md"` for market data.
- `logs/data/bbo-YYYY-MM-DD.jsonl` — clean BBO per tick with `exchange_ts_ns` + `local_ts_ns` (for quant analysis)
- `logs/data/fills-YYYY-MM-DD.jsonl` — per fill with `session_pnl` + `mark_pnl` (flushed immediately)

**See `RUNBOOK.md` for full live operations guide, health indicators, tuning parameters, and Binance-readiness exit criteria.**

## HlSpreadQuoter — Current Params (`configs/quoter.toml`)

- `level_bps = [5, 10]` — 2-level spread, 5 and 10 bps half-spread
- `order_size = 0.01` — 0.01 ETH per side per level
- `drift_bps = 3` — pull quotes if market moves > 3 bps from resting prices
- `fill_pause_secs = 10` — seconds to wait after any fill before requoting
- `skew_factor_bps_per_unit = 50` — shift reservation mid by 50 bps per ETH of net position
- `taker_fee_bps = 0.2` — HL maker rebate, used for P&L reporting accuracy
- `max_position = 0.1` — stops placing orders on accumulating side beyond this

Inventory skew: `reservation_mid = mid - skew_factor_bps_per_unit * net_pos / 10_000 * mid`. At 0.1 ETH long → reservation shifts 5 bps down. Drift check uses raw `mid`, not reservation (responds to market movement only).

## HL Idiosyncrasies

- `cancel_all` uses per-OID `BatchCancel` — tracks OIDs from `place_batch` responses, cancels only those. Works for all accounts regardless of volume. `scheduleCancel` (removed) required $1M+ traded volume and cancelled ALL instruments — unsafe for multi-strategy.
- `place_batch` uses `BatchOrder` — all orders in one call (not N sequential REST calls)
- Price rounding: use `PriceTick::tick_for(price).normalize().scale()` — raw `.scale()` is wrong (returns 2 for 0.1)
- Symbol names: perp = "ETH", "BTC" etc. Spot = "@N" format. Auto-detected in `resolve_symbol()`
- ALO (post-only) TIF = `HlTif::Alo` — always use for maker orders to prevent crossing
- `scheduleCancel` cancels ALL instruments — not safe for multi-strategy. Use BatchCancel per-OID then.
- Optimal deployment: Tokyo (ap-northeast-1)

## Patterns

- **New exchange connector**: Copy `connectors/hyperliquid/`. Implement `ExchangeConnector` + `place_batch`.
- **New strategy**: Copy `strategies/hl_spread_quoter/`. Implement `Strategy` trait.
- **New ADR**: Copy `decisions/template.md`. Number sequentially.
- **Adding an instrument**: pre-register with `md_bus.sender(&instrument)` before spawning the feed.

## Testing

- Unit tests per crate
- Integration tests via `SimConnector` + `SimMarketDataFeed` in `backtest`
- No `#[cfg(test)]` gates that change strategy behavior
