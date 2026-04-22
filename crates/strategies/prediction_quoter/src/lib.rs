//! Prediction market quoting strategy.
//!
//! ## Design
//!
//! Uses independent per-side pricing for binary markets:
//!   - `yes_bid = poly_mid - spread_cents`
//!   - `no_bid  = (1 - poly_mid) - spread_cents`
//!
//! Each side is priced independently from the Polymarket fair value (poly_mid).
//! A side that would cross its ask estimate is skipped entirely rather than
//! clamped to mid. See `pricing` module for the full decision tree and tuning guide.
//!
//! ## Key invariants
//!   - Never place at mid (no `best_bid + tick` clamping)
//!   - `yes_bid + no_bid < 1.00` when both placed (= `1 - 2×spread_cents`)
//!   - No quoting without a fresh Polymarket FV signal
//!
//! See `PREDICT_QUOTING_DESIGN.md` at the workspace root for the full design doc.

pub mod params;
pub mod pricing;
pub mod quoter;

pub use params::QuoterParams;
pub use pricing::{calculate as calculate_quotes, PricingResult, MAX_PRICE, MIN_PRICE};
pub use quoter::PredictionQuoter;
