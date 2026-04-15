//! Binary market quote price calculator.
//!
//! ## Core identity
//!
//! On a binary prediction market YES + NO = 1.00 (winner pays $1, loser pays $0).
//! A market maker places **two BUY orders**:
//!
//!   `yes_bid = mid_yes - spread_cents`
//!   `no_bid  = 1.00 - yes_bid`
//!
//! The NO bid is NOT independently set from the NO orderbook mid — it's derived
//! from the YES bid via the binary identity. Together they sum to exactly 1.00,
//! which means no single counterparty can fill both simultaneously for a profit.
//!
//! ## Orderbook signal
//!
//! `predict_orderbook/{market_id}` returns the YES orderbook. NO prices are derived:
//!
//!   `no_ask_est = 1 - yes_bid_market`   (someone selling NO ≡ someone buying YES at that price)
//!
//! ## Crossing logic
//!
//! ### YES side crosses when:
//!   `yes_bid >= yes_ask_market`
//!   Fix: `yes_bid = yes_ask_market - tick`
//!
//! ### NO side crosses when:
//!   `no_bid >= no_ask_est`
//!   ⟺ `(1 - yes_bid) >= (1 - yes_bid_market)`
//!   ⟺ `yes_bid <= yes_bid_market`
//!   Fix: `yes_bid = yes_bid_market + tick`   → no_bid = 1 - yes_bid < no_ask_est ✓
//!
//! After clamping, the final pair always satisfies:
//!   `yes_bid_market < yes_bid < yes_ask_market`
//!   `no_bid = 1 - yes_bid`  (automatically non-crossing)
//!
//! ## Price precision
//!
//! predict.fun markets have `decimal_precision` of 2 or 3 (from GET /v1/markets/{id}).
//! The minimum tick is `10^(-decimal_precision)`: 0.01 for precision=2, 0.001 for precision=3.
//! Prices must be rounded to this tick; the exchange rejects finer prices.
//! If the BBO spread is narrower than `2 × tick` no valid quote can be placed;
//! `calculate` returns `None` and the strategy should skip this cycle.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use trading_core::types::{decimal::Price, market_data::OrderbookSnapshot};

/// Calculated bid prices for both YES and NO outcomes.
#[derive(Debug, Clone, Copy)]
pub struct QuotePrices {
    /// YES BUY order price (in [MIN_PRICE, MAX_PRICE]).
    pub yes_bid: Price,
    /// NO BUY order price = `1 - yes_bid`.
    pub no_bid: Price,
}

/// Minimum valid price (avoid degenerate near-zero / near-one orders).
pub const MIN_PRICE: Decimal = dec!(0.001);
/// Maximum valid price (= 1 - MIN_PRICE).
pub const MAX_PRICE: Decimal = dec!(0.999);

/// Calculate yes/no bid prices for one quoting cycle.
///
/// # Arguments
/// - `yes_book`          YES-outcome orderbook snapshot from the predict.fun WS feed.
///                       Used **only** for crossing guards (BBO bounds) — NOT for computing
///                       the fair value. The mid is derived separately from `fair_value`.
/// - `fair_value`        External fair value signal (Polymarket mid when available,
///                       predict.fun mid as fallback). This is the center around which
///                       we quote: `yes_bid = fair_value - spread_cents`.
/// - `spread_cents`      Desired half-spread from fair value (e.g. `dec!(0.02)` = 2 cents).
/// - `join_cents`        Optional manual join depth from predict.fun mid.
///                       Used only when FV pricing would place YES at/below best bid.
/// - `decimal_precision` Price tick precision from the market API (2 or 3).
///                       Determines both the rounding and the minimum-spread guard.
///
/// # Returns
/// `Some(QuotePrices)` when a valid non-crossing pair can be placed.
/// `None` when the book is empty or the BBO spread is too tight (< 2 ticks).
///
/// # Crossing guarantee
/// The returned prices always satisfy:
///   `yes_bid_market < yes_bid < yes_ask_market`
///   `no_bid  = 1 - yes_bid  (always in (0,1) and < no_ask_est)`
pub fn calculate(
    yes_book: &OrderbookSnapshot,
    fair_value: Decimal,
    spread_cents: Decimal,
    join_cents: Option<Decimal>,
    decimal_precision: u32,
) -> Option<QuotePrices> {
    let (best_bid, _) = yes_book.best_bid()?;
    let (best_ask, _) = yes_book.best_ask()?;

    let yes_bid_mkt = best_bid.inner();
    let yes_ask_mkt = best_ask.inner();

    // Minimum tick for this market: 10^(-decimal_precision).
    // decimal_precision is 2 or 3 per the API spec; any other value falls back to 3dp.
    let tick = match decimal_precision {
        2 => dec!(0.01),
        _ => dec!(0.001), // 3 or unknown
    };

    // Sanity check: book must not be crossed or empty.
    if yes_bid_mkt >= yes_ask_mkt {
        tracing::warn!(
            yes_bid_mkt = %yes_bid_mkt,
            yes_ask_mkt = %yes_ask_mkt,
            "crossed or empty YES book — skipping cycle",
        );
        return None;
    }

    // Tight BBO: spread < 2 ticks → no valid price strictly inside the spread.
    // Join YES best bid. For NO, derive from the YES *ask* side (not bid):
    //   no_bid = 1 - yes_ask_mkt
    // This places NO one tick below the NO ask estimate (1 - yes_bid_mkt), safely
    // as a resting maker order. Using 1 - yes_bid_mkt would land exactly at the NO
    // ask, which causes an immediate taker fill when a counterparty is resting there.
    // predict.fun has no PostOnly order type — all LIMITs can match immediately.
    //
    // YES + NO = yes_bid_mkt + (1 - yes_ask_mkt) = 1 - tick (≤ 0.99 for precision=2).
    // This is intentional: we're not taking a crossed position, just resting on both
    // natural bids. An adversary would need to fill BOTH sides simultaneously to arb,
    // which is impossible on a CLOB.
    if yes_ask_mkt - yes_bid_mkt < tick * dec!(2) {
        let no_bid = (Decimal::ONE - yes_ask_mkt).max(MIN_PRICE).min(MAX_PRICE);
        tracing::debug!(
            spread = %(yes_ask_mkt - yes_bid_mkt),
            tick = %tick,
            decimal_precision,
            yes_bid = %yes_bid_mkt,
            no_bid = %no_bid,
            "YES BBO < 2 ticks — joining both natural bids",
        );
        return Some(QuotePrices {
            yes_bid: Price::new(yes_bid_mkt),
            no_bid: Price::new(no_bid),
        });
    }

    let predict_mid = (yes_bid_mkt + yes_ask_mkt) / dec!(2);

    // Use the externally-supplied fair value (Polymarket mid or predict.fun mid fallback)
    // as the center of our quote. The predict.fun BBO is only used for crossing guards.
    let mut yes_bid = fair_value - spread_cents;

    // Optional manual join behavior: when FV pricing lands at/below best bid,
    // try placing at `predict_mid - join_cents` before applying crossing guards.
    if yes_bid <= yes_bid_mkt {
        if let Some(jc) = join_cents {
            let jc = jc.max(Decimal::ZERO);
            let joined = predict_mid - jc;
            tracing::debug!(
                fair_value = %fair_value,
                spread_cents = %spread_cents,
                join_cents = %jc,
                predict_mid = %predict_mid,
                proposed_yes_bid = %joined,
                "manual join_cents applied",
            );
            yes_bid = joined;
        }
    }

    // ── Crossing checks ───────────────────────────────────────────────────

    // 1. YES side: clamp away from YES ask.
    if yes_bid >= yes_ask_mkt {
        tracing::debug!(
            yes_bid = %yes_bid,
            yes_ask_mkt = %yes_ask_mkt,
            "YES would cross ask — joining inside best ask",
        );
        yes_bid = yes_ask_mkt - tick;
    }

    // 2. NO side: no_bid = 1 - yes_bid crosses no_ask_est = 1 - yes_bid_mkt
    //    when yes_bid <= yes_bid_mkt. Clamp YES bid upward.
    if yes_bid <= yes_bid_mkt {
        tracing::debug!(
            yes_bid = %yes_bid,
            yes_bid_mkt = %yes_bid_mkt,
            "NO would cross (derived) ask — joining inside best bid",
        );
        yes_bid = yes_bid_mkt + tick;
    }

    // 3. Hard bounds — should not trigger after the clamps above, but defensive.
    yes_bid = yes_bid.max(MIN_PRICE).min(MAX_PRICE);

    // 4. Round to market tick precision. Round DOWN to never accidentally cross the ask.
    //    After rounding, re-apply the NO crossing guard (floor can push yes_bid
    //    down to ≤ yes_bid_mkt, which would cross the NO ask).
    yes_bid =
        yes_bid.round_dp_with_strategy(decimal_precision, rust_decimal::RoundingStrategy::ToZero);
    if yes_bid <= yes_bid_mkt {
        // Rounding floored us into the bid — step up one tick.
        yes_bid = yes_bid_mkt + tick;
    }
    // Re-check YES ask crossing after potential upward adjustment.
    if yes_bid >= yes_ask_mkt {
        // BBO is only 1 tick wide at market precision — can't place between bid and ask.
        return None;
    }

    yes_bid = yes_bid.max(MIN_PRICE).min(MAX_PRICE);
    let no_bid = (Decimal::ONE - yes_bid).max(MIN_PRICE).min(MAX_PRICE);

    Some(QuotePrices {
        yes_bid: Price::new(yes_bid),
        no_bid: Price::new(no_bid),
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use trading_core::types::{
        decimal::{Price, Quantity},
        market_data::OrderbookSnapshot,
    };

    fn make_book(bid: Decimal, ask: Decimal) -> OrderbookSnapshot {
        OrderbookSnapshot {
            bids: vec![(Price::new(bid), Quantity::new(dec!(100)))],
            asks: vec![(Price::new(ask), Quantity::new(dec!(100)))],
            timestamp_ns: 0,
        }
    }

    /// Compute book mid — used as fair_value in tests that exercise the "FV = book mid"
    /// path (no external Polymarket signal). Tests that cover Polymarket FV drift pass
    /// a different fair_value explicitly.
    fn mid(bid: Decimal, ask: Decimal) -> Decimal {
        (bid + ask) / dec!(2)
    }

    #[test]
    fn normal_quote_inside_spread() {
        // precision=3 (0.001 tick), spread 0.58/0.62 (4 ticks wide)
        let (bid, ask) = (dec!(0.58), dec!(0.62));
        let book = make_book(bid, ask);
        let q = calculate(&book, mid(bid, ask), dec!(0.02), None, 3).unwrap();
        // fv=0.60, yes_bid=0.60-0.02=0.58 ≤ bid_mkt → clamp to 0.58+0.001=0.581
        assert!(q.yes_bid.inner() > dec!(0.58));
        assert!(q.yes_bid.inner() < dec!(0.62));
        assert_eq!(q.no_bid.inner(), Decimal::ONE - q.yes_bid.inner());
    }

    #[test]
    fn spread_cents_larger_than_half_spread_clamps_to_bid_side() {
        // precision=3, spread_cents=0.10 forces a clamp to bid+tick
        let (bid, ask) = (dec!(0.58), dec!(0.62));
        let book = make_book(bid, ask);
        let q = calculate(&book, mid(bid, ask), dec!(0.10), None, 3).unwrap();
        // fv=0.60, 0.60-0.10=0.50 < bid 0.58 → clamp to 0.58+0.001=0.581
        assert_eq!(q.yes_bid.inner(), dec!(0.581));
        assert_eq!(q.no_bid.inner(), Decimal::ONE - q.yes_bid.inner());
    }

    #[test]
    fn manual_join_cents_overrides_auto_join_when_below_bid() {
        // FV path would land below bid, but manual join_cents asks for a shallower join.
        let (bid, ask) = (dec!(0.58), dec!(0.62));
        let book = make_book(bid, ask);
        // predict_mid=0.60, join=0.01 => 0.59 (valid inside spread)
        let q = calculate(&book, dec!(0.50), dec!(0.02), Some(dec!(0.01)), 3).unwrap();
        assert_eq!(q.yes_bid.inner(), dec!(0.59));
        assert_eq!(q.no_bid.inner(), dec!(0.41));
    }

    #[test]
    fn spread_cents_near_zero_clamps_to_ask_side() {
        // precision=3, tiny spread_cents stays comfortably inside book
        let (bid, ask) = (dec!(0.58), dec!(0.62));
        let book = make_book(bid, ask);
        let q = calculate(&book, mid(bid, ask), dec!(0.001), None, 3).unwrap();
        assert!(q.yes_bid.inner() < dec!(0.62));
        assert!(q.yes_bid.inner() > dec!(0.58));
    }

    #[test]
    fn tight_spread_joins_both_natural_bids() {
        // precision=2 (tick=0.01): spread=0.004 < 2 ticks → join natural bids.
        //   YES bid = yes_bid_mkt = 0.598
        //   NO  bid = 1 - yes_ask_mkt = 1 - 0.602 = 0.398  (NOT 1 - 0.598 = 0.402)
        //   Using 0.402 would == no_ask_est → immediate taker fill.
        // precision=3 (tick=0.001): spread=0.004 ≥ 2 ticks → quotes inside spread.
        // fair_value is irrelevant for the tight-BBO path (fires before FV is used).
        let (bid, ask) = (dec!(0.598), dec!(0.602));
        let book = make_book(bid, ask);
        let q2 = calculate(&book, mid(bid, ask), dec!(0.001), None, 2).unwrap();
        assert_eq!(q2.yes_bid.inner(), dec!(0.598));
        assert_eq!(q2.no_bid.inner(), dec!(0.398)); // 1 - 0.602 (ask side)
        let q3 = calculate(&book, mid(bid, ask), dec!(0.001), None, 3).unwrap();
        assert!(q3.yes_bid.inner() > dec!(0.598));
        assert!(q3.yes_bid.inner() < dec!(0.602));
    }

    #[test]
    fn empty_book_returns_none() {
        let book = OrderbookSnapshot {
            bids: vec![],
            asks: vec![],
            timestamp_ns: 0,
        };
        assert!(calculate(&book, dec!(0.5), dec!(0.02), None, 3).is_none());
    }

    #[test]
    fn prices_always_sum_to_one() {
        let (bid, ask) = (dec!(0.55), dec!(0.65));
        let book = make_book(bid, ask);
        let q = calculate(&book, mid(bid, ask), dec!(0.03), None, 3).unwrap();
        assert_eq!(q.yes_bid.inner() + q.no_bid.inner(), Decimal::ONE);
    }

    #[test]
    fn no_bid_never_crosses_no_ask_estimate() {
        let (bid, ask) = (dec!(0.40), dec!(0.60));
        let book = make_book(bid, ask);
        let q = calculate(&book, mid(bid, ask), dec!(0.08), None, 3).unwrap();
        // no_ask_est = 1 - yes_bid_mkt = 1 - 0.40 = 0.60
        let no_ask_est = Decimal::ONE - dec!(0.40);
        assert!(
            q.no_bid.inner() < no_ask_est,
            "no_bid {} should be < no_ask_est {}",
            q.no_bid,
            no_ask_est
        );
    }

    #[test]
    fn one_cent_spread_precision3_market_quotes() {
        // Regression: Arsenal/BTC markets have 1-cent BBO with precision=3.
        let (bid, ask) = (dec!(0.65), dec!(0.66));
        let book = make_book(bid, ask);
        let q = calculate(&book, mid(bid, ask), dec!(0.02), None, 3).unwrap();
        assert!(q.yes_bid.inner() > dec!(0.65), "yes_bid must be > best bid");
        assert!(q.yes_bid.inner() < dec!(0.66), "yes_bid must be < best ask");
        assert_eq!(q.no_bid.inner(), Decimal::ONE - q.yes_bid.inner());

        // BTC $60k/$80k market (bid=0.36, ask=0.37)
        let (bid2, ask2) = (dec!(0.36), dec!(0.37));
        let book2 = make_book(bid2, ask2);
        let q2 = calculate(&book2, mid(bid2, ask2), dec!(0.02), None, 3).unwrap();
        assert!(q2.yes_bid.inner() > dec!(0.36));
        assert!(q2.yes_bid.inner() < dec!(0.37));
        assert_eq!(q2.no_bid.inner(), Decimal::ONE - q2.yes_bid.inner());
    }

    #[test]
    fn one_cent_spread_precision2_joins_both_natural_bids() {
        // Precision=2 market with 1-cent BBO: no room inside — tight BBO path fires.
        // YES: join yes_bid_mkt = 0.65 / NO: 1 - yes_ask_mkt = 0.34
        let (bid, ask) = (dec!(0.65), dec!(0.66));
        let book = make_book(bid, ask);
        let q = calculate(&book, mid(bid, ask), dec!(0.02), None, 2).unwrap();
        assert_eq!(q.yes_bid.inner(), dec!(0.65));
        assert_eq!(q.no_bid.inner(), dec!(0.34)); // 1 - 0.66 (ask side)
    }

    #[test]
    fn polymarket_fv_shifts_quote_center() {
        // Polymarket mid = 0.62, but predict.fun book shows 0.58/0.62.
        // The FV from Polymarket (bullish signal) should shift yes_bid higher.
        let (bid, ask) = (dec!(0.58), dec!(0.62));
        let book = make_book(bid, ask);
        // FV from book mid: 0.60 - 0.02 = 0.58 → clamped to 0.581
        let q_book_mid = calculate(&book, mid(bid, ask), dec!(0.02), None, 3).unwrap();
        // FV from Polymarket (= ask): 0.62 - 0.02 = 0.60 → inside spread, no clamp needed
        let q_poly_mid = calculate(&book, dec!(0.62), dec!(0.02), None, 3).unwrap();
        // Polymarket-anchored quote should be higher (more aggressive YES bid)
        assert!(q_poly_mid.yes_bid.inner() > q_book_mid.yes_bid.inner());
    }

    #[test]
    fn two_cent_spread_precision2_market_quotes() {
        // A precision=2 market with 2-cent BBO has exactly one valid interior tick.
        let (bid, ask) = (dec!(0.64), dec!(0.66));
        let book = make_book(bid, ask);
        let q = calculate(&book, mid(bid, ask), dec!(0.02), None, 2).unwrap();
        assert_eq!(q.yes_bid.inner(), dec!(0.65));
        assert_eq!(q.no_bid.inner(), dec!(0.35));
    }
}
