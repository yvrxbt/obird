# connector-polymarket — LLM Context

## What This Crate Does
Implements `ExchangeConnector` for Polymarket.
Handles WebSocket market data, order submission, and exchange-specific normalization.

## Key Details

- NO RUST SDK EXISTS — native implementation needed
- Protocol: CLOB on Polygon (chain ID 137), USDC positions
- Auth: Two-layer EIP-712 → API creds, HMAC-SHA256 per request, EIP-712 per order
- Use alloy crate for EIP-712 typed data signing
- WebSocket: wss://ws-subscriptions-clob.polymarket.com

## Public API
- Struct implementing `ExchangeConnector` trait
- Market data WebSocket connection producing `Event`s
- Exchange-specific type normalization to core types

## Dependencies
- `trading-core` for traits and types
- Exchange-specific SDK or native HTTP/WS client
