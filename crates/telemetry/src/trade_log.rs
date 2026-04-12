//! Structured trade logging.
//! Writes every fill and order update to a JSONL file for post-trade analysis.

use trading_core::types::order::OrderUpdate;
use trading_core::types::position::Fill;
use tokio::io::AsyncWriteExt;
use std::path::Path;

pub struct TradeLogger {
    // TODO: async file writer
}

impl TradeLogger {
    pub async fn new(_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Ok(Self {})
    }

    pub async fn log_fill(&mut self, _fill: &Fill) -> anyhow::Result<()> {
        // TODO: serialize to JSON, write to file
        Ok(())
    }

    pub async fn log_order_update(&mut self, _update: &OrderUpdate) -> anyhow::Result<()> {
        // TODO: serialize to JSON, write to file
        Ok(())
    }
}
