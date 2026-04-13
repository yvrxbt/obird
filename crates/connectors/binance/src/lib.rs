//! Binance USD-M Futures exchange connector.
//!
//! `BinanceClient` implements `ExchangeConnector` (order placement, cancellation, positions).
//! `BinanceMarketDataFeed` provides real-time BBO and fill events via WebSocket.
//!
//! Authentication: HMAC-SHA256 signed requests. Set env vars for API key and secret.
//! Post-only orders use `timeInForce=GTX` — rejected by Binance if they would cross.
//!
//! Usage:
//! ```ignore
//! let client = BinanceClient::from_env("BINANCE_API_KEY", "BINANCE_SECRET", "ETHUSDT", false)?;
//! let feed = BinanceMarketDataFeed::new(
//!     "ETHUSDT".into(), client.instrument(), client.api_key().to_string(), false
//! );
//! tokio::spawn(async move { feed.run(sink).await });
//! ```

pub mod client;
pub mod market_data;
pub mod normalize;

pub use client::BinanceClient;
pub use market_data::BinanceMarketDataFeed;
