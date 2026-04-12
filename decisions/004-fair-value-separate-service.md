# ADR-004: Fair value model as a separate service

## Status
Accepted

## Context
The prediction market quoting strategy needs a fair value (probability estimate).
This model can be computationally expensive and changes frequently.
When embedded in the strategy engine, it competes for CPU with order management
and makes the binary large. Multiple strategy instances may need the same fair values.

## Decision
Run the fair value model as a separate binary. Publish fair values over Unix domain
sockets at 1-10 Hz. The strategy engine receives these as Event::FairValueUpdate.

## Consequences
- Strategy engine stays lean and latency-focused
- Can redeploy model without restarting trading
- Multiple instances can share the same fair values
- Adds operational complexity of running a second process
- Need a wire protocol (length-prefixed JSON over UDS)
- Can swap to NATS for cross-host deployment with same message format

## Alternatives Considered
- **Embedded in strategy crate**: Simpler but bloats the binary, competes for CPU
- **gRPC service**: Over-engineered for 1-10 Hz updates between two local processes
- **Shared memory**: Faster but more complex, not needed at this update rate
