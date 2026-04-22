//! DataRecorder — dedicated market data and fill capture for quantitative analysis.
//!
//! Subscribes to a MarketDataBus channel and writes clean JSONL files that are
//! easy to load into pandas/polars without filtering operational log noise:
//!
//!   logs/data/bbo-YYYY-MM-DD.jsonl   — one line per BBO update
//!   logs/data/fills-YYYY-MM-DD.jsonl — one line per fill
//!
//! BBO schema:
//!   { exchange_ts_ns, local_ts_ns, instrument, bid_px, bid_sz, ask_px, ask_sz }
//!
//! Fill schema:
//!   { timestamp_ns, instrument, side, price, quantity, fee, order_id }
//!
//! Separate from tracing — no tracing overhead per tick, no parsing required.
//! The date suffix is the startup date (no mid-session rollover).
//! Fills are flushed immediately; BBO is flushed every BBO_FLUSH_INTERVAL records
//! and on shutdown.

use chrono::Utc;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::broadcast;
use trading_core::Event;

/// Flush the BBO buffer after this many records to bound data loss on crash.
const BBO_FLUSH_INTERVAL: usize = 500;

pub struct DataRecorder {
    rx: broadcast::Receiver<Event>,
}

impl DataRecorder {
    pub fn new(rx: broadcast::Receiver<Event>) -> Self {
        Self { rx }
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        let date = Utc::now().format("%Y-%m-%d");
        std::fs::create_dir_all("logs/data")?;

        let bbo_path = format!("logs/data/bbo-{date}.jsonl");
        let fills_path = format!("logs/data/fills-{date}.jsonl");

        let mut bbo_writer = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&bbo_path)
                .await?,
        );
        let mut fills_writer = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&fills_path)
                .await?,
        );

        tracing::info!(bbo = %bbo_path, fills = %fills_path, "DataRecorder started");

        let mut bbo_since_flush: usize = 0;

        loop {
            match self.rx.recv().await {
                Ok(Event::BookUpdate {
                    instrument,
                    book,
                    exchange_ts_ns,
                    local_ts_ns,
                }) => {
                    let Some((bid_px, bid_sz)) = book.best_bid() else {
                        continue;
                    };
                    let Some((ask_px, ask_sz)) = book.best_ask() else {
                        continue;
                    };

                    let line = serde_json::json!({
                        "exchange_ts_ns": exchange_ts_ns,
                        "local_ts_ns":    local_ts_ns,
                        "instrument":     instrument.to_string(),
                        "bid_px":         bid_px.inner(),
                        "bid_sz":         bid_sz.inner(),
                        "ask_px":         ask_px.inner(),
                        "ask_sz":         ask_sz.inner(),
                    });
                    bbo_writer.write_all(line.to_string().as_bytes()).await?;
                    bbo_writer.write_all(b"\n").await?;

                    bbo_since_flush += 1;
                    if bbo_since_flush >= BBO_FLUSH_INTERVAL {
                        bbo_writer.flush().await?;
                        bbo_since_flush = 0;
                    }
                }

                Ok(Event::Fill { fill, .. }) => {
                    let line = serde_json::json!({
                        "timestamp_ns": fill.timestamp_ns,
                        "instrument":   fill.instrument.to_string(),
                        "side":         format!("{:?}", fill.side),
                        "price":        fill.price.inner(),
                        "quantity":     fill.quantity.inner(),
                        "fee":          fill.fee.inner(),
                        "order_id":     fill.order_id,
                    });
                    fills_writer.write_all(line.to_string().as_bytes()).await?;
                    fills_writer.write_all(b"\n").await?;
                    // Fills are critical — flush immediately and sync BBO buffer too.
                    fills_writer.flush().await?;
                    bbo_writer.flush().await?;
                    bbo_since_flush = 0;
                    tracing::info!(
                        instrument = %fill.instrument,
                        side = ?fill.side,
                        price = %fill.price.inner(),
                        qty = %fill.quantity.inner(),
                        "DataRecorder: fill written"
                    );
                }

                Ok(_) => {} // OrderUpdate, Tick, etc — not needed for quant studies

                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Recorder is a non-critical subscriber — log and continue.
                    // Increase BROADCAST_BUFFER in market_data_bus.rs if this is frequent.
                    tracing::warn!(dropped = n, "DataRecorder lagged — BBO records dropped");
                }

                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!("DataRecorder: channel closed, flushing and stopping");
                    bbo_writer.flush().await?;
                    fills_writer.flush().await?;
                    break;
                }
            }
        }

        Ok(())
    }
}
