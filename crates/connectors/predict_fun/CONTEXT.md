# connector-predict_fun — LLM Context

## What This Crate Does
Implements `ExchangeConnector` for Predict.fun.
Handles WebSocket market data, order submission, and exchange-specific normalization.

## Key Details

- NO RUST SDK EXISTS — native implementation needed
- Protocol: CLOB on BNB Chain (BSC mainnet)
- Auth: API key on mainnet, EIP-712 order signing
- Rate limits: 240 req/min per API key
- Similar signing model to Polymarket but on BSC
- REST: https://api.predict.fun/

## Public API
- Struct implementing `ExchangeConnector` trait
- Market data WebSocket connection producing `Event`s
- Exchange-specific type normalization to core types

## Dependencies
- `trading-core` for traits and types
- Exchange-specific SDK or native HTTP/WS client
