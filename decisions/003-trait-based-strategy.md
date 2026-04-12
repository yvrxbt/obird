# ADR-003: Trait-based strategy abstraction

## Status
Accepted

## Context
We need strategies to work identically in live trading and backtesting.
The strategy must not know whether it's receiving real or simulated data.

## Decision
Define a `Strategy` trait in core that receives `Event`s and emits `Action`s.
Strategies never import connector crates or call exchange APIs.
The engine handles routing Actions to the correct connector.

## Consequences
- Strategy code is 100% identical between live and backtest
- Strategies are independently testable
- Adding a new strategy doesn't require touching the engine
- Strategies can't optimize for exchange-specific features (acceptable tradeoff)
