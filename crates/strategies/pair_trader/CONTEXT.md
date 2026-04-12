# strategy-pair-trader — LLM Context

## What This Crate Does
Statistical arbitrage / mean reversion strategy for correlated crypto pairs.
Trades spread between two instruments across exchanges.

## Strategy Logic
1. Monitor spread between two correlated instruments (e.g., BTC on Hyperliquid vs Binance)
2. Calculate z-score of spread using rolling window
3. Enter when spread exceeds threshold (e.g., 2σ)
4. Exit when spread reverts to mean (or hits stop)
5. Hedge ratio dynamically calculated

## Key Files
- `trader.rs` — Implements Strategy trait
- `spread_model.rs` — Z-score calculation, half-life estimation
- `params.rs` — Strategy parameters

## Edge Source
Statistical — spread mean reversion. Latency matters for entry timing.
