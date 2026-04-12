# fair-value-service — LLM Context

## What This Crate Does
SEPARATE BINARY that computes fair values for prediction markets.
Publishes fair values over Unix domain sockets (or NATS) for strategy engines to consume.

## Why Separate
1. Keeps strategy engine lean — model code can be large
2. CPU-intensive model updates don't compete with order management
3. Multiple strategy instances can share the same fair values
4. Can redeploy model without restarting trading engine

## Architecture
- Ingests prices from exchanges, news, signals
- Computes P(outcome) for prediction market instruments
- Publishes FairValueMessage at 1-10 Hz over UDS
- Strategy engine receives as Event::FairValueUpdate

## Communication
Wire format: length-prefixed bincode over Unix domain socket.
Can swap to NATS for cross-host deployment — same message format.
