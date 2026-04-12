//! Configuration types. Deserialized from TOML config files.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub engine: EngineConfig,
    pub exchanges: Vec<ExchangeConfig>,
    pub strategies: Vec<StrategyConfig>,
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Deserialize)]
pub struct EngineConfig {
    pub tick_interval_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct ExchangeConfig {
    pub name: String,
    pub api_key_env: String,
    pub secret_key_env: String,
    pub testnet: bool,
}

#[derive(Debug, Deserialize)]
pub struct StrategyConfig {
    pub name: String,
    pub strategy_type: String,
    pub instruments: Vec<String>,
    pub params: toml::Value,
}

#[derive(Debug, Deserialize)]
pub struct TelemetryConfig {
    pub log_level: String,
    pub metrics_port: u16,
    pub enable_tracing: bool,
}

impl AppConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
}
