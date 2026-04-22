---
title: "[AGENT] Phase 1a T5: Multi-market predict.fun in one engine process"
labels: agent-task,phase-1a,difficulty-medium,area-cli
---

## Task
Add a `--configs <path>...` flag to `trading-cli live` that accepts multiple TOMLs and wires all of them into a single engine process with a single MarketDataBus, a single Polymarket WS feed (union of tokens), and one `StrategyInstance` per market's quoter + hedger.

## Context
Maps to `PROJECT_PLAN.md` §1.8.3. This is the user-facing payoff of the Phase 1a refactor: one process replaces the `scripts/farm.py` fleet. Do NOT delete `farm.py` in this ticket — keep both paths working during rollout. Retirement is T7 (validation-gated).

## Files to Touch
- `crates/cli/src/main.rs` (or wherever the Clap `live` subcommand is defined — grep `pub struct.*Cli` / `enum Command`)
- `crates/cli/src/live.rs`

## Cursor prompt

```
Extend the CLI to run multiple predict.fun markets in one obird engine process.

1. In crates/cli/src/main.rs (find the Clap `live` subcommand), change the args:

   Current (likely):
       Live { #[arg(long)] config: String }

   To:
       Live {
           #[arg(long, conflicts_with = "configs")] config: Option<String>,
           #[arg(long, num_args = 1.., conflicts_with = "config")] configs: Vec<String>,
       }

   Dispatch:
       Command::Live { config, configs } => {
           if !configs.is_empty() {
               live::run_multi(&configs).await?
           } else {
               live::run(&config.expect("--config or --configs required")).await?
           }
       }

2. In crates/cli/src/live.rs, add `pub async fn run_multi(paths: &[String]) -> anyhow::Result<()>`
   that:
   - Loads each config via `AppConfig::load`.
   - Asserts all configs use `strategy_type = "prediction_quoter"` (not mixing HL
     and predict). Fail fast with clear error otherwise.
   - Creates ONE MarketDataBus shared across all markets. Pre-register every
     instrument's sender before spawning feeds (per the "Adding an instrument"
     pattern — call `md_bus.sender(&inst)` for each YES, NO, and poly token
     instrument).
   - For each config: builds its PredictFunClient + PredictionQuoter + optional
     PredictHedgeStrategy, inserting the connector Arc's instruments() into the
     shared connectors HashMap.
   - For Polymarket: collects the union of poly_yes_token_id + poly_no_token_id
     across all configs, spawns ONE PolymarketMarketDataFeed with all tokens,
     and builds ONE shared PolymarketExecutionClient (only if any config has
     hedging enabled). Same Arc inserted under every poly InstrumentId.
   - Spawns per-market PredictFunMarketDataFeed tasks.
   - Collects all ShutdownHandles into Vec<_>; on Ctrl+C after the engine exits,
     cancel_all on each.
   - Collects all StrategyInstances (quoter + hedger per market) into one Vec
     and passes to EngineRunner::new.

3. Reuse existing helpers from run_predict (e.g., `parse_instrument`) — don't
   duplicate. Extract shared setup into private functions if clean to do so,
   but keep run_predict (single-config) working as-is.

4. Verify: `cargo build --release && ./target/release/trading-cli live --help`
   shows the new --configs flag. Do NOT run against live markets.
```

## Acceptance Criteria
- [ ] `cargo build --release` passes
- [ ] `trading-cli live --help` shows both `--config` and `--configs`
- [ ] `trading-cli live --configs configs/markets_poly/21177.toml configs/markets_poly/52261.toml` starts up (without funds at risk — kill immediately after startup logs confirm 2 markets + poly feed merged)
- [ ] Startup logs show: 2 `PredictFunMarketDataFeed` tasks, 1 `PolymarketMarketDataFeed` with union of tokens, ≥4 strategy instances (2 quoters + up to 2 hedgers)
- [ ] `run_predict` (single-config) path is unchanged and still works

## Complexity
- [x] Medium (30-60 min) — likely closer to 60

## Blocked by
T4

## Blocks
T7 (validation)
