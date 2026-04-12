# ADR-002: Broadcast channels, not NATS, for market data hot path

## Status
Accepted

## Context
Initial design used NATS for all market data distribution. Encountered slow consumer
issues where NATS disconnects subscribers that can't keep up. JetStream fixes this
with persistence but adds latency (~200μs vs ~10ns for in-process channels).

For market data, we want LATEST-VALUE semantics (skip stale data), not
RELIABLE-DELIVERY semantics (process everything in order). NATS provides the latter.

## Decision
Use `tokio::sync::broadcast` channels for the market data hot path within the process.
Each instrument gets its own broadcast channel. Strategies subscribe via
`broadcast::Receiver`. Lagging receivers automatically skip stale messages
(`RecvError::Lagged`).

NATS is relegated to optional warm/cold path uses: cross-process fan-out for
monitoring dashboards, market data recording to a separate process, etc.

## Consequences
- ~10,000x lower latency for market data delivery (10ns vs 100μs)
- No serialization overhead (events are Rust structs, not bytes)
- No external dependency on the hot path
- Lagged receivers skip stale data — correct for market data
- Cannot fan out to external processes without adding a bridge
- Market data recording must subscribe to broadcast channels from within the process

## Alternatives Considered
- **NATS Core**: Disconnects slow consumers. Wrong semantics for market data.
- **NATS JetStream**: Adds persistence but ~200μs latency. Overkill for in-process.
- **ZeroMQ**: ~30μs latency. Better than NATS but still needless serialization overhead.
- **crossbeam SPSC**: ~5ns but no fan-out. Would need one channel per strategy per instrument.
