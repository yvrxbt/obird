# connector-binance — LLM Context

## What This Crate Does
Implements `ExchangeConnector` for Binance.
Handles WebSocket market data, order submission, and exchange-specific normalization.

## Key Details

- Standard HMAC-SHA256 signing — no nonce management needed
- Multiple Rust SDKs available (binance-rs, etc.)
- Most mature API with well-documented rate limits
- WebSocket: full market data + user data streams
- This is the latency benchmark — tightest round-trip of all venues

## Public API
- Struct implementing `ExchangeConnector` trait
- Market data WebSocket connection producing `Event`s
- Exchange-specific type normalization to core types

## Dependencies
- `trading-core` for traits and types
- Exchange-specific SDK or native HTTP/WS client
