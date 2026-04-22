//! `PredictHedgeStrategy` — hedges predict.fun fill exposure on Polymarket.
//!
//! ## Hedge logic
//!
//! predict.fun is BUY-only (we always buy outcome tokens). When filled:
//!   - Bought YES → hedge by buying NO on Polymarket  (YES + NO = $1)
//!   - Bought NO  → hedge by buying YES on Polymarket
//!
//! Both sides of the hedge are `Buy` orders on Polymarket.
//!
//! ## Pricing (Phase 2 — taker)
//!
//! Orders are placed GTC at `best_ask` for immediate fill.
//! Slippage guard: reject if `best_ask > (1 - avg_predict_fill_price) + max_slippage_cents`.
//!
//! ## Position accounting
//!
//! **Optimistic**: the unhedged qty is consumed immediately when a hedge order is
//! emitted. If the order fails (`Event::PlaceFailed`), the qty is restored.
//! This works correctly because GTC orders at best_ask fill immediately on Polymarket.
//!
//! ## Market mapping
//!
//! Each `MarketMapping` provides the link between predict.fun and Polymarket:
//!   predict_yes_fill → buy poly_no_instrument
//!   predict_no_fill  → buy poly_yes_instrument

use std::collections::HashMap;
use std::time::Instant;

use async_trait::async_trait;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use trading_core::{
    traits::{strategy::StrategyState, Strategy},
    types::{
        decimal::{Price, Quantity},
        instrument::{Exchange, InstrumentId},
        order::{OrderRequest, OrderSide, TimeInForce},
        position::Fill,
    },
    Action, Event,
};

use crate::params::HedgeParams;

// ── Per-poly-instrument unhedged state ───────────────────────────────────────

#[derive(Debug, Default)]
struct UnhedgedState {
    /// Accumulated unhedged qty (outcome tokens).
    qty: Decimal,
    /// Running USDC notional of unhedged predict fills (for slippage reference).
    /// notional / qty = weighted average predict fill price.
    notional: Decimal,
    /// Wall-clock time of the oldest unhedged predict fill (for urgency detection).
    first_unhedged_ts: Option<Instant>,
}

impl UnhedgedState {
    fn add_fill(&mut self, fill_qty: Decimal, fill_price: Decimal) {
        self.qty += fill_qty;
        self.notional += fill_qty * fill_price;
        if self.first_unhedged_ts.is_none() {
            self.first_unhedged_ts = Some(Instant::now());
        }
    }

    /// Mark qty as hedged (optimistic — consume before order confirmation).
    fn consume_all(&mut self) {
        self.qty = Decimal::ZERO;
        self.notional = Decimal::ZERO;
        self.first_unhedged_ts = None;
    }

    /// Restore qty (called on PlaceFailed — undo optimistic consume).
    fn restore(&mut self, qty: Decimal, notional_per_token: Decimal) {
        self.qty += qty;
        self.notional += qty * notional_per_token;
        if self.first_unhedged_ts.is_none() {
            self.first_unhedged_ts = Some(Instant::now());
        }
    }

    fn is_urgent(&self, params: &HedgeParams) -> bool {
        self.first_unhedged_ts
            .map(|ts| ts.elapsed().as_secs() >= params.max_unhedged_duration_secs)
            .unwrap_or(false)
    }

    fn avg_predict_fill_price(&self) -> Option<Decimal> {
        if self.qty.is_zero() {
            None
        } else {
            Some(self.notional / self.qty)
        }
    }
}

// ── Market mapping ────────────────────────────────────────────────────────────

/// Wiring for one predict ↔ Polymarket market pair.
pub struct MarketMapping {
    /// predict.fun YES instrument (e.g. `PredictFun.Binary.143028-Yes`)
    pub predict_yes: InstrumentId,
    /// predict.fun NO instrument (e.g. `PredictFun.Binary.143028-No`)
    pub predict_no: InstrumentId,
    /// Polymarket YES token (`Polymarket.Binary.{yes_token_id}`)
    pub poly_yes: InstrumentId,
    /// Polymarket NO token (`Polymarket.Binary.{no_token_id}`)
    pub poly_no: InstrumentId,
}

// ── Strategy ──────────────────────────────────────────────────────────────────

/// Hedges predict.fun fill exposure by placing opposite-side buy orders on Polymarket.
pub struct PredictHedgeStrategy {
    id: String,
    params: HedgeParams,

    /// predict instrument → target Polymarket instrument (hedge token).
    /// predict_yes → poly_no, predict_no → poly_yes.
    hedge_map: HashMap<InstrumentId, InstrumentId>,

    /// Polymarket instrument → cached (best_bid, best_ask) from last BookUpdate.
    poly_bbo: HashMap<InstrumentId, (Price, Price)>,

    /// Polymarket instrument → unhedged position state.
    unhedged: HashMap<InstrumentId, UnhedgedState>,

    /// Polymarket instrument → pending hedged qty (optimistically consumed but not yet confirmed).
    /// Used to restore on PlaceFailed.
    pending_hedge: HashMap<InstrumentId, (Decimal, Decimal)>, // (qty, avg_price)

    /// All instruments we need market data for.
    all_subscriptions: Vec<InstrumentId>,
}

impl PredictHedgeStrategy {
    pub fn new(id: impl Into<String>, mappings: Vec<MarketMapping>, params: HedgeParams) -> Self {
        let mut hedge_map = HashMap::new();
        let mut all_subscriptions = Vec::new();
        let mut unhedged = HashMap::new();

        for m in mappings {
            hedge_map.insert(m.predict_yes.clone(), m.poly_no.clone());
            hedge_map.insert(m.predict_no.clone(), m.poly_yes.clone());

            all_subscriptions.push(m.predict_yes);
            all_subscriptions.push(m.predict_no);
            all_subscriptions.push(m.poly_yes.clone());
            all_subscriptions.push(m.poly_no.clone());

            unhedged.insert(m.poly_yes, UnhedgedState::default());
            unhedged.insert(m.poly_no, UnhedgedState::default());
        }

        Self {
            id: id.into(),
            params,
            hedge_map,
            poly_bbo: HashMap::new(),
            unhedged,
            pending_hedge: HashMap::new(),
            all_subscriptions,
        }
    }

    // ── Event handlers ────────────────────────────────────────────────────────

    fn on_predict_fill(&mut self, instrument: &InstrumentId, fill: &Fill) -> Vec<Action> {
        let Some(poly_inst) = self.hedge_map.get(instrument).cloned() else {
            return vec![];
        };

        let qty = fill.quantity.inner();
        let price = fill.price.inner();

        tracing::info!(
            target: "quoter",
            predict_inst  = %instrument,
            poly_inst     = %poly_inst,
            fill_qty      = %qty,
            fill_price    = %price,
            "HEDGE_TRIGGER",
        );

        let state = self.unhedged.entry(poly_inst.clone()).or_default();
        state.add_fill(qty, price);

        self.try_hedge(poly_inst)
    }

    /// Evaluate and optionally place a hedge order for `poly_inst`.
    fn try_hedge(&mut self, poly_inst: InstrumentId) -> Vec<Action> {
        if !self.params.enabled {
            return vec![];
        }

        let state = match self.unhedged.get(&poly_inst) {
            Some(s) => s,
            None => return vec![],
        };

        let poly_ask = match self.poly_bbo.get(&poly_inst) {
            Some((_, ask)) => ask.inner(),
            None => {
                tracing::warn!(
                    poly_inst = %poly_inst,
                    "HEDGE_SKIP no poly book",
                );
                return vec![];
            }
        };

        let hedge_notional = state.qty * poly_ask;
        let urgent = state.is_urgent(&self.params);

        if hedge_notional < self.params.hedge_min_notional && !urgent {
            tracing::debug!(
                target: "quoter",
                poly_inst      = %poly_inst,
                hedge_notional = %hedge_notional,
                min_notional   = %self.params.hedge_min_notional,
                unhedged_qty   = %state.qty,
                "HEDGE_BATCH below min notional, accumulating",
            );
            return vec![];
        }

        // Slippage guard: check we are not crossing more than max_slippage_cents
        // above the Polymarket mid for the target token.
        //
        // We compare poly_ask against poly_mid (NOT against 1 - predict_fill_price).
        // Using the predict fill price as reference is an arbitrage check, which fails
        // whenever venues diverge significantly. We are hedging for risk reduction, not
        // arbitrage — we should pay Polymarket's market price regardless of what we
        // filled on predict.fun. max_slippage_cents just limits how much we cross the
        // Polymarket spread (e.g. 0.03 = allow up to 3 ticks above mid on Polymarket).
        let poly_bid = match self.poly_bbo.get(&poly_inst) {
            Some((bid, _)) => bid.inner(),
            None => {
                // Can't compute mid without bid — skip
                tracing::warn!(target: "quoter", poly_inst = %poly_inst, "HEDGE_SKIP no bid in poly_bbo");
                return vec![];
            }
        };
        let poly_mid = (poly_bid + poly_ask) / dec!(2);
        let spread_cross = poly_ask - poly_mid; // half the bid-ask spread
        if spread_cross > self.params.max_slippage_cents {
            tracing::warn!(
                target: "quoter",
                poly_inst    = %poly_inst,
                poly_bid     = %poly_bid,
                poly_ask     = %poly_ask,
                poly_mid     = %poly_mid,
                spread_cross = %spread_cross,
                max_slippage = %self.params.max_slippage_cents,
                "HEDGE_SKIP Polymarket spread too wide",
            );
            return vec![];
        }

        // Informational: log the implied hedge cost vs predict fill prices.
        // This is for auditing only — it does NOT block the hedge.
        if let Some(avg_predict_price) = state.avg_predict_fill_price() {
            let combined_cost = avg_predict_price + poly_ask;
            let hedge_cost_vs_breakeven = combined_cost - dec!(1);
            tracing::info!(
                target: "quoter",
                poly_inst            = %poly_inst,
                avg_predict_fill     = %avg_predict_price,
                poly_ask             = %poly_ask,
                combined_cost        = %combined_cost,
                hedge_cost_per_share = %hedge_cost_vs_breakeven,
                "HEDGE_COST_INFO (positive = paying above $1 for guaranteed $1 payout)",
            );
        }

        // Polymarket CLOB minimum order size is 5 shares.
        let qty = state.qty.round_dp(2);
        if qty < dec!(5) {
            tracing::debug!(
                poly_inst = %poly_inst,
                qty = %qty,
                "HEDGE_SKIP qty < 5 shares (Polymarket min)",
            );
            return vec![];
        }

        let price = poly_ask.round_dp(2);
        let avg_price = state.avg_predict_fill_price().unwrap_or(dec!(0.5));

        tracing::info!(
            target: "quoter",
            poly_inst      = %poly_inst,
            hedge_qty      = %qty,
            hedge_price    = %price,
            hedge_notional = %hedge_notional,
            urgent         = %urgent,
            "HEDGE_PLAN",
        );

        // Optimistically consume: assume the order will fill.
        // On PlaceFailed, we restore.
        if let Some(state) = self.unhedged.get_mut(&poly_inst) {
            state.consume_all();
        }
        self.pending_hedge.insert(poly_inst.clone(), (qty, avg_price));

        let order = OrderRequest {
            instrument: poly_inst,
            side: OrderSide::Buy,
            price: Price::new(price),
            quantity: Quantity::new(qty),
            tif: TimeInForce::Gtc,
            client_order_id: None,
        };

        vec![Action::PlaceOrder(order)]
    }

    fn on_place_failed(&mut self, instrument: &InstrumentId) {
        if instrument.exchange != Exchange::Polymarket {
            return;
        }
        // Restore unhedged qty — order didn't land.
        if let Some((qty, avg_price)) = self.pending_hedge.remove(instrument) {
            tracing::warn!(
                target: "quoter",
                poly_inst = %instrument,
                qty = %qty,
                "HEDGE_REJECT placement failed — restoring unhedged qty",
            );
            if let Some(state) = self.unhedged.get_mut(instrument) {
                state.restore(qty, avg_price);
            }
        }
    }

    fn check_urgency(&mut self) -> Vec<Action> {
        if !self.params.enabled {
            return vec![];
        }
        let urgent: Vec<InstrumentId> = self
            .unhedged
            .iter()
            .filter(|(_, s)| s.is_urgent(&self.params) && !s.qty.is_zero())
            .map(|(k, _)| k.clone())
            .collect();

        let mut actions = Vec::new();
        for inst in urgent {
            tracing::warn!(
                target: "quoter",
                poly_inst = %inst,
                "HEDGE_URGENT time threshold breached",
            );
            actions.extend(self.try_hedge(inst));
        }
        actions
    }
}

// ── Strategy trait ────────────────────────────────────────────────────────────

#[async_trait]
impl Strategy for PredictHedgeStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn subscriptions(&self) -> Vec<InstrumentId> {
        self.all_subscriptions.clone()
    }

    async fn on_event(&mut self, event: &Event) -> Vec<Action> {
        match event {
            Event::Fill { instrument, fill } if instrument.exchange == Exchange::PredictFun => {
                self.on_predict_fill(instrument, fill)
            }

            Event::Fill { instrument, fill } if instrument.exchange == Exchange::Polymarket => {
                // Poly fill confirmed — pending hedge is already consumed.
                tracing::info!(
                    target: "quoter",
                    poly_inst  = %instrument,
                    filled_qty = %fill.quantity,
                    fill_price = %fill.price,
                    "HEDGE_FILL confirmed",
                );
                self.pending_hedge.remove(&instrument.clone());
                vec![]
            }

            Event::PlaceFailed { instrument, reason } => {
                tracing::error!(
                    target: "quoter",
                    instrument = %instrument,
                    reason = %reason,
                    "PLACE_FAILED",
                );
                self.on_place_failed(instrument);
                vec![]
            }

            Event::BookUpdate { instrument, book, .. }
                if instrument.exchange == Exchange::Polymarket =>
            {
                if let (Some(bid), Some(ask)) = (book.best_bid(), book.best_ask()) {
                    self.poly_bbo.insert(instrument.clone(), (bid.0, ask.0));
                }
                vec![]
            }

            Event::Tick { .. } => self.check_urgency(),

            _ => vec![],
        }
    }

    async fn initialize(&mut self, _state: &StrategyState) -> Vec<Action> {
        tracing::info!(
            id       = %self.id,
            markets  = self.unhedged.len(),
            enabled  = self.params.enabled,
            min_notional = %self.params.hedge_min_notional,
            "PredictHedgeStrategy initialized",
        );
        vec![]
    }

    async fn shutdown(&mut self) -> Vec<Action> {
        // Cancel any poly instruments that have pending hedges.
        self.pending_hedge
            .keys()
            .cloned()
            .map(|inst| Action::CancelAll { instrument: inst })
            .collect()
    }
}
