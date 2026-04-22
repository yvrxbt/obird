//! predict.fun ExchangeConnector — one connector per **market** (not per outcome).
//!
//! ## Design: dual-outcome, BUY-only market making
//!
//! A binary prediction market has two complementary outcomes (e.g. YES / NO).
//! Instead of quoting one BUY and one SELL, we place **two BUY orders**:
//!
//!   - BUY YES at price `P`
//!   - BUY NO  at price `1 - P`
//!
//! YES_price + NO_price = 1 (by definition of a binary market). Quoting
//! both sides as buys lets the exchange match each independently without
//! requiring a single counterparty to fill both at once.
//!
//! The `prediction_quoter` strategy calculates `P` (the YES bid price) and
//! derives the NO bid automatically. The connector just executes both orders.
//!
//! ## InstrumentId → token mapping
//!
//! Each outcome gets its own `InstrumentId`:
//!   - YES:  `PredictFun.Binary.{market_id}-{yes_name}`  (e.g. "42-YES")
//!   - NO:   `PredictFun.Binary.{market_id}-{no_name}`   (e.g. "42-NO")
//!
//! `place_order` looks up the right `token_id` from the instrument symbol.
//!
//! ## Cancel
//! `POST /v1/orders/remove` — REST only, no on-chain tx required.
//! `active_orders` map tracks `hash → OrderEntry` for both outcomes.
//!
//! ## Auth
//! JWT fetched on startup. `PredictFunMarketDataFeed` refreshes it on
//! WS `invalid_credentials` and writes back to the shared `Arc<RwLock<String>>`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use predict_sdk::{PredictClient, Side as PredictSide};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::{mpsc, RwLock};
use trading_core::{
    error::ConnectorError,
    traits::ExchangeConnector,
    types::{
        decimal::{Price, Quantity},
        instrument::{Exchange, InstrumentId, InstrumentKind},
        order::{
            OpenOrder, OrderId, OrderRequest, OrderSide, OrderStatus, OrderUpdate, TimeInForce,
        },
        position::Position,
    },
};

use crate::normalize;

const MAINNET_REST: &str = "https://api.predict.fun";
const TESTNET_REST: &str = "https://api-testnet.predict.fun";
const BNB_MAINNET_CHAIN_ID: u64 = 56;
const BNB_TESTNET_CHAIN_ID: u64 = 97;

/// Minimum valid price on predict.fun — anything closer to 0 or 1 is implausible.
pub const MIN_PRICE: Decimal = dec!(0.001);
/// Maximum valid price (1 - MIN_PRICE).
pub const MAX_PRICE: Decimal = dec!(0.999);

// ── Order tracking ────────────────────────────────────────────────────────────

/// Per-order state kept for cancel-by-id and modify (cancel+replace).
#[derive(Debug, Clone)]
pub(crate) struct OrderEntry {
    /// Numeric order ID used by the REST cancel endpoint.
    pub predict_id: String,
    /// Original side (always `Buy` in the dual-BUY design, kept for correctness).
    pub side: OrderSide,
    /// Which outcome instrument this order belongs to.
    pub instrument: InstrumentId,
}

// ── Shutdown handle ───────────────────────────────────────────────────────────

/// Extracted from `PredictFunClient` before it moves into the engine runner.
///
/// Mirrors the pattern from `HyperliquidClient::ShutdownHandle`.
///
/// Shutdown sequence (coordinated with `EngineRunner`):
///   1. `EngineRunner` sets the shutdown flag → `place_order` blocks new places.
///   2. `EngineRunner` drains the router → any in-flight place call completes and
///      records its hash in `active_orders`.
///   3. `cancel_all()` here — iterates `active_orders` for predict_ids, fires a
///      single batched REST cancel, awaits the HTTP response, and logs confirmation.
pub struct PredictShutdownHandle {
    inner: Arc<PredictClient>,
    active_orders: Arc<Mutex<HashMap<String, OrderEntry>>>,
    market_id: u64,
    pub shutting_down: Arc<AtomicBool>,
}

impl PredictShutdownHandle {
    /// Signal the connector to stop accepting new place requests.
    /// Call this before draining the router.
    pub fn set_shutting_down(&self) {
        self.shutting_down.store(true, Ordering::Release);
        tracing::info!(
            market_id = self.market_id,
            "SHUTDOWN flag set — new places will be rejected"
        );
    }

    /// Cancel all tracked resting orders via REST.
    ///
    /// Call AFTER `EngineRunner::run()` returns so any in-flight place has already
    /// recorded its hash in `active_orders`. Awaits HTTP confirmation before returning.
    pub async fn cancel_all(&self) -> anyhow::Result<()> {
        let predict_ids: Vec<String> = {
            self.active_orders
                .lock()
                .unwrap()
                .values()
                .map(|e| e.predict_id.clone())
                .collect()
        };

        if predict_ids.is_empty() {
            tracing::info!(
                market_id = self.market_id,
                "SHUTDOWN no tracked orders — nothing to cancel"
            );
            return Ok(());
        }

        tracing::info!(
            market_id = self.market_id,
            n = predict_ids.len(),
            ids = ?predict_ids,
            "SHUTDOWN cancel submitted — awaiting ack",
        );

        self.inner
            .cancel_orders(&predict_ids)
            .await
            .map_err(|e| anyhow::anyhow!("SHUTDOWN cancel_orders: {e}"))?;

        tracing::info!(
            market_id = self.market_id,
            n = predict_ids.len(),
            "SHUTDOWN cancel ack received — all orders cancelled",
        );
        Ok(())
    }
}

// ── Config ────────────────────────────────────────────────────────────────────

/// Parameters extracted from TOML `[exchanges.params]` for a predict.fun market.
///
/// One connector covers the whole market — both YES and NO outcomes.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PredictFunParams {
    /// predict.fun integer market ID.
    pub market_id: u64,

    /// Human-readable name for the YES outcome (e.g. "YES", "UP", "OVER").
    /// Used as the last segment of the YES instrument symbol.
    pub yes_outcome_name: String,
    /// On-chain token ID for the YES outcome (`outcomes[].onChainId` from GET /v1/markets).
    pub yes_token_id: String,

    /// Human-readable name for the NO outcome (e.g. "NO", "DOWN", "UNDER").
    pub no_outcome_name: String,
    /// On-chain token ID for the NO outcome.
    pub no_token_id: String,

    pub is_neg_risk: bool,
    pub is_yield_bearing: bool,

    /// Fee rate in basis points — from `GET /v1/markets` → `feeRateBps`.
    #[serde(default)]
    pub fee_rate_bps: u64,

    /// Polymarket YES outcome CLOB token ID — from `polymarketConditionIds[0]` resolved
    /// via the Gamma API (`connector_polymarket::client::lookup_token_ids`).
    ///
    /// When set, `live.rs` subscribes to the Polymarket CLOB WS feed and wires the
    /// resulting mid into `PredictionQuoter` as the fair value signal. If absent,
    /// the quoter falls back to the predict.fun mid.
    ///
    /// To populate: run `trading-cli predict-markets` — it prints both token IDs.
    #[serde(default)]
    pub polymarket_yes_token_id: Option<String>,

    /// Polymarket NO outcome CLOB token ID.
    ///
    /// When both `polymarket_yes_token_id` and this field are set, the hedge
    /// strategy (`PredictHedgeStrategy`) is enabled for this market:
    ///   - predict YES fill → buy poly NO token
    ///   - predict NO fill  → buy poly YES token
    #[serde(default)]
    pub polymarket_no_token_id: Option<String>,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// predict.fun connector — one instance covers an entire binary market.
///
/// Manages two instruments (YES and NO) and always places `Side::Buy` orders
/// on both outcomes. Use `yes_instrument()` and `no_instrument()` to register
/// both with the `MarketDataBus` before starting the feed.
pub struct PredictFunClient {
    inner: Arc<PredictClient>,

    market_id: u64,
    is_neg_risk: bool,
    is_yield_bearing: bool,
    fee_rate_bps: u64,
    /// Price tick precision fetched from GET /v1/markets/{id} at startup.
    /// 2 → 0.01 ticks, 3 → 0.001 ticks.
    decimal_precision: u32,

    /// YES outcome instrument: `PredictFun.Binary.{market_id}-{yes_name}`
    yes_instrument: InstrumentId,
    /// NO outcome instrument:  `PredictFun.Binary.{market_id}-{no_name}`
    no_instrument: InstrumentId,

    /// Maps instrument symbol → on-chain token ID for order placement.
    token_map: HashMap<String, String>,

    /// Maps order hash → OrderEntry (predict_id + side + instrument).
    /// Shared with `PredictFunMarketDataFeed` — the feed removes entries on fill/cancel.
    pub active_orders: Arc<Mutex<HashMap<String, OrderEntry>>>,

    /// Maps order hash → instrument — populated on place, cleared only on WS fill/expiry.
    /// Survives `cancel_all()` so fills arriving after cancels resolve to the correct
    /// instrument rather than falling back to the YES instrument.
    pub placed_instruments: Arc<Mutex<HashMap<String, InstrumentId>>>,

    /// Shared JWT — updated by the feed on re-auth after expiry.
    pub jwt: Arc<RwLock<String>>,

    /// Set by `PredictShutdownHandle::set_shutting_down()`. Checked at the top of
    /// `place_order` so orders queued after Ctrl+C never hit the network.
    shutting_down: Arc<AtomicBool>,

    update_tx: mpsc::UnboundedSender<OrderUpdate>,
    update_rx: mpsc::UnboundedReceiver<OrderUpdate>,
}

impl PredictFunClient {
    /// Construct from env-var names and market params.
    ///
    /// Authenticates immediately (fetches JWT).
    pub async fn from_env(
        api_key_env: &str,
        private_key_env: &str,
        params: &PredictFunParams,
        testnet: bool,
    ) -> Result<Self, ConnectorError> {
        let api_key = std::env::var(api_key_env)
            .map_err(|_| ConnectorError::AuthFailed(format!("missing env var: {api_key_env}")))?;
        let private_key = std::env::var(private_key_env).map_err(|_| {
            ConnectorError::AuthFailed(format!("missing env var: {private_key_env}"))
        })?;

        let (api_base_url, chain_id) = if testnet {
            (TESTNET_REST.to_string(), BNB_TESTNET_CHAIN_ID)
        } else {
            (MAINNET_REST.to_string(), BNB_MAINNET_CHAIN_ID)
        };

        let predict_client =
            PredictClient::new(chain_id, &private_key, api_base_url, Some(api_key))
                .map_err(|e| ConnectorError::AuthFailed(format!("PredictClient init: {e}")))?;

        let jwt = predict_client
            .authenticate_and_store()
            .await
            .map_err(|e| ConnectorError::AuthFailed(format!("predict.fun auth: {e}")))?;

        let signer = predict_client
            .signer_address()
            .map_err(|e| ConnectorError::AuthFailed(format!("signer: {e}")))?;

        let yes_instrument = InstrumentId::new(
            Exchange::PredictFun,
            InstrumentKind::Binary,
            format!("{}-{}", params.market_id, params.yes_outcome_name),
        );
        let no_instrument = InstrumentId::new(
            Exchange::PredictFun,
            InstrumentKind::Binary,
            format!("{}-{}", params.market_id, params.no_outcome_name),
        );

        let mut token_map = HashMap::new();
        token_map.insert(yes_instrument.symbol.clone(), params.yes_token_id.clone());
        token_map.insert(no_instrument.symbol.clone(), params.no_token_id.clone());

        // Pre-populate placed_instruments and active_orders from any open orders left
        // over from a previous session. Without this, fills for those orders arrive via
        // the WS wallet feed with hashes not in our maps and fall back to YES instrument,
        // corrupting position tracking. Also ensures cancel_all at startup can actually
        // cancel those orders (it iterates active_orders for predict_ids).
        let mut seed_active: HashMap<String, OrderEntry> = HashMap::new();
        let mut seed_placed: HashMap<String, InstrumentId> = HashMap::new();
        match predict_client.get_open_orders().await {
            Ok(orders) => {
                for o in &orders {
                    // Only seed orders belonging to this market's token IDs.
                    let inst = if o.order.token_id == params.yes_token_id {
                        yes_instrument.clone()
                    } else if o.order.token_id == params.no_token_id {
                        no_instrument.clone()
                    } else {
                        continue;
                    };
                    let side = if o.order.side == 0 {
                        OrderSide::Buy
                    } else {
                        OrderSide::Sell
                    };
                    seed_active.insert(
                        o.order.hash.clone(),
                        OrderEntry {
                            predict_id: o.id.clone(),
                            side,
                            instrument: inst.clone(),
                        },
                    );
                    seed_placed.insert(o.order.hash.clone(), inst);
                }
                tracing::info!(
                    market_id = params.market_id,
                    seeded = seed_active.len(),
                    "Pre-populated order maps from existing open orders",
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to fetch open orders at startup — pre-existing fills may default to YES instrument",
                );
            }
        }

        // Fetch market metadata to get decimal_precision (tick size).
        // This is required for correct price rounding — precision=2 → 0.01 ticks,
        // precision=3 → 0.001 ticks. The exchange rejects prices outside this precision.
        let decimal_precision = match predict_client.get_market_by_id(params.market_id).await {
            Ok(m) => {
                let prec = m.decimal_precision.unwrap_or(2);
                tracing::info!(
                    market_id = params.market_id,
                    decimal_precision = prec,
                    spread_threshold = ?m.spread_threshold,
                    share_threshold  = ?m.share_threshold,
                    "Fetched market metadata",
                );
                prec
            }
            Err(e) => {
                return Err(ConnectorError::AuthFailed(format!(
                    "Failed to fetch market {}: {e}. Cannot determine price precision.",
                    params.market_id
                )));
            }
        };

        let (update_tx, update_rx) = mpsc::unbounded_channel();

        tracing::info!(
            market_id = params.market_id,
            yes = %yes_instrument.symbol,
            no  = %no_instrument.symbol,
            signer = %signer,
            decimal_precision,
            testnet,
            "PredictFunClient ready (dual-outcome BUY-only market maker)",
        );

        Ok(Self {
            inner: Arc::new(predict_client),
            market_id: params.market_id,
            is_neg_risk: params.is_neg_risk,
            is_yield_bearing: params.is_yield_bearing,
            fee_rate_bps: params.fee_rate_bps,
            decimal_precision,
            yes_instrument,
            no_instrument,
            token_map,
            active_orders: Arc::new(Mutex::new(seed_active)),
            placed_instruments: Arc::new(Mutex::new(seed_placed)),
            jwt: Arc::new(RwLock::new(jwt)),
            shutting_down: Arc::new(AtomicBool::new(false)),
            update_tx,
            update_rx,
        })
    }

    // ── Accessors (used by the market data feed) ─────────────────────────────

    /// InstrumentId for the YES outcome. Register with `MarketDataBus` before feed start.
    pub fn yes_instrument(&self) -> InstrumentId {
        self.yes_instrument.clone()
    }

    /// InstrumentId for the NO outcome. Register with `MarketDataBus` before feed start.
    pub fn no_instrument(&self) -> InstrumentId {
        self.no_instrument.clone()
    }

    /// Both instruments as a pair `(yes, no)`.
    pub fn instruments(&self) -> (InstrumentId, InstrumentId) {
        (self.yes_instrument.clone(), self.no_instrument.clone())
    }

    pub fn inner(&self) -> Arc<PredictClient> {
        Arc::clone(&self.inner)
    }
    pub fn market_id(&self) -> u64 {
        self.market_id
    }
    pub fn is_neg_risk(&self) -> bool {
        self.is_neg_risk
    }
    pub fn is_yield_bearing(&self) -> bool {
        self.is_yield_bearing
    }
    pub fn update_tx(&self) -> mpsc::UnboundedSender<OrderUpdate> {
        self.update_tx.clone()
    }
    pub fn active_orders(&self) -> Arc<Mutex<HashMap<String, OrderEntry>>> {
        Arc::clone(&self.active_orders)
    }
    pub fn placed_instruments(&self) -> Arc<Mutex<HashMap<String, InstrumentId>>> {
        Arc::clone(&self.placed_instruments)
    }

    /// Extract a shutdown handle BEFORE moving this connector into the engine runner.
    /// The handle shares `active_orders` and `shutting_down` with the connector so
    /// graceful shutdown can block new places and cancel all resting orders.
    pub fn shutdown_handle(&self) -> PredictShutdownHandle {
        PredictShutdownHandle {
            inner: Arc::clone(&self.inner),
            active_orders: Arc::clone(&self.active_orders),
            market_id: self.market_id,
            shutting_down: Arc::clone(&self.shutting_down),
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Resolve the on-chain token ID from an instrument.
    ///
    /// Returns an error if the instrument doesn't belong to this connector.
    fn token_id_for(&self, instrument: &InstrumentId) -> Result<&str, ConnectorError> {
        self.token_map
            .get(&instrument.symbol)
            .map(|s| s.as_str())
            .ok_or_else(|| {
                ConnectorError::OrderRejected(format!(
                    "unknown instrument '{}' — expected one of: {:?}",
                    instrument.symbol,
                    self.token_map.keys().collect::<Vec<_>>()
                ))
            })
    }

    fn price_to_wei(p: &Price) -> Decimal {
        p.inner() * dec!(1_000_000_000_000_000_000)
    }

    fn qty_to_wei(q: &Quantity) -> Decimal {
        q.inner() * dec!(1_000_000_000_000_000_000)
    }
}

// ── ExchangeConnector impl ────────────────────────────────────────────────────

#[async_trait]
impl ExchangeConnector for PredictFunClient {
    fn exchange(&self) -> Exchange {
        Exchange::PredictFun
    }

    /// Place a single order (always `Side::Buy` in the dual-BUY design).
    ///
    /// `req.instrument` must be either the YES or NO instrument for this market.
    /// `req.price`      must be in (MIN_PRICE, MAX_PRICE).
    /// `req.side`       should always be `Buy`; the strategy must compute prices
    ///                  such that both YES and NO are BUY orders.
    async fn place_order(&self, req: &OrderRequest) -> Result<OrderId, ConnectorError> {
        // Block new places during shutdown — mirrors HyperliquidClient::place_batch behaviour.
        if self.shutting_down.load(Ordering::Acquire) {
            tracing::info!(
                instrument = %req.instrument,
                "PLACE_SKIPPED shutting down — order not sent to exchange",
            );
            return Err(ConnectorError::OrderRejected("shutting down".into()));
        }

        // Reject obvious out-of-range prices early.
        let p = req.price.inner();
        if p < MIN_PRICE || p > MAX_PRICE {
            return Err(ConnectorError::OrderRejected(format!(
                "price {p} out of bounds ({MIN_PRICE}, {MAX_PRICE})"
            )));
        }

        let token_id = self.token_id_for(&req.instrument)?.to_string();
        let side = match req.side {
            OrderSide::Buy => PredictSide::Buy,
            OrderSide::Sell => PredictSide::Sell, // allowed but unusual on predict.fun
        };
        let price_wei = Self::price_to_wei(&req.price);
        let qty_wei = Self::qty_to_wei(&req.quantity);

        let resp = self
            .inner
            .place_limit_order(
                &token_id,
                side,
                price_wei,
                qty_wei,
                self.is_neg_risk,
                self.is_yield_bearing,
                self.fee_rate_bps,
            )
            .await
            .map_err(|e| ConnectorError::OrderRejected(format!("{e}")))?;

        let data = resp
            .data
            .ok_or_else(|| ConnectorError::OrderRejected("empty response data".into()))?;

        {
            let mut map = self.active_orders.lock().unwrap();
            map.insert(
                data.order_hash.clone(),
                OrderEntry {
                    predict_id: data.order_id.clone(),
                    side: req.side,
                    instrument: req.instrument.clone(),
                },
            );
        }
        // Also track in placed_instruments — this map is NOT cleared on cancel_all,
        // so fills arriving after cancels still resolve to the correct instrument.
        {
            let mut pi = self.placed_instruments.lock().unwrap();
            pi.insert(data.order_hash.clone(), req.instrument.clone());
        }

        tracing::debug!(
            instrument = %req.instrument,
            hash = %data.order_hash,
            predict_id = %data.order_id,
            side = ?req.side,
            price = %req.price,
            qty = %req.quantity,
            "PLACE",
        );

        Ok(data.order_hash)
    }

    async fn cancel_order(
        &self,
        _instrument: &InstrumentId,
        order_id: &OrderId,
    ) -> Result<(), ConnectorError> {
        let predict_id = {
            let map = self.active_orders.lock().unwrap();
            map.get(order_id).map(|e| e.predict_id.clone())
        };

        let Some(predict_id) = predict_id else {
            tracing::debug!(hash = %order_id, "CANCEL no-op (unknown OID)");
            return Ok(());
        };

        self.inner
            .cancel_orders(&[predict_id])
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("cancel_order: {e}")))?;

        self.active_orders.lock().unwrap().remove(order_id);
        tracing::debug!(hash = %order_id, "CANCEL OK");
        Ok(())
    }

    /// Cancel all resting orders on BOTH YES and NO outcomes in one REST call.
    async fn cancel_all(&self, _instrument: &InstrumentId) -> Result<(), ConnectorError> {
        let predict_ids: Vec<String> = {
            let map = self.active_orders.lock().unwrap();
            map.values().map(|e| e.predict_id.clone()).collect()
        };

        if predict_ids.is_empty() {
            tracing::debug!(market_id = self.market_id, "CANCEL_ALL no tracked orders");
            return Ok(());
        }

        tracing::info!(
            market_id = self.market_id,
            n = predict_ids.len(),
            "CANCEL_ALL"
        );

        for chunk in predict_ids.chunks(100) {
            self.inner
                .cancel_orders(chunk)
                .await
                .map_err(|e| ConnectorError::Other(anyhow::anyhow!("cancel_all: {e}")))?;
        }

        self.active_orders.lock().unwrap().clear();
        Ok(())
    }

    /// Cancel and re-place at new price/qty.
    ///
    /// `new_price` should already be validated (in bounds, non-crossing) by the
    /// calling strategy before this is invoked.
    async fn modify_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
        new_price: Price,
        new_qty: Quantity,
    ) -> Result<OrderId, ConnectorError> {
        // Retrieve original side before cancelling
        let side = {
            let map = self.active_orders.lock().unwrap();
            map.get(order_id).map(|e| e.side).unwrap_or(OrderSide::Buy)
        };

        self.cancel_order(instrument, order_id).await?;

        self.place_order(&OrderRequest {
            instrument: instrument.clone(),
            side,
            price: new_price,
            quantity: new_qty,
            tif: TimeInForce::PostOnly,
            client_order_id: None,
        })
        .await
    }

    /// Positions for both YES and NO outcome tokens on this market.
    async fn positions(&self) -> Result<Vec<Position>, ConnectorError> {
        let raw = self
            .inner
            .get_positions()
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("positions: {e}")))?;

        let positions = raw
            .into_iter()
            .map(|p| {
                let symbol = format!("{}-{}", p.market.id, p.outcome.name);
                Position {
                    instrument: InstrumentId::new(
                        Exchange::PredictFun,
                        InstrumentKind::Binary,
                        symbol,
                    ),
                    size: Quantity::new(normalize::from_wei(&p.amount)),
                    avg_entry_price: Price::zero(),
                    unrealized_pnl: Price::zero(),
                }
            })
            .collect();

        Ok(positions)
    }

    /// Open orders for BOTH outcomes on this market.
    async fn open_orders(
        &self,
        _instrument: &InstrumentId,
    ) -> Result<Vec<OpenOrder>, ConnectorError> {
        let raw = self
            .inner
            .get_open_orders()
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("open_orders: {e}")))?;

        let both_token_ids: Vec<&str> = self.token_map.values().map(|s| s.as_str()).collect();

        let orders = raw
            .into_iter()
            .filter(|o| both_token_ids.contains(&o.order.token_id.as_str()))
            .filter_map(|o| {
                // Resolve instrument from token_id
                let instrument = self
                    .token_map
                    .iter()
                    .find(|(_, v)| *v == &o.order.token_id)
                    .map(|(sym, _)| {
                        if sym == &self.yes_instrument.symbol {
                            self.yes_instrument.clone()
                        } else {
                            self.no_instrument.clone()
                        }
                    })?;

                let side = if o.order.side == 0 {
                    OrderSide::Buy
                } else {
                    OrderSide::Sell
                };
                let maker: Decimal = o.order.maker_amount.parse().unwrap_or(Decimal::ZERO);
                let taker: Decimal = o.order.taker_amount.parse().unwrap_or(Decimal::ZERO);

                let (price_dec, qty_dec) = match side {
                    OrderSide::Buy => {
                        let p = if taker > Decimal::ZERO {
                            maker / taker
                        } else {
                            Decimal::ZERO
                        };
                        (p, normalize::from_wei(&taker.trunc().to_string()))
                    }
                    OrderSide::Sell => {
                        let p = if maker > Decimal::ZERO {
                            taker / maker
                        } else {
                            Decimal::ZERO
                        };
                        (p, normalize::from_wei(&maker.trunc().to_string()))
                    }
                };

                Some(OpenOrder {
                    order_id: o.order.hash.clone(),
                    instrument,
                    side,
                    price: Price::new(price_dec),
                    quantity: Quantity::new(qty_dec),
                    filled_qty: Quantity::new(normalize::from_wei(&o.amount_filled)),
                })
            })
            .collect();

        Ok(orders)
    }

    fn order_update_rx(&mut self) -> &mut mpsc::UnboundedReceiver<OrderUpdate> {
        &mut self.update_rx
    }

    fn decimal_precision(&self, _instrument: &InstrumentId) -> Option<u32> {
        // One connector per market (see crate docs): precision is fixed per instance
        // and was fetched from GET /v1/markets/{id} at construction time, so the
        // instrument argument is irrelevant here.
        Some(self.decimal_precision)
    }
}
