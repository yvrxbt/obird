# trading-engine — LLM Context

## What This Crate Does
The runtime that wires connectors, strategies, and risk management together.
Contains the main event loop, OrderRouter, and MarketDataBus.

## Key Components
- `runner.rs` — Main event loop. Spawns strategy tasks, manages lifecycle.
- `order_router.rs` — Routes Actions to correct ExchangeConnector by exchange. Applies unified risk.
- `order_manager.rs` — Per-exchange order submission with nonce serialization.
- `risk.rs` — Unified pre-trade risk checks across all exchanges.
- `position_tracker.rs` — Aggregates positions across all exchanges.
- `market_data_bus.rs` — Manages tokio::broadcast channels for market data fan-out.

## Architecture
Strategies run as tokio tasks. They receive Events via broadcast channels
and send Actions via mpsc to the OrderRouter. The OrderRouter validates
(risk check) and routes to the correct per-exchange OrderManager.

## Invariants
- OrderRouter is the ONLY path from strategy to exchange
- Risk checks are synchronous in the OrderRouter hot path
- Each exchange has its own OrderManager (serializes nonce)
- Market data flows via tokio::broadcast (lagged receivers skip stale data)
