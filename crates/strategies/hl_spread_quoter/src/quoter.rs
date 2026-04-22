//! HlSpreadQuoter — two-level symmetric spread market maker with inventory skew.
//!
//! Always: CancelAll → BatchPlace. Never tracks individual order state.
//!
//! State machine:
//!   Empty    → first mid arrives or cooldown expires → BatchPlace
//!   Quoting  → mid drifts > drift_bps → CancelAll + DriftPause
//!   Quoting  → fill received → CancelAll + FillPause
//!   Paused   → cooldown expires → BatchPlace
//!
//! Strategy returns [CancelAll, PlaceOrder×N] as a single Vec<Action>.
//! The router guarantees: CancelAll completes before PlaceOrders are submitted.
//! HL executes the PlaceOrders as a single BatchOrder API call.
//!
//! Inventory skew:
//!   When net_position ≠ 0, quotes are placed around a shifted reservation mid
//!   rather than the raw market mid. When long, the reservation shifts down —
//!   making the ask cheaper and the bid more expensive — steering fills toward
//!   mean-reverting the position. Controlled by `skew_factor_bps_per_unit`.

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use trading_core::{
    traits::{strategy::StrategyState, Strategy},
    types::order::{OrderRequest, OrderSide, TimeInForce},
    Action, Event, InstrumentId, Price, Quantity,
};

use crate::params::QuoterParams;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum State {
    /// No orders on book (startup or post-cooldown).
    Empty,
    /// Orders are on the book. last_quoted_mid is set.
    Quoting,
    /// Pulled quotes. Waiting until `Instant` before re-quoting.
    Cooldown(Instant),
}

impl State {
    fn is_quoting(&self) -> bool {
        matches!(self, State::Quoting)
    }
}

// ── Strategy ──────────────────────────────────────────────────────────────────

pub struct HlSpreadQuoter {
    id: String,
    instrument: InstrumentId,
    params: QuoterParams,
    state: State,
    net_position: Decimal,
    latest_mid: Option<Decimal>,
    /// Prices our orders are currently resting at, per level: [(bid, ask), ...]
    /// Set when we place orders, cleared on cancel/fill/pause.
    /// Drift is measured against these prices, not against mid-to-mid.
    resting_prices: Vec<(Decimal, Decimal)>,
    /// Running cash-flow P&L for this session.
    /// Formula: +price*qty - fee on sells; -price*qty - fee on buys.
    /// Converges to realized P&L as position returns to flat.
    session_pnl: Decimal,
    /// Total fills this session, for performance reporting.
    fill_count: u64,
}

impl HlSpreadQuoter {
    pub fn new(id: String, instrument: InstrumentId, params: QuoterParams) -> Self {
        let n = params.level_bps.len();
        Self {
            id,
            instrument,
            params,
            state: State::Empty,
            net_position: Decimal::ZERO,
            latest_mid: None,
            resting_prices: vec![(Decimal::ZERO, Decimal::ZERO); n],
            session_pnl: Decimal::ZERO,
            fill_count: 0,
        }
    }

    /// Reservation mid — shifts the quoting reference based on inventory.
    ///
    /// When long, shifts down (makes asks cheaper, bids more expensive).
    /// When short, shifts up (makes bids cheaper, asks more expensive).
    /// This steers fills toward flattening the position without widening spreads.
    ///
    /// With skew_factor_bps_per_unit = 0, returns raw mid unchanged.
    fn reservation_mid(&self, mid: Decimal) -> Decimal {
        if self.params.skew_factor_bps_per_unit == Decimal::ZERO {
            return mid;
        }
        let shift_bps = self.params.skew_factor_bps_per_unit * self.net_position;
        mid * (Decimal::ONE - shift_bps / Decimal::from(10_000))
    }

    fn bid_price(&self, ref_mid: Decimal, level: usize) -> Decimal {
        ref_mid * (Decimal::ONE - self.params.level_ratio(level))
    }

    fn ask_price(&self, ref_mid: Decimal, level: usize) -> Decimal {
        ref_mid * (Decimal::ONE + self.params.level_ratio(level))
    }

    /// Drift = max across all levels of how far new target prices (based on raw mid,
    /// not reservation) are from resting prices. Using raw mid here ensures we respond
    /// to actual market movement, independent of inventory-driven reservation shifts.
    fn max_drift_bps(&self, mid: Decimal) -> Decimal {
        let mut max_drift = Decimal::ZERO;
        for (level, &(resting_bid, resting_ask)) in self.resting_prices.iter().enumerate() {
            if resting_bid.is_zero() && resting_ask.is_zero() {
                continue;
            }
            // Compare against unskewed targets — measures market movement only.
            let target_bid = self.bid_price(mid, level);
            let target_ask = self.ask_price(mid, level);

            if !resting_bid.is_zero() {
                let d = ((target_bid - resting_bid).abs() / resting_bid) * Decimal::from(10_000);
                if d > max_drift {
                    max_drift = d;
                }
            }
            if !resting_ask.is_zero() {
                let d = ((target_ask - resting_ask).abs() / resting_ask) * Decimal::from(10_000);
                if d > max_drift {
                    max_drift = d;
                }
            }
        }
        max_drift
    }

    fn clear_resting(&mut self) {
        for slot in &mut self.resting_prices {
            *slot = (Decimal::ZERO, Decimal::ZERO);
        }
    }

    fn build_place_actions(&self, reservation: Decimal) -> Vec<Action> {
        let mut orders = vec![];

        for level in 0..self.params.level_bps.len() {
            let bid = self.bid_price(reservation, level);
            let ask = self.ask_price(reservation, level);
            let size = self.params.order_size;

            if self.net_position < self.params.max_position {
                orders.push(Action::PlaceOrder(OrderRequest {
                    instrument: self.instrument.clone(),
                    side: OrderSide::Buy,
                    price: Price::new(bid),
                    quantity: Quantity::new(size),
                    tif: TimeInForce::PostOnly,
                    client_order_id: Some(format!("b{level}")),
                }));
            }

            if self.net_position > -self.params.max_position {
                orders.push(Action::PlaceOrder(OrderRequest {
                    instrument: self.instrument.clone(),
                    side: OrderSide::Sell,
                    price: Price::new(ask),
                    quantity: Quantity::new(size),
                    tif: TimeInForce::PostOnly,
                    client_order_id: Some(format!("a{level}")),
                }));
            }
        }
        orders
    }

    fn requote(&mut self, mid: Decimal) -> Vec<Action> {
        let reservation = self.reservation_mid(mid);

        let mut actions = Vec::with_capacity(5);
        actions.push(Action::CancelAll {
            instrument: self.instrument.clone(),
        });
        let place_actions = self.build_place_actions(reservation);

        // Store the reservation-adjusted prices so drift (measured vs raw mid) is correct.
        let new_resting: Vec<(Decimal, Decimal)> = (0..self.params.level_bps.len())
            .map(|l| {
                (
                    self.bid_price(reservation, l),
                    self.ask_price(reservation, l),
                )
            })
            .collect();
        self.resting_prices = new_resting;

        let skew_bps = if self.params.skew_factor_bps_per_unit != Decimal::ZERO {
            (self.params.skew_factor_bps_per_unit * self.net_position).round_dp(2)
        } else {
            Decimal::ZERO
        };

        tracing::info!(
            target: "quoter",
            strategy = %self.id,
            mid = %mid,
            reservation = %reservation,
            skew_bps = %skew_bps,
            n_orders = place_actions.len(),
            net_pos = %self.net_position,
            "REQUOTE cancel_all + batch_place"
        );

        for action in &place_actions {
            if let Action::PlaceOrder(req) = action {
                tracing::info!(
                    target: "quoter",
                    strategy = %self.id,
                    side = ?req.side,
                    price = %req.price.inner(),
                    qty = %req.quantity.inner(),
                    "ORDER_SUBMIT"
                );
            }
        }

        actions.extend(place_actions);
        self.state = State::Quoting;
        actions
    }

    fn pull_quotes(&mut self, reason: &str, pause_secs: u64, mid: Decimal) -> Vec<Action> {
        // If already in cooldown, quotes are already off the book. Skip the duplicate cancel
        // to avoid spurious zero-latency ROUNDTRIP logs from back-to-back fill notifications.
        if matches!(self.state, State::Cooldown(_)) {
            tracing::debug!(
                target: "quoter",
                strategy = %self.id,
                reason,
                "PULL_QUOTES skipped: already in cooldown"
            );
            return vec![];
        }

        tracing::warn!(
            target: "quoter",
            strategy = %self.id,
            reason,
            mid = %mid,
            pause_secs,
            "PULL_QUOTES"
        );
        self.state = State::Cooldown(Instant::now() + Duration::from_secs(pause_secs));
        self.clear_resting();
        vec![Action::CancelAll {
            instrument: self.instrument.clone(),
        }]
    }
}

#[async_trait::async_trait]
impl Strategy for HlSpreadQuoter {
    fn id(&self) -> &str {
        &self.id
    }

    fn subscriptions(&self) -> Vec<InstrumentId> {
        vec![self.instrument.clone()]
    }

    async fn on_event(&mut self, event: &Event) -> Vec<Action> {
        match event {
            Event::BookUpdate { book, .. } => {
                let mid = match book.mid_price() {
                    Some(m) => m.inner(),
                    None => return vec![],
                };
                self.latest_mid = Some(mid);

                let drift_now = self.max_drift_bps(mid);
                tracing::debug!(
                    target: "quoter",
                    strategy = %self.id,
                    mid = %mid,
                    state = ?self.state,
                    drift_bps = %drift_now.round_dp(2),
                    net_pos = %self.net_position,
                    "MID"
                );

                // Advance cooldown state
                match self.state {
                    State::Cooldown(t) if Instant::now() >= t => {
                        tracing::info!(
                            target: "quoter",
                            strategy = %self.id,
                            mid = %mid,
                            "COOLDOWN_EXPIRED"
                        );
                        self.state = State::Empty;
                    }
                    State::Cooldown(_) => return vec![],
                    _ => {}
                }

                if self.state.is_quoting() {
                    if drift_now > Decimal::from(self.params.drift_bps) {
                        tracing::warn!(
                            target: "quoter",
                            strategy = %self.id,
                            mid = %mid,
                            resting_bid_l0 = %self.resting_prices.first().map(|(b,_)| *b).unwrap_or_default(),
                            target_bid_l0 = %self.bid_price(mid, 0),
                            drift_bps = %drift_now.round_dp(2),
                            threshold = self.params.drift_bps,
                            "DRIFT"
                        );
                        return self.pull_quotes("drift", self.params.drift_pause_secs, mid);
                    }
                    return vec![];
                }

                // Empty → place quotes
                self.requote(mid)
            }

            Event::Fill { fill, .. } => {
                match fill.side {
                    OrderSide::Buy => self.net_position += fill.quantity.inner(),
                    OrderSide::Sell => self.net_position -= fill.quantity.inner(),
                }
                self.fill_count += 1;

                // Cash-flow P&L: positive cash in on sells, negative on buys, always minus fee.
                let cash_flow = match fill.side {
                    OrderSide::Sell => {
                        fill.price.inner() * fill.quantity.inner() - fill.fee.inner()
                    }
                    OrderSide::Buy => {
                        -(fill.price.inner() * fill.quantity.inner()) - fill.fee.inner()
                    }
                };
                self.session_pnl += cash_flow;

                // Mark-to-market P&L: session cash flow + open position valued at current mid.
                let mid = self.latest_mid.unwrap_or(fill.price.inner());
                let mark_pnl = self.session_pnl + self.net_position * mid;

                tracing::info!(
                    target: "quoter",
                    strategy = %self.id,
                    side = ?fill.side,
                    price = %fill.price.inner(),
                    qty = %fill.quantity.inner(),
                    fee = %fill.fee.inner(),
                    net_pos = %self.net_position,
                    session_pnl = %self.session_pnl.round_dp(4),
                    mark_pnl = %mark_pnl.round_dp(4),
                    fill_count = self.fill_count,
                    pause_secs = self.params.fill_pause_secs,
                    "FILL"
                );

                let mut actions = self.pull_quotes("fill", self.params.fill_pause_secs, mid);
                actions.push(Action::LogDecision {
                    strategy_id: self.id.clone(),
                    decision: "fill".into(),
                    context: serde_json::json!({
                        "side":         format!("{:?}", fill.side),
                        "price":        fill.price.inner().to_string(),
                        "qty":          fill.quantity.inner().to_string(),
                        "fee":          fill.fee.inner().to_string(),
                        "net_position": self.net_position.to_string(),
                        "session_pnl":  self.session_pnl.round_dp(4).to_string(),
                        "mark_pnl":     mark_pnl.round_dp(4).to_string(),
                        "fill_count":   self.fill_count,
                    }),
                });
                actions
            }

            Event::OrderUpdate { update, .. } => {
                use trading_core::types::order::OrderStatus;
                if matches!(update.status, OrderStatus::Rejected) {
                    tracing::error!(
                        target: "quoter",
                        strategy = %self.id,
                        oid = %update.order_id,
                        "ORDER_REJECTED"
                    );
                }
                vec![]
            }

            // Every order in a place_batch failed — orders never landed on the exchange.
            // Without this handler, the strategy stays in Quoting state with resting_prices
            // set, believing it has active orders while having none ("ghost quoting").
            //
            // On HL rate-limit errors ("Too many cumulative requests"), back off 5 minutes
            // before retrying so we don't hammer a rejected endpoint every ~13s for hours.
            // On other transient failures, use a short backoff before retrying.
            Event::PlaceFailed { reason, .. } => {
                // Already in cooldown from an earlier failure — don't re-enter.
                if matches!(self.state, State::Cooldown(_)) {
                    return vec![];
                }
                let is_rate_limited = reason.contains("Too many cumulative");
                let pause_secs = if is_rate_limited { 300 } else { 10 };
                self.state = State::Cooldown(Instant::now() + Duration::from_secs(pause_secs));
                self.clear_resting();
                tracing::warn!(
                    target: "quoter",
                    strategy = %self.id,
                    reason = %reason,
                    pause_secs,
                    "PLACE_FAILED — orders did not land, entering backoff"
                );
                // No CancelAll needed — orders never reached the exchange.
                vec![]
            }

            _ => vec![],
        }
    }

    async fn initialize(&mut self, state: &StrategyState) -> Vec<Action> {
        for pos in &state.positions {
            if pos.instrument == self.instrument {
                self.net_position = pos.size.inner();
                tracing::info!(
                    target: "quoter",
                    strategy = %self.id,
                    net_pos = %self.net_position,
                    "INIT existing position"
                );
            }
        }
        tracing::info!(
            target: "quoter",
            strategy = %self.id,
            instrument = %self.instrument,
            levels = ?self.params.level_bps,
            order_size = %self.params.order_size,
            drift_bps = self.params.drift_bps,
            skew_factor_bps_per_unit = %self.params.skew_factor_bps_per_unit,
            "INIT"
        );
        vec![Action::CancelAll {
            instrument: self.instrument.clone(),
        }]
    }

    async fn shutdown(&mut self) -> Vec<Action> {
        tracing::info!(
            target: "quoter",
            strategy = %self.id,
            session_pnl = %self.session_pnl.round_dp(4),
            fill_count = self.fill_count,
            net_pos = %self.net_position,
            "SHUTDOWN"
        );
        vec![Action::CancelAll {
            instrument: self.instrument.clone(),
        }]
    }
}
