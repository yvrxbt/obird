//! `PredictHedgeStrategy` — hedges predict.fun fill exposure on Polymarket.
//!
//! ## Usage
//!
//! ```no_run
//! use strategy_predict_hedger::{PredictHedgeStrategy, MarketMapping, HedgeParams};
//! use trading_core::types::instrument::{Exchange, InstrumentId, InstrumentKind};
//!
//! let params = HedgeParams::default();
//! let mapping = MarketMapping {
//!     predict_yes: InstrumentId::new(Exchange::PredictFun, InstrumentKind::Binary, "143028-Yes"),
//!     predict_no:  InstrumentId::new(Exchange::PredictFun, InstrumentKind::Binary, "143028-No"),
//!     poly_yes:    InstrumentId::new(Exchange::Polymarket, InstrumentKind::Binary, "8501497..."),
//!     poly_no:     InstrumentId::new(Exchange::Polymarket, InstrumentKind::Binary, "2527312..."),
//! };
//! let strategy = PredictHedgeStrategy::new("predict_hedge_v1", vec![mapping], params);
//! ```

pub mod params;
pub mod strategy;

pub use params::HedgeParams;
pub use strategy::{MarketMapping, PredictHedgeStrategy};
