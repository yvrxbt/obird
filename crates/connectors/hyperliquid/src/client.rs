//! Hyperliquid ExchangeConnector backed by hypersdk.

use std::str::FromStr;

use anyhow::Context;
use hypersdk::{
    Address,
    hypercore::{
        self as hypercore,
        BatchCancel, BatchModify, Cancel, Cloid, Modify, NonceHandler, OidOrCloid,
        PerpMarket, PriceTick, SpotMarket,
        types::{
            BatchOrder, OrderGrouping, OrderRequest as HlOrderRequest, OrderResponseStatus,
            OrderTypePlacement, Side, TimeInForce as HlTif,
        },
        PrivateKeySigner,
    },
};
use rust_decimal::{Decimal, MathematicalOps, RoundingStrategy};
use tokio::sync::mpsc;
use trading_core::{
    Price, Quantity,
    error::ConnectorError,
    traits::ExchangeConnector,
    types::{
        instrument::{Exchange, InstrumentId, InstrumentKind},
        order::{
            OpenOrder, OrderId, OrderRequest, OrderSide, OrderStatus, OrderUpdate, TimeInForce,
        },
        position::Position,
    },
};

use crate::{market_data::AssetInfo, normalize};

// ── Shutdown handle ───────────────────────────────────────────────────────────

/// Handle for emergency cancel-all on process shutdown.
/// Extracted from HyperliquidClient before it moves into the engine runner.
/// Creates a fresh HttpClient on use (not hot path, called once on exit).
pub struct ShutdownHandle {
    pub signer: PrivateKeySigner,
    pub nonce: std::sync::Arc<NonceHandler>,
    pub market_index: usize,
    pub instrument: InstrumentId,
    pub mids_key: String,
    pub testnet: bool,
}

impl ShutdownHandle {
    /// Cancel all open orders using scheduleCancel(now). Called once on Ctrl+C.
    pub async fn cancel_all(&self) -> anyhow::Result<()> {
        use chrono::Utc;
        // Build a fresh client — HttpClient is not Clone, and this is not hot path
        let http = if self.testnet { hypercore::testnet() } else { hypercore::mainnet() };

        tracing::info!(instrument = %self.instrument, "SHUTDOWN schedule_cancel(now)");

        http.schedule_cancel(
            &self.signer,
            self.nonce.next(),
            Utc::now(),
            None,
            None,
        ).await.context("schedule_cancel on shutdown")?;

        tracing::info!(instrument = %self.instrument, "SHUTDOWN all orders cancelled");
        Ok(())
    }
}

// ── Market resolution ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub enum ResolvedMarket {
    Perp(PerpMarket),
    Spot(SpotMarket),
}

impl ResolvedMarket {
    pub fn index(&self) -> usize {
        match self {
            ResolvedMarket::Perp(m) => m.index,
            ResolvedMarket::Spot(m) => m.index,
        }
    }

    pub fn sz_decimals(&self) -> u32 {
        match self {
            ResolvedMarket::Perp(m) => m.sz_decimals as u32,
            ResolvedMarket::Spot(m) => m.tokens[0].sz_decimals as u32,
        }
    }

    pub fn price_tick(&self) -> &PriceTick {
        match self {
            ResolvedMarket::Perp(m) => &m.table,
            ResolvedMarket::Spot(m) => &m.table,
        }
    }

    /// Key in AllMids mids hashmap.
    pub fn mids_key(&self) -> String {
        match self {
            ResolvedMarket::Perp(m) => m.name.clone(),
            ResolvedMarket::Spot(m) => m.name.clone(),
        }
    }

    /// Coin string for WS L2Book/Trades subscriptions.
    pub fn ws_coin(&self) -> String {
        match self {
            ResolvedMarket::Perp(m) => m.name.clone(),
            ResolvedMarket::Spot(m) => format!("@{}", m.index - 10_000),
        }
    }

    pub fn instrument_kind(&self) -> InstrumentKind {
        match self {
            ResolvedMarket::Perp(_) => InstrumentKind::Perpetual,
            ResolvedMarket::Spot(_) => InstrumentKind::Spot,
        }
    }

    pub fn min_size(&self) -> Decimal {
        Decimal::TEN.powi(-(self.sz_decimals() as i64))
    }
}

/// Resolve a symbol to a Hyperliquid market.
/// Tries perps first (name match), then spot (base token, pair name, @N).
pub async fn resolve_symbol(
    client: &hypercore::HttpClient,
    symbol: &str,
) -> anyhow::Result<ResolvedMarket> {
    let perps = client.perps().await.context("fetching perps")?;
    if let Some(m) = perps.into_iter().find(|p| p.name == symbol) {
        tracing::info!(symbol, index = m.index, "Resolved as perp");
        return Ok(ResolvedMarket::Perp(m));
    }

    let spots = client.spot().await.context("fetching spot")?;

    if let Some(m) = spots.iter().find(|s| s.tokens[0].name == symbol) {
        tracing::info!(symbol, index = m.index, "Resolved as spot (base token)");
        return Ok(ResolvedMarket::Spot(m.clone()));
    }
    if let Some(m) = spots.iter().find(|s| s.name == symbol || s.symbol() == symbol) {
        tracing::info!(symbol, index = m.index, "Resolved as spot (pair name)");
        return Ok(ResolvedMarket::Spot(m.clone()));
    }
    if let Some(n) = symbol.strip_prefix('@').and_then(|s| s.parse::<usize>().ok()) {
        if let Some(m) = spots.into_iter().find(|s| s.index == 10_000 + n) {
            tracing::info!(symbol, index = m.index, "Resolved as spot (@N)");
            return Ok(ResolvedMarket::Spot(m));
        }
    }

    anyhow::bail!(
        "symbol '{}' not found on HL. \
         For perps use the market name (e.g. 'ETH'). \
         For spot use the base token name (e.g. 'PURR') or '@N' format.",
        symbol
    )
}

// ── Connector ─────────────────────────────────────────────────────────────────

pub struct HyperliquidClient {
    http: hypercore::HttpClient,
    signer: PrivateKeySigner,
    market: ResolvedMarket,
    instrument: InstrumentId,
    nonce: std::sync::Arc<NonceHandler>,
    vault_address: Option<Address>,
    update_tx: mpsc::UnboundedSender<OrderUpdate>,
    update_rx: mpsc::UnboundedReceiver<OrderUpdate>,
}

impl HyperliquidClient {
    pub async fn from_env(
        private_key_env: &str,
        symbol: &str,
        testnet: bool,
    ) -> Result<Self, ConnectorError> {
        let pk = std::env::var(private_key_env).map_err(|_| {
            ConnectorError::AuthFailed(format!("missing env var: {private_key_env}"))
        })?;

        let signer = PrivateKeySigner::from_str(pk.trim())
            .map_err(|e| ConnectorError::AuthFailed(format!("invalid private key: {e}")))?;

        let http = if testnet { hypercore::testnet() } else { hypercore::mainnet() };
        let market = resolve_symbol(&http, symbol)
            .await
            .map_err(ConnectorError::Other)?;

        let instrument = InstrumentId::new(
            Exchange::Hyperliquid,
            market.instrument_kind(),
            symbol,
        );

        let (update_tx, update_rx) = mpsc::unbounded_channel();

        tracing::info!(
            symbol, testnet,
            index = market.index(),
            sz_decimals = market.sz_decimals(),
            "HyperliquidClient ready"
        );

        Ok(Self {
            http,
            signer,
            market,
            instrument,
            nonce: std::sync::Arc::new(NonceHandler::default()),
            vault_address: None,
            update_tx,
            update_rx,
        })
    }

    pub fn instrument(&self) -> InstrumentId { self.instrument.clone() }
    pub fn wallet_address(&self) -> Address { self.signer.address() }

    /// Extract a cancel handle BEFORE moving this connector into the engine runner.
    /// Use it to cancel all open orders on Ctrl+C shutdown.
    pub fn shutdown_handle(&self, testnet: bool) -> ShutdownHandle {
        ShutdownHandle {
            signer: self.signer.clone(),
            nonce: self.nonce.clone(),
            market_index: self.market.index(),
            instrument: self.instrument.clone(),
            mids_key: self.market.mids_key(),
            testnet,
        }
    }

    pub fn asset_info(&self) -> AssetInfo {
        AssetInfo {
            mids_key: self.market.mids_key(),
            ws_coin: self.market.ws_coin(),
            instrument: self.instrument.clone(),
        }
    }

    fn round_price(&self, price: Decimal, is_buy: bool) -> Option<Decimal> {
        // PriceTick::round_by_side uses tick.scale() which returns 2 for Decimal::TEN.powi(-1)
        // due to rust_decimal's internal representation. tick.normalize().scale() gives the
        // correct decimal places. We reimplement the rounding here using normalize().
        let tick = self.market.price_tick().tick_for(price)?;
        let dp = tick.normalize().scale(); // e.g. 0.1 → normalize → 0.1 → scale=1
        let strategy = if is_buy {
            RoundingStrategy::ToNegativeInfinity // bid: round down (better price for maker)
        } else {
            RoundingStrategy::ToPositiveInfinity  // ask: round up
        };
        let rounded = price.round_dp_with_strategy(dp, strategy);
        tracing::debug!(
            raw = %price, rounded = %rounded, dp, is_buy, "price rounded"
        );
        Some(rounded)
    }

    fn round_size(&self, size: Decimal) -> Decimal {
        size.round_dp(self.market.sz_decimals())
    }

    fn to_hl_tif(tif: TimeInForce) -> HlTif {
        match tif {
            TimeInForce::PostOnly => HlTif::Alo, // ALO = post-only, never crosses
            TimeInForce::Ioc => HlTif::Ioc,
            TimeInForce::Gtc => HlTif::Gtc,
        }
    }
}

#[async_trait::async_trait]
impl ExchangeConnector for HyperliquidClient {
    fn exchange(&self) -> Exchange { Exchange::Hyperliquid }

    async fn place_order(&self, req: &OrderRequest) -> Result<OrderId, ConnectorError> {
        let is_buy = normalize::to_hl_side(req.side);

        let rounded_price = self
            .round_price(req.price.inner(), is_buy)
            .ok_or_else(|| ConnectorError::Other(anyhow::anyhow!("price rounding failed")))?;

        let rounded_size = self.round_size(req.quantity.inner());
        if rounded_size < self.market.min_size() {
            return Err(ConnectorError::OrderRejected(format!(
                "size {} < min {}",
                rounded_size, self.market.min_size()
            )));
        }

        let cloid = req.client_order_id
            .as_ref()
            .and_then(|s| s.parse::<Cloid>().ok())
            .unwrap_or_else(Cloid::random);

        let resp = self.http
            .place(
                &self.signer,
                BatchOrder {
                    orders: vec![HlOrderRequest {
                        asset: self.market.index(),
                        is_buy,
                        limit_px: rounded_price,
                        sz: rounded_size,
                        reduce_only: false,
                        order_type: OrderTypePlacement::Limit {
                            tif: Self::to_hl_tif(req.tif),
                        },
                        cloid,
                    }],
                    grouping: OrderGrouping::Na,
                },
                self.nonce.next(),
                self.vault_address,
                None,
            )
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("place: {}", e)))?;

        match resp.into_iter().next() {
            Some(OrderResponseStatus::Resting { oid, .. }) => {
                let id = normalize::order_id_from_oid(oid);
                let _ = self.update_tx.send(OrderUpdate {
                    instrument: req.instrument.clone(),
                    order_id: id.clone(),
                    status: OrderStatus::Acknowledged,
                    filled_qty: Quantity::zero(),
                    remaining_qty: req.quantity,
                    avg_fill_price: None,
                    timestamp_ns: normalize::now_ns(),
                });
                Ok(id)
            }
            Some(OrderResponseStatus::Filled { oid, avg_px, .. }) => {
                let id = normalize::order_id_from_oid(oid);
                let _ = self.update_tx.send(OrderUpdate {
                    instrument: req.instrument.clone(),
                    order_id: id.clone(),
                    status: OrderStatus::Filled,
                    filled_qty: req.quantity,
                    remaining_qty: Quantity::zero(),
                    avg_fill_price: Some(Price::new(avg_px)),
                    timestamp_ns: normalize::now_ns(),
                });
                Ok(id)
            }
            Some(OrderResponseStatus::Success) => {
                // Generic ack without oid — use cloid as order id
                Ok(cloid.to_string())
            }
            Some(OrderResponseStatus::Error(msg)) => Err(ConnectorError::OrderRejected(msg)),
            None => Err(ConnectorError::OrderRejected("empty response".into())),
        }
    }

    /// HL-native batch placement — submits all orders in a single `BatchOrder` call.
    /// Far more efficient than N sequential place_order calls.
    async fn place_batch(&self, reqs: &[OrderRequest]) -> Vec<Result<OrderId, ConnectorError>> {
        if reqs.is_empty() {
            return vec![];
        }

        // Build HL order requests
        let mut hl_reqs = Vec::with_capacity(reqs.len());
        let mut cloids = Vec::with_capacity(reqs.len());

        for req in reqs {
            let is_buy = normalize::to_hl_side(req.side);
            let rounded_price = match self.round_price(req.price.inner(), is_buy) {
                Some(p) => p,
                None => {
                    // Can't batch if any price fails; fall back handled by returning errors
                    return reqs.iter().map(|_| {
                        Err(ConnectorError::Other(anyhow::anyhow!("price rounding failed")))
                    }).collect();
                }
            };
            let rounded_size = self.round_size(req.quantity.inner());
            if rounded_size < self.market.min_size() {
                return reqs.iter().map(|r| {
                    Err(ConnectorError::OrderRejected(format!(
                        "size {} < min {}", rounded_size, self.market.min_size()
                    )))
                }).collect();
            }
            let cloid = req.client_order_id
                .as_ref()
                .and_then(|s| s.parse::<Cloid>().ok())
                .unwrap_or_else(Cloid::random);
            cloids.push(cloid);
            hl_reqs.push(HlOrderRequest {
                asset: self.market.index(),
                is_buy,
                limit_px: rounded_price,
                sz: rounded_size,
                reduce_only: false,
                order_type: OrderTypePlacement::Limit { tif: Self::to_hl_tif(req.tif) },
                cloid,
            });
        }

        let resp = match self.http
            .place(
                &self.signer,
                BatchOrder { orders: hl_reqs, grouping: OrderGrouping::Na },
                self.nonce.next(),
                self.vault_address,
                None,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let err_str = e.to_string();
                return reqs.iter().map(|_| {
                    Err(ConnectorError::Other(anyhow::anyhow!("batch place: {}", err_str)))
                }).collect();
            }
        };

        // Map each response status back to the corresponding request
        resp.into_iter().zip(reqs.iter()).map(|(status, req)| {
            match status {
                OrderResponseStatus::Resting { oid, .. } => {
                    let id = normalize::order_id_from_oid(oid);
                    let _ = self.update_tx.send(OrderUpdate {
                        instrument: req.instrument.clone(),
                        order_id: id.clone(),
                        status: OrderStatus::Acknowledged,
                        filled_qty: Quantity::zero(),
                        remaining_qty: req.quantity,
                        avg_fill_price: None,
                        timestamp_ns: normalize::now_ns(),
                    });
                    Ok(id)
                }
                OrderResponseStatus::Filled { oid, avg_px, .. } => {
                    let id = normalize::order_id_from_oid(oid);
                    let _ = self.update_tx.send(OrderUpdate {
                        instrument: req.instrument.clone(),
                        order_id: id.clone(),
                        status: OrderStatus::Filled,
                        filled_qty: req.quantity,
                        remaining_qty: Quantity::zero(),
                        avg_fill_price: Some(Price::new(avg_px)),
                        timestamp_ns: normalize::now_ns(),
                    });
                    Ok(id)
                }
                OrderResponseStatus::Success => Ok(cloids[0].to_string()),
                OrderResponseStatus::Error(msg) => Err(ConnectorError::OrderRejected(msg)),
            }
        }).collect()
    }

    async fn cancel_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
    ) -> Result<(), ConnectorError> {
        let oid = normalize::oid_from_order_id(order_id).map_err(ConnectorError::Other)?;

        self.http
            .cancel(
                &self.signer,
                BatchCancel { cancels: vec![Cancel { asset: self.market.index(), oid }] },
                self.nonce.next(),
                self.vault_address,
                None,
            )
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("cancel: {}", e)))?;

        let _ = self.update_tx.send(OrderUpdate {
            instrument: instrument.clone(),
            order_id: order_id.clone(),
            status: OrderStatus::Cancelled,
            filled_qty: Quantity::zero(),
            remaining_qty: Quantity::zero(),
            avg_fill_price: None,
            timestamp_ns: normalize::now_ns(),
        });
        Ok(())
    }

    /// Cancel all resting orders immediately using HL's `scheduleCancel(now)`.
    ///
    /// This is a single API call with no OID lookup — significantly faster than
    /// fetching open orders and cancelling individually.
    ///
    /// Note: `scheduleCancel` cancels ALL orders for this address/signer across all
    /// instruments. With a single strategy this is correct. With multiple strategies
    /// on different instruments, consider per-OID BatchCancel instead.
    async fn cancel_all(&self, _instrument: &InstrumentId) -> Result<(), ConnectorError> {
        use chrono::Utc;
        self.http
            .schedule_cancel(
                &self.signer,
                self.nonce.next(),
                Utc::now(), // time=now → immediate
                self.vault_address,
                None,
            )
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("schedule_cancel: {e}")))?;
        Ok(())
    }

    async fn modify_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
        new_price: Price,
        new_qty: Quantity,
    ) -> Result<OrderId, ConnectorError> {
        let oid = normalize::oid_from_order_id(order_id).map_err(ConnectorError::Other)?;

        let rounded_price = self
            .round_price(new_price.inner(), true) // conservative neutral rounding for modify
            .ok_or_else(|| ConnectorError::Other(anyhow::anyhow!("price rounding failed")))?;

        let rounded_size = self.round_size(new_qty.inner());
        let new_cloid = Cloid::random();

        let resp = self.http
            .modify(
                &self.signer,
                BatchModify {
                    modifies: vec![Modify {
                        oid: OidOrCloid::Left(oid),
                        order: HlOrderRequest {
                            asset: self.market.index(),
                            is_buy: true, // side must be passed — engine preserves original
                            limit_px: rounded_price,
                            sz: rounded_size,
                            reduce_only: false,
                            order_type: OrderTypePlacement::Limit { tif: HlTif::Alo },
                            cloid: new_cloid,
                        },
                    }],
                },
                self.nonce.next(),
                self.vault_address,
                None,
            )
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("modify: {}", e)))?;

        match resp.into_iter().next() {
            Some(OrderResponseStatus::Resting { oid: new_oid, .. })
            | Some(OrderResponseStatus::Filled { oid: new_oid, .. }) => {
                Ok(normalize::order_id_from_oid(new_oid))
            }
            Some(OrderResponseStatus::Error(msg)) => Err(ConnectorError::OrderRejected(msg)),
            _ => Err(ConnectorError::OrderRejected("unexpected modify response".into())),
        }
    }

    async fn positions(&self) -> Result<Vec<Position>, ConnectorError> {
        let state = self.http
            .clearinghouse_state(self.signer.address(), None)
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("clearinghouse_state: {e}")))?;

        let positions = state.asset_positions.into_iter()
            .filter(|ap| !ap.position.szi.is_zero())
            .map(|ap| {
                let coin = &ap.position.coin;
                let kind = if coin.starts_with('@') {
                    InstrumentKind::Spot
                } else {
                    InstrumentKind::Perpetual
                };
                Position {
                    instrument: InstrumentId::new(Exchange::Hyperliquid, kind, coin),
                    size: Quantity::new(ap.position.szi),
                    avg_entry_price: Price::new(ap.position.entry_px.unwrap_or(Decimal::ZERO)),
                    unrealized_pnl: Price::new(ap.position.unrealized_pnl),
                }
            })
            .collect();

        Ok(positions)
    }

    async fn open_orders(&self, _instrument: &InstrumentId) -> Result<Vec<OpenOrder>, ConnectorError> {
        let orders = self.http
            .open_orders(self.signer.address(), None)
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("open_orders: {e}")))?;

        let my_coin = self.market.mids_key();
        let open = orders.into_iter()
            .filter(|o| o.coin == my_coin)
            .map(|o| OpenOrder {
                order_id: o.oid.to_string(),
                instrument: self.instrument.clone(),
                side: if o.side == Side::Bid { OrderSide::Buy } else { OrderSide::Sell },
                price: Price::new(o.limit_px),
                quantity: Quantity::new(o.sz),
                filled_qty: Quantity::zero(),
            })
            .collect();

        Ok(open)
    }

    fn order_update_rx(&mut self) -> &mut mpsc::UnboundedReceiver<OrderUpdate> {
        &mut self.update_rx
    }
}
