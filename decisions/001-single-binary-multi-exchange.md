# ADR-001: Single binary, multi-exchange with OrderRouter

## Status
Accepted

## Context
Need to quote prediction markets AND hedge on CEX/DEX AND pair trade. This requires
unified risk management across exchanges. Previous iteration used one binary per
exchange per strategy, which required distributed nonce management for Hyperliquid
and prevented unified risk checks.

## Decision
One binary connects to all exchanges. An OrderRouter directs Actions to the correct
ExchangeConnector by inspecting the Exchange field of the InstrumentId. Each exchange
gets its own OrderManager with independent nonce serialization.

## Consequences
- Unified risk management is trivial (one RiskManager, one PositionTracker)
- Nonce management is per-exchange, no distributed coordination needed
- Cross-exchange hedging is straightforward (strategy emits actions for both exchanges)
- Can't restart one exchange connection without affecting others
- Single point of failure — but at this team size, simpler to operate

## Alternatives Considered
- **One binary per exchange**: Requires IPC for cross-exchange risk. Too complex for 2-3 person team.
- **One binary per strategy**: Nonce sharing problem across processes. Already tried, rejected.
