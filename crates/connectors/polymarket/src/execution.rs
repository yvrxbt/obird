//! Polymarket CLOB execution client — order placement, cancellation, position queries.
//!
//! ## Auth model
//!
//! Two auth layers:
//! 1. **REST auth (HMAC)** — per-request headers derived from `POLY_API_KEY`,
//!    `POLY_SECRET`, `POLY_PASSPHRASE`. Handled transparently by the SDK.
//! 2. **Order signing (EIP-712)** — per-order signature created with the Ethereum
//!    private key (`PREDICT_PRIVATE_KEY`). Required for every order placed.
//!
//! ## Order semantics
//!
//! All hedge orders are **BUY** side. The instrument `symbol` field is the
//! Polymarket CLOB token ID (large decimal string, e.g. `"8501497..."`).
//!
//! `place_order` places a GTC limit order at the requested price. The hedge
//! strategy is responsible for choosing a price that falls within the CLOB's
//! tick constraints (0.01 for most markets).
//!
//! ## Order tracking
//!
//! Active order IDs are tracked in `active_orders` (order_hash → InstrumentId).
//! `cancel_all` cancels only our tracked orders (not account-wide), matching
//! the predict.fun connector's behaviour.

use std::collections::HashMap;
use std::str::FromStr as _;
use std::sync::Mutex;

use alloy::signers::Signer as _;
use alloy::signers::local::PrivateKeySigner;
use async_trait::async_trait;
use polymarket_client_sdk::{
    POLYGON,
    auth::{Normal, state::Authenticated},
    clob::{Client, Config},
    clob::types::{OrderType, Side},
    types::U256,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::mpsc;
use trading_core::{
    error::ConnectorError,
    traits::ExchangeConnector,
    types::{
        decimal::{Price, Quantity},
        instrument::{Exchange, InstrumentId},
        order::{OpenOrder, OrderId, OrderRequest, OrderSide, OrderUpdate},
        position::Position,
    },
};

const CLOB_HOST: &str = "https://clob.polymarket.com";
const MIN_PRICE: Decimal = dec!(0.01);
const MAX_PRICE: Decimal = dec!(0.99);

// ── Client ────────────────────────────────────────────────────────────────────

/// Polymarket CLOB execution client.
///
/// One instance manages all Polymarket hedge orders across all markets.
///
/// **Auth**: API credentials are derived deterministically from `PREDICT_PRIVATE_KEY`
/// using the SDK's `create_or_derive_api_key` flow. No separate `POLY_API_KEY` /
/// `POLY_SECRET` / `POLY_PASSPHRASE` env vars needed — the private key is sufficient.
///
/// This is the recommended auth path: the key is tied to the wallet address and
/// recreated automatically if it doesn't exist yet.
pub struct PolymarketExecutionClient {
    client: Client<Authenticated<Normal>>,
    signer: PrivateKeySigner,
    active_orders: Mutex<HashMap<String, InstrumentId>>,
    update_tx: mpsc::UnboundedSender<OrderUpdate>,
    update_rx: mpsc::UnboundedReceiver<OrderUpdate>,
}

impl PolymarketExecutionClient {
    /// Construct from a single private key env var.
    ///
    /// The Polymarket API key is derived (or created) from `PREDICT_PRIVATE_KEY`
    /// using the SDK's `create_or_derive_api_key` flow. No separate `POLY_API_KEY` /
    /// `POLY_SECRET` / `POLY_PASSPHRASE` needed.
    pub async fn from_env(private_key_env: &str) -> Result<Self, ConnectorError> {
        let private_key = std::env::var(private_key_env)
            .map_err(|_| {
                ConnectorError::AuthFailed(format!("missing env var: {private_key_env}"))
            })?;

        let signer = PrivateKeySigner::from_str(&private_key)
            .map_err(|e| ConnectorError::AuthFailed(format!("invalid private key: {e}")))?
            .with_chain_id(Some(POLYGON));

        let config = Config::builder().use_server_time(true).build();
        let client = Client::new(CLOB_HOST, config)
            .map_err(|e| ConnectorError::AuthFailed(format!("polymarket client init: {e}")))?
            .authentication_builder(&signer)
            // No .credentials() — derive from private key automatically
            .authenticate()
            .await
            .map_err(|e| ConnectorError::AuthFailed(format!("polymarket authenticate: {e}")))?;

        let address = signer.address();
        let (update_tx, update_rx) = mpsc::unbounded_channel();

        tracing::info!(
            address = %address,
            "PolymarketExecutionClient ready",
        );

        Ok(Self {
            client,
            signer,
            active_orders: Mutex::new(HashMap::new()),
            update_tx,
            update_rx,
        })
    }

    /// Parse the CLOB token ID (decimal string) from an instrument's symbol.
    fn token_id_u256(instrument: &InstrumentId) -> Result<U256, ConnectorError> {
        U256::from_str(&instrument.symbol).map_err(|e| {
            ConnectorError::OrderRejected(format!(
                "instrument symbol '{}' is not a valid U256 token ID: {e}",
                instrument.symbol
            ))
        })
    }
}

// ── ExchangeConnector impl ────────────────────────────────────────────────────

#[async_trait]
impl ExchangeConnector for PolymarketExecutionClient {
    fn exchange(&self) -> Exchange {
        Exchange::Polymarket
    }

    /// Place a GTC limit order on the Polymarket CLOB.
    ///
    /// `req.instrument.symbol` must be a valid Polymarket CLOB token ID
    /// (large decimal string). Price must be in [0.01, 0.99].
    async fn place_order(&self, req: &OrderRequest) -> Result<OrderId, ConnectorError> {
        let p = req.price.inner();
        if p < MIN_PRICE || p > MAX_PRICE {
            return Err(ConnectorError::OrderRejected(format!(
                "price {p} out of bounds [{MIN_PRICE}, {MAX_PRICE}]"
            )));
        }

        let token_id = Self::token_id_u256(&req.instrument)?;

        let side = match req.side {
            OrderSide::Buy => Side::Buy,
            OrderSide::Sell => Side::Sell,
        };

        let signable = self
            .client
            .limit_order()
            .token_id(token_id)
            .order_type(OrderType::GTC)
            .price(p)
            .size(req.quantity.inner())
            .side(side)
            .build()
            .await
            .map_err(|e| ConnectorError::OrderRejected(format!("build limit order: {e}")))?;

        let signed = self
            .client
            .sign(&self.signer, signable)
            .await
            .map_err(|e| ConnectorError::OrderRejected(format!("sign order: {e}")))?;

        let resp = self
            .client
            .post_order(signed)
            .await
            .map_err(|e| ConnectorError::OrderRejected(format!("post_order: {e}")))?;

        if !resp.success {
            return Err(ConnectorError::OrderRejected(format!(
                "post_order rejected: {:?}",
                resp.error_msg
            )));
        }

        let order_id = resp.order_id.clone();
        self.active_orders
            .lock()
            .unwrap()
            .insert(order_id.clone(), req.instrument.clone());

        tracing::info!(
            instrument = %req.instrument,
            order_id   = %order_id,
            side       = ?req.side,
            price      = %req.price,
            qty        = %req.quantity,
            status     = ?resp.status,
            "POLY_PLACE",
        );

        Ok(order_id)
    }

    async fn cancel_order(
        &self,
        _instrument: &InstrumentId,
        order_id: &OrderId,
    ) -> Result<(), ConnectorError> {
        self.client
            .cancel_order(order_id.as_str())
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("cancel_order: {e}")))?;

        self.active_orders.lock().unwrap().remove(order_id);
        tracing::debug!(order_id = %order_id, "POLY_CANCEL OK");
        Ok(())
    }

    /// Cancel all orders placed this session.
    async fn cancel_all(&self, _instrument: &InstrumentId) -> Result<(), ConnectorError> {
        let ids: Vec<String> = self
            .active_orders
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();

        if ids.is_empty() {
            return Ok(());
        }

        let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        self.client
            .cancel_orders(&id_refs)
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("cancel_all: {e}")))?;

        self.active_orders.lock().unwrap().clear();
        tracing::info!(n = ids.len(), "POLY_CANCEL_ALL");
        Ok(())
    }

    async fn modify_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
        new_price: Price,
        new_qty: Quantity,
    ) -> Result<OrderId, ConnectorError> {
        self.cancel_order(instrument, order_id).await?;
        self.place_order(&trading_core::types::order::OrderRequest {
            instrument: instrument.clone(),
            side: OrderSide::Buy,
            price: new_price,
            quantity: new_qty,
            tif: trading_core::types::order::TimeInForce::Gtc,
            client_order_id: None,
        })
        .await
    }

    async fn positions(&self) -> Result<Vec<Position>, ConnectorError> {
        Ok(vec![])
    }

    async fn open_orders(
        &self,
        instrument: &InstrumentId,
    ) -> Result<Vec<OpenOrder>, ConnectorError> {
        let map = self.active_orders.lock().unwrap();
        let orders = map
            .iter()
            .filter(|(_, inst)| *inst == instrument)
            .map(|(id, inst)| OpenOrder {
                order_id: id.clone(),
                instrument: inst.clone(),
                side: OrderSide::Buy,
                price: Price::zero(),
                quantity: Quantity::zero(),
                filled_qty: Quantity::zero(),
            })
            .collect();
        Ok(orders)
    }

    fn order_update_rx(&mut self) -> &mut mpsc::UnboundedReceiver<OrderUpdate> {
        &mut self.update_rx
    }

    fn decimal_precision(&self, _instrument: &InstrumentId) -> Option<u32> {
        // Polymarket CLOB: most markets use 0.01 ticks (precision=2), but some
        // markets advertise 0.001 via the Gamma API's `orderPriceMinTickSize`.
        // Today Polymarket is only wired as a hedge/FV leg (never the quoting
        // venue), so the value is unused — we return the CLOB default. If a
        // future strategy quotes maker on Polymarket, populate a
        // `HashMap<InstrumentId, u32>` from Gamma at connection setup and look
        // up here by the `instrument` arg.
        Some(2)
    }
}
