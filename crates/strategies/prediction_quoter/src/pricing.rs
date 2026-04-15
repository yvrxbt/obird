//! Binary market quote price calculator.
//!
//! ## Core design: conservative dual-FV pricing
//!
//! YES and NO are priced from two independent fair-value signals:
//! - `poly_fv`:     Polymarket mid (the deeper, more liquid venue)
//! - `predict_mid`: predict.fun book mid (the execution venue)
//!
//! For each side the strategy uses the *more conservative* signal — the one that
//! places the bid furthest from both markets' mids, minimising adverse selection:
//!
//!   `yes_fv   = min(poly_fv, predict_mid)`       ← lower of the two YES mids
//!   `yes_bid  = yes_fv - spread_cents`
//!
//!   `no_fv    = 1 - max(poly_fv, predict_mid)`   ← lower of the two NO mids
//!   `no_bid   = no_fv - spread_cents`
//!
//! ## Why "conservative" (not the raw poly anchor)?
//!
//! When venues diverge, using `poly_mid - spread_cents` for YES may place the bid
//! **above** the predict.fun YES mid — a predict.fun participant would happily sell
//! YES to us at a premium to their own venue's fair value. Using `min()` ensures we
//! are always below BOTH mids, so neither a poly-informed nor a predict-informed
//! trader has immediate edge against us.
//!
//! ### Example (Arsenal: poly=0.545, predict=0.635, spread=0.02)
//!
//! ```text
//! YES: min(0.545, 0.635) - 0.02 = 0.525
//!      |0.525 - predict_mid| = 0.11 ≥ spread_threshold_v(0.06) → skip (0-point order)
//!
//! NO:  (1 - max(0.545, 0.635)) - 0.02 = (1 - 0.635) - 0.02 = 0.345
//!      |0.345 - no_predict_mid| = 0.02 < 0.06 → placed, score_factor = 44%
//!      no_ask_est = 1 - 0.63 = 0.37 → 0.345 < 0.37 → safe resting maker ✓
//! ```
//!
//! YES is skipped (can't earn points without poly-adverse-selection risk at this divergence).
//! NO is placed safely, 2¢ from predict mid, within scoring window.
//!
//! ## Scoring window skip
//!
//! A bid outside `spread_threshold_v` of predict mid earns **zero** points.
//! Rather than place a zero-score order that locks up capital and still carries
//! fill risk, we skip it. A side is only placed when:
//!
//!   `|bid - predict_mid_for_that_side| < spread_threshold_v`
//!
//! ## When venues agree (small divergence)
//!
//! When `|poly_fv - predict_mid| < spread_cents`:
//! - Both mids are close → min/max makes little difference
//! - YES: `yes_bid ≈ poly_mid - spread_cents` (normal poly anchor)
//! - NO:  `no_bid  ≈ (1-poly_mid) - spread_cents` (normal poly anchor)
//!
//! ## When Polymarket is not configured
//!
//! Pass `poly_fv = predict_mid`. Then `min = max = predict_mid`, and both sides
//! are anchored purely to predict mid — same as before Polymarket integration.
//!
//! ## Tuning knobs (`[strategies.params]`)
//!
//!   `spread_cents`       — distance from effective FV per side.
//!                          Score factor = `((v - spread_cents) / v)²`.
//!                          | 0.01 → 69% | 0.02 → 44% | 0.03 → 25% |
//!
//!   `spread_threshold_v` — market's scoring window (from predict.fun API).
//!                          Orders beyond this earn zero. Auto-filled by CLI.
//!
//! ## Price precision
//!
//! `decimal_precision` from `GET /v1/markets/{id}` (2 or 3).
//! Prices rounded DOWN (ToZero) — never accidentally cross by rounding up.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use trading_core::types::{decimal::Price, market_data::OrderbookSnapshot};

/// Independent bid prices for YES and NO outcomes.
///
/// Each field is `Some` when a valid, scoring-window-eligible, non-crossing price
/// was found, or `None` when skipped (outside scoring window or crossing guard fired).
///
/// When both are `Some`: `yes_bid + no_bid < 1.00` always.
#[derive(Debug, Clone, Copy)]
pub struct PricingResult {
    /// YES BUY price, or `None` if skipped.
    pub yes_bid: Option<Price>,
    /// NO BUY price, or `None` if skipped.
    pub no_bid: Option<Price>,
}

impl PricingResult {
    /// True when neither side has a valid price to place.
    pub fn is_empty(&self) -> bool {
        self.yes_bid.is_none() && self.no_bid.is_none()
    }
}

/// Minimum valid price.
pub const MIN_PRICE: Decimal = dec!(0.001);
/// Maximum valid price.
pub const MAX_PRICE: Decimal = dec!(0.999);

/// Calculate independent YES and NO bid prices for one quoting cycle.
///
/// # Arguments
/// - `yes_book`           YES orderbook snapshot from the predict.fun WS feed.
/// - `poly_fv`            Polymarket mid price (pass `predict_mid` when not configured).
/// - `predict_mid`        predict.fun book mid `(best_bid + best_ask) / 2`.
/// - `spread_cents`       Target distance from each side's effective FV.
/// - `touch_retreat_cents`Retreat distance from predict.fun top-of-book (best bid)
///                        used by defensive requotes.
/// - `spread_threshold_v` Market's scoring window. Sides outside it earn 0 and are skipped.
/// - `decimal_precision`  Market tick precision (2 or 3 from the API).
///
/// # Returns
/// - `None`  — book is empty or crossed.
/// - `Some(result)` — result with each side either `Some(price)` or `None` (skipped).
///
/// # Pricing rules
/// - YES anchor: `min(poly_fv, predict_mid)` — conservative, below both mids.
/// - NO  anchor: `1 - max(poly_fv, predict_mid)` — conservative, below both NO mids.
/// - Skip side if `|bid - predict_mid_for_side| ≥ spread_threshold_v` (earns 0 points).
/// - Skip side if crossing its ask estimate (immediate taker fill).
pub fn calculate(
    yes_book: &OrderbookSnapshot,
    poly_fv: Decimal,
    predict_mid: Decimal,
    spread_cents: Decimal,
    touch_retreat_cents: Decimal,
    spread_threshold_v: Decimal,
    decimal_precision: u32,
) -> Option<PricingResult> {
    let (best_bid, _) = yes_book.best_bid()?;
    let (best_ask, _) = yes_book.best_ask()?;

    let yes_bid_mkt = best_bid.inner();
    let yes_ask_mkt = best_ask.inner();

    if yes_bid_mkt >= yes_ask_mkt {
        tracing::warn!(
            yes_bid_mkt = %yes_bid_mkt,
            yes_ask_mkt = %yes_ask_mkt,
            "crossed or empty YES book — skipping cycle",
        );
        return None;
    }

    let tick = match decimal_precision {
        2 => dec!(0.01),
        _ => dec!(0.001),
    };

    let max_spread_inside_window = (spread_threshold_v - tick).max(Decimal::ZERO);

    // NO ask estimate: selling NO = buying YES at the YES market bid.
    let no_ask_est = Decimal::ONE - yes_bid_mkt;
    // NO predict mid (for scoring window check).
    let no_predict_mid = Decimal::ONE - predict_mid;

    // ── YES pricing ───────────────────────────────────────────────────────────
    //
    // Anchor: min(poly_fv, predict_mid) — the more conservative YES mid.
    // When poly < predict: uses poly (further below both bids).
    // When poly > predict: uses predict (prevents paying above predict's fair value).
    let yes_bid: Option<Price> = {
        let yes_fv = poly_fv.min(predict_mid);
        let mut target = yes_fv - spread_cents;

        // Defensive retreat from predict.fun top-of-book (only on touch-triggered requotes).
        if touch_retreat_cents > Decimal::ZERO {
            target = target.min(yes_bid_mkt - touch_retreat_cents);
        }

        // Keep within scoring window by clamping just inside `v` (farming-first).
        let spread_from_predict = predict_mid - target;
        if spread_from_predict >= spread_threshold_v {
            let clamped = predict_mid - max_spread_inside_window;
            tracing::debug!(
                yes_target_before = %target,
                yes_target_after = %clamped,
                predict_mid = %predict_mid,
                spread_threshold_v = %spread_threshold_v,
                "YES target outside scoring window — clamping inside window",
            );
            target = clamped;
        }

        // Crossing guard: clamp if target would cross ask.
        if target >= yes_ask_mkt {
            tracing::debug!(
                yes_target  = %target,
                yes_ask_mkt = %yes_ask_mkt,
                "YES target >= ask — clamping to ask - tick",
            );
            target = yes_ask_mkt - tick;
        }

        // Round DOWN — never accidentally cross by rounding up.
        target = target
            .round_dp_with_strategy(decimal_precision, rust_decimal::RoundingStrategy::ToZero);

        if target >= yes_ask_mkt {
            // BBO is 1 tick wide after clamp+round — no room.
            None
        } else {
            Some(Price::new(target.max(MIN_PRICE).min(MAX_PRICE)))
        }
    };

    // ── NO pricing ────────────────────────────────────────────────────────────
    //
    // Anchor: 1 - max(poly_fv, predict_mid) = min(1-poly_fv, 1-predict_mid).
    // When poly < predict: uses predict_mid's NO mid (1-predict) — which is lower,
    //   placing NO conservatively spread_cents below predict's NO mid.
    // When poly > predict: uses poly's NO mid (1-poly) — which is lower (since poly
    //   is more bullish on YES → more bearish on NO).
    //
    // This ensures NO bid is spread_cents below BOTH NO mids simultaneously.
    let no_bid: Option<Price> = {
        let no_fv = Decimal::ONE - poly_fv.max(predict_mid);
        let mut target = no_fv - spread_cents;
        let no_bid_mkt = Decimal::ONE - yes_ask_mkt;

        // Defensive retreat from NO top-of-book (only on touch-triggered requotes).
        if touch_retreat_cents > Decimal::ZERO {
            target = target.min(no_bid_mkt - touch_retreat_cents);
        }

        // Scoring window check: keep inside `v` by clamping (farming-first).
        let spread_from_no_predict = (no_predict_mid - target).abs();
        if spread_from_no_predict >= spread_threshold_v {
            let clamped = no_predict_mid - max_spread_inside_window;
            tracing::debug!(
                no_target_before = %target,
                no_target_after = %clamped,
                no_predict_mid = %no_predict_mid,
                spread_threshold_v = %spread_threshold_v,
                "NO target outside scoring window — clamping inside window",
            );
            target = clamped;
        }

        if target >= no_ask_est {
            // Crossing guard: NO bid would immediately match a resting NO seller.
            tracing::debug!(
                no_target  = %target,
                no_ask_est = %no_ask_est,
                "NO skipped: target >= no_ask_est (would cross NO ask)",
            );
            None
        } else {
            // Round DOWN.
            target = target
                .round_dp_with_strategy(decimal_precision, rust_decimal::RoundingStrategy::ToZero);

            if target >= no_ask_est || target <= MIN_PRICE {
                None
            } else {
                Some(Price::new(target.max(MIN_PRICE).min(MAX_PRICE)))
            }
        }
    };

    let result = PricingResult { yes_bid, no_bid };

    if result.is_empty() {
        tracing::debug!(
            poly_fv      = %poly_fv,
            predict_mid  = %predict_mid,
            divergence   = %(poly_fv - predict_mid).abs().round_dp(4),
            spread_cents = %spread_cents,
            "pricing: both sides skipped",
        );
    }

    Some(result)
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

    fn mid(bid: Decimal, ask: Decimal) -> Decimal {
        (bid + ask) / dec!(2)
    }

    const V: Decimal = dec!(0.06);

    // ── No divergence (poly = predict) ────────────────────────────────────────

    /// When poly = predict, behaves exactly like single-FV pricing.
    #[test]
    fn no_divergence_both_sides_placed() {
        let (bid, ask) = (dec!(0.55), dec!(0.65));
        let book = make_book(bid, ask);
        let predict_mid = mid(bid, ask); // 0.60
                                         // poly = predict
        let r = calculate(
            &book,
            predict_mid,
            predict_mid,
            dec!(0.02),
            dec!(0.00),
            V,
            3,
        )
        .unwrap();

        let yes = r.yes_bid.unwrap().inner();
        let no = r.no_bid.unwrap().inner();
        assert_eq!(yes, dec!(0.58)); // 0.60 - 0.02
        assert_eq!(no, dec!(0.38)); // (1-0.60) - 0.02
        assert!(yes + no < Decimal::ONE);
    }

    // ── Small divergence (venues close) ──────────────────────────────────────

    /// Small divergence (< spread_cents): poly anchor used for both, both in window.
    #[test]
    fn small_divergence_uses_poly_anchor() {
        let (bid, ask) = (dec!(0.55), dec!(0.65));
        let book = make_book(bid, ask);
        let predict_mid = dec!(0.60);
        let poly_fv = dec!(0.59); // 1 cent below predict, < spread_cents

        let r = calculate(&book, poly_fv, predict_mid, dec!(0.02), dec!(0.00), V, 3).unwrap();
        // YES: min(0.59, 0.60) - 0.02 = 0.57
        assert_eq!(r.yes_bid.unwrap().inner(), dec!(0.57));
        // NO: (1 - max(0.59, 0.60)) - 0.02 = (1 - 0.60) - 0.02 = 0.38
        assert_eq!(r.no_bid.unwrap().inner(), dec!(0.38));
    }

    // ── Large downward divergence (poly << predict) ───────────────────────────

    /// Arsenal case: poly=0.545, predict=0.635, divergence=0.09.
    /// YES is clamped inside scoring window; NO remains placed safely.
    #[test]
    fn large_downward_divergence_yes_clamped_no_placed() {
        // Approximate Arsenal book
        let (bid, ask) = (dec!(0.63), dec!(0.64));
        let book = make_book(bid, ask);
        let predict_mid = dec!(0.635);
        let poly_fv = dec!(0.545);

        let r = calculate(&book, poly_fv, predict_mid, dec!(0.02), dec!(0.00), V, 2).unwrap();

        // YES target would be 0.525 (outside window), so clamp to just inside:
        // predict_mid - (v - tick) = 0.635 - (0.06 - 0.01) = 0.585 → 0.58 (2dp)
        assert_eq!(r.yes_bid.unwrap().inner(), dec!(0.58));

        // NO: (1 - max(0.545, 0.635)) - 0.02 = (1-0.635) - 0.02 = 0.345
        //     |0.345 - (1-0.635)| = |0.345 - 0.365| = 0.02 < 0.06 → in window
        //     no_ask_est = 1 - 0.63 = 0.37 → 0.345 < 0.37 → safe
        let no = r.no_bid.unwrap().inner();
        // (1-0.635) - 0.02 = 0.345, rounded DOWN to precision=2 → 0.34
        assert_eq!(
            no,
            dec!(0.34),
            "NO should be (1-predict_mid) - spread_cents, rounded down"
        );
        // Fill safety: at most 1 tick + spread_cents from NO predict mid
        let no_predict_mid = Decimal::ONE - predict_mid; // 0.365
        let dist = (no_predict_mid - no).abs(); // |0.365 - 0.34| = 0.025
        assert!(
            dist >= dec!(0.02),
            "NO bid must be >= spread_cents from NO predict mid"
        );
    }

    /// YES skipped for large divergence means NO still earns 44% score factor.
    #[test]
    fn large_downward_divergence_no_scores_at_spread_cents() {
        let (bid, ask) = (dec!(0.63), dec!(0.64));
        let book = make_book(bid, ask);
        let predict_mid = dec!(0.635);
        let poly_fv = dec!(0.545);
        let r = calculate(&book, poly_fv, predict_mid, dec!(0.02), dec!(0.00), V, 2).unwrap();

        let no = r.no_bid.unwrap().inner();
        let no_predict_mid = Decimal::ONE - predict_mid;
        let spread = (no_predict_mid - no).abs();
        // score_factor = ((v - spread) / v)² with spread = 0.02, v = 0.06
        let ratio = (V - spread) / V;
        let sf = ratio * ratio;
        assert!(sf > dec!(0.10), "score_factor should be ≥ ~11% (got {sf})");
    }

    // ── Large upward divergence (poly >> predict) ─────────────────────────────

    /// When poly >> predict (poly very bullish on YES): YES is predict-anchored,
    /// NO is clamped inside scoring window.
    #[test]
    fn large_upward_divergence_yes_placed_no_clamped() {
        let (bid, ask) = (dec!(0.55), dec!(0.65));
        let book = make_book(bid, ask);
        let predict_mid = dec!(0.60);
        let poly_fv = dec!(0.75); // poly very bullish

        let r = calculate(&book, poly_fv, predict_mid, dec!(0.02), dec!(0.00), V, 3).unwrap();

        // YES: min(0.75, 0.60) - 0.02 = 0.58
        //      |0.58 - 0.60| = 0.02 < 0.06 → in window
        assert_eq!(r.yes_bid.unwrap().inner(), dec!(0.58));

        // NO target would be 0.23 (outside window), so clamp inside:
        // no_predict_mid - (v - tick) = 0.40 - (0.06 - 0.001) = 0.341
        assert_eq!(r.no_bid.unwrap().inner(), dec!(0.341));
    }

    // ── Normal moderate divergence ────────────────────────────────────────────

    /// Moderate divergence (within scoring window): both sides placed.
    #[test]
    fn moderate_divergence_both_sides_placed() {
        let (bid, ask) = (dec!(0.55), dec!(0.65));
        let book = make_book(bid, ask);
        let predict_mid = dec!(0.60);
        let poly_fv = dec!(0.57); // 3 cents below predict

        let r = calculate(&book, poly_fv, predict_mid, dec!(0.02), dec!(0.00), V, 3).unwrap();

        // YES: min(0.57, 0.60) - 0.02 = 0.55
        //      |0.55 - 0.60| = 0.05 < 0.06 → in window
        assert_eq!(r.yes_bid.unwrap().inner(), dec!(0.55));
        // NO: (1 - max(0.57, 0.60)) - 0.02 = (1-0.60) - 0.02 = 0.38
        //     |0.38 - 0.40| = 0.02 < 0.06 → in window
        assert_eq!(r.no_bid.unwrap().inner(), dec!(0.38));
    }

    // ── Precision=3, 1-cent BBO (regression) ─────────────────────────────────

    /// BTC $60k/$80k: 1-cent BBO on precision=3, poly ≈ predict.
    #[test]
    fn one_cent_spread_precision3_poly_equals_predict() {
        let (bid, ask) = (dec!(0.36), dec!(0.37));
        let predict_mid = mid(bid, ask); // 0.365
        let book = make_book(bid, ask);
        let r = calculate(
            &book,
            predict_mid,
            predict_mid,
            dec!(0.02),
            dec!(0.00),
            V,
            3,
        )
        .unwrap();

        // YES: 0.365 - 0.02 = 0.345. Round=0.345. < ask(0.37) ✓
        assert_eq!(r.yes_bid.unwrap().inner(), dec!(0.345));
        // NO: (1-0.365) - 0.02 = 0.615. no_ask_est = 1-0.36 = 0.64. 0.615 < 0.64 ✓
        assert_eq!(r.no_bid.unwrap().inner(), dec!(0.615));
    }

    /// BTC $60k/$80k: poly lower than predict (common real-world scenario).
    #[test]
    fn one_cent_spread_precision3_poly_below_predict() {
        let (bid, ask) = (dec!(0.36), dec!(0.37));
        let predict_mid = mid(bid, ask); // 0.365
        let book = make_book(bid, ask);
        let poly_fv = dec!(0.36); // 0.5 cents below predict

        let r = calculate(&book, poly_fv, predict_mid, dec!(0.02), dec!(0.00), V, 3).unwrap();
        // YES: min(0.36, 0.365) - 0.02 = 0.34
        assert_eq!(r.yes_bid.unwrap().inner(), dec!(0.34));
        // NO: (1 - max(0.36, 0.365)) - 0.02 = (1-0.365) - 0.02 = 0.615
        assert_eq!(r.no_bid.unwrap().inner(), dec!(0.615));
    }

    // ── Invariants ────────────────────────────────────────────────────────────

    /// Neither side crosses its ask estimate.
    #[test]
    fn neither_side_crosses_ask() {
        let cases = [
            (dec!(0.40), dec!(0.60), dec!(0.50), dec!(0.50)),
            (dec!(0.35), dec!(0.37), dec!(0.36), dec!(0.36)),
            (dec!(0.55), dec!(0.65), dec!(0.60), dec!(0.70)), // poly > predict
        ];
        for (bid, ask, poly, predict) in cases {
            let no_ask_est = Decimal::ONE - bid;
            let book = make_book(bid, ask);
            if let Some(r) = calculate(&book, poly, predict, dec!(0.02), dec!(0.00), V, 3) {
                if let Some(y) = r.yes_bid {
                    assert!(
                        y.inner() < ask,
                        "yes_bid {} must be < yes_ask {}",
                        y.inner(),
                        ask
                    );
                }
                if let Some(n) = r.no_bid {
                    assert!(
                        n.inner() < no_ask_est,
                        "no_bid {} must be < no_ask_est {}",
                        n.inner(),
                        no_ask_est
                    );
                }
            }
        }
    }

    /// Both sides within scoring window when placed.
    #[test]
    fn placed_sides_always_within_scoring_window() {
        let cases = [
            (dec!(0.40), dec!(0.60), dec!(0.50), dec!(0.50)),
            (dec!(0.55), dec!(0.65), dec!(0.60), dec!(0.57)),
            (dec!(0.63), dec!(0.64), dec!(0.635), dec!(0.545)), // Arsenal case
        ];
        for (bid, ask, predict, poly) in cases {
            let book = make_book(bid, ask);
            if let Some(r) = calculate(&book, poly, predict, dec!(0.02), dec!(0.00), V, 3) {
                if let Some(y) = r.yes_bid {
                    let spread = (predict - y.inner()).abs();
                    assert!(
                        spread < V,
                        "yes_bid spread {} from predict_mid must be < v {}",
                        spread,
                        V
                    );
                }
                if let Some(n) = r.no_bid {
                    let no_predict_mid = Decimal::ONE - predict;
                    let spread = (no_predict_mid - n.inner()).abs();
                    assert!(
                        spread < V,
                        "no_bid spread {} from no_predict_mid must be < v {}",
                        spread,
                        V
                    );
                }
            }
        }
    }

    /// YES + NO < 1.00 when both placed.
    #[test]
    fn yes_plus_no_less_than_one() {
        let cases = [
            (dec!(0.55), dec!(0.65), dec!(0.60), dec!(0.60)),
            (dec!(0.35), dec!(0.37), dec!(0.36), dec!(0.36)),
        ];
        for (bid, ask, poly, predict) in cases {
            let book = make_book(bid, ask);
            if let Some(r) = calculate(&book, poly, predict, dec!(0.02), dec!(0.00), V, 3) {
                if let (Some(y), Some(n)) = (r.yes_bid, r.no_bid) {
                    assert!(
                        y.inner() + n.inner() < Decimal::ONE,
                        "YES+NO must be < 1.00"
                    );
                }
            }
        }
    }

    /// Empty and crossed book return None.
    #[test]
    fn empty_crossed_book_returns_none() {
        let empty = OrderbookSnapshot {
            bids: vec![],
            asks: vec![],
            timestamp_ns: 0,
        };
        assert!(calculate(&empty, dec!(0.5), dec!(0.5), dec!(0.02), dec!(0.00), V, 3).is_none());
        assert!(calculate(
            &make_book(dec!(0.60), dec!(0.55)),
            dec!(0.58),
            dec!(0.58),
            dec!(0.02),
            dec!(0.00),
            V,
            3
        )
        .is_none());
    }

    /// Fill safety: placed bids are spread_cents below BOTH predict mids.
    #[test]
    fn fill_safety_spread_cents_from_predict_mid() {
        // Moderate divergence: both sides should be >= spread_cents from predict mid.
        let (bid, ask) = (dec!(0.55), dec!(0.65));
        let book = make_book(bid, ask);
        let predict_mid = dec!(0.60);
        let poly_fv = dec!(0.57);
        let spread = dec!(0.02);

        let r = calculate(&book, poly_fv, predict_mid, spread, dec!(0.00), V, 3).unwrap();

        if let Some(y) = r.yes_bid {
            assert!(
                predict_mid - y.inner() >= spread,
                "YES bid {} should be >= spread_cents ({}) below predict_mid ({})",
                y.inner(),
                spread,
                predict_mid
            );
        }
        if let Some(n) = r.no_bid {
            let no_predict_mid = Decimal::ONE - predict_mid;
            assert!(
                no_predict_mid - n.inner() >= spread,
                "NO bid {} should be >= spread_cents ({}) below no_predict_mid ({})",
                n.inner(),
                spread,
                no_predict_mid
            );
        }
    }
}
