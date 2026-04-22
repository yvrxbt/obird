//! `PredictionQuoter` — predict.fun points-farming market maker.
//!
//! ## Strategy logic
//!
//! Always: CancelAll → PlaceOrder(YES BUY) + PlaceOrder(NO BUY).
//! Never executes: the engine handles order submission.
//!
//! ### Points scoring context
//!
//! Predict.fun (like Polymarket) rewards resting limit orders quadratically:
//!   `score = ((v - spread) / v)² × size`
//! where `v` = max_spread window (market's `spreadThreshold`).
//! Two-sided (YES + NO) is required for full score on extreme-probability markets.
//! **Makers pay 0 fee** — getting filled is not costly, only directional exposure.
//!
//! ### Quoting
//!
//! `yes_bid = mid - spread_cents`
//! `no_bid  = 1 - yes_bid`  (binary market identity)
//!
//! Both are placed as BUY orders. The connector routes to the correct token.
//!
//! ### State machine
//! ```text
//! Empty → first BookUpdate → CancelAll + PlaceOrders → Quoting
//! Quoting → |mid_now - last_quoted_mid| > drift_cents → PULL + Cooldown
//! Quoting → Fill → PULL + Cooldown
//! Cooldown → timer expires → Empty → requote
//! ```
//!
//! ### Position limits
//!
//! Each outcome is tracked independently. When `yes_tokens >= max_position_tokens`,
//! skip YES placement. When `no_tokens >= max_position_tokens`, skip NO placement.
//! On shutdown, CancelAll is sent for both instruments.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Staleness threshold is now a strategy param (`fv_stale_secs`, default 90s).
// See QuoterParams for the tuning rationale — short summary:
//   Polymarket WS recv timeout = 60s. After 60s silence the feed reconnects and
//   re-delivers a fresh `book` snapshot. fv_stale_secs must exceed 60s to avoid
//   false stale-pauses on quiet (illiquid) markets.

use rust_decimal::Decimal;
use trading_core::{
    traits::{strategy::StrategyState, Strategy},
    types::order::{OrderRequest, OrderSide, TimeInForce},
    Action, Event, InstrumentId, Quantity,
};

use crate::params::QuoterParams;
use crate::pricing;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum State {
    /// No orders on book. Ready to quote on next tick.
    Empty,
    /// Orders are resting. `last_quoted_mid` is valid.
    Quoting,
    /// Quotes pulled. Waiting until this `Instant` to requote.
    Cooldown(Instant),
}

// ── Strategy ──────────────────────────────────────────────────────────────────

/// One completed quoting cycle — accumulated for the session points estimate.
#[derive(Debug, Clone)]
struct CycleRecord {
    on_book_secs: f64,
    yes_qty: Decimal,
    no_qty: Decimal,
    yes_bid: Decimal,
    no_bid: Decimal,
    /// Actual distance of yes_bid from predict.fun mid at placement time.
    /// Used for score_factor computation. 0 if YES was not placed this cycle.
    yes_spread_from_mid: Decimal,
    /// Actual distance of no_bid from the NO mid (= 1 - predict.fun mid) at placement.
    /// 0 if NO was not placed this cycle.
    no_spread_from_mid: Decimal,
    score_factor: Decimal,
    ended_by: &'static str, // "fill" | "drift" | "shutdown"
    yes_placed: bool,
    no_placed: bool,
}

pub struct PredictionQuoter {
    id: String,
    /// YES outcome instrument — subscribed for BookUpdate events.
    yes_instrument: InstrumentId,
    /// NO outcome instrument — receives Fill events via the connector.
    no_instrument: InstrumentId,
    /// Polymarket YES instrument — subscribed for BBO updates used as external FV.
    /// When `Some`, the Polymarket mid replaces the predict.fun mid as the fair value
    /// center. When `None` (or stale), falls back to predict.fun mid.
    polymarket_fv_instrument: Option<InstrumentId>,
    params: QuoterParams,
    /// Price tick precision fetched from the exchange at startup via StrategyState.
    /// 2 → 0.01 ticks (precision=2 markets), 3 → 0.001 ticks (precision=3 markets).
    decimal_precision: u32,

    state: State,

    /// Most recent Polymarket mid price (the external fair value signal).
    polymarket_mid: Option<Decimal>,
    /// Wall-clock instant when `polymarket_mid` was last updated. Used for staleness check.
    polymarket_mid_ts: Option<Instant>,

    /// Fair value at last requote (for drift detection — compares FV, not predict.fun mid).
    last_quoted_fv: Option<Decimal>,
    /// When orders were last placed. Used to enforce min_quote_hold_secs.
    last_place_time: Option<Instant>,
    /// Latch for ask-risk trigger; prevents repeated touch requotes on every tick
    /// while we remain in the same risk regime.
    touch_risk_latched: bool,

    /// Running token balance for YES outcome (filled tokens we hold).
    yes_tokens: Decimal,
    /// Running token balance for NO outcome (filled tokens we hold).
    no_tokens: Decimal,

    /// Approximate cost basis for P&L reporting. In USDT (price × qty).
    session_cost: Decimal,
    session_proceeds: Decimal,
    fill_count: u64,

    /// Per-side fill counters for adversarial selection diagnosis.
    /// Large yes_fills vs no_fills = consistently hit on YES = we are above poly FV on YES.
    session_yes_fills: u64,
    session_no_fills: u64,
    /// Per-side filled quantity this session (shares).
    session_yes_fill_qty: Decimal,
    session_no_fill_qty: Decimal,
    /// Cumulative notional (qty×price) per side this session.
    session_yes_notional: Decimal,
    session_no_notional: Decimal,
    /// Running sum of (fill_price - poly_fv_at_fill) per YES fill.
    /// Positive = we overpaid vs Polymarket ("adverse selection cost").
    session_yes_adverse_cents_total: Decimal,
    /// Same for NO.
    session_no_adverse_cents_total: Decimal,

    /// Most recent YES mid (for fill P&L reporting).
    latest_mid: Option<Decimal>,

    // ── Per-cycle points tracking ─────────────────────────────────────────────
    /// Wall-clock time when the engine started (for session duration).
    session_started_at: Option<Instant>,
    /// When the current cycle's orders were placed.
    cycle_placed_at: Option<Instant>,
    /// YES qty placed in the current cycle (shares, not USDT). 0 if YES was skipped.
    cycle_yes_qty: Decimal,
    /// NO qty placed in the current cycle (shares, not USDT). 0 if NO was skipped.
    cycle_no_qty: Decimal,
    /// Actual spread of yes_bid from predict.fun mid at placement. 0 if YES skipped.
    cycle_yes_spread_from_mid: Decimal,
    /// Actual spread of no_bid from NO mid (= 1 - predict mid) at placement. 0 if NO skipped.
    cycle_no_spread_from_mid: Decimal,
    /// Prices for the current cycle (0 if that side was skipped).
    cycle_yes_bid: Decimal,
    cycle_no_bid: Decimal,
    cycle_yes_placed: bool,
    cycle_no_placed: bool,
    /// Completed cycles this session.
    cycles: Vec<CycleRecord>,
}

impl PredictionQuoter {
    pub fn new(
        id: String,
        yes_instrument: InstrumentId,
        no_instrument: InstrumentId,
        params: QuoterParams,
        // Polymarket YES instrument for external FV. Pass `None` to fall back to predict.fun mid.
        polymarket_fv_instrument: Option<InstrumentId>,
    ) -> Self {
        Self {
            id,
            yes_instrument,
            no_instrument,
            polymarket_fv_instrument,
            params,
            decimal_precision: 3, // default; overridden in initialize() from StrategyState
            state: State::Empty,
            polymarket_mid: None,
            polymarket_mid_ts: None,
            last_quoted_fv: None,
            last_place_time: None,
            touch_risk_latched: false,
            yes_tokens: Decimal::ZERO,
            no_tokens: Decimal::ZERO,
            session_cost: Decimal::ZERO,
            session_proceeds: Decimal::ZERO,
            fill_count: 0,
            session_yes_fills: 0,
            session_no_fills: 0,
            session_yes_fill_qty: Decimal::ZERO,
            session_no_fill_qty: Decimal::ZERO,
            session_yes_notional: Decimal::ZERO,
            session_no_notional: Decimal::ZERO,
            session_yes_adverse_cents_total: Decimal::ZERO,
            session_no_adverse_cents_total: Decimal::ZERO,
            latest_mid: None,
            session_started_at: None,
            cycle_placed_at: None,
            cycle_yes_qty: Decimal::ZERO,
            cycle_no_qty: Decimal::ZERO,
            cycle_yes_spread_from_mid: Decimal::ZERO,
            cycle_no_spread_from_mid: Decimal::ZERO,
            cycle_yes_bid: Decimal::ZERO,
            cycle_no_bid: Decimal::ZERO,
            cycle_yes_placed: false,
            cycle_no_placed: false,
            cycles: Vec::new(),
        }
    }

    // ── Points helpers ────────────────────────────────────────────────────────

    /// `((v - spread) / v)^2` — the quadratic score multiplier for a given spread.
    /// Returns 0 if spread ≥ v (quote is outside the earning window).
    fn score_factor(&self, spread_from_mid: Decimal) -> Decimal {
        let v = self.params.spread_threshold_v;
        if v <= Decimal::ZERO || spread_from_mid >= v {
            return Decimal::ZERO;
        }
        let ratio = (v - spread_from_mid) / v;
        ratio * ratio
    }

    /// Close out the current cycle, record it, reset cycle state.
    fn close_cycle(&mut self, ended_by: &'static str) {
        let Some(placed_at) = self.cycle_placed_at.take() else {
            return;
        };
        let on_book_secs = placed_at.elapsed().as_secs_f64();
        let t = Decimal::try_from(on_book_secs).unwrap_or(Decimal::ZERO);

        // Compute per-side score factors from actual spread at placement.
        let sf_yes = self.score_factor(self.cycle_yes_spread_from_mid);
        let sf_no = self.score_factor(self.cycle_no_spread_from_mid);

        tracing::info!(
            target: "quoter",
            strategy      = %self.id,
            ended_by,
            on_book_secs  = format!("{:.1}", on_book_secs),
            yes_qty       = %self.cycle_yes_qty,
            no_qty        = %self.cycle_no_qty,
            yes_bid       = %self.cycle_yes_bid,
            no_bid        = %self.cycle_no_bid,
            yes_placed    = self.cycle_yes_placed,
            no_placed     = self.cycle_no_placed,
            yes_spread    = %self.cycle_yes_spread_from_mid.round_dp(4),
            no_spread     = %self.cycle_no_spread_from_mid.round_dp(4),
            score_factor_yes = %sf_yes.round_dp(4),
            score_factor_no  = %sf_no.round_dp(4),
            est_yes_score = %( sf_yes * self.cycle_yes_qty * t ).round_dp(1),
            est_no_score  = %( sf_no  * self.cycle_no_qty  * t ).round_dp(1),
            "CYCLE_END",
        );

        // Use YES score factor for the record (or NO if YES not placed).
        let sf = if self.cycle_yes_placed { sf_yes } else { sf_no };

        self.cycles.push(CycleRecord {
            on_book_secs,
            yes_qty: self.cycle_yes_qty,
            no_qty: self.cycle_no_qty,
            yes_bid: self.cycle_yes_bid,
            no_bid: self.cycle_no_bid,
            yes_spread_from_mid: self.cycle_yes_spread_from_mid,
            no_spread_from_mid: self.cycle_no_spread_from_mid,
            score_factor: sf,
            ended_by,
            yes_placed: self.cycle_yes_placed,
            no_placed: self.cycle_no_placed,
        });
    }

    /// Start tracking a new cycle.
    fn open_cycle(
        &mut self,
        yes_qty: Decimal,
        no_qty: Decimal,
        yes_bid: Decimal,
        no_bid: Decimal,
        yes_placed: bool,
        no_placed: bool,
        predict_mid: Decimal,
    ) {
        self.cycle_placed_at = Some(Instant::now());
        self.cycle_yes_qty = yes_qty;
        self.cycle_no_qty = no_qty;
        self.cycle_yes_bid = yes_bid;
        self.cycle_no_bid = no_bid;
        self.cycle_yes_placed = yes_placed;
        self.cycle_no_placed = no_placed;
        // Spread = distance from respective mid at placement time.
        self.cycle_yes_spread_from_mid = if yes_placed {
            (predict_mid - yes_bid).abs()
        } else {
            Decimal::ZERO
        };
        self.cycle_no_spread_from_mid = if no_placed {
            let no_mid = Decimal::ONE - predict_mid;
            (no_mid - no_bid).abs()
        } else {
            Decimal::ZERO
        };
    }

    fn utc_now_iso() -> String {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Simple ISO-8601 without chrono dependency.
        let s = secs;
        let (y, mo, d, h, mi, sec) = epoch_to_ymd_hms(s);
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}Z")
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Return the raw Polymarket mid, or `None` to pause quoting.
    ///
    /// Invariant #5: no quoting without fresh Polymarket FV — never fall back to
    /// predict.fun mid. If Polymarket is unconfigured, the feed is waiting for its
    /// first update, or the last update is stale, this returns `None` and the
    /// caller pauses/cancels quotes.
    ///
    /// The per-side min/max FV logic that keeps bids within the scoring window lives
    /// in `pricing::calculate`, not here. This function is purely a freshness gate.
    fn poly_fv(&self, _predict_mid: Decimal) -> Option<Decimal> {
        if self.polymarket_fv_instrument.is_none() {
            tracing::warn!(
                target: "quoter",
                strategy = %self.id,
                "Polymarket FV not configured — refusing to quote (invariant #5)",
            );
            return None;
        }

        match (self.polymarket_mid, self.polymarket_mid_ts) {
            (Some(pm), Some(ts)) if ts.elapsed().as_secs() < self.params.fv_stale_secs => Some(pm),
            (Some(_), _) => {
                tracing::warn!(
                    target: "quoter",
                    strategy = %self.id,
                    fv_stale_secs = self.params.fv_stale_secs,
                    "Polymarket FV stale — pausing quotes until feed recovers",
                );
                None
            }
            _ => {
                tracing::info!(
                    target: "quoter",
                    strategy = %self.id,
                    "Waiting for first Polymarket FV update before quoting",
                );
                None
            }
        }
    }

    /// True when the fair value has drifted beyond `drift_cents` since last requote.
    fn fv_drifted(&self, current_fv: Decimal) -> bool {
        match self.last_quoted_fv {
            None => true,
            Some(last) => (current_fv - last).abs() > self.params.drift_cents,
        }
    }

    /// True when currently resting quotes are at/near ask (hit-risk zone).
    ///
    /// YES hit-risk distance = `yes_ask - yes_bid`.
    /// NO  hit-risk distance = `no_ask_est - no_bid`, where `no_ask_est = 1 - yes_best_bid`.
    fn near_ask(&self, yes_book: &trading_core::types::market_data::OrderbookSnapshot) -> bool {
        let trigger = self.params.touch_trigger_cents;

        let (yes_best_bid, _) = match yes_book.best_bid() {
            Some(v) => v,
            None => return false,
        };
        let (yes_best_ask, _) = match yes_book.best_ask() {
            Some(v) => v,
            None => return false,
        };

        let yes_ask = yes_best_ask.inner();
        let no_ask_est = Decimal::ONE - yes_best_bid.inner();

        let yes_dist = yes_ask - self.cycle_yes_bid;
        let no_dist = no_ask_est - self.cycle_no_bid;

        let yes_touch = self.cycle_yes_placed && yes_dist <= trigger;
        let no_touch = self.cycle_no_placed && no_dist <= trigger;

        if yes_touch || no_touch {
            tracing::info!(
                target: "quoter",
                strategy = %self.id,
                yes_touch,
                no_touch,
                yes_dist = %yes_dist.round_dp(4),
                no_dist = %no_dist.round_dp(4),
                trigger = %trigger,
                "Near ask detected — defensive requote",
            );
        }

        yes_touch || no_touch
    }

    /// Cancel all resting orders (connector clears both YES and NO).
    /// We use the YES instrument — `cancel_all` in `PredictFunClient`
    /// cancels every tracked order across both outcomes.
    fn cancel_all(&self) -> Action {
        Action::CancelAll {
            instrument: self.yes_instrument.clone(),
        }
    }

    /// Build BUY orders for one quoting cycle from independently priced YES and NO.
    ///
    /// Either side may be `None` in `pricing` (skipped due to crossing guard) or
    /// suppressed here due to position limits. Returns placed orders and the
    /// effective qty for each side (0 = not placed).
    fn build_place_actions(
        &self,
        pricing: &pricing::PricingResult,
    ) -> (Vec<Action>, Decimal, Decimal) {
        let mut actions = Vec::with_capacity(2);
        let mut yes_qty_placed = Decimal::ZERO;
        let mut no_qty_placed = Decimal::ZERO;

        // YES BUY
        match pricing.yes_bid {
            Some(price) if self.yes_tokens < self.params.max_position_tokens => {
                let qty = (self.params.order_size_usdt / price.inner())
                    .round_dp(4)
                    .max(Decimal::ONE / Decimal::from(10_000));
                actions.push(Action::PlaceOrder(OrderRequest {
                    instrument: self.yes_instrument.clone(),
                    side: OrderSide::Buy,
                    price,
                    quantity: Quantity::new(qty),
                    tif: TimeInForce::PostOnly,
                    client_order_id: Some("yes".into()),
                }));
                yes_qty_placed = qty;
            }
            Some(_) => {
                tracing::info!(
                    target: "quoter",
                    strategy   = %self.id,
                    yes_tokens = %self.yes_tokens,
                    max        = %self.params.max_position_tokens,
                    "YES position limit reached — skipping YES quote",
                );
            }
            None => {} // pricing guard skipped this side
        }

        // NO BUY
        match pricing.no_bid {
            Some(price) if self.no_tokens < self.params.max_position_tokens => {
                let qty = (self.params.order_size_usdt / price.inner())
                    .round_dp(4)
                    .max(Decimal::ONE / Decimal::from(10_000));
                actions.push(Action::PlaceOrder(OrderRequest {
                    instrument: self.no_instrument.clone(),
                    side: OrderSide::Buy,
                    price,
                    quantity: Quantity::new(qty),
                    tif: TimeInForce::PostOnly,
                    client_order_id: Some("no".into()),
                }));
                no_qty_placed = qty;
            }
            Some(_) => {
                tracing::info!(
                    target: "quoter",
                    strategy  = %self.id,
                    no_tokens = %self.no_tokens,
                    max       = %self.params.max_position_tokens,
                    "NO position limit reached — skipping NO quote",
                );
            }
            None => {} // pricing guard skipped this side
        }

        (actions, yes_qty_placed, no_qty_placed)
    }

    /// Full requote cycle: cancel everything → place YES and/or NO independently.
    ///
    /// `predict_mid`: current predict.fun YES mid (used for score/cycle tracking).
    /// `fv`:          effective fair value used for pricing (Polymarket mid, or
    ///                predict.fun mid when Polymarket is not configured).
    /// `pricing`:     independently computed YES and NO prices (either may be None
    ///                if a crossing guard blocked that side).
    fn requote(
        &mut self,
        predict_mid: Decimal,
        fv: Decimal,
        yes_book: &trading_core::types::market_data::OrderbookSnapshot,
        pricing: pricing::PricingResult,
        ended_by: &'static str,
    ) -> Vec<Action> {
        // Close the previous cycle before starting a new one.
        self.close_cycle(ended_by);

        let mut actions = Vec::with_capacity(3);
        actions.push(self.cancel_all());
        let (place, yes_qty, no_qty) = self.build_place_actions(&pricing);

        let yes_bid_val = pricing.yes_bid.map(|p| p.inner()).unwrap_or(Decimal::ZERO);
        let no_bid_val = pricing.no_bid.map(|p| p.inner()).unwrap_or(Decimal::ZERO);

        let yes_placed = yes_qty > Decimal::ZERO;
        let no_placed = no_qty > Decimal::ZERO;

        // Score factor for each placed side.
        let sf_yes = if yes_placed {
            self.score_factor((predict_mid - yes_bid_val).abs())
        } else {
            Decimal::ZERO
        };
        let sf_no = if no_placed {
            let no_mid = Decimal::ONE - predict_mid;
            self.score_factor((no_mid - no_bid_val).abs())
        } else {
            Decimal::ZERO
        };

        let qualifies_yes = yes_qty >= self.params.min_shares_per_side;
        let qualifies_no = no_qty >= self.params.min_shares_per_side;

        let poly_divergence = self
            .polymarket_mid
            .map(|p| (p - predict_mid).abs().round_dp(4));

        let (yes_best_bid, yes_best_bid_qty) = yes_book
            .best_bid()
            .map(|(p, q)| (p.inner(), q.inner()))
            .unwrap_or((Decimal::ZERO, Decimal::ZERO));
        let (yes_best_ask, yes_best_ask_qty) = yes_book
            .best_ask()
            .map(|(p, q)| (p.inner(), q.inner()))
            .unwrap_or((Decimal::ZERO, Decimal::ZERO));
        let yes_spread = (yes_best_ask - yes_best_bid).max(Decimal::ZERO);
        let no_ask_est = (Decimal::ONE - yes_best_bid).max(Decimal::ZERO);

        tracing::info!(
            target: "quoter",
            strategy        = %self.id,
            predict_mid     = %predict_mid,
            poly_fv         = ?self.polymarket_mid,
            poly_divergence = ?poly_divergence,
            yes_fv_used     = %(fv.min(predict_mid)),
            no_fv_used      = %(Decimal::ONE - fv.max(predict_mid)),
            yes_best_bid    = %yes_best_bid,
            yes_best_bid_qty = %yes_best_bid_qty,
            yes_best_ask    = %yes_best_ask,
            yes_best_ask_qty = %yes_best_ask_qty,
            yes_spread      = %yes_spread.round_dp(4),
            no_ask_est      = %no_ask_est.round_dp(4),
            yes_bid         = ?pricing.yes_bid.map(|p| p.inner()),
            no_bid          = ?pricing.no_bid.map(|p| p.inner()),
            yes_qty    = %yes_qty,
            no_qty     = %no_qty,
            yes_placed,
            no_placed,
            n_orders           = place.len(),
            yes_pos            = %self.yes_tokens,
            no_pos             = %self.no_tokens,
            // Adverse selection health indicators:
            // yes_pos_pct >70% + yes_fills_pct >70% = being adversely selected on YES
            yes_pos_pct = %{
                let total = self.yes_tokens + self.no_tokens;
                if total.is_zero() { Decimal::ZERO }
                else { (self.yes_tokens / total * Decimal::ONE_HUNDRED).round_dp(1) }
            },
            session_yes_fills  = self.session_yes_fills,
            session_no_fills   = self.session_no_fills,
            yes_adverse_total  = %self.session_yes_adverse_cents_total.round_dp(4),
            score_factor_yes   = %sf_yes.round_dp(4),
            score_factor_no    = %sf_no.round_dp(4),
            qualifies_yes,
            qualifies_no,
            min_shares         = %self.params.min_shares_per_side,
            spread_threshold_v = %self.params.spread_threshold_v,
            "REQUOTE",
        );

        if !place.is_empty() {
            self.open_cycle(
                yes_qty,
                no_qty,
                yes_bid_val,
                no_bid_val,
                yes_placed,
                no_placed,
                predict_mid,
            );
        }

        actions.extend(place);
        self.state = State::Quoting;
        self.last_quoted_fv = Some(fv);
        self.last_place_time = Some(Instant::now());
        actions
    }

    /// Pull quotes and enter cooldown.
    fn pull_quotes(&mut self, reason: &'static str, pause_secs: u64, mid: Decimal) -> Vec<Action> {
        if matches!(self.state, State::Cooldown(_)) {
            tracing::debug!(
                target: "quoter",
                strategy = %self.id,
                reason,
                "PULL_QUOTES skipped (already in cooldown)",
            );
            return vec![];
        }

        // Close the cycle that's being pulled.
        self.close_cycle(reason);

        tracing::info!(
            target: "quoter",
            strategy = %self.id,
            reason,
            mid = %mid,
            pause_secs,
            "PULL_QUOTES",
        );

        self.state = State::Cooldown(Instant::now() + Duration::from_secs(pause_secs));
        self.last_quoted_fv = None;
        self.touch_risk_latched = false;
        vec![self.cancel_all()]
    }
}

// ── Strategy trait ────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl Strategy for PredictionQuoter {
    fn id(&self) -> &str {
        &self.id
    }

    fn subscriptions(&self) -> Vec<InstrumentId> {
        // predict.fun YES + NO for book/fill events.
        // Polymarket YES for external fair value (if configured).
        let mut subs = vec![self.yes_instrument.clone(), self.no_instrument.clone()];
        if let Some(ref poly) = self.polymarket_fv_instrument {
            subs.push(poly.clone());
        }
        subs
    }

    async fn on_event(&mut self, event: &Event) -> Vec<Action> {
        match event {
            Event::BookUpdate {
                instrument, book, ..
            } => {
                // ── Polymarket FV update ──────────────────────────────────────
                // Store the Polymarket mid for use in the next predict.fun requote cycle.
                // Does NOT trigger a requote by itself — the predict.fun book tick drives
                // the quoting loop. (The Polymarket mid will be consumed on the next
                // predict.fun BookUpdate if the FV has drifted past drift_cents.)
                if Some(instrument) == self.polymarket_fv_instrument.as_ref() {
                    if let Some(m) = book.mid_price() {
                        self.polymarket_mid = Some(m.inner());
                        self.polymarket_mid_ts = Some(Instant::now());
                        tracing::info!(
                            target: "quoter",
                            strategy = %self.id,
                            poly_mid = %m.inner(),
                            "POLY_FV_UPDATE",
                        );
                    }
                    return vec![];
                }

                // ── predict.fun YES book update (drives quoting) ──────────────
                if instrument != &self.yes_instrument {
                    return vec![];
                }

                let mid = match book.mid_price() {
                    Some(m) => m.inner(),
                    None => return vec![],
                };
                self.latest_mid = Some(mid);

                // Raw Polymarket mid (freshness gate only). If unavailable, pause.
                // The min/max per-side conservative pricing is done inside pricing::calculate.
                let fv = match self.poly_fv(mid) {
                    Some(v) => v,
                    None => {
                        if matches!(self.state, State::Quoting) {
                            return self.pull_quotes(
                                "poly_fv_unavailable",
                                self.params.fill_pause_secs,
                                mid,
                            );
                        }
                        return vec![];
                    }
                };

                tracing::debug!(
                    target: "quoter",
                    strategy = %self.id,
                    predict_mid = %mid,
                    fv = %fv,
                    poly_mid = ?self.polymarket_mid,
                    state = ?self.state,
                    yes_pos = %self.yes_tokens,
                    no_pos  = %self.no_tokens,
                    "BOOK_UPDATE",
                );

                // Advance cooldown.
                match self.state {
                    State::Cooldown(t) if Instant::now() >= t => {
                        tracing::info!(target: "quoter", strategy = %self.id, fv = %fv, "COOLDOWN_EXPIRED");
                        self.state = State::Empty;
                        self.touch_risk_latched = false;
                    }
                    State::Cooldown(_) => return vec![],
                    _ => {}
                }

                let touch_risk_now = matches!(self.state, State::Quoting) && self.near_ask(book);
                let touch_requote = matches!(self.state, State::Quoting)
                    && touch_risk_now
                    && !self.touch_risk_latched;
                if matches!(self.state, State::Quoting) {
                    self.touch_risk_latched = touch_risk_now;
                }
                let fv_requote = matches!(self.state, State::Quoting) && self.fv_drifted(fv);

                // While quoting, requote only on FV drift or touch-risk.
                if matches!(self.state, State::Quoting) && !(fv_requote || touch_requote) {
                    return vec![];
                }

                // Enforce minimum hold time — don't pull quotes just because FV
                // twitched within drift range and back. Fill-triggered cancels bypass this.
                // Touch-risk requotes bypass this guard.
                if matches!(self.state, State::Quoting) && !touch_requote {
                    if let Some(placed_at) = self.last_place_time {
                        let hold = Duration::from_secs(self.params.min_quote_hold_secs);
                        if Instant::now() < placed_at + hold {
                            return vec![];
                        }
                    }
                }

                // Calculate per-side prices using conservative dual-FV pricing:
                //   YES uses min(poly_fv, predict_mid) — below both YES mids
                //   NO  uses 1 - max(poly_fv, predict_mid) — below both NO mids
                // For touch-triggered requotes, enforce retreat from top-of-book.
                let retreat_cents = if touch_requote {
                    self.params.touch_retreat_cents
                } else {
                    Decimal::ZERO
                };

                let pricing = match pricing::calculate(
                    book,
                    fv,  // poly_fv (or predict_mid when poly not configured)
                    mid, // predict.fun book mid
                    self.params.spread_cents,
                    retreat_cents,
                    self.params.spread_threshold_v,
                    self.decimal_precision,
                ) {
                    Some(p) => p,
                    None => {
                        tracing::debug!(
                            target: "quoter",
                            strategy = %self.id,
                            fv = %fv,
                            "pricing returned None (empty/crossed book) — skipping tick",
                        );
                        return vec![];
                    }
                };

                if pricing.is_empty() {
                    tracing::debug!(
                        target: "quoter",
                        strategy     = %self.id,
                        fv           = %fv,
                        predict_mid  = %mid,
                        "both sides skipped by pricing — not requoting",
                    );
                    return vec![];
                }

                let reason = if touch_requote { "touch" } else { "drift" };
                self.requote(mid, fv, book, pricing, reason)
            }

            Event::Fill {
                instrument, fill, ..
            } => {
                let fill_price = fill.price.inner();
                let fill_qty   = fill.quantity.inner();
                let is_yes     = instrument == &self.yes_instrument;

                // Track positions per outcome.
                if is_yes {
                    self.yes_tokens           += fill_qty;
                    self.session_yes_fills    += 1;
                    self.session_yes_fill_qty += fill_qty;
                    self.session_yes_notional += fill_price * fill_qty;
                } else if instrument == &self.no_instrument {
                    self.no_tokens           += fill_qty;
                    self.session_no_fills    += 1;
                    self.session_no_fill_qty += fill_qty;
                    self.session_no_notional += fill_price * fill_qty;
                }

                self.fill_count   += 1;
                self.session_cost += fill_price * fill_qty;

                // ── Adverse selection accounting ──────────────────────────────
                // How much did we overpay vs Polymarket fair value at fill time?
                // Positive adverse_sel_cents = we paid above what Polymarket thinks
                // (we are being filled by informed traders).
                let adverse_sel_cents = self.polymarket_mid.map(|poly_fv| {
                    if is_yes {
                        fill_price - poly_fv           // paid above poly YES mid
                    } else {
                        fill_price - (Decimal::ONE - poly_fv)  // paid above poly NO mid
                    }
                });
                if let Some(adv) = adverse_sel_cents {
                    if is_yes {
                        self.session_yes_adverse_cents_total += adv * fill_qty;
                    } else {
                        self.session_no_adverse_cents_total  += adv * fill_qty;
                    }
                }

                // ── Mark-to-market ───────────────────────────────────────────
                // Mark against poly FV if available (more reliable than predict mid).
                // `session_pnl` = mark value of session fills - session cost.
                // NOTE: does not include pre-existing position from prior sessions.
                let mark_price = self.polymarket_mid.unwrap_or_else(||
                    self.latest_mid.unwrap_or(fill_price)
                );
                let mid = self.latest_mid.unwrap_or(fill_price);
                let open_value = self.yes_tokens * mark_price
                    + self.no_tokens * (Decimal::ONE - mark_price);
                let session_pnl = open_value - self.session_cost + self.session_proceeds;

                // Imbalance: what fraction of total fill count is YES?
                let yes_fill_pct = if self.fill_count > 0 {
                    Decimal::from(self.session_yes_fills) * Decimal::ONE_HUNDRED
                        / Decimal::from(self.fill_count)
                } else {
                    Decimal::ZERO
                };

                let yes_adv_per_share = if self.session_yes_fill_qty > Decimal::ZERO {
                    self.session_yes_adverse_cents_total / self.session_yes_fill_qty
                } else {
                    Decimal::ZERO
                };
                let no_adv_per_share = if self.session_no_fill_qty > Decimal::ZERO {
                    self.session_no_adverse_cents_total / self.session_no_fill_qty
                } else {
                    Decimal::ZERO
                };

                let cycle_side_placed = if is_yes {
                    self.cycle_yes_placed
                } else {
                    self.cycle_no_placed
                };
                let cycle_bid = if is_yes {
                    self.cycle_yes_bid
                } else {
                    self.cycle_no_bid
                };
                let fill_vs_cycle_bid = if cycle_side_placed {
                    Some(fill_price - cycle_bid)
                } else {
                    None
                };
                let fill_age_ms = self.cycle_placed_at.map(|t| t.elapsed().as_millis() as u64);

                tracing::info!(
                    target: "quoter",
                    strategy   = %self.id,
                    instrument = %instrument,
                    order_id   = %fill.order_id,
                    side       = ?fill.side,
                    price      = %fill_price,
                    qty        = %fill_qty,
                    cycle_side_placed,
                    cycle_bid = %cycle_bid,
                    fill_vs_cycle_bid = ?fill_vs_cycle_bid.map(|v| v.round_dp(4)),
                    fill_age_ms,
                    // Position
                    yes_tokens = %self.yes_tokens,
                    no_tokens  = %self.no_tokens,
                    yes_fill_pct = %yes_fill_pct.round_dp(1),  // >70% → yes adverse sel
                    // P&L
                    poly_fv_at_fill = ?self.polymarket_mid,
                    adverse_sel_cents = ?adverse_sel_cents.map(|v| v.round_dp(4)),
                    session_cost = %self.session_cost.round_dp(4),
                    session_pnl  = %session_pnl.round_dp(4),   // vs poly FV mark
                    // Cumulative adverse selection cost this session
                    yes_adverse_total = %self.session_yes_adverse_cents_total.round_dp(4),
                    no_adverse_total  = %self.session_no_adverse_cents_total.round_dp(4),
                    yes_adverse_per_share = %yes_adv_per_share.round_dp(4),
                    no_adverse_per_share  = %no_adv_per_share.round_dp(4),
                    fill_count = self.fill_count,
                    pause_secs = self.params.fill_pause_secs,
                    "FILL",
                );

                // Pull and cooldown.
                self.pull_quotes("fill", self.params.fill_pause_secs, mid)
            }

            Event::OrderUpdate { update, .. } => {
                use trading_core::types::order::OrderStatus;
                if matches!(update.status, OrderStatus::Rejected) {
                    tracing::error!(
                        target: "quoter",
                        strategy = %self.id,
                        oid = %update.order_id,
                        "ORDER_REJECTED",
                    );
                }
                vec![]
            }

            Event::PlaceFailed { reason, .. } => {
                if matches!(self.state, State::Cooldown(_)) {
                    return vec![];
                }
                let pause_secs = 30u64;
                self.state = State::Cooldown(Instant::now() + Duration::from_secs(pause_secs));
                self.last_quoted_fv = None;
                tracing::warn!(
                    target: "quoter",
                    strategy = %self.id,
                    reason = %reason,
                    pause_secs,
                    "PLACE_FAILED — backing off",
                );
                vec![] // orders never landed, no cancel needed
            }

            _ => vec![],
        }
    }

    async fn initialize(&mut self, state: &StrategyState) -> Vec<Action> {
        self.session_started_at = Some(Instant::now());

        // Set price precision for our YES quoting instrument from StrategyState.
        // We look up by the YES instrument (not NO or the Polymarket FV leg) so
        // a multi-connector engine can never hand us a different venue's tick
        // size. Default to 3 (0.001 ticks) if not present — safer for unknown
        // markets, and the predict.fun connector always registers a value so
        // the default is only hit in tests / backtest.
        if let Some(prec) = state.decimal_precisions.get(&self.yes_instrument) {
            self.decimal_precision = *prec;
        }

        // Load any existing positions (in case of restart).
        for pos in &state.positions {
            if pos.instrument == self.yes_instrument {
                self.yes_tokens = pos.size.inner();
                tracing::info!(target: "quoter", strategy = %self.id, yes_tokens = %self.yes_tokens, "INIT existing YES position");
            } else if pos.instrument == self.no_instrument {
                self.no_tokens = pos.size.inner();
                tracing::info!(target: "quoter", strategy = %self.id, no_tokens = %self.no_tokens, "INIT existing NO position");
            }
        }

        tracing::info!(
            target: "quoter",
            strategy = %self.id,
            yes_instrument = %self.yes_instrument,
            no_instrument  = %self.no_instrument,
            polymarket_fv  = ?self.polymarket_fv_instrument,
            spread_cents        = %self.params.spread_cents,
            decimal_precision   = self.decimal_precision,
            order_size_usdt     = %self.params.order_size_usdt,
            drift_cents         = %self.params.drift_cents,
            touch_trigger_cents = %self.params.touch_trigger_cents,
            touch_retreat_cents = %self.params.touch_retreat_cents,
            fill_pause_secs     = self.params.fill_pause_secs,
            max_position_tokens = %self.params.max_position_tokens,
            spread_threshold_v  = %self.params.spread_threshold_v,
            min_shares_per_side = %self.params.min_shares_per_side,
            fv_stale_secs       = self.params.fv_stale_secs,
            "INIT",
        );

        // Wipe any stale orders from a previous run.
        vec![Action::CancelAll {
            instrument: self.yes_instrument.clone(),
        }]
    }

    async fn shutdown(&mut self) -> Vec<Action> {
        // Close any in-flight cycle.
        self.close_cycle("shutdown");

        let mid = self.latest_mid.unwrap_or(Decimal::ZERO);
        // Mark against poly FV if available (more reliable than predict mid).
        let mark = self.polymarket_mid.unwrap_or(mid);
        let open_value = self.yes_tokens * mark + self.no_tokens * (Decimal::ONE - mark);
        let unrealized_pnl = open_value - self.session_cost + self.session_proceeds;
        let runtime_secs = self
            .session_started_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        // ── Aggregate points estimate ─────────────────────────────────────────
        // est_score_raw = Σ score_factor × qty × on_book_secs.
        // Time unit is unknown from the docs — record the raw accumulator.
        // When Z reports dashboard points, we back-infer:
        //   points_per_score_second = reported_points / est_score_raw
        // and use that rate for future projections.
        let mut est_yes_score_raw = Decimal::ZERO;
        let mut est_no_score_raw = Decimal::ZERO;
        let mut total_on_book_secs = 0f64;

        for c in &self.cycles {
            let t = Decimal::try_from(c.on_book_secs).unwrap_or(Decimal::ZERO);
            let sf_yes = self.score_factor(c.yes_spread_from_mid);
            let sf_no = self.score_factor(c.no_spread_from_mid);
            est_yes_score_raw += sf_yes * c.yes_qty * t;
            est_no_score_raw += sf_no * c.no_qty * t;
            total_on_book_secs += c.on_book_secs;
        }

        let pct_on_book = if runtime_secs > 0 {
            (total_on_book_secs / runtime_secs as f64 * 100.0).round() as u64
        } else {
            0
        };

        tracing::info!(
            target: "quoter",
            strategy          = %self.id,
            yes_tokens        = %self.yes_tokens,
            no_tokens         = %self.no_tokens,
            yes_no_ratio      = %{
                let total = self.yes_tokens + self.no_tokens;
                if total.is_zero() { Decimal::ZERO }
                else { (self.yes_tokens / total * Decimal::ONE_HUNDRED).round_dp(1) }
            },  // >70% = heavily yes-skewed = adverse selection signal
            fill_count        = self.fill_count,
            session_yes_fills = self.session_yes_fills,
            session_no_fills  = self.session_no_fills,
            session_yes_fill_qty = %self.session_yes_fill_qty.round_dp(4),
            session_no_fill_qty  = %self.session_no_fill_qty.round_dp(4),
            session_yes_vwap = %{
                if self.session_yes_fill_qty > Decimal::ZERO {
                    (self.session_yes_notional / self.session_yes_fill_qty).round_dp(4)
                } else {
                    Decimal::ZERO
                }
            },
            session_no_vwap = %{
                if self.session_no_fill_qty > Decimal::ZERO {
                    (self.session_no_notional / self.session_no_fill_qty).round_dp(4)
                } else {
                    Decimal::ZERO
                }
            },
            session_cost      = %self.session_cost.round_dp(4),
            session_pnl_poly_mark = %unrealized_pnl.round_dp(4),
            poly_mark_price   = ?self.polymarket_mid,
            // Adverse selection: total USDT overpaid vs poly FV this session
            yes_adverse_total_usdt = %self.session_yes_adverse_cents_total.round_dp(4),
            no_adverse_total_usdt  = %self.session_no_adverse_cents_total.round_dp(4),
            yes_adverse_per_share = %{
                if self.session_yes_fill_qty > Decimal::ZERO {
                    (self.session_yes_adverse_cents_total / self.session_yes_fill_qty).round_dp(4)
                } else {
                    Decimal::ZERO
                }
            },
            no_adverse_per_share = %{
                if self.session_no_fill_qty > Decimal::ZERO {
                    (self.session_no_adverse_cents_total / self.session_no_fill_qty).round_dp(4)
                } else {
                    Decimal::ZERO
                }
            },
            runtime_secs,
            cycles            = self.cycles.len(),
            on_book_secs      = format!("{:.0}", total_on_book_secs),
            pct_on_book,
            est_yes_score_raw = %est_yes_score_raw.round_dp(1),
            est_no_score_raw  = %est_no_score_raw.round_dp(1),
            "SESSION_SUMMARY",
        );

        // ── Write session record to points log ────────────────────────────────
        let session_end = Self::utc_now_iso();
        let cycle_json: Vec<serde_json::Value> = self
            .cycles
            .iter()
            .map(|c| {
                serde_json::json!({
                    "on_book_secs":        (c.on_book_secs * 10.0).round() / 10.0,
                    "yes_qty":             c.yes_qty,
                    "no_qty":              c.no_qty,
                    "yes_bid":             c.yes_bid,
                    "no_bid":              c.no_bid,
                    "yes_placed":          c.yes_placed,
                    "no_placed":           c.no_placed,
                    "yes_spread_from_mid": c.yes_spread_from_mid,
                    "no_spread_from_mid":  c.no_spread_from_mid,
                    "score_factor":        c.score_factor,
                    "ended_by":            c.ended_by,
                })
            })
            .collect();

        let record = serde_json::json!({
            "session_end_utc":      session_end,
            "strategy":             self.id,
            "runtime_secs":         runtime_secs,
            "spread_threshold_v":   self.params.spread_threshold_v,
            "min_shares_per_side":  self.params.min_shares_per_side,
            "cycles":               cycle_json,
            "totals": {
                "cycle_count":         self.cycles.len(),
                "fill_count":          self.fill_count,
                "on_book_secs":        (total_on_book_secs * 10.0).round() / 10.0,
                "pct_on_book":         pct_on_book,
                "est_yes_score_raw":   est_yes_score_raw,
                "est_no_score_raw":    est_no_score_raw,
                "final_yes_tokens":    self.yes_tokens,
                "final_no_tokens":     self.no_tokens,
                "unrealized_pnl":      unrealized_pnl.round_dp(4),
            },
            // Fill this in from the dashboard after each week:
            // "dashboard_points_reported": null,
            // Then compute: points_per_score_second = dashboard_points / (est_yes_score_raw + est_no_score_raw)
        });

        let path = "logs/data/points-sessions.jsonl";
        let line = format!("{}\n", record);
        // Best-effort append — don't crash the shutdown if the write fails.
        if let Err(e) = std::fs::create_dir_all("logs/data").and_then(|_| {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| f.write_all(line.as_bytes()))
        }) {
            tracing::warn!(error = %e, path, "Failed to write points session log");
        } else {
            tracing::info!(path, "Points session record written");
        }

        vec![Action::CancelAll {
            instrument: self.yes_instrument.clone(),
        }]
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Convert Unix epoch seconds to (year, month, day, hour, min, sec).
/// Minimal implementation — no chrono dependency.
fn epoch_to_ymd_hms(epoch: u64) -> (u64, u64, u64, u64, u64, u64) {
    let sec = epoch % 60;
    let min = (epoch / 60) % 60;
    let hr = (epoch / 3600) % 24;
    let days = epoch / 86400;
    // Gregorian calendar approximation from days since 1970-01-01.
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hr, min, sec)
}
