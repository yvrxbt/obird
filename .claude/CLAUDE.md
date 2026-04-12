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
6. Market data flows through `tokio::broadcast` channels, NOT external message buses
7. Fair value model lives in `fair_value_service`, NOT in strategy crates

## Patterns

- **New exchange connector**: Copy `connectors/hyperliquid/`. Implement `ExchangeConnector`.
- **New strategy**: Copy `strategies/pair_trader/`. Implement `Strategy` trait.
- **New ADR**: Copy `decisions/template.md`. Number sequentially.

## Testing

- Unit tests per crate
- Integration tests via `SimConnector` + `SimMarketDataFeed` in `backtest`
- No `#[cfg(test)]` gates that change strategy behavior
