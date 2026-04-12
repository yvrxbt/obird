//! Live trading mode.

use std::str::FromStr;

use connector_hyperliquid::HyperliquidClient;
use rust_decimal::Decimal;
use trading_core::traits::ExchangeConnector;
use trading_core::types::decimal::{Price, Quantity};
use trading_core::types::instrument::{Exchange, InstrumentId, InstrumentKind};
use trading_core::types::order::{OrderRequest, OrderSide, TimeInForce};

/// Minimal live-mode smoke: place one testnet order via Hyperliquid connector.
pub async fn run_once() -> anyhow::Result<()> {
    let symbol = std::env::var("HL_SYMBOL").unwrap_or_else(|_| "ETH".to_string());
    let price = std::env::var("HL_TEST_ORDER_PRICE").unwrap_or_else(|_| "1800".to_string());
    let size = std::env::var("HL_TEST_ORDER_SIZE").unwrap_or_else(|_| "0.01".to_string());

    let connector = HyperliquidClient::from_env("HL_SECRET_KEY", true).await?;

    let req = OrderRequest {
        instrument: InstrumentId::new(Exchange::Hyperliquid, InstrumentKind::Perpetual, symbol.clone()),
        side: OrderSide::Buy,
        price: Price::new(Decimal::from_str(&price)?),
        quantity: Quantity::new(Decimal::from_str(&size)?),
        tif: TimeInForce::Gtc,
        client_order_id: None,
    };

    let order_id = connector.place_order(&req).await?;
    tracing::info!(%symbol, %order_id, "Placed Hyperliquid testnet order");

    Ok(())
}
