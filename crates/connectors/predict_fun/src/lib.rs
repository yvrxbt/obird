//! predict.fun exchange connector — dual-outcome BUY-only market maker.
//!
//! One `PredictFunClient` covers an entire binary market (YES + NO outcomes).
//! Both quotes are placed as `Side::Buy`; the NO price is derived as `1 - YES_price`.
//!
//! # Architecture
//!
//! ```text
//!   PredictFunClient           (ExchangeConnector)
//!     ├── yes_instrument       InstrumentId  "42-YES"
//!     ├── no_instrument        InstrumentId  "42-NO"
//!     ├── token_map            symbol → on-chain token ID
//!     └── active_orders        Arc<Mutex<HashMap<hash, OrderEntry>>>  ← shared ↓
//!   PredictFunMarketDataFeed   (background task)
//!     ├── WS predictOrderbook → BookUpdate on yes_instrument
//!     └── WS predictWalletEvents → Fill/OrderUpdate on correct outcome
//! ```
//!
//! # Quick start
//!
//! ```no_run
//! # use connector_predict_fun::{PredictFunClient, PredictFunMarketDataFeed, PredictFunParams};
//! # async fn example() -> anyhow::Result<()> {
//! let params = PredictFunParams {
//!     market_id: 42,
//!     yes_outcome_name: "YES".into(),
//!     yes_token_id: "11111...".into(),
//!     no_outcome_name: "NO".into(),
//!     no_token_id: "22222...".into(),
//!     is_neg_risk: false,
//!     is_yield_bearing: true,
//!     fee_rate_bps: 0,
//!     polymarket_yes_token_id: None, // optional: Polymarket YES CLOB token for WS FV
//!     polymarket_no_token_id: None,  // optional: Polymarket NO CLOB token for hedge pricing
//! };
//! let client = PredictFunClient::from_env(
//!     "PREDICT_API_KEY", "PREDICT_PRIVATE_KEY", &params, false,
//! ).await?;
//! let feed = PredictFunMarketDataFeed::from_client(&client);
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod market_data;
pub mod normalize;

pub use client::{PredictFunClient, PredictFunParams, PredictShutdownHandle, MAX_PRICE, MIN_PRICE};
pub use market_data::PredictFunMarketDataFeed;
