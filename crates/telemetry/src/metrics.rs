//! Prometheus metrics definitions.
//!
//! All metrics are defined here and registered once at startup.
//! Components throughout the system use these metrics via shared references.

// TODO: Add prometheus crate dependency and implement:
//
// pub struct TradingMetrics {
//     pub orders_placed: IntCounterVec,       // labels: strategy, exchange, side
//     pub orders_filled: IntCounterVec,       // labels: strategy, exchange, side
//     pub orders_rejected: IntCounterVec,     // labels: strategy, exchange, reason
//     pub order_roundtrip_ms: HistogramVec,   // labels: strategy, exchange
//     pub position_notional: GaugeVec,        // labels: strategy, exchange
//     pub portfolio_pnl: Gauge,
//     pub broadcast_lagged: IntCounterVec,    // labels: subscriber
//     pub risk_rejections: IntCounterVec,     // labels: strategy, reason
//     pub md_latency_us: HistogramVec,        // labels: exchange
// }
