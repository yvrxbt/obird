# connector-hyperliquid — LLM Context

## What This Crate Does
Implements `ExchangeConnector` for Hyperliquid.
Handles WebSocket market data, order submission, and exchange-specific normalization.

## Key Details

- Nonce-based order submission — NonceManager required
- Official Rust SDK: hyperliquid_rust_sdk. Community: hypersdk, hyperliquid-sdk-rs
- WebSocket: L2 book, trades, user events, active asset data
- EIP-712 signing via private key
- Rate limits: standard API limits
- Recommendation: Wrap hyperliquid-sdk-rs (simd-json, fastwebsockets)

## Public API
- Struct implementing `ExchangeConnector` trait
- Market data WebSocket connection producing `Event`s
- Exchange-specific type normalization to core types

## Dependencies
- `trading-core` for traits and types
- Exchange-specific SDK or native HTTP/WS client
