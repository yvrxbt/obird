# ADR-006: OpenTelemetry + Prometheus + structured JSON for observability

## Status
Accepted

## Context
Need three kinds of observability:
1. Real-time metrics (positions, PnL, latency) for dashboards
2. Distributed tracing for debugging latency
3. Decision audit trail for post-trade analysis and LLM debugging

## Decision
- `tracing` crate for structured logging (JSON in prod, pretty in dev)
- `prometheus` crate for metrics, exposed on HTTP endpoint
- `opentelemetry` for distributed traces when needed
- Custom JSONL decision audit log for strategy reasoning

## Consequences
- Standard tooling (Grafana, Prometheus, Jaeger) works out of the box
- Decision audit log is LLM-readable — can feed to Claude for debugging
- Multiple crates to integrate but each is well-documented
- Metrics on hot path must be cheap (counter increment, not histogram)
