# connector-lighter — LLM Context

## What This Crate Does
Implements `ExchangeConnector` for Lighter.
Handles WebSocket market data, order submission, and exchange-specific normalization.

## Key Details

- Nonce-based with CUSTOM CRYPTO: Schnorr over Poseidon2 (Goldilocks field, ecGFP5)
- DO NOT reimplement the crypto — use the community Rust SDK
- Community SDK: github.com/robustfengbin/lighter-sdk
- WebSocket: orderbook streaming, account fills
- Sub-accounts support up to 256 API keys each

## Public API
- Struct implementing `ExchangeConnector` trait
- Market data WebSocket connection producing `Event`s
- Exchange-specific type normalization to core types

## Dependencies
- `trading-core` for traits and types
- Exchange-specific SDK or native HTTP/WS client
