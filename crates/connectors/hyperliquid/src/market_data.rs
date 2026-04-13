//! Hyperliquid WebSocket market data feed — background task.
//!
//! Subscribes to L2Book (per-instrument BBO + depth), OrderUpdates, and UserFills.
//! Publishes normalized Events to a MarketDataSink.
//!
//! L2Book gives real best-bid/ask prices and the exchange-side timestamp, enabling
//! accurate feed-latency measurement (exchange_ts_ns vs local_ts_ns in BookUpdate).

use std::sync::Arc;

use futures::StreamExt;
use hypersdk::hypercore::{
    types::{Incoming, Subscription, UserEvent},
    ws::Event as WsEvent,
};
use trading_core::{Event, InstrumentId, MarketDataSink};

use crate::normalize;

#[derive(Clone)]
pub struct AssetInfo {
    /// Key used in AllMids mids hashmap (perp: "ETH", spot: "PURR/USDC" or "@N")
    pub mids_key: String,
    /// Coin string for L2Book/Trades WS subscriptions
    pub ws_coin: String,
    pub instrument: InstrumentId,
}

pub struct HlMarketDataFeed {
    asset: AssetInfo,
    user: hypersdk::Address,
    testnet: bool,
}

impl HlMarketDataFeed {
    pub fn new(asset: AssetInfo, user: hypersdk::Address, testnet: bool) -> Self {
        Self { asset, user, testnet }
    }

    pub async fn run(self, sink: Arc<dyn MarketDataSink>) {
        loop {
            if let Err(e) = self.run_once(sink.clone()).await {
                tracing::error!("HL feed error, retrying in 2s: {e}");
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        }
    }

    async fn run_once(&self, sink: Arc<dyn MarketDataSink>) -> anyhow::Result<()> {
        let mut ws = if self.testnet {
            hypersdk::hypercore::testnet_ws()
        } else {
            hypersdk::hypercore::mainnet_ws()
        };

        // L2Book gives real BBO + exchange timestamp. One subscription per instrument.
        // AllMids (removed) gave only a synthetic mid with no per-instrument timestamp.
        ws.subscribe(Subscription::L2Book { coin: self.asset.ws_coin.clone() });
        ws.subscribe(Subscription::OrderUpdates { user: self.user });
        ws.subscribe(Subscription::UserFills { user: self.user });

        tracing::info!(instrument = %self.asset.instrument, "HL market data feed started");

        while let Some(event) = ws.next().await {
            match event {
                WsEvent::Connected => tracing::info!("HL WS connected"),
                WsEvent::Disconnected => tracing::warn!("HL WS disconnected — reconnecting"),
                WsEvent::Message(msg) => self.handle_message(msg, &sink),
            }
        }
        Ok(())
    }

    fn handle_message(&self, msg: Incoming, sink: &Arc<dyn MarketDataSink>) {
        match msg {
            Incoming::L2Book(book) => {
                // HL sends book.time in milliseconds → convert to nanoseconds.
                // exchange_ts_ns: when HL produced this snapshot (their clock)
                // local_ts_ns:    when we received and processed it (our clock)
                // Δ = local - exchange gives one-way feed latency estimate.
                let exchange_ts_ns = book.time * 1_000_000;
                let local_ts_ns = normalize::now_ns();
                let snap = normalize::l2book_to_snapshot(&book, exchange_ts_ns);

                tracing::debug!(
                    target: "md",
                    instrument = %self.asset.instrument,
                    feed_latency_us = (local_ts_ns.saturating_sub(exchange_ts_ns)) / 1_000,
                    best_bid = ?snap.best_bid().map(|(p, _)| p.inner()),
                    best_ask = ?snap.best_ask().map(|(p, _)| p.inner()),
                    "L2BOOK"
                );

                sink.publish(
                    &self.asset.instrument,
                    Event::BookUpdate {
                        instrument: self.asset.instrument.clone(),
                        book: snap,
                        exchange_ts_ns,
                        local_ts_ns,
                    },
                );
            }

            Incoming::OrderUpdates(updates) => {
                for update in &updates {
                    let core_update = normalize::order_update(&self.asset.instrument, update);
                    sink.publish(
                        &self.asset.instrument,
                        Event::OrderUpdate {
                            instrument: self.asset.instrument.clone(),
                            update: core_update,
                        },
                    );
                }
            }

            Incoming::UserFills { fills, is_snapshot, .. } => {
                if is_snapshot {
                    return; // skip historical snapshot on subscription
                }
                for fill in &fills {
                    let core_fill = normalize::fill(&self.asset.instrument, fill);
                    sink.publish(
                        &self.asset.instrument,
                        Event::Fill {
                            instrument: self.asset.instrument.clone(),
                            fill: core_fill,
                        },
                    );
                }
            }

            Incoming::UserEvents(event) => {
                if let UserEvent::Fills { fills } = event {
                    for fill in &fills {
                        let core_fill = normalize::fill(&self.asset.instrument, fill);
                        sink.publish(
                            &self.asset.instrument,
                            Event::Fill {
                                instrument: self.asset.instrument.clone(),
                                fill: core_fill,
                            },
                        );
                    }
                }
            }

            _ => {}
        }
    }
}
