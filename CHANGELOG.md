# Changelog

All notable iterations, experiments, and decisions are logged here.
This file is designed to give LLMs context on what has been tried and why.

## 2026-04-11 — Initial Architecture

- Designed tiered messaging architecture (broadcast channels, not NATS on hot path)
- Chose single-binary multi-exchange design with OrderRouter
- Separated fair value service from strategy engine
- Defined core traits: Strategy, ExchangeConnector, RiskCheck
- Established workspace layout with 13 crates
- Created LLM-friendly documentation pattern (CONTEXT.md + ADRs)
