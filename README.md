# obird — Rust HFT Framework

A multi-crate Rust workspace for low-latency trading execution across two verticals:

1. **Crypto perp market-making** — Hyperliquid ETH spread MM, live on mainnet
2. **Prediction market points farming** — predict.fun dual-BUY quoter with Polymarket FV anchor

---

## What's live

| Component | Status | Notes |
|---|---|---|
| `HlSpreadQuoter` | ✅ Live mainnet | 2-level symmetric MM, inventory skew, session P&L |
| `PredictionQuoter` | ✅ Live mainnet | Conservative dual-FV pricing, poly FV gate |
| Hyperliquid connector | ✅ Full | place/cancel/modify, per-OID BatchCancel, L2Book WS |
| Polymarket connector | ✅ Full | CLOB WS feed, text PING/PONG heartbeat, multi-token |
| predict.fun connector | ✅ Full | EIP-712 signing, yield-bearing + negRisk contracts |
| `predict-markets` CLI | ✅ Full | Auto-generates market configs with poly token resolution |
| `predict-liquidate` CLI | ✅ Full | Passive limit-only position unwind helper |
| Multi-market farm | ✅ Full | `scripts/farm.py` — N markets, crash-loop protection |
| Binance connector | ⚠️ Built | Not wired into live runner |
| Backtest harness | ⚠️ Partial | Primitives exist in `crates/backtest`, CLI not wired |
| Risk manager | ⚠️ Stub | Passes all orders, no portfolio limits yet |

---

## Quick start

### HL spread MM

```bash
source .env   # needs HL_SECRET_KEY
cargo build --release --bin trading-cli
RUST_LOG=quoter=info,connector_hyperliquid=info \
  ./target/release/trading-cli live --config configs/quoter.toml
```

### predict.fun multi-market farm

```bash
source .env   # needs PREDICT_API_KEY, PREDICT_PRIVATE_KEY
cargo build --release --bin trading-cli

# Discover live boosted markets and write configs (poly-linked only)
./target/release/trading-cli predict-markets \
  --all --write-configs --fail-on-missing-poly-token \
  --output-dir configs/markets_poly

# Optional: passive unwind (dry-run first)
./target/release/trading-cli predict-liquidate --dry-run --config configs/markets_poly/143028.toml

# Start all markets
python3 scripts/farm.py
```

See `PREDICT_RUNBOOK.md` and `PREDICT_QUOTING_DESIGN.md` for full operating guide.

---

## Architecture overview

```
Exchange connectors (WS feeds)
       │  BookUpdate events
       ▼
  MarketDataBus  (tokio::broadcast, per instrument)
       │
       ▼
  Strategy  ──on_event──►  Action::PlaceOrder / CancelAll / ...
  (pure logic, no exchange imports)
       │
       ▼
  OrderRouter  (risk gate, routes by exchange)
       │
       ▼
  OrderManager  (per-exchange serialisation)
       │
       ▼
  ExchangeConnector  (REST + WS, per venue)
```

Key invariants — see `CLAUDE.md` and `ARCHITECTURE.md`:
- Strategies never import connector crates
- `Action` is the only way strategies express intent
- `Event` is the only way the engine talks to strategies
- `MarketDataSink` trait decouples feed publishing from bus implementation

---

## Workspace structure

```
crates/
├── core/                  # Shared types, traits, Event/Action enums
├── engine/                # Runner, router, risk, MarketDataBus
├── backtest/              # SimConnector, replay feed, report
├── cli/                   # trading-cli entrypoint (live / predict-markets / backtest)
├── connectors/
│   ├── hyperliquid/       # ✅ Full — place/cancel/BBO WS
│   ├── polymarket/        # ✅ Full — CLOB WS FV feed (text PING/PONG, multi-token)
│   ├── predict_fun/       # ✅ Full — EIP-712 signing, fill tracking, cancel-all
│   └── binance/           # ⚠️  Built, not wired
└── strategies/
    ├── hl_spread_quoter/  # ✅ Live — 2-level symmetric MM, inventory skew
    └── prediction_quoter/ # ✅ Live — conservative dual-FV, scoring-window pricing
configs/
├── quoter.toml            # HL MM config
├── markets_poly/          # Auto-generated predict.fun configs (one per market)
scripts/
├── farm.py                # Multi-market launcher with crash-loop protection
decisions/                 # ADRs (001–006)
```

---

## Build

```bash
cargo build --workspace           # debug
cargo build --release --bin trading-cli   # release (use for live)
cargo test --workspace
cargo clippy --workspace
```

---

## Key environment variables

| Variable | Used by | Required |
|---|---|---|
| `HL_SECRET_KEY` | Hyperliquid connector | ✅ for HL MM |
| `PREDICT_API_KEY` | predict.fun connector | ✅ for farming |
| `PREDICT_PRIVATE_KEY` | predict.fun connector | ✅ for farming |

---

## Runbooks

| What | Document |
|---|---|
| HL MM live ops, monitoring, P&L | `RUNBOOK.md` |
| predict.fun farming ops, market refresh, log analysis | `PREDICT_RUNBOOK.md` |
| Pricing decision tree, FV logic, tuning knobs | `PREDICT_QUOTING_DESIGN.md` |
| Historical bug fixes and strategy evolution | `PREDICT_FARMING_NOTES.md` |
| Polymarket hedge roadmap + architecture (design) | `POLY_HEDGING_ARCHITECTURE.md` |
| Architecture deep-dive and invariants | `ARCHITECTURE.md` |
