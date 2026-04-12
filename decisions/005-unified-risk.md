# ADR-005: Unified risk management across all exchanges

## Status
Accepted

## Context
The system trades prediction markets AND hedges on CEX/DEX AND pair trades.
A prediction market short on BTC needs awareness of the hedge position on
Hyperliquid. Separate risk managers per exchange can't enforce portfolio limits.

## Decision
One UnifiedRiskManager sees all positions across all exchanges.
Called synchronously by the OrderRouter on every action (fast: HashMap lookups).
Supports per-strategy limits, portfolio-level limits, and correlated exposure limits.

## Consequences
- Cross-exchange hedging is trivial
- Can enforce "total BTC exposure" limits across verticals
- Single point of failure for risk checks
- Must be fast (microseconds) since it's on the critical path
- Position reconciliation must handle all exchanges
