//! Hyperliquid API client and ExchangeConnector implementation.

use std::str::FromStr;
use std::sync::Arc;

use chrono::Utc;
use ethers::signers::{LocalWallet, Signer};
use ethers::types::H160;
use hyperliquid_sdk::{
    BaseUrl, ClientCancelRequest, ClientLimit, ClientModifyRequest, ClientOrder, ClientOrderRequest,
    ExchangeClient, ExchangeDataStatus, ExchangeResponseStatus, InfoClient,
};
use rust_decimal::Decimal;
use tokio::sync::{mpsc, Mutex};
use trading_core::error::ConnectorError;
use trading_core::traits::ExchangeConnector;
use trading_core::types::decimal::{Price, Quantity};
use trading_core::types::instrument::{Exchange, InstrumentId, InstrumentKind};
use trading_core::types::order::{
    OpenOrder, OrderId, OrderRequest, OrderSide, OrderStatus, OrderUpdate, TimeInForce,
};
use trading_core::types::position::Position;

/// Hyperliquid connector backed by `hyperliquid_sdk`.
pub struct HyperliquidClient {
    exchange: Arc<Mutex<ExchangeClient>>,
    info: Arc<InfoClient>,
    wallet_address: H160,
    update_tx: mpsc::UnboundedSender<OrderUpdate>,
    update_rx: mpsc::UnboundedReceiver<OrderUpdate>,
}

impl HyperliquidClient {
    /// Build a connector from a private key in env var (hex string, no 0x required).
    pub async fn from_env(private_key_env: &str, testnet: bool) -> Result<Self, ConnectorError> {
        let private_key = std::env::var(private_key_env)
            .map_err(|_| ConnectorError::AuthFailed(format!("missing env var: {private_key_env}")))?;

        let wallet = LocalWallet::from_str(private_key.trim_start_matches("0x"))
            .map_err(|e| ConnectorError::AuthFailed(format!("invalid private key: {e}")))?;
        Self::new(wallet, testnet).await
    }

    /// Build a connector from a wallet.
    pub async fn new(wallet: LocalWallet, testnet: bool) -> Result<Self, ConnectorError> {
        let base = if testnet {
            BaseUrl::Testnet
        } else {
            BaseUrl::Mainnet
        };

        let info = InfoClient::new(None, Some(base))
            .await
            .map_err(|e| ConnectorError::ConnectionFailed(e.to_string()))?;
        let exchange = ExchangeClient::new(None, wallet.clone(), Some(base), None, None)
            .await
            .map_err(|e| ConnectorError::ConnectionFailed(e.to_string()))?;

        let (update_tx, update_rx) = mpsc::unbounded_channel();

        Ok(Self {
            exchange: Arc::new(Mutex::new(exchange)),
            info: Arc::new(info),
            wallet_address: wallet.address(),
            update_tx,
            update_rx,
        })
    }

    fn now_ns() -> u64 {
        Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
            .max(0) as u64
    }

    fn parse_decimal(value: &str) -> Result<Decimal, ConnectorError> {
        Decimal::from_str(value)
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("decimal parse failed: {e}")))
    }

    fn to_hl_tif(tif: TimeInForce) -> String {
        match tif {
            TimeInForce::Gtc => "Gtc".to_string(),
            TimeInForce::Ioc => "Ioc".to_string(),
            TimeInForce::PostOnly => "Alo".to_string(),
        }
    }

    fn to_hl_side(side: OrderSide) -> bool {
        matches!(side, OrderSide::Buy)
    }

    fn to_core_side(side: &str) -> OrderSide {
        if side.eq_ignore_ascii_case("b") || side.eq_ignore_ascii_case("buy") {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        }
    }

    fn parse_oid(status: &ExchangeDataStatus) -> Option<u64> {
        match status {
            ExchangeDataStatus::Resting(order) => Some(order.oid),
            ExchangeDataStatus::Filled(order) => Some(order.oid),
            _ => None,
        }
    }

    fn to_instrument(symbol: &str) -> InstrumentId {
        InstrumentId::new(Exchange::Hyperliquid, InstrumentKind::Perpetual, symbol)
    }
}

#[async_trait::async_trait]
impl ExchangeConnector for HyperliquidClient {
    fn exchange(&self) -> Exchange {
        Exchange::Hyperliquid
    }

    async fn place_order(&self, req: &OrderRequest) -> Result<OrderId, ConnectorError> {
        let order = ClientOrderRequest {
            asset: req.instrument.symbol.clone(),
            is_buy: Self::to_hl_side(req.side),
            reduce_only: false,
            limit_px: req.price.inner().to_string().parse::<f64>().map_err(|e| {
                ConnectorError::Other(anyhow::anyhow!("price conversion failed: {e}"))
            })?,
            sz: req.quantity.inner().to_string().parse::<f64>().map_err(|e| {
                ConnectorError::Other(anyhow::anyhow!("quantity conversion failed: {e}"))
            })?,
            cloid: None,
            order_type: ClientOrder::Limit(ClientLimit {
                tif: Self::to_hl_tif(req.tif),
            }),
        };

        let response = self
            .exchange
            .lock()
            .await
            .order(order, None)
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("order failed: {e}")))?;

        let status = match response {
            ExchangeResponseStatus::Ok(ok) => ok
                .data
                .and_then(|d| d.statuses.into_iter().next())
                .ok_or_else(|| ConnectorError::OrderRejected("empty order status".to_string()))?,
            ExchangeResponseStatus::Err(err) => {
                return Err(ConnectorError::OrderRejected(err));
            }
        };

        let order_id = Self::parse_oid(&status)
            .ok_or_else(|| ConnectorError::OrderRejected(format!("unexpected status: {status:?}")))?
            .to_string();

        let update = match status {
            ExchangeDataStatus::Resting(_) => OrderUpdate {
                instrument: req.instrument.clone(),
                order_id: order_id.clone(),
                status: OrderStatus::Acknowledged,
                filled_qty: Quantity::zero(),
                remaining_qty: req.quantity,
                avg_fill_price: None,
                timestamp_ns: Self::now_ns(),
            },
            ExchangeDataStatus::Filled(f) => {
                let filled = Self::parse_decimal(&f.total_sz)?;
                let avg = Self::parse_decimal(&f.avg_px)?;
                OrderUpdate {
                    instrument: req.instrument.clone(),
                    order_id: order_id.clone(),
                    status: OrderStatus::Filled,
                    filled_qty: Quantity::new(filled),
                    remaining_qty: Quantity::zero(),
                    avg_fill_price: Some(Price::new(avg)),
                    timestamp_ns: Self::now_ns(),
                }
            }
            _ => OrderUpdate {
                instrument: req.instrument.clone(),
                order_id: order_id.clone(),
                status: OrderStatus::Acknowledged,
                filled_qty: Quantity::zero(),
                remaining_qty: req.quantity,
                avg_fill_price: None,
                timestamp_ns: Self::now_ns(),
            },
        };

        let _ = self.update_tx.send(update);
        Ok(order_id)
    }

    async fn cancel_order(&self, instrument: &InstrumentId, order_id: &OrderId) -> Result<(), ConnectorError> {
        let oid = order_id
            .parse::<u64>()
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("invalid oid: {e}")))?;

        self.exchange
            .lock()
            .await
            .cancel(
                ClientCancelRequest {
                    asset: instrument.symbol.clone(),
                    oid,
                },
                None,
            )
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("cancel failed: {e}")))?;

        let _ = self.update_tx.send(OrderUpdate {
            instrument: instrument.clone(),
            order_id: order_id.clone(),
            status: OrderStatus::Cancelled,
            filled_qty: Quantity::zero(),
            remaining_qty: Quantity::zero(),
            avg_fill_price: None,
            timestamp_ns: Self::now_ns(),
        });

        Ok(())
    }

    async fn cancel_all(&self, instrument: &InstrumentId) -> Result<(), ConnectorError> {
        let open = self
            .info
            .open_orders(self.wallet_address)
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("open_orders failed: {e}")))?;

        for order in open.into_iter().filter(|o| o.coin == instrument.symbol) {
            self.cancel_order(instrument, &order.oid.to_string()).await?;
        }

        Ok(())
    }

    async fn modify_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
        new_price: Price,
        new_qty: Quantity,
    ) -> Result<OrderId, ConnectorError> {
        let oid = order_id
            .parse::<u64>()
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("invalid oid: {e}")))?;

        let req = ClientOrderRequest {
            asset: instrument.symbol.clone(),
            is_buy: true,
            reduce_only: false,
            limit_px: new_price.inner().to_string().parse::<f64>().map_err(|e| {
                ConnectorError::Other(anyhow::anyhow!("price conversion failed: {e}"))
            })?,
            sz: new_qty.inner().to_string().parse::<f64>().map_err(|e| {
                ConnectorError::Other(anyhow::anyhow!("quantity conversion failed: {e}"))
            })?,
            cloid: None,
            order_type: ClientOrder::Limit(ClientLimit {
                tif: "Gtc".to_string(),
            }),
        };

        let response = self
            .exchange
            .lock()
            .await
            .modify(ClientModifyRequest { oid, order: req }, None)
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("modify failed: {e}")))?;

        let status = match response {
            ExchangeResponseStatus::Ok(ok) => ok
                .data
                .and_then(|d| d.statuses.into_iter().next())
                .ok_or_else(|| ConnectorError::OrderRejected("empty modify status".to_string()))?,
            ExchangeResponseStatus::Err(err) => {
                return Err(ConnectorError::OrderRejected(err));
            }
        };

        let new_order_id = Self::parse_oid(&status)
            .ok_or_else(|| ConnectorError::OrderRejected(format!("unexpected status: {status:?}")))?
            .to_string();

        Ok(new_order_id)
    }

    async fn positions(&self) -> Result<Vec<Position>, ConnectorError> {
        let state = self
            .info
            .user_state(self.wallet_address)
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("user_state failed: {e}")))?;

        let mut positions = Vec::with_capacity(state.asset_positions.len());
        for asset in state.asset_positions {
            let sz = Self::parse_decimal(&asset.position.szi)?;
            let entry = asset
                .position
                .entry_px
                .as_deref()
                .map(Self::parse_decimal)
                .transpose()?
                .unwrap_or(Decimal::ZERO);
            let upl = Self::parse_decimal(&asset.position.unrealized_pnl)?;

            positions.push(Position {
                instrument: Self::to_instrument(&asset.position.coin),
                size: Quantity::new(sz),
                avg_entry_price: Price::new(entry),
                unrealized_pnl: Price::new(upl),
            });
        }

        Ok(positions)
    }

    async fn open_orders(&self, instrument: &InstrumentId) -> Result<Vec<OpenOrder>, ConnectorError> {
        let orders = self
            .info
            .open_orders(self.wallet_address)
            .await
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("open_orders failed: {e}")))?;

        let mut out = Vec::new();
        for order in orders.into_iter().filter(|o| o.coin == instrument.symbol) {
            out.push(OpenOrder {
                order_id: order.oid.to_string(),
                instrument: instrument.clone(),
                side: Self::to_core_side(&order.side),
                price: Price::new(Self::parse_decimal(&order.limit_px)?),
                quantity: Quantity::new(Self::parse_decimal(&order.sz)?),
                filled_qty: Quantity::zero(),
            });
        }

        Ok(out)
    }

    fn order_update_rx(&mut self) -> &mut mpsc::UnboundedReceiver<OrderUpdate> {
        &mut self.update_rx
    }
}
