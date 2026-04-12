# trading-backtest — LLM Context

## What This Crate Does
Backtesting harness that replays recorded market data through strategies.
Uses SimConnector (simulated matching engine) and SimMarketDataFeed (file replay).

## Key Principle
Strategy code is IDENTICAL between live and backtest. No mode flags.
The harness swaps ExchangeConnector and market data source — the strategy doesn't know.

## Key Components
- `harness.rs` — Drives the backtest loop
- `sim_connector.rs` — Implements ExchangeConnector with simulated fills
- `sim_market_data.rs` — Replays recorded market data from files
- `matching_engine.rs` — Price-time priority matching simulation
- `recorder.rs` — Records live market data for later replay
- `report.rs` — PnL, Sharpe, drawdown analysis

## Fill Models
- TradeThrough: fills when a trade occurs at or through our price (recommended default)
- Optimistic: fills immediately on price cross (overstates PnL)
- Probabilistic: fills with configurable probability (for sensitivity analysis)
