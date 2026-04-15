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

/// Staleness threshold for the Polymarket fair-value signal.
/// If no Polymarket `BookUpdate` arrives within this window, the strategy falls
/// back to the predict.fun mid as fair value and logs a warning.
const POLY_FV_STALE_SECS: u64 = 30;

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
    spread_from_mid: Decimal,
    score_factor: Decimal,
    ended_by: &'static str, // "fill" | "drift" | "shutdown"
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

    /// Running token balance for YES outcome (filled tokens we hold).
    yes_tokens: Decimal,
    /// Running token balance for NO outcome (filled tokens we hold).
    no_tokens: Decimal,

    /// Approximate cost basis for P&L reporting. In USDT (price × qty).
    session_cost: Decimal,
    session_proceeds: Decimal,
    fill_count: u64,

    /// Most recent YES mid (for fill P&L reporting).
    latest_mid: Option<Decimal>,

    // ── Per-cycle points tracking ─────────────────────────────────────────────
    /// Wall-clock time when the engine started (for session duration).
    session_started_at: Option<Instant>,
    /// When the current cycle's orders were placed.
    cycle_placed_at: Option<Instant>,
    /// YES qty placed in the current cycle (shares, not USDT).
    cycle_yes_qty: Decimal,
    /// NO qty placed in the current cycle (shares, not USDT).
    cycle_no_qty: Decimal,
    /// Spread from mid used in the current cycle.
    cycle_spread_from_mid: Decimal,
    /// Prices for the current cycle.
    cycle_yes_bid: Decimal,
    cycle_no_bid: Decimal,
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
            yes_tokens: Decimal::ZERO,
            no_tokens: Decimal::ZERO,
            session_cost: Decimal::ZERO,
            session_proceeds: Decimal::ZERO,
            fill_count: 0,
            latest_mid: None,
            session_started_at: None,
            cycle_placed_at: None,
            cycle_yes_qty: Decimal::ZERO,
            cycle_no_qty: Decimal::ZERO,
            cycle_spread_from_mid: Decimal::ZERO,
            cycle_yes_bid: Decimal::ZERO,
            cycle_no_bid: Decimal::ZERO,
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
        let sf = self.score_factor(self.cycle_spread_from_mid);

        tracing::info!(
            target: "quoter",
            strategy = %self.id,
            ended_by,
            on_book_secs = format!("{:.1}", on_book_secs),
            yes_qty      = %self.cycle_yes_qty,
            no_qty       = %self.cycle_no_qty,
            yes_bid      = %self.cycle_yes_bid,
            no_bid       = %self.cycle_no_bid,
            spread_from_mid = %self.cycle_spread_from_mid.round_dp(4),
            score_factor = %sf.round_dp(4),
            est_yes_score = %( sf * self.cycle_yes_qty * Decimal::try_from(on_book_secs).unwrap_or(Decimal::ZERO) ).round_dp(1),
            est_no_score  = %( sf * self.cycle_no_qty  * Decimal::try_from(on_book_secs).unwrap_or(Decimal::ZERO) ).round_dp(1),
            "CYCLE_END",
        );

        self.cycles.push(CycleRecord {
            on_book_secs,
            yes_qty: self.cycle_yes_qty,
            no_qty: self.cycle_no_qty,
            yes_bid: self.cycle_yes_bid,
            no_bid: self.cycle_no_bid,
            spread_from_mid: self.cycle_spread_from_mid,
            score_factor: sf,
            ended_by,
        });
    }

    /// Start tracking a new cycle.
    fn open_cycle(
        &mut self,
        yes_qty: Decimal,
        no_qty: Decimal,
        yes_bid: Decimal,
        no_bid: Decimal,
        mid: Decimal,
    ) {
        self.cycle_placed_at = Some(Instant::now());
        self.cycle_yes_qty = yes_qty;
        self.cycle_no_qty = no_qty;
        self.cycle_yes_bid = yes_bid;
        self.cycle_no_bid = no_bid;
        // spread from mid = |yes_bid - mid|; always positive since yes_bid < mid.
        self.cycle_spread_from_mid = (mid - yes_bid).abs();
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

    /// Return the Polymarket mid if it is fresh (< POLY_FV_STALE_SECS old),
    /// otherwise fall back to the predict.fun mid.
    ///
    /// When Polymarket FV is stale we log a warning so the operator knows the
    /// fallback is active. The warn fires at most once per stale period because
    /// the next fresh update resets `polymarket_mid_ts`.
    fn effective_fv(&self, predict_mid: Decimal) -> Decimal {
        match (self.polymarket_mid, self.polymarket_mid_ts) {
            (Some(pm), Some(ts)) if ts.elapsed().as_secs() < POLY_FV_STALE_SECS => pm,
            (Some(_), _) => {
                // Had a Polymarket signal but it's now stale.
                tracing::warn!(
                    target: "quoter",
                    strategy = %self.id,
                    stale_threshold_secs = POLY_FV_STALE_SECS,
                    "Polymarket FV stale — falling back to predict.fun mid",
                );
                predict_mid
            }
            _ => predict_mid, // No Polymarket signal configured or not yet received.
        }
    }

    /// True when the fair value has drifted beyond `drift_cents` since last requote.
    fn fv_drifted(&self, current_fv: Decimal) -> bool {
        match self.last_quoted_fv {
            None => true,
            Some(last) => (current_fv - last).abs() > self.params.drift_cents,
        }
    }

    /// Cancel all resting orders (connector clears both YES and NO).
    /// We use the YES instrument — `cancel_all` in `PredictFunClient`
    /// cancels every tracked order across both outcomes.
    fn cancel_all(&self) -> Action {
        Action::CancelAll {
            instrument: self.yes_instrument.clone(),
        }
    }

    /// Build the two BUY orders (YES + NO) for one quoting cycle.
    fn build_place_actions(&self, prices: pricing::QuotePrices) -> Vec<Action> {
        let mut actions = Vec::with_capacity(2);

        // YES BUY: skip if we're already at max position on this outcome.
        if self.yes_tokens < self.params.max_position_tokens {
            let yes_qty = (self.params.order_size_usdt / prices.yes_bid.inner())
                .round_dp(4)
                .max(Decimal::ONE / Decimal::from(10_000)); // never zero

            actions.push(Action::PlaceOrder(OrderRequest {
                instrument: self.yes_instrument.clone(),
                side: OrderSide::Buy,
                price: prices.yes_bid,
                quantity: Quantity::new(yes_qty),
                tif: TimeInForce::PostOnly,
                client_order_id: Some("yes".into()),
            }));
        } else {
            tracing::info!(
                target: "quoter",
                strategy = %self.id,
                yes_tokens = %self.yes_tokens,
                max = %self.params.max_position_tokens,
                "YES position limit reached — skipping YES quote",
            );
        }

        // NO BUY: skip if at max position on this outcome.
        if self.no_tokens < self.params.max_position_tokens {
            let no_qty = (self.params.order_size_usdt / prices.no_bid.inner())
                .round_dp(4)
                .max(Decimal::ONE / Decimal::from(10_000));

            actions.push(Action::PlaceOrder(OrderRequest {
                instrument: self.no_instrument.clone(),
                side: OrderSide::Buy,
                price: prices.no_bid,
                quantity: Quantity::new(no_qty),
                tif: TimeInForce::PostOnly,
                client_order_id: Some("no".into()),
            }));
        } else {
            tracing::info!(
                target: "quoter",
                strategy = %self.id,
                no_tokens = %self.no_tokens,
                max = %self.params.max_position_tokens,
                "NO position limit reached — skipping NO quote",
            );
        }

        actions
    }

    /// Full requote cycle: cancel everything → place YES + NO.
    ///
    /// `predict_mid`: current predict.fun YES mid (used for score/cycle tracking).
    /// `fv`:          effective fair value used for pricing (Polymarket or predict_mid fallback).
    fn requote(
        &mut self,
        predict_mid: Decimal,
        fv: Decimal,
        prices: pricing::QuotePrices,
        ended_by: &'static str,
    ) -> Vec<Action> {
        // Close the previous cycle before starting a new one.
        self.close_cycle(ended_by);

        let mut actions = Vec::with_capacity(3);
        actions.push(self.cancel_all());
        let place = self.build_place_actions(prices);

        let yes_qty = if self.yes_tokens < self.params.max_position_tokens {
            (self.params.order_size_usdt / prices.yes_bid.inner()).round_dp(4)
        } else {
            Decimal::ZERO
        };
        let no_qty = if self.no_tokens < self.params.max_position_tokens {
            (self.params.order_size_usdt / prices.no_bid.inner()).round_dp(4)
        } else {
            Decimal::ZERO
        };

        let sf = self.score_factor((predict_mid - prices.yes_bid.inner()).abs());
        let qualifies_yes = yes_qty >= self.params.min_shares_per_side;
        let qualifies_no = no_qty >= self.params.min_shares_per_side;

        tracing::info!(
            target: "quoter",
            strategy   = %self.id,
            predict_mid = %predict_mid,
            poly_fv    = ?self.polymarket_mid,
            fv_used    = %fv,
            yes_bid    = %prices.yes_bid,
            no_bid     = %prices.no_bid,
            yes_qty    = %yes_qty,
            no_qty     = %no_qty,
            n_orders   = place.len(),
            yes_pos    = %self.yes_tokens,
            no_pos     = %self.no_tokens,
            score_factor       = %sf.round_dp(4),
            qualifies_yes      = qualifies_yes,
            qualifies_no       = qualifies_no,
            min_shares         = %self.params.min_shares_per_side,
            spread_threshold_v = %self.params.spread_threshold_v,
            "REQUOTE",
        );

        // Open a new cycle only if at least one side placed qualifying orders.
        if !place.is_empty() {
            self.open_cycle(
                yes_qty,
                no_qty,
                prices.yes_bid.inner(),
                prices.no_bid.inner(),
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
                        tracing::debug!(
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

                // Effective fair value: Polymarket mid if fresh, else predict.fun mid.
                let fv = self.effective_fv(mid);

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
                    }
                    State::Cooldown(_) => return vec![],
                    _ => {}
                }

                // Skip if fair value has not drifted enough to warrant a requote.
                if matches!(self.state, State::Quoting) && !self.fv_drifted(fv) {
                    return vec![];
                }

                // Enforce minimum hold time — don't pull quotes just because FV
                // twitched within drift range and back. Fill-triggered cancels bypass this.
                if matches!(self.state, State::Quoting) {
                    if let Some(placed_at) = self.last_place_time {
                        let hold = Duration::from_secs(self.params.min_quote_hold_secs);
                        if Instant::now() < placed_at + hold {
                            return vec![];
                        }
                    }
                }

                // Calculate quote prices using the effective FV + predict.fun BBO crossing guards.
                let prices = match pricing::calculate(
                    book,
                    fv,
                    self.params.spread_cents,
                    self.params.join_cents,
                    self.decimal_precision,
                ) {
                    Some(p) => p,
                    None => {
                        tracing::debug!(
                            target: "quoter",
                            strategy = %self.id,
                            fv = %fv,
                            "pricing returned None (thin book) — skipping cycle",
                        );
                        return vec![];
                    }
                };

                self.requote(mid, fv, prices, "drift")
            }

            Event::Fill {
                instrument, fill, ..
            } => {
                // Track positions per outcome.
                if instrument == &self.yes_instrument {
                    self.yes_tokens += fill.quantity.inner();
                } else if instrument == &self.no_instrument {
                    self.no_tokens += fill.quantity.inner();
                }

                self.fill_count += 1;
                self.session_cost += fill.price.inner() * fill.quantity.inner();

                let mid = self.latest_mid.unwrap_or(fill.price.inner());
                let open_value = self.yes_tokens * mid + self.no_tokens * (Decimal::ONE - mid);
                let unrealized_pnl = open_value - self.session_cost + self.session_proceeds;

                tracing::info!(
                    target: "quoter",
                    strategy = %self.id,
                    instrument = %instrument,
                    side = ?fill.side,
                    price = %fill.price.inner(),
                    qty = %fill.quantity.inner(),
                    yes_tokens = %self.yes_tokens,
                    no_tokens  = %self.no_tokens,
                    session_cost = %self.session_cost.round_dp(4),
                    unrealized_pnl = %unrealized_pnl.round_dp(4),
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

        // Set price precision from the exchange connector (via StrategyState).
        // Default to 3 (0.001 ticks) if not provided — safer for unknown markets.
        if let Some(prec) = state.decimal_precision {
            self.decimal_precision = prec;
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
            spread_cents = %self.params.spread_cents,
            join_cents = ?self.params.join_cents,
            decimal_precision = self.decimal_precision,
            order_size_usdt    = %self.params.order_size_usdt,
            drift_cents        = %self.params.drift_cents,
            fill_pause_secs    = self.params.fill_pause_secs,
            max_position_tokens = %self.params.max_position_tokens,
            spread_threshold_v  = %self.params.spread_threshold_v,
            min_shares_per_side = %self.params.min_shares_per_side,
            fv_stale_secs = POLY_FV_STALE_SECS,
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
        let open_value = self.yes_tokens * mid + self.no_tokens * (Decimal::ONE - mid);
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
            est_yes_score_raw += c.score_factor * c.yes_qty * t;
            est_no_score_raw += c.score_factor * c.no_qty * t;
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
            fill_count        = self.fill_count,
            session_cost      = %self.session_cost.round_dp(4),
            unrealized_pnl    = %unrealized_pnl.round_dp(4),
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
                    "on_book_secs":    (c.on_book_secs * 10.0).round() / 10.0,
                    "yes_qty":         c.yes_qty,
                    "no_qty":          c.no_qty,
                    "yes_bid":         c.yes_bid,
                    "no_bid":          c.no_bid,
                    "spread_from_mid": c.spread_from_mid,
                    "score_factor":    c.score_factor,
                    "ended_by":        c.ended_by,
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
