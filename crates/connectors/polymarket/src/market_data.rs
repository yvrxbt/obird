//! Polymarket CLOB WebSocket market data feed.
//!
//! ## Design: single multiplexed connection
//!
//! A single `PolymarketMarketDataFeed` instance handles ALL subscribed markets
//! over ONE WebSocket connection to:
//!   `wss://ws-subscriptions-clob.polymarket.com/ws/market`
//!
//! This is critical when quoting 20+ predict.fun markets simultaneously — opening
//! one WS connection per market would spam Polymarket and hit rate limits.
//!
//! ## Subscription protocol
//!
//! After connecting, send one subscribe message with all token IDs:
//! ```json
//! {"type": "market", "assets_ids": ["token_id_1", "token_id_2", ...]}
//! ```
//! `"type"` must be lowercase `"market"` — the server silently ignores the message
//! otherwise (no error, just no updates). `initial_dump` defaults to `true`, so
//! a full `book` snapshot arrives immediately after subscribing.
//!
//! ## Events received
//!
//! - `book`: Full orderbook snapshot (sent on subscribe and after fills).
//! - `price_change`: Incremental level update (`size="0"` means remove the level).
//!
//! Both update the per-token `BookState` and trigger a `BookUpdate` publish to
//! the `MarketDataSink`. The strategy consumes this as the Polymarket fair value.
//!
//! ## Heartbeat (keep-alive + liveness proof)
//!
//! Per Polymarket docs: the client sends a TEXT frame containing the literal string
//! `"PING"` every 10 seconds. The server responds with a TEXT frame `"PONG"`.
//!
//! This is NOT WebSocket-level ping/pong frames — it is application-level text messages.
//!
//! On every `"PONG"` receipt, we **re-publish the last known book state** for every
//! subscribed token. This resets `polymarket_mid_ts` in the strategy, decoupling
//! "feed is alive" from "book changed recently". A market that is quiet for hours
//! still produces PONG heartbeats every 10 seconds, keeping the FV fresh.
//!
//! ## Reconnection
//!
//! Reconnects with exponential backoff (1s → 2s → 4s → ... → 30s cap) on any
//! error or clean close. Book state is cleared on reconnect; the initial `book`
//! snapshot restores it.
//!
//! ## Architecture invariant
//!
//! This feed publishes to `MarketDataSink` (abstracted over in-process bus or
//! NATS/Redis for distributed deployment). Strategies never reference this
//! struct directly — they receive `Event::BookUpdate` from the bus.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use trading_core::{Event, InstrumentId, MarketDataSink};

use crate::normalize::{self, BookState, PolymarketEvent};

const WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

/// How often to send the application-level TEXT "PING" heartbeat.
/// Per Polymarket docs: every 10 seconds. The server responds with TEXT "PONG".
/// On PONG we re-publish last known book state to prove feed liveness to strategies.
const PING_INTERVAL_SECS: u64 = 10;

/// Hard recv timeout — if no message arrives for this long despite pings,
/// the connection is genuinely dead and we reconnect.
/// Must be > PING_INTERVAL_SECS. 60s gives 5 missed PINGs before giving up.
const RECV_TIMEOUT_SECS: u64 = 60;

/// A single multiplexed Polymarket CLOB WS feed for one or more YES token IDs.
///
/// Create one instance per process. Add markets by including their
/// `(yes_token_id, InstrumentId)` pairs in `subscriptions`.
pub struct PolymarketMarketDataFeed {
    /// `(token_id, InstrumentId)` pairs — determines which MarketDataBus channel
    /// receives the `BookUpdate` for each token.
    subscriptions: Vec<(String, InstrumentId)>,
}

impl PolymarketMarketDataFeed {
    /// Create a new feed for the given `(yes_token_id, instrument)` pairs.
    ///
    /// Pre-register all instruments on the `MarketDataBus` with `bus.sender(&instrument)`
    /// before calling `run()`, otherwise the first events may be dropped.
    pub fn new(subscriptions: Vec<(String, InstrumentId)>) -> Self {
        assert!(
            !subscriptions.is_empty(),
            "PolymarketMarketDataFeed: at least one subscription required"
        );
        Self { subscriptions }
    }

    /// Run forever — reconnects with exponential backoff on any error or close.
    pub async fn run(self, sink: Arc<dyn MarketDataSink>) {
        let mut backoff = 1u64;
        loop {
            match self.run_once(sink.clone()).await {
                Ok(()) => {
                    tracing::warn!("Polymarket CLOB WS closed cleanly — reconnecting in {backoff}s")
                }
                Err(e) => tracing::error!(
                    error = %e,
                    "Polymarket CLOB WS error — reconnecting in {backoff}s"
                ),
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    }

    async fn run_once(&self, sink: Arc<dyn MarketDataSink>) -> anyhow::Result<()> {
        let (mut ws, _) = connect_async(WS_URL)
            .await
            .map_err(|e| anyhow::anyhow!("Polymarket WS connect: {e}"))?;

        // Send subscription. IMPORTANT: type must be lowercase "market" — the server
        // silently ignores subscriptions with any other casing.
        let token_ids: Vec<&str> = self.subscriptions.iter().map(|(t, _)| t.as_str()).collect();
        tracing::debug!(
            n_subscriptions = self.subscriptions.len(),
            subscriptions = ?self.subscriptions,
            "Polymarket subscriptions before sending"
        );
        let sub_msg = serde_json::json!({
            "type": "market",
            "assets_ids": token_ids,
        });
        ws.send(Message::Text(sub_msg.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("Polymarket WS subscribe send: {e}"))?;

        tracing::info!(
            tokens = ?token_ids,
            "Polymarket CLOB WS connected and subscribed",
        );

        // Build lookup: token_id → InstrumentId.
        let token_to_instrument: HashMap<String, InstrumentId> = self
            .subscriptions
            .iter()
            .map(|(t, i)| (t.clone(), i.clone()))
            .collect();

        // Per-token book state — cleared on reconnect, restored by initial `book` snapshot.
        let mut book_states: HashMap<String, BookState> = self
            .subscriptions
            .iter()
            .map(|(t, _)| (t.clone(), BookState::default()))
            .collect();

        // Application-level TEXT "PING" heartbeat — per Polymarket docs, every 10s.
        // Discard the first immediate tick.
        let mut ping_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(PING_INTERVAL_SECS));
        ping_interval.tick().await;

        let mut last_recv = std::time::Instant::now();

        loop {
            let msg: Message = tokio::select! {
                result = ws.next() => {
                    last_recv = std::time::Instant::now();
                    match result {
                        Some(Ok(m))  => m,
                        Some(Err(e)) => return Err(anyhow::anyhow!("Polymarket WS recv error: {e}")),
                        None         => return Err(anyhow::anyhow!("Polymarket WS stream closed")),
                    }
                }

                _ = ping_interval.tick() => {
                    // Hard backstop: if we haven't received anything despite sending pings,
                    // the connection is dead.
                    if last_recv.elapsed().as_secs() >= RECV_TIMEOUT_SECS {
                        return Err(anyhow::anyhow!(
                            "Polymarket WS recv timeout after {RECV_TIMEOUT_SECS}s"
                        ));
                    }
                    // Per Polymarket docs: TEXT "PING" (not a WebSocket Ping frame).
                    tracing::debug!(target: "md", "Polymarket WS → PING");
                    ws.send(Message::Text("PING".to_string()))
                        .await
                        .map_err(|e| anyhow::anyhow!("Polymarket WS PING send failed: {e}"))?;
                    continue;
                }
            };

            match msg {
                Message::Text(text) => {
                    // Application-level PONG response to our TEXT "PING".
                    // Re-publish all known book snapshots so strategies know the feed is
                    // alive even if no book events have arrived recently.
                    if text.trim() == "PONG" {
                        tracing::debug!(target: "md", "Polymarket WS ← PONG (heartbeat ok)");
                        let local_ts = normalize::now_ns();
                        for (token_id, state) in &book_states {
                            if let Some(snap) = state.to_snapshot(local_ts) {
                                if let Some(instrument) = token_to_instrument.get(token_id) {
                                    sink.publish(
                                        instrument,
                                        Event::BookUpdate {
                                            instrument: instrument.clone(),
                                            book: snap,
                                            exchange_ts_ns: local_ts,
                                            local_ts_ns: local_ts,
                                        },
                                    );
                                }
                            }
                        }
                        continue;
                    }

                    // Regular market data events (book snapshot or price_change).
                    let local_ts = normalize::now_ns();
                    let events = normalize::parse_message(&text);

                    for event in events {
                        match event {
                            PolymarketEvent::Book {
                                asset_id,
                                bids,
                                asks,
                                timestamp,
                            } => {
                                let Some(state) = book_states.get_mut(&asset_id) else {
                                    tracing::debug!(
                                        asset_id,
                                        "Polymarket book event for unsubscribed token — ignoring"
                                    );
                                    continue;
                                };
                                state.apply_snapshot(&bids, &asks);

                                let ts_ns = parse_ts_ms(&timestamp, local_ts);
                                if let Some(snap) = state.to_snapshot(ts_ns) {
                                    if let Some(instrument) = token_to_instrument.get(&asset_id) {
                                        tracing::debug!(
                                            target: "md",
                                            instrument = %instrument,
                                            best_bid = ?snap.best_bid().map(|(p, _)| p.inner()),
                                            best_ask = ?snap.best_ask().map(|(p, _)| p.inner()),
                                            "POLY_BOOK",
                                        );
                                        sink.publish(
                                            instrument,
                                            Event::BookUpdate {
                                                instrument: instrument.clone(),
                                                book: snap,
                                                exchange_ts_ns: ts_ns,
                                                local_ts_ns: local_ts,
                                            },
                                        );
                                    }
                                }
                            }

                            PolymarketEvent::PriceChange {
                                asset_id,
                                changes,
                                timestamp,
                            } => {
                                let Some(state) = book_states.get_mut(&asset_id) else {
                                    continue;
                                };
                                state.apply_changes(&changes);

                                let ts_ns = parse_ts_ms(&timestamp, local_ts);
                                if let Some(snap) = state.to_snapshot(ts_ns) {
                                    if let Some(instrument) = token_to_instrument.get(&asset_id) {
                                        tracing::debug!(
                                            target: "md",
                                            instrument = %instrument,
                                            best_bid = ?snap.best_bid().map(|(p, _)| p.inner()),
                                            best_ask = ?snap.best_ask().map(|(p, _)| p.inner()),
                                            "POLY_PRICE_CHANGE",
                                        );
                                        sink.publish(
                                            instrument,
                                            Event::BookUpdate {
                                                instrument: instrument.clone(),
                                                book: snap,
                                                exchange_ts_ns: ts_ns,
                                                local_ts_ns: local_ts,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                // WebSocket protocol-level ping — respond with pong (from proxies/load balancers).
                Message::Ping(payload) => {
                    ws.send(Message::Pong(payload)).await.ok();
                }

                // WebSocket protocol-level pong — ignore (we use text PING/PONG per docs).
                Message::Pong(_) => {}

                Message::Close(_) => {
                    return Err(anyhow::anyhow!("Polymarket WS: server sent Close frame"))
                }

                _ => {} // Binary frames — ignore
            }
        }
    }
}

/// Parse a Unix-millisecond timestamp string into nanoseconds, falling back to local_ts.
/// Polymarket timestamps are milliseconds per API docs.
fn parse_ts_ms(ts: &Option<String>, fallback_ns: u64) -> u64 {
    ts.as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ms| ms * 1_000_000) // ms → ns
        .unwrap_or(fallback_ns)
}
