//! Simulated market data feed for backtesting.
//! Replays recorded market data from newline-delimited JSON files.

use trading_core::Event;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::fs::File;

/// Replays recorded market data from a directory of NDJSON files.
pub struct SimMarketDataFeed {
    data_dir: PathBuf,
}

impl SimMarketDataFeed {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    /// Load all events from recorded files, sorted by timestamp.
    pub async fn load_events(&self) -> anyhow::Result<Vec<Event>> {
        let mut all_events = Vec::new();

        let mut entries = tokio::fs::read_dir(&self.data_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let file = File::open(&path).await?;
                let reader = BufReader::new(file);
                let mut lines = reader.lines();

                while let Some(line) = lines.next_line().await? {
                    match serde_json::from_str::<Event>(&line) {
                        Ok(event) => all_events.push(event),
                        Err(e) => {
                            tracing::warn!(file = ?path, error = %e, "Failed to parse event line");
                        }
                    }
                }
            }
        }

        // Sort by timestamp
        all_events.sort_by_key(|e| match e {
            Event::BookUpdate { exchange_ts_ns, .. } => *exchange_ts_ns,
            Event::MarketTrade { trade, .. } => trade.timestamp_ns,
            Event::Tick { timestamp_ns } => *timestamp_ns,
            _ => 0,
        });

        tracing::info!(count = all_events.len(), "Loaded recorded events");
        Ok(all_events)
    }
}
