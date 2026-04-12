//! Shared error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConnectorError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Order rejected by exchange: {0}")]
    OrderRejected(String),
    #[error("Rate limited")]
    RateLimited,
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
    #[error("Nonce error: {0}")]
    NonceError(String),
    #[error("Unknown error: {0}")]
    Other(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum RiskRejection {
    #[error("Position limit exceeded: {0}")]
    PositionLimitExceeded(String),
    #[error("Max notional exceeded: {0}")]
    MaxNotionalExceeded(String),
    #[error("Max drawdown exceeded")]
    MaxDrawdownExceeded,
    #[error("Max open orders exceeded")]
    MaxOpenOrdersExceeded,
}
