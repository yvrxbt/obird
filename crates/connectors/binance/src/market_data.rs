//! Binance USD-M Futures WebSocket market data feed.
//!
//! Runs two concurrent WS connections:
//! 1. `<symbol>@bookTicker` — real-time BBO, fires on every change.
//! 2. User data stream (listenKey) — order status changes and fills.
//!
//! Fills are published as `Event::Fill` on the MarketDataSink (strategy bus).
//! BBO updates are published as `Event::BookUpdate`.
//! ListenKey is kept alive with a PUT every 30 minutes.

use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use trading_core::{Event, InstrumentId, MarketDataSink};

use crate::normalize::{self, BookTickerMsg, OrderTradeUpdate, UserDataEnvelope};

const MAINNET_WS: &str = "wss://fstream.binance.com/ws";
const TESTNET_WS: &str = "wss://stream.binancefuture.com/ws";
const MAINNET_REST: &str = "https://fapi.binance.com";
const TESTNET_REST: &str = "https://testnet.binancefuture.com";
/// Binance listenKey expiry is 60 min; renew every 30 to be safe.
const LISTEN_KEY_RENEW_SECS: u64 = 30 * 60;

pub struct BinanceMarketDataFeed {
    symbol: String,
    instrument: InstrumentId,
    api_key: String,
    testnet: bool,
}

impl BinanceMarketDataFeed {
    pub fn new(symbol: String, instrument: InstrumentId, api_key: String, testnet: bool) -> Self {
        Self { symbol, instrument, api_key, testnet }
    }

    pub async fn run(self, sink: Arc<dyn MarketDataSink>) {
        let sink_md = sink.clone();
        let sink_ud = sink.clone();

        let symbol_md = self.symbol.clone();
        let symbol_ud = self.symbol.clone();
        let instrument_md = self.instrument.clone();
        let instrument_ud = self.instrument.clone();
        let ws_base = if self.testnet { TESTNET_WS } else { MAINNET_WS };
        let rest_base = if self.testnet { TESTNET_REST } else { MAINNET_REST };
        let api_key = self.api_key.clone();

        // Task 1: public bookTicker feed
        let md_handle = tokio::spawn(async move {
            run_book_ticker(ws_base, &symbol_md, instrument_md, sink_md).await;
        });

        // Task 2: private user data stream (fills + order updates)
        let ud_handle = tokio::spawn(async move {
            run_user_data(ws_base, rest_base, &api_key, &symbol_ud, instrument_ud, sink_ud).await;
        });

        let _ = tokio::join!(md_handle, ud_handle);
    }
}

// ── Public bookTicker feed ────────────────────────────────────────────────────

async fn run_book_ticker(
    ws_base: &str,
    symbol: &str,
    instrument: InstrumentId,
    sink: Arc<dyn MarketDataSink>,
) {
    let stream = format!("{}/{}", ws_base, symbol.to_lowercase() + "@bookTicker");

    loop {
        match run_book_ticker_once(&stream, &instrument, &sink).await {
            Ok(()) => tracing::warn!(symbol, "bookTicker stream ended — reconnecting"),
            Err(e) => tracing::error!(symbol, error = %e, "bookTicker error — reconnecting"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn run_book_ticker_once(
    url: &str,
    instrument: &InstrumentId,
    sink: &Arc<dyn MarketDataSink>,
) -> anyhow::Result<()> {
    let (ws, _) = connect_async(url).await?;
    let (_, mut read) = ws.split();

    tracing::info!(instrument = %instrument, "Binance bookTicker connected");

    while let Some(msg) = read.next().await {
        let text = match msg? {
            Message::Text(t) => t,
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => continue,
            Message::Close(_) => break,
        };

        let ticker: BookTickerMsg = match serde_json::from_str(&text) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, raw = %text, "bookTicker parse error");
                continue;
            }
        };

        let (snap, exchange_ts_ns) = normalize::book_ticker_to_snapshot(&ticker);
        let local_ts_ns = normalize::now_ns();

        tracing::debug!(
            target: "md",
            instrument = %instrument,
            feed_latency_us = (local_ts_ns.saturating_sub(exchange_ts_ns)) / 1_000,
            best_bid = %ticker.best_bid,
            best_ask = %ticker.best_ask,
            "BINANCE_BBO"
        );

        sink.publish(
            instrument,
            Event::BookUpdate {
                instrument: instrument.clone(),
                book: snap,
                exchange_ts_ns,
                local_ts_ns,
            },
        );
    }

    Ok(())
}

// ── Private user data stream ──────────────────────────────────────────────────

async fn run_user_data(
    ws_base: &str,
    rest_base: &str,
    api_key: &str,
    symbol: &str,
    instrument: InstrumentId,
    sink: Arc<dyn MarketDataSink>,
) {
    loop {
        match run_user_data_once(ws_base, rest_base, api_key, symbol, &instrument, &sink).await {
            Ok(()) => tracing::warn!(symbol, "user data stream ended — reconnecting"),
            Err(e) => tracing::error!(symbol, error = %e, "user data stream error — reconnecting"),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn run_user_data_once(
    ws_base: &str,
    rest_base: &str,
    api_key: &str,
    symbol: &str,
    instrument: &InstrumentId,
    sink: &Arc<dyn MarketDataSink>,
) -> anyhow::Result<()> {
    // 1. Request a listenKey
    let listen_key = create_listen_key(rest_base, api_key).await?;
    tracing::info!(symbol, "Binance listenKey created");

    // 2. Spawn keepalive task — PUT every 30 min to prevent expiry
    let rest_base_owned = rest_base.to_string();
    let api_key_owned = api_key.to_string();
    let listen_key_owned = listen_key.clone();
    let keepalive = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(LISTEN_KEY_RENEW_SECS));
        interval.tick().await; // skip first immediate tick
        loop {
            interval.tick().await;
            if let Err(e) = renew_listen_key(&rest_base_owned, &api_key_owned, &listen_key_owned).await {
                tracing::warn!(error = %e, "listenKey renewal failed");
            } else {
                tracing::debug!("listenKey renewed");
            }
        }
    });

    // 3. Connect to user data stream
    let url = format!("{}/{}", ws_base, listen_key);
    let (ws, _) = connect_async(&url).await?;
    let (_, mut read) = ws.split();

    tracing::info!(symbol, "Binance user data stream connected");

    while let Some(msg) = read.next().await {
        let text = match msg? {
            Message::Text(t) => t,
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => continue,
            Message::Close(_) => break,
        };

        // Peek at event type before full parse
        let envelope: UserDataEnvelope = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(_) => continue,
        };

        if envelope.event_type != "ORDER_TRADE_UPDATE" {
            continue; // ignore ACCOUNT_UPDATE, MARGIN_CALL, etc.
        }

        let update: OrderTradeUpdate = match serde_json::from_str(&text) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(error = %e, "ORDER_TRADE_UPDATE parse error");
                continue;
            }
        };

        let inner = &update.order;

        // Publish order status update to strategy
        let order_update = normalize::ws_order_update(instrument, inner);
        sink.publish(
            instrument,
            Event::OrderUpdate {
                instrument: instrument.clone(),
                update: order_update,
            },
        );

        // If last_qty > 0, this message carries an actual fill leg
        if inner.last_qty > rust_decimal::Decimal::ZERO {
            let fill = normalize::ws_fill(instrument, inner);
            tracing::info!(
                target: "quoter",
                instrument = %instrument,
                order_id = inner.order_id,
                side = %inner.side,
                price = %inner.last_price,
                qty = %inner.last_qty,
                fee = %inner.commission,
                cum_qty = %inner.cum_qty,
                "BINANCE_FILL"
            );
            sink.publish(
                instrument,
                Event::Fill {
                    instrument: instrument.clone(),
                    fill,
                },
            );
        }
    }

    keepalive.abort();
    Ok(())
}

// ── ListenKey management ──────────────────────────────────────────────────────

async fn create_listen_key(rest_base: &str, api_key: &str) -> anyhow::Result<String> {
    let http = reqwest::Client::new();
    let url = format!("{}/fapi/v1/listenKey", rest_base);

    let resp = http.post(&url)
        .header("X-MBX-APIKEY", api_key)
        .send().await?
        .error_for_status()?;

    let body: normalize::ListenKeyResponse = resp.json().await?;
    Ok(body.listen_key)
}

async fn renew_listen_key(rest_base: &str, api_key: &str, listen_key: &str) -> anyhow::Result<()> {
    let http = reqwest::Client::new();
    let url = format!("{}/fapi/v1/listenKey", rest_base);

    http.put(&url)
        .header("X-MBX-APIKEY", api_key)
        .query(&[("listenKey", listen_key)])
        .send().await?
        .error_for_status()?;

    Ok(())
}
