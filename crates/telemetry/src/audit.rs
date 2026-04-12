//! Decision audit trail.
//!
//! Records WHY a strategy made each decision, not just WHAT it did.
//! This is queryable structured JSON that enables post-trade analysis
//! and gives LLMs context when debugging strategy behavior.
//!
//! Format:
//! {
//!   "timestamp_ns": 1712851200000000000,
//!   "strategy_id": "pair_trader_btc_eth",
//!   "decision": "place_order",
//!   "context": {
//!     "spread_zscore": 2.34,
//!     "reason": "spread exceeded 2σ threshold"
//!   }
//! }

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp_ns: u64,
    pub strategy_id: String,
    pub decision: String,
    pub context: serde_json::Value,
}

pub struct AuditLogger {
    // TODO: async file writer + optional NATS publisher
}

impl AuditLogger {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn log(&mut self, _entry: AuditEntry) -> anyhow::Result<()> {
        // TODO: write to file, optionally publish to NATS
        Ok(())
    }
}
