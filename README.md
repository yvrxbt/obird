# obird — Rust HFT Framework

A multi-crate Rust workspace for low-latency trading research and execution.

Current state: **HL spread MM live on mainnet ETH perp.**
- ✅ `HlSpreadQuoter`: 2-level symmetric MM with inventory skew, session P&L tracking
- ✅ Hyperliquid connector: place/cancel/modify, per-OID BatchCancel, L2Book WS feed
- ✅ `DataRecorder`: dedicated BBO + fill JSONL capture for quant analysis (`logs/data/`)
- ✅ Engine: multi-instrument fan-out, cross-exchange concurrent dispatch, graceful shutdown
- ✅ Backtest harness + simulation components exist (not yet wired to CLI)
- ⚠️ Binance connector: built, not wired into live runner
- ⚠️ CLI `backtest` and `record` subcommands are stubs
- ⚠️ Risk manager, Prometheus metrics, FairValueService: stubbed

**See `RUNBOOK.md` for live operation guide, monitoring, and tuning.**

## Architecture Overview

The system is built around two core abstractions:

- `ExchangeConnector` trait: exchange-side order + state operations
- `Strategy` trait: pure decision logic (`Event -> Vec<Action>`)

This enforces strategy portability across live and simulation modes.

### High-level data flow

1. Exchange connectors produce market/order events
2. `MarketDataBus` fan-outs events via `tokio::broadcast` (per instrument)
3. Strategies consume `Event`s and emit `Action`s
4. `OrderRouter` performs unified risk checks then routes actions by exchange
5. Per-exchange `OrderManager` submits to connector

## Workspace Structure

```text
trading-system/
├── Cargo.toml                     # Workspace manifest
├── ARCHITECTURE.md                # Detailed architecture + implementation notes
├── configs/
│   └── example.toml               # Example engine/strategy/exchange config
├── scripts/
│   └── run_backtest.sh            # Convenience wrapper (CLI args not fully wired yet)
├── infra/docker/
│   ├── Dockerfile.engine          # Builds trading-cli image
│   └── docker-compose.yml         # Prometheus + Grafana local stack
├── decisions/                     # ADRs
└── crates/
    ├── core/                      # Shared types, traits, config, errors
    ├── engine/                    # Runner, router, risk, market data bus
    ├── backtest/                  # SimConnector, replay feed, report, recorder
    ├── cli/                       # `trading-cli` entrypoint
    ├── telemetry/                 # Metrics/audit/log plumbing
    ├── fair_value_service/        # Separate fair-value binary (currently TODO skeleton)
    ├── connectors/
    │   ├── hyperliquid/           # Implemented connector (via hyperliquid_sdk)
    │   ├── binance/               # Scaffold
    │   ├── lighter/               # Scaffold
    │   ├── polymarket/            # Scaffold
    │   └── predict_fun/           # Scaffold
    └── strategies/
        ├── pair_trader/           # Strategy skeleton
        └── prediction_quoter/     # Strategy skeleton
```

## Prerequisites

- Rust stable (1.82+ recommended)
- Cargo
- Optional: Docker + Docker Compose (for Prometheus/Grafana)

## Build

```bash
cargo build --workspace
```

Release build:

```bash
cargo build --workspace --release
```

## Running

### 1) Live mode — HL spread MM

```bash
cp .env.example .env
# Set HL_SECRET_KEY in .env

source .env && RUST_LOG=quoter=info,connector_hyperliquid=info,trading_engine=info \
  ./target/release/trading-cli live --config configs/quoter.toml
```

Environment variables:
- `HL_SECRET_KEY` (required) — mainnet private key (hex)

**See `RUNBOOK.md` for full operational guide including monitoring, log analysis, and parameter tuning.**

### 2) Backtest mode (CLI path currently stubbed)

Current status:
- `trading-cli backtest` logs a message but does not execute harness wiring yet.
- Backtest primitives **are implemented** in `crates/backtest`.

Current command (no-op scaffold):

```bash
cargo run -p trading-cli -- backtest
```

Planned command shape (see script/config):

```bash
cargo run -p trading-cli -- backtest --config configs/example.toml --data data/recordings/latest --output data/backtest_results/<run_id>
```

### 3) Record mode (CLI path currently stubbed)

`trading-cli record` is present in command dispatch but not implemented yet.

## Configuration

`configs/example.toml` defines:
- `[engine]` tick interval
- `[[exchanges]]` list with env-var names for credentials
- `[[strategies]]` instances and params
- `[telemetry]` logging + metrics settings

`AppConfig` loader lives in `crates/core/src/config.rs`.

## Common Commands / Workflows

### Quality checks

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

### Quick connector smoke (Hyperliquid)

```bash
set -a; source .env; set +a
RUST_LOG=info cargo run -p trading-cli -- live
```

### Run monitoring stack

```bash
docker compose -f infra/docker/docker-compose.yml up -d
```

### Build dockerized CLI binary

```bash
docker build -f infra/docker/Dockerfile.engine -t trading-cli:local .
```

## Environment Variables

### Required for current live smoke flow

- `HL_SECRET_KEY`: Hyperliquid private key (hex, with/without `0x`)

### Optional for current live smoke flow

- `HL_SYMBOL`
- `HL_TEST_ORDER_PRICE`
- `HL_TEST_ORDER_SIZE`

### Declared in config for future multi-exchange live mode

- `HL_API_KEY`, `HL_SECRET_KEY`
- `BINANCE_API_KEY`, `BINANCE_SECRET_KEY`
- (and equivalent for other connectors once implemented)

## Troubleshooting

### `missing env var: HL_SECRET_KEY`
Set and export `HL_SECRET_KEY` before running `live`.

### `invalid private key`
Ensure key is valid hex and not truncated.

### `order failed` / `open_orders failed` (Hyperliquid)
Usually one of:
- wrong network mode (testnet vs mainnet expectation)
- insufficient balance/permissions
- transient API/network issue

Retry with `RUST_LOG=debug` and inspect connector logs.

### `Usage: trading-cli <live|backtest|record> [options]`
The CLI currently parses only the first positional command. Extra flags are not wired yet.

### Backtest command appears to do nothing
Expected right now: command dispatch exists, harness integration in CLI is still TODO.
Use this as implementation target in `crates/cli/src/backtest.rs`.

## Implementation Notes (Important)

- Monetary values are represented with `rust_decimal::Decimal` wrappers (`Price`, `Quantity`), not `f64`.
- `tokio::broadcast` is the market-data hot path.
- `OrderRouter` is the single route from strategy actions to connectors.
- Current engine runner only awaits the **first** market-data subscription per strategy; multi-subscription merge is still TODO.

## Next Practical Milestones

1. Wire `trading-cli backtest` to `BacktestHarness` end-to-end.
2. Wire `record` mode to `MarketDataRecorder`.
3. Complete at least one additional live connector (Binance) + market data ingestion.
4. Implement real strategy logic in `pair_trader` and `prediction_quoter`.
5. Replace TODO risk checks with portfolio-level limits and drawdown constraints.
