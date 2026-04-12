//! Publishes fair values over Unix domain sockets.
//!
//! Protocol: length-prefixed bincode frames over UDS.
//! Each frame: [4 bytes LE: payload length][N bytes: bincode FairValueMessage]
//!
//! The strategy engine connects as a client and reads frames.
//! Multiple clients can connect simultaneously.

use trading_core::types::decimal::Price;
use trading_core::types::instrument::InstrumentId;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::Path;

/// Wire format for fair value messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FairValueMessage {
    pub instrument: InstrumentId,
    pub fair_value: Decimal,
    /// 0.0–1.0 confidence in the estimate
    pub confidence: f64,
    pub model_version: String,
    pub timestamp_ns: u64,
    /// Feature values that drove this estimate (for audit)
    pub features: HashMap<String, f64>,
}

/// UDS-based publisher. Accepts multiple client connections.
pub struct FairValuePublisher {
    socket_path: String,
}

impl FairValuePublisher {
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Start the publisher. Listens for client connections on the UDS.
    /// Broadcasts FairValueMessages to all connected clients.
    pub async fn run(
        &self,
        mut rx: tokio::sync::broadcast::Receiver<FairValueMessage>,
    ) -> anyhow::Result<()> {
        // Clean up stale socket
        let path = Path::new(&self.socket_path);
        if path.exists() {
            tokio::fs::remove_file(path).await?;
        }

        let listener = tokio::net::UnixListener::bind(&self.socket_path)?;
        tracing::info!(path = %self.socket_path, "Fair value publisher listening");

        // Track connected clients
        let clients: std::sync::Arc<tokio::sync::Mutex<Vec<tokio::net::UnixStream>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let clients_accept = clients.clone();

        // Accept connections in background
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        tracing::info!("Fair value client connected");
                        clients_accept.lock().await.push(stream);
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to accept connection");
                    }
                }
            }
        });

        // Broadcast fair values to all connected clients
        while let Ok(msg) = rx.recv().await {
            let payload = serde_json::to_vec(&msg)?;
            let len = (payload.len() as u32).to_le_bytes();

            let mut connected = clients.lock().await;
            let mut disconnected = Vec::new();

            for (i, client) in connected.iter_mut().enumerate() {
                use tokio::io::AsyncWriteExt;
                if client.write_all(&len).await.is_err()
                    || client.write_all(&payload).await.is_err()
                {
                    disconnected.push(i);
                }
            }

            // Remove disconnected clients (reverse order to preserve indices)
            for i in disconnected.into_iter().rev() {
                tracing::info!("Fair value client disconnected");
                connected.remove(i);
            }
        }

        Ok(())
    }
}
