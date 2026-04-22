//! Records live market data to disk for later backtest replay.
//!
//! Subscribes to broadcast channels in the MarketDataBus and writes
//! events as newline-delimited JSON. If the recorder lags behind,
//! it logs a warning and skips — acceptable for recording.

use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use trading_core::Event;

pub struct MarketDataRecorder {
    output_dir: PathBuf,
}

impl MarketDataRecorder {
    pub fn new(output_dir: impl AsRef<Path>) -> Self {
        Self {
            output_dir: output_dir.as_ref().to_path_buf(),
        }
    }

    /// Record from a broadcast receiver to a JSONL file.
    /// Runs until the channel closes or an error occurs.
    pub async fn record(
        &self,
        mut rx: broadcast::Receiver<Event>,
        filename: &str,
    ) -> anyhow::Result<()> {
        let path = self.output_dir.join(format!("{}.jsonl", filename));
        tokio::fs::create_dir_all(&self.output_dir).await?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        tracing::info!(?path, "Recording market data");

        let mut event_count = 0u64;
        let mut lag_count = 0u64;

        loop {
            match rx.recv().await {
                Ok(event) => {
                    let line = serde_json::to_string(&event)?;
                    file.write_all(line.as_bytes()).await?;
                    file.write_all(b"\n").await?;

                    event_count += 1;
                    if event_count % 10_000 == 0 {
                        file.flush().await?;
                        tracing::debug!(
                            events = event_count,
                            lags = lag_count,
                            "Recording progress"
                        );
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    lag_count += n;
                    tracing::warn!(
                        skipped = n,
                        total_lags = lag_count,
                        "Recorder lagged — some events not recorded"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!(events = event_count, "Recording channel closed");
                    break;
                }
            }
        }

        file.flush().await?;
        Ok(())
    }
}
