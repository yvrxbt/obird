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
    /// Env var name holding the API key (or leave empty for exchanges that don't use one).
    #[serde(default)]
    pub api_key_env: String,
    /// Env var name holding the signing secret / private key.
    #[serde(default)]
    pub secret_key_env: String,
    pub testnet: bool,
    /// Exchange-specific extra parameters.
    /// Each connector crate defines its own params struct and deserializes from this.
    /// Missing in config → empty table (equivalent to no params).
    #[serde(default = "default_toml_table")]
    pub params: toml::Value,
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

fn default_toml_table() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

impl AppConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
}
