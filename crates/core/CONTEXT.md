# core — LLM Context

## What This Crate Does
Defines ALL shared types, traits, and enums. Every other crate depends on `core`.
`core` depends on nothing internal — only external crates.

## Key Types
- `InstrumentId` — (Exchange, InstrumentKind, symbol). Uniquely identifies a tradeable.
- `Price`, `Quantity` — Newtypes over `rust_decimal::Decimal`. NEVER use f64 for money.
- `OrderRequest` — Everything to place an order (instrument, side, price, qty, TIF).
- `OrderbookSnapshot` — Bids (desc) and asks (asc) as `Vec<(Price, Quantity)>`.
- `Position` — Current position (size, avg_entry, unrealized_pnl).

## Key Traits
- `Strategy` — Receives Events, emits Actions. Must be identical in live and backtest.
- `ExchangeConnector` — Order submission per exchange. Also implemented by backtest sim.
- `RiskCheck` — Synchronous pre-trade validation.

## Key Enums
- `Event` — Inputs to strategies (book updates, trades, fills, fair values, ticks).
- `Action` — Strategy outputs (place/cancel/modify orders, log decisions).

## Gotchas
- Bids sorted descending, asks ascending.
- InstrumentId includes Exchange — same symbol on different exchanges = different instrument.
- All timestamps are nanoseconds (`u64`), not SystemTime.
- Strategy trait is `Send + Sync + 'static`.
