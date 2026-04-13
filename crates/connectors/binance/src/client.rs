//! Binance USD-M Futures REST client implementing ExchangeConnector.
//!
//! Targets fapi.binance.com (production) or testnet.binancefuture.com.
//!
//! Authentication: HMAC-SHA256 signed query strings. API key in X-MBX-APIKEY header.
//! Post-only orders use timeInForce=GTX (Good Till Crossing — rejected if would cross).
//!
//! Batch placement uses /fapi/v1/batchOrders (up to 5 orders per call).

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use async_trait::async_trait;
use hex;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::mpsc;
use trading_core::{
    Price, Quantity,
    error::ConnectorError,
    traits::ExchangeConnector,
    types::{
        instrument::{Exchange, InstrumentId, InstrumentKind},
        order::{OpenOrder, OrderId, OrderRequest, OrderSide, OrderUpdate, TimeInForce},
        position::Position,
    },
};

use crate::normalize::{self, BatchOrderResult, OpenOrderResponse, PlaceOrderResponse, PositionRiskResponse};

type HmacSha256 = Hmac<Sha256>;

const MAINNET_REST: &str = "https://fapi.binance.com";
const TESTNET_REST: &str = "https://testnet.binancefuture.com";
const RECV_WINDOW: u64 = 5000;

// ── Client ────────────────────────────────────────────────────────────────────

pub struct BinanceClient {
    http: reqwest::Client,
    base_url: &'static str,
    api_key: String,
    secret: String,
    /// Resolved symbol, e.g. "ETHUSDT"
    symbol: String,
    instrument: InstrumentId,
    update_tx: mpsc::UnboundedSender<OrderUpdate>,
    update_rx: mpsc::UnboundedReceiver<OrderUpdate>,
}

impl BinanceClient {
    /// Create a client from environment variables.
    /// `api_key_env` and `secret_env` are the env var names (not values).
    pub fn from_env(
        api_key_env: &str,
        secret_env: &str,
        symbol: &str,
        testnet: bool,
    ) -> Result<Self, ConnectorError> {
        let api_key = std::env::var(api_key_env).map_err(|_| {
            ConnectorError::AuthFailed(format!("missing env var: {api_key_env}"))
        })?;
        let secret = std::env::var(secret_env).map_err(|_| {
            ConnectorError::AuthFailed(format!("missing env var: {secret_env}"))
        })?;

        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| ConnectorError::Other(anyhow::anyhow!("reqwest init: {e}")))?;

        let base_url = if testnet { TESTNET_REST } else { MAINNET_REST };
        let instrument = InstrumentId::new(Exchange::Binance, InstrumentKind::Perpetual, symbol);
        let (update_tx, update_rx) = mpsc::unbounded_channel();

        tracing::info!(symbol, testnet, "BinanceClient ready");

        Ok(Self {
            http,
            base_url,
            api_key: api_key.trim().to_string(),
            secret: secret.trim().to_string(),
            symbol: symbol.to_string(),
            instrument,
            update_tx,
            update_rx,
        })
    }

    pub fn instrument(&self) -> InstrumentId { self.instrument.clone() }
    pub fn api_key(&self) -> &str { &self.api_key }
    pub fn symbol(&self) -> &str { &self.symbol }
    pub fn testnet(&self) -> bool { self.base_url == TESTNET_REST }

    // ── Signing ───────────────────────────────────────────────────────────────

    fn sign(&self, data: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(self.secret.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(data.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    fn timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    // ── HTTP helpers ──────────────────────────────────────────────────────────

    async fn get_signed<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &str,
    ) -> anyhow::Result<T> {
        let ts = Self::timestamp_ms();
        let query = if params.is_empty() {
            format!("timestamp={ts}&recvWindow={RECV_WINDOW}")
        } else {
            format!("{params}&timestamp={ts}&recvWindow={RECV_WINDOW}")
        };
        let sig = self.sign(&query);
        let url = format!("{}/{}?{}&signature={}", self.base_url, path, query, sig);

        let resp = self.http.get(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send().await
            .context("GET request failed")?
            .error_for_status()
            .context("GET returned error status")?;

        resp.json().await.context("GET response parse failed")
    }

    async fn post_signed<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &str,
    ) -> anyhow::Result<T> {
        let ts = Self::timestamp_ms();
        let body_ts = format!("{body}&timestamp={ts}&recvWindow={RECV_WINDOW}");
        let sig = self.sign(&body_ts);
        let full_body = format!("{body_ts}&signature={sig}");
        let url = format!("{}/{}", self.base_url, path);

        let resp = self.http.post(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(full_body)
            .send().await
            .context("POST request failed")?
            .error_for_status()
            .context("POST returned error status")?;

        resp.json().await.context("POST response parse failed")
    }

    async fn delete_signed<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &str,
    ) -> anyhow::Result<T> {
        let ts = Self::timestamp_ms();
        let query = format!("{params}&timestamp={ts}&recvWindow={RECV_WINDOW}");
        let sig = self.sign(&query);
        let url = format!("{}/{}?{}&signature={}", self.base_url, path, query, sig);

        let resp = self.http.delete(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send().await
            .context("DELETE request failed")?
            .error_for_status()
            .context("DELETE returned error status")?;

        resp.json().await.context("DELETE response parse failed")
    }

    // ── Order helpers ─────────────────────────────────────────────────────────

    fn side_str(side: OrderSide) -> &'static str {
        match side { OrderSide::Buy => "BUY", OrderSide::Sell => "SELL" }
    }

    /// GTX = Good Till Crossing (post-only, rejected if it would take liquidity).
    fn tif_str(tif: TimeInForce) -> &'static str {
        match tif {
            TimeInForce::PostOnly => "GTX",
            TimeInForce::Gtc => "GTC",
            TimeInForce::Ioc => "IOC",
        }
    }

    fn build_order_params(&self, req: &OrderRequest) -> String {
        format!(
            "symbol={}&side={}&type=LIMIT&timeInForce={}&quantity={}&price={}",
            self.symbol,
            Self::side_str(req.side),
            Self::tif_str(req.tif),
            req.quantity.inner(),
            req.price.inner(),
        )
    }
}

// ── ExchangeConnector ─────────────────────────────────────────────────────────

#[async_trait]
impl ExchangeConnector for BinanceClient {
    fn exchange(&self) -> Exchange { Exchange::Binance }

    async fn place_order(&self, req: &OrderRequest) -> Result<OrderId, ConnectorError> {
        let body = self.build_order_params(req);

        let resp: PlaceOrderResponse = self.post_signed("fapi/v1/order", &body).await
            .map_err(|e| ConnectorError::Other(e))?;

        tracing::info!(
            symbol = %self.symbol,
            order_id = resp.order_id,
            status = %resp.status,
            side = ?req.side,
            price = %req.price.inner(),
            qty = %req.quantity.inner(),
            "BINANCE_PLACE"
        );

        let update = normalize::place_to_update(&self.instrument, &resp);
        let _ = self.update_tx.send(update);

        Ok(resp.order_id.to_string())
    }

    /// Batch place using /fapi/v1/batchOrders — up to 5 orders per call.
    /// Falls back to sequential place_order calls if batch > 5.
    async fn place_batch(&self, reqs: &[OrderRequest]) -> Vec<Result<OrderId, ConnectorError>> {
        if reqs.is_empty() {
            return vec![];
        }

        // Binance batch endpoint accepts up to 5. Split if needed.
        if reqs.len() > 5 {
            let mut all = Vec::with_capacity(reqs.len());
            for chunk in reqs.chunks(5) {
                let results = self.place_batch(chunk).await;
                all.extend(results);
            }
            return all;
        }

        // Serialize orders as JSON array for the batchOrders param.
        let orders_json: Vec<serde_json::Value> = reqs.iter().map(|req| {
            serde_json::json!({
                "symbol": self.symbol,
                "side": Self::side_str(req.side),
                "type": "LIMIT",
                "timeInForce": Self::tif_str(req.tif),
                "quantity": req.quantity.inner().to_string(),
                "price": req.price.inner().to_string(),
            })
        }).collect();

        let batch_str = serde_json::to_string(&orders_json).unwrap_or_default();
        // URL-encode the JSON array for the form body
        let encoded = urlencoding_simple(&batch_str);
        let body = format!("batchOrders={encoded}");

        let results: Vec<BatchOrderResult> = match self.post_signed("fapi/v1/batchOrders", &body).await {
            Ok(r) => r,
            Err(e) => {
                let err_str = e.to_string();
                return reqs.iter().map(|_| {
                    Err(ConnectorError::Other(anyhow::anyhow!("batch place: {err_str}")))
                }).collect();
            }
        };

        results.into_iter().zip(reqs.iter()).map(|(result, req)| {
            match result {
                BatchOrderResult::Ok(resp) => {
                    tracing::info!(
                        order_id = resp.order_id,
                        status = %resp.status,
                        side = ?req.side,
                        price = %req.price.inner(),
                        qty = %req.quantity.inner(),
                        "BINANCE_BATCH_ORDER"
                    );
                    let update = normalize::place_to_update(&self.instrument, &resp);
                    let _ = self.update_tx.send(update);
                    Ok(resp.order_id.to_string())
                }
                BatchOrderResult::Err { code, msg } => {
                    tracing::error!(code, %msg, "BINANCE_BATCH_ORDER_REJECTED");
                    Err(ConnectorError::OrderRejected(format!("code={code}: {msg}")))
                }
            }
        }).collect()
    }

    async fn cancel_order(
        &self,
        _instrument: &InstrumentId,
        order_id: &OrderId,
    ) -> Result<(), ConnectorError> {
        let params = format!("symbol={}&orderId={}", self.symbol, order_id);
        // Returns cancelled order details — we ignore them
        let _: serde_json::Value = self.delete_signed("fapi/v1/order", &params).await
            .map_err(|e| ConnectorError::Other(e))?;

        tracing::info!(order_id, "BINANCE_CANCEL");
        Ok(())
    }

    /// Cancel all open orders for this symbol in one call.
    async fn cancel_all(&self, _instrument: &InstrumentId) -> Result<(), ConnectorError> {
        let params = format!("symbol={}", self.symbol);
        let _: serde_json::Value = self.delete_signed("fapi/v1/allOpenOrders", &params).await
            .map_err(|e| ConnectorError::Other(e))?;

        tracing::info!(symbol = %self.symbol, "BINANCE_CANCEL_ALL");
        Ok(())
    }

    /// Modify via cancel + replace (Binance PUT /fapi/v1/order requires original OID).
    async fn modify_order(
        &self,
        instrument: &InstrumentId,
        order_id: &OrderId,
        new_price: Price,
        new_qty: Quantity,
    ) -> Result<OrderId, ConnectorError> {
        // Cancel the existing order first
        self.cancel_order(instrument, order_id).await?;

        // Re-place with new params (use PostOnly / GTX to stay maker)
        let body = format!(
            "symbol={}&side=BUY&type=LIMIT&timeInForce=GTX&quantity={}&price={}",
            self.symbol,
            new_qty.inner(),
            new_price.inner(),
        );
        let resp: PlaceOrderResponse = self.post_signed("fapi/v1/order", &body).await
            .map_err(|e| ConnectorError::Other(e))?;

        tracing::info!(old_order_id = %order_id, new_order_id = resp.order_id, "BINANCE_MODIFY");
        Ok(resp.order_id.to_string())
    }

    async fn positions(&self) -> Result<Vec<Position>, ConnectorError> {
        let params = format!("symbol={}", self.symbol);
        let resp: Vec<PositionRiskResponse> = self.get_signed("fapi/v2/positionRisk", &params).await
            .map_err(|e| ConnectorError::Other(e))?;

        Ok(resp.iter()
            .filter_map(|p| normalize::position_from_risk(&self.instrument, p))
            .collect())
    }

    async fn open_orders(&self, _instrument: &InstrumentId) -> Result<Vec<OpenOrder>, ConnectorError> {
        let params = format!("symbol={}", self.symbol);
        let resp: Vec<OpenOrderResponse> = self.get_signed("fapi/v1/openOrders", &params).await
            .map_err(|e| ConnectorError::Other(e))?;

        Ok(resp.iter()
            .map(|o| normalize::open_order_from_rest(&self.instrument, o))
            .collect())
    }

    fn order_update_rx(&mut self) -> &mut mpsc::UnboundedReceiver<OrderUpdate> {
        &mut self.update_rx
    }
}

// ── URL encoding helper ───────────────────────────────────────────────────────

/// Minimal percent-encoding for the batchOrders JSON param value.
/// Only encodes characters that break form-encoded bodies.
fn urlencoding_simple(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' | b',' | b':' | b'"' => {
                out.push(byte as char);
            }
            other => {
                out.push('%');
                out.push_str(&format!("{other:02X}"));
            }
        }
    }
    out
}
