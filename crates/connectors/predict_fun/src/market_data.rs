//! predict.fun WebSocket market data feed — background task.
//!
//! Subscribes to:
//!   `predictOrderbook/{market_id}` — full YES orderbook snapshots on every change.
//!   `predictWalletEvents/{jwt}`    — per-order lifecycle events.
//!
//! ## Orderbook semantics
//! The WS topic returns the YES-outcome orderbook. The strategy derives NO prices
//! as `1 - YES_price` (binary market identity). Both instruments share the same
//! mid-price signal; the feed publishes `BookUpdate` only to the YES instrument.
//! The strategy subscribes to the YES instrument for book-driven requoting.
//!
//! ## Fill routing
//! Wallet events carry an `order_hash`. The feed looks up `active_orders[hash].instrument`
//! to publish the fill to the correct outcome instrument (YES or NO).
//!
//! ## JWT refresh
//! On `invalid_credentials` from the server the feed re-authenticates, writes the new
//! JWT to the shared `Arc<RwLock<String>>`, and reconnects.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use predict_sdk::{
    websocket::{
        parse_orderbook_update, parse_wallet_event, PredictWebSocket, PredictWsStream, WsMessage,
    },
    PredictClient, PredictWalletEvent,
};
use rust_decimal::Decimal;
use tokio::sync::{mpsc, RwLock};
use trading_core::{
    types::{
        decimal::{Price, Quantity},
        instrument::InstrumentId,
        order::{OrderSide, OrderStatus, OrderUpdate},
        position::Fill,
    },
    Event, MarketDataSink,
};

use crate::{client::OrderEntry, normalize};

const WS_URL: &str = "wss://ws.predict.fun/ws";

/// WebSocket feed for a single predict.fun market (covers both YES and NO outcomes).
pub struct PredictFunMarketDataFeed {
    inner: Arc<PredictClient>,
    market_id: u64,
    /// YES instrument — receives `BookUpdate` events (primary orderbook signal).
    yes_instrument: InstrumentId,
    /// NO instrument — receives `Fill` and `OrderUpdate` events only.
    no_instrument: InstrumentId,
    jwt: Arc<RwLock<String>>,
    /// Shared with client — cleared on cancel_all (active order tracking).
    active_orders: Arc<Mutex<HashMap<String, OrderEntry>>>,
    /// Shared with client — survives cancel_all so late fills resolve correctly.
    placed_instruments: Arc<Mutex<HashMap<String, InstrumentId>>>,
    update_tx: mpsc::UnboundedSender<OrderUpdate>,
}

impl PredictFunMarketDataFeed {
    pub fn new(
        inner: Arc<PredictClient>,
        market_id: u64,
        yes_instrument: InstrumentId,
        no_instrument: InstrumentId,
        jwt: Arc<RwLock<String>>,
        active_orders: Arc<Mutex<HashMap<String, OrderEntry>>>,
        placed_instruments: Arc<Mutex<HashMap<String, InstrumentId>>>,
        update_tx: mpsc::UnboundedSender<OrderUpdate>,
    ) -> Self {
        Self {
            inner,
            market_id,
            yes_instrument,
            no_instrument,
            jwt,
            active_orders,
            placed_instruments,
            update_tx,
        }
    }

    /// Convenience constructor from a `PredictFunClient`.
    pub fn from_client(client: &crate::client::PredictFunClient) -> Self {
        let (yes, no) = client.instruments();
        Self::new(
            client.inner(),
            client.market_id(),
            yes,
            no,
            client.jwt.clone(),
            client.active_orders(),
            client.placed_instruments(),
            client.update_tx(),
        )
    }

    /// Run forever; reconnects (with JWT refresh if needed) on any error.
    pub async fn run(self, sink: Arc<dyn MarketDataSink>) {
        loop {
            match self.run_once(sink.clone()).await {
                Ok(()) => tracing::warn!(
                    market_id = self.market_id,
                    "PredictFun WS ended cleanly, reconnecting",
                ),
                Err(e) => tracing::error!(
                    market_id = self.market_id,
                    error = %e,
                    "PredictFun WS error, reconnecting in 2s",
                ),
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }

    async fn run_once(&self, sink: Arc<dyn MarketDataSink>) -> anyhow::Result<()> {
        let jwt = self.jwt.read().await.clone();

        let ws = PredictWebSocket::new(WS_URL.to_string());
        let mut stream: PredictWsStream = ws
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("WS connect: {e}"))?;

        ws.subscribe_orderbook(self.market_id)
            .await
            .map_err(|e| anyhow::anyhow!("subscribe orderbook: {e}"))?;
        ws.subscribe_wallet_events(&jwt)
            .await
            .map_err(|e| anyhow::anyhow!("subscribe wallet events: {e}"))?;

        tracing::info!(
            market_id = self.market_id,
            yes = %self.yes_instrument,
            no  = %self.no_instrument,
            "PredictFun WS feed connected",
        );

        loop {
            match stream.next().await {
                None => return Err(anyhow::anyhow!("WS stream closed")),
                Some(Err(e)) => return Err(anyhow::anyhow!("WS stream error: {e}")),

                Some(Ok(WsMessage::RequestResponse(resp))) => {
                    if !resp.success {
                        let code = resp
                            .error
                            .as_ref()
                            .map(|e| e.code.as_str())
                            .unwrap_or("unknown");
                        if code == "invalid_credentials" {
                            tracing::warn!("JWT expired — re-authenticating predict.fun");
                            let new_jwt = self
                                .inner
                                .authenticate_and_store()
                                .await
                                .map_err(|e| anyhow::anyhow!("re-auth: {e}"))?;
                            *self.jwt.write().await = new_jwt;
                            return Err(anyhow::anyhow!("JWT refreshed, reconnecting"));
                        }
                        tracing::warn!(
                            request_id = resp.request_id,
                            code,
                            "WS subscription failed"
                        );
                    }
                }

                Some(Ok(WsMessage::PushMessage(push))) => {
                    let local_ts = normalize::now_ns();

                    if push.is_orderbook() {
                        match parse_orderbook_update(&push) {
                            Ok(book) => {
                                let exchange_ts = book.timestamp.unwrap_or(0) * 1_000_000;
                                let snap = normalize::orderbook_to_snapshot(&book, local_ts);

                                tracing::debug!(
                                    target: "md",
                                    instrument = %self.yes_instrument,
                                    best_bid = ?snap.best_bid().map(|(p,_)| p.inner()),
                                    best_ask = ?snap.best_ask().map(|(p,_)| p.inner()),
                                    feed_latency_us = local_ts.saturating_sub(exchange_ts) / 1_000,
                                    "BOOK",
                                );

                                // Publish to YES instrument only — strategy derives NO prices.
                                sink.publish(
                                    &self.yes_instrument,
                                    Event::BookUpdate {
                                        instrument: self.yes_instrument.clone(),
                                        book: snap,
                                        exchange_ts_ns: exchange_ts,
                                        local_ts_ns: local_ts,
                                    },
                                );
                            }
                            Err(e) => tracing::warn!("orderbook parse error: {e}"),
                        }
                    } else if push.is_wallet_event() {
                        match parse_wallet_event(&push) {
                            Ok(ev) => self.handle_wallet_event(ev, local_ts, &sink),
                            Err(e) => tracing::warn!("wallet event parse error: {e}"),
                        }
                    }
                }
            }
        }
    }

    /// Resolve which instrument an order belongs to.
    ///
    /// Checks `placed_instruments` first (survives cancel_all), then falls back to
    /// `active_orders`. Unknown hashes are ignored (wallet stream can include
    /// events from other markets/orders not owned by this connector instance).
    fn instrument_for(&self, order_hash: &str) -> Option<InstrumentId> {
        // placed_instruments is the authoritative source — it is only cleared on fill/expiry.
        if let Some(inst) = self
            .placed_instruments
            .lock()
            .unwrap()
            .get(order_hash)
            .cloned()
        {
            return Some(inst);
        }
        // Fallback: active_orders (may have been cleared by cancel_all).
        if let Some(entry) = self.active_orders.lock().unwrap().get(order_hash).cloned() {
            return Some(entry.instrument);
        }
        tracing::warn!(hash = %order_hash, "instrument_for: unknown order hash, ignoring wallet event");
        None
    }

    fn remove_from_active(&self, order_hash: &str) {
        self.active_orders.lock().unwrap().remove(order_hash);
        // NOTE: do NOT remove from placed_instruments here.
        // The exchange sometimes sends duplicate fill events for the same hash
        // within milliseconds. Keeping placed_instruments intact means the second
        // event still resolves to the correct instrument instead of defaulting to YES
        // and corrupting position tracking. The map is small (one entry per order
        // placed this session) so growth is negligible.
    }

    fn handle_wallet_event(
        &self,
        event: PredictWalletEvent,
        ts_ns: u64,
        sink: &Arc<dyn MarketDataSink>,
    ) {
        match event {
            PredictWalletEvent::OrderAccepted { order_hash, .. } => {
                tracing::debug!(hash = %order_hash, "order accepted");
                let Some(instrument) = self.instrument_for(&order_hash) else {
                    return;
                };
                let _ = self.update_tx.send(OrderUpdate {
                    instrument,
                    order_id: order_hash,
                    status: OrderStatus::Acknowledged,
                    filled_qty: Quantity::zero(),
                    remaining_qty: Quantity::zero(),
                    avg_fill_price: None,
                    timestamp_ns: ts_ns,
                });
            }

            PredictWalletEvent::OrderNotAccepted {
                order_hash, reason, ..
            } => {
                tracing::warn!(hash = %order_hash, reason = ?reason, "order not accepted");
                let Some(instrument) = self.instrument_for(&order_hash) else {
                    return;
                };
                self.remove_from_active(&order_hash);
                let _ = self.update_tx.send(OrderUpdate {
                    instrument,
                    order_id: order_hash,
                    status: OrderStatus::Rejected,
                    filled_qty: Quantity::zero(),
                    remaining_qty: Quantity::zero(),
                    avg_fill_price: None,
                    timestamp_ns: ts_ns,
                });
            }

            PredictWalletEvent::OrderTransactionSuccess {
                order_hash,
                details,
                ..
            } => {
                tracing::info!(hash = %order_hash, "order filled");
                let Some(instrument) = self.instrument_for(&order_hash) else {
                    return;
                };
                self.remove_from_active(&order_hash);

                let fill_price: Decimal = details
                    .price
                    .as_deref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(Decimal::ZERO);
                let fill_qty: Decimal = details
                    .quantity_filled
                    .as_deref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(Decimal::ZERO);
                let side = match details.quote_type.as_deref() {
                    Some("ASK") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                sink.publish(
                    &instrument,
                    Event::Fill {
                        instrument: instrument.clone(),
                        fill: Fill {
                            order_id: order_hash.clone(),
                            instrument: instrument.clone(),
                            side,
                            price: Price::new(fill_price),
                            quantity: Quantity::new(fill_qty),
                            fee: Price::zero(),
                            timestamp_ns: ts_ns,
                        },
                    },
                );
                let _ = self.update_tx.send(OrderUpdate {
                    instrument,
                    order_id: order_hash,
                    status: OrderStatus::Filled,
                    filled_qty: Quantity::new(fill_qty),
                    remaining_qty: Quantity::zero(),
                    avg_fill_price: Some(Price::new(fill_price)),
                    timestamp_ns: ts_ns,
                });
            }

            PredictWalletEvent::OrderTransactionFailed { order_hash, .. } => {
                tracing::warn!(hash = %order_hash, "order tx failed (order may still be resting)");
            }

            PredictWalletEvent::OrderCancelled { order_hash, .. } => {
                tracing::debug!(hash = %order_hash, "order cancelled");
                let Some(instrument) = self.instrument_for(&order_hash) else {
                    return;
                };
                self.remove_from_active(&order_hash);
                let _ = self.update_tx.send(OrderUpdate {
                    instrument,
                    order_id: order_hash,
                    status: OrderStatus::Cancelled,
                    filled_qty: Quantity::zero(),
                    remaining_qty: Quantity::zero(),
                    avg_fill_price: None,
                    timestamp_ns: ts_ns,
                });
            }

            PredictWalletEvent::OrderExpired { order_hash, .. } => {
                tracing::debug!(hash = %order_hash, "order expired");
                let Some(instrument) = self.instrument_for(&order_hash) else {
                    return;
                };
                self.remove_from_active(&order_hash);
                let _ = self.update_tx.send(OrderUpdate {
                    instrument,
                    order_id: order_hash,
                    status: OrderStatus::Cancelled,
                    filled_qty: Quantity::zero(),
                    remaining_qty: Quantity::zero(),
                    avg_fill_price: None,
                    timestamp_ns: ts_ns,
                });
            }

            PredictWalletEvent::OrderTransactionSubmitted { order_hash, .. } => {
                tracing::debug!(hash = %order_hash, "order tx submitted");
            }

            PredictWalletEvent::Unknown { event_type, .. } => {
                tracing::debug!(event_type, "unknown wallet event (ignoring)");
            }
        }
    }
}
