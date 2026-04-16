# Changelog

All notable iterations, experiments, and decisions are logged here.
This file is designed to give LLMs context on what has been tried and why.

## 2026-04-16 — Polymarket Hedge Execution Layer (Phase 2)

**Branch:** `master`

**What shipped:**

### New crate: `crates/strategies/predict_hedger/`

`PredictHedgeStrategy` — a second strategy that runs alongside `PredictionQuoter` in the same engine process. Its sole job is to hedge predict.fun fill exposure by placing opposite-side orders on Polymarket.

**Core hedge logic:**
- predict.fun is BUY-only (always buys outcome tokens)
- When predict.fun fills YES → buy NO on Polymarket (YES + NO = $1, so both = flat delta)
- When predict.fun fills NO → buy YES on Polymarket
- Same quantity, opposite outcome token

**Key implementation details:**

- `MarketMapping` struct: wires `(predict_yes, predict_no, poly_yes_token, poly_no_token)` per market
- `UnhedgedState` per (poly_instrument): accumulates qty + USDC notional of unhedged predict fills
- Pricing: GTC limit at `best_ask` (taker fill, immediate execution)
- Slippage guard: reference price = `1 - avg_predict_fill_price`. Skips if `poly_ask > reference + max_slippage_cents`
- **Optimistic consume**: unhedged qty is consumed when `Action::PlaceOrder` is emitted (not on fill confirmation). On `Event::PlaceFailed`, qty is restored to `unhedged` for retry.
- Urgency check on every `Event::Tick`: if `first_unhedged_ts` age > `max_unhedged_duration_secs`, triggers hedge regardless of min_notional
- `HedgeParams` (all have defaults, no TOML required to get running):
  - `hedge_min_notional` = 5 USDC — batches tiny fills before placing
  - `max_unhedged_notional` = 100 USDC — (for future urgency-tier logic)
  - `max_unhedged_duration_secs` = 60 — escalation timer
  - `max_slippage_cents` = 0.05 — max acceptable price above reference
  - `enabled` = true — kill-switch

**Fill event routing**: predict.fun `Fill` events are published by `PredictFunMarketDataFeed` to the `MarketDataBus`. Since `PredictHedgeStrategy::subscriptions()` includes both predict instruments, these events are delivered via the existing `merged_md` stream — no new plumbing needed.

---

### New file: `crates/connectors/polymarket/src/execution.rs`

`PolymarketExecutionClient` — implements `ExchangeConnector` for `Exchange::Polymarket`.

**Auth model:**
- REST auth: HMAC-SHA256 per request, using `POLY_API_KEY` / `POLY_SECRET` / `POLY_PASSPHRASE`
- Order signing: EIP-712 per order, using `PREDICT_PRIVATE_KEY` (shared with predict.fun)
- Uses pre-existing credentials via `Credentials::new(key_uuid, secret, passphrase)` — does NOT create new API keys
- SDK: `polymarket-client-sdk = "0.4.4"` with `features = ["clob"]`

**Order flow:**
1. `limit_order()` builder → `.token_id(U256::from_str(&instrument.symbol))` → `.price()` → `.size()` → `.side(Side::Buy)` → `.order_type(OrderType::GTC)`
2. `client.sign(&signer, signable_order)` — EIP-712 signed by `PrivateKeySigner`
3. `client.post_order(signed_order)` → checks `resp.success`, tracks `resp.order_id` in `active_orders`

**Key decisions:**
- Token ID stored as `instrument.symbol` string (large decimal number like `"8501497..."`). Parsed to `U256` via `U256::from_str` (decimal-first, no `0x` prefix needed).
- `PrivateKeySigner` stored directly in struct — avoids re-parsing the key on every sign call
- `cancel_all`: cancels only our tracked orders (not account-wide), matching predict.fun pattern
- `decimal_precision()` returns `Some(2)` — 0.01 tick is standard on Polymarket CLOB
- `positions()` returns empty — position tracking done in strategy via fill events

---

### Modified: `crates/connectors/polymarket/src/lib.rs`

Added `pub mod execution` alongside existing `client`, `market_data`, `normalize`.

---

### Modified: `crates/connectors/predict_fun/src/client.rs`

Added `polymarket_no_token_id: Option<String>` to `PredictFunParams`.

- Previously only YES token ID was stored (for FV subscription)
- NO token ID is needed so the hedge strategy can: (a) subscribe to the NO book for pricing, (b) place buy orders on the NO token
- Both fields optional — hedge is disabled if either is absent

---

### Modified: `crates/cli/src/live.rs` (run_predict)

**New wiring in `run_predict`:**

1. **NO token instrument** built from `polymarket_no_token_id` (same pattern as FV instrument)
2. **Single WS feed** now subscribes to BOTH YES and NO tokens — `PolymarketMarketDataFeed::new(vec![(yes_token_id, yes_inst), (no_token_id, no_inst)])` — one connection handles both
3. **Conditional hedge path**: if both `polymarket_yes_token_id` AND `polymarket_no_token_id` are set:
   - Calls `PolymarketExecutionClient::from_env("POLY_API_KEY", "POLY_SECRET", "POLY_PASSPHRASE", secret_key_env)` — fails gracefully with a warning if env vars are absent
   - Builds `PredictHedgeStrategy` with the market mapping
   - Adds `Exchange::Polymarket → Box::new(poly_client)` to the connectors map
   - Appends hedger to the strategies vec (both strategies share the same `EngineRunner`)
4. If init fails: logs warning, continues with farming-only mode (no crash)

**Required env vars**: none new. `PREDICT_PRIVATE_KEY` (already in `.env` for predict.fun) is sufficient.

The SDK derives the Polymarket API key deterministically from the private key via `create_or_derive_api_key` — no separate `POLY_API_KEY` / `POLY_SECRET` / `POLY_PASSPHRASE` needed. Initially attempted to use pre-existing `POLY_*` creds but they returned 401 (likely stale/wrong address). Key derivation works and is the correct long-term approach.

Verified working (`poly-check --live`): place + cancel on market 143028 NO token.

---

### Modified: `configs/markets_poly/143028.toml`

Added:
```toml
polymarket_no_token_id  = "2527312495175492857904889758552137141356236738032676480522356889996545113869"
```

NO token ID derived via `GET gamma-api.polymarket.com/markets?clob_token_ids={yes_token_id}` — returns `clobTokenIds` array, index 0 = YES, index 1 = NO.

Market 143028 poly tokens:
- YES: `8501497159083948713316135768103773293754490207922884688769443031624417212426`
- NO:  `2527312495175492857904889758552137141356236738032676480522356889996545113869`

---

### Modified: `crates/cli/src/predict_markets.rs`

`render_market_toml` now accepts and writes `polymarket_no_token_id: Option<&str>` — future `--write-configs` runs will auto-populate both fields for all poly-linked markets.

---

**Operational notes:**
- No new CLI flags — hedge auto-enables when both token IDs are present
- `PREDICT_PRIVATE_KEY` only — no separate `POLY_API_KEY` / `POLY_SECRET` / `POLY_PASSPHRASE`; SDK derives key automatically
- Prerequisite: USDC deposited on Polygon at `0xA27D22701Bf0f222467673F563e59aA0E38df847`
- On shutdown: `cancel_all` fires for Polymarket tracked orders (same path as predict.fun)

**First live run observations (2026-04-16):**

1. **Poly FV timing gap**: initial book dump from Polymarket WS arrives before strategy subscribes to `MarketDataBus` (broadcast receivers don't see pre-subscription events). Strategy logs "Waiting for first Polymarket FV update before quoting" until the PONG re-publish fires (~10s). Fix: not needed — 10s startup lag is acceptable. Root cause documented here for awareness.

2. **YES position above max**: Market 143028 had `yes_tokens=724.08` vs `max_position_tokens=500` at startup. Quoter correctly skips YES bids when YES ≥ max. Result: only NO bids placed. To resume YES quoting, either raise `max_position_tokens` to e.g. 800, or use `predict-liquidate` to sell down YES.

3. **`poly-check --live` confirmed**: auth, books, order place+cancel all work end-to-end. Min order size on Polymarket CLOB is 5 shares (not 1).

4. **Hedge is passive by design**: hedge strategy fires only on predict fills, not on every tick. This is correct — farming is the primary activity, hedging is reactive.

5. **Slippage guard bug found and fixed**: Original slippage reference used `1 - predict_fill_price` (break-even check). When predict and Polymarket prices diverge (e.g. predict=0.67 vs poly=0.54), this always rejects. Fixed to use `poly_ask - poly_mid` (Polymarket half-spread check). Now the hedge fires regardless of venue divergence; the cost relative to predict fill price is logged as `HEDGE_COST_INFO` for auditing only. The `max_slippage_cents` parameter now means "max half-spread to cross on Polymarket" (not "max cost above break-even"). With Polymarket's tight 0.01 spreads this should never block a hedge.

6. **min_notional silent skip**: First fill (9 shares, $4.18 notional) was silently skipped because it was below `hedge_min_notional=$5`. Now logs at DEBUG level so the batching is visible.

**What's NOT done yet (Phase 3 / next session):**
- Passive maker pricing (place at best_bid + tick, not best_ask)
- Urgency price tiers (Tier A/B/C from architecture doc)
- Polymarket user WS feed for real-time fill confirmation (currently optimistic)
- Multi-market hedge support (currently one mapping per run_predict call)
- Hedge ledger / structured audit log (HEDGE_SUMMARY etc.)
- Kill-switch wired to config TOML (`[hedge] enabled = false`)
- HedgeParams not yet loaded from config — defaults only

---

## 2026-04-15 — Predict Ops Hardening, Liquidation CLI, Touch-Risk Requote

**Commits:** `4b62a68`, `f44a8f8`, `049401c`, `3315ce0`, `eff1367`

**What shipped:**

1) **Market discovery strictness** (`predict-markets`)
- Added `--fail-on-missing-poly-token`.
- In strict mode, markets missing `polymarket_yes_token_id` are skipped from config writes and command exits non-zero.
- Purpose: prevent accidentally farming markets that cannot be poly-anchored.

2) **Startup safety gate for predict quoter**
- Quoter now requires fresh Polymarket FV before quoting.
- If FV is stale/unavailable, quotes are pulled and strategy pauses.
- Removed blind fallback-to-predict-mid behavior for poly-configured markets.

3) **Passive liquidation command** (`predict-liquidate`)
- New CLI command to unwind positions with SELL LIMITs (no market crossing intent).
- Cancels existing open orders on the target market, computes passive sell levels from current book, supports `--dry-run`.

4) **Touch-risk requote iteration (audit trail)**
- Initial top-of-book trigger caused cancel/replace thrash under certain book states.
- Refined to **ask-distance hit-risk trigger** + **risk latch**:
  - trigger when resting bid gets within `touch_trigger_cents` of ask
  - requote out by `touch_retreat_cents`
  - one trigger per risk-regime entry (avoids immediate retrigger loops)
- Pricing remains poly-anchored with scoring-window clamp (`spread_threshold_v`) as farming guardrail.

5) **Config/docs sync updates**
- Added new strategy knobs to templates/generator output:
  - `touch_trigger_cents`
  - `touch_retreat_cents`
- Updated 143028 live config and startup logging to expose these parameters.

**Operational observations:**
- 143028 session became strongly YES-heavy; rough MTM can diverge materially from resolution payoff.
- Shutdown safety: graceful cancellation path is tied to SIGINT/Ctrl-C in runner flow; avoid SIGKILL.

---

## 2026-04-13 — Inventory Skew, P&L Tracking, DataRecorder

**What shipped:**

*Strategy improvements (`crates/strategies/hl_spread_quoter/`):*
- **Inventory skew**: quotes placed around `reservation_mid = mid - skew_factor_bps_per_unit * net_pos`. When long, asks shift cheaper to steer mean-reversion. `skew_factor_bps_per_unit=50` in config = 5 bps shift at max_position (0.1 ETH).
- **Session P&L tracking**: every FILL log now includes `session_pnl` (running cash flow) and `mark_pnl` (cash + position marked to current mid). SHUTDOWN log emits session summary.
- **Duplicate CancelAll fix**: `pull_quotes()` returns empty when already in Cooldown state — eliminates spurious zero-latency ROUNDTRIP logs from double-fill notifications.
- **`taker_fee_bps` param** added to config (default 0.2 = HL maker rebate) — used for P&L accuracy.
- `max_drift_bps()` uses raw mid (not reservation) so drift responds to market movement only, not inventory changes.

*Data capture (`crates/telemetry/src/recorder.rs` — new):*
- **`DataRecorder`**: background task subscribed to MarketDataBus. Writes clean JSONL for quant analysis:
  - `logs/data/bbo-YYYY-MM-DD.jsonl` — one line per BBO tick with `exchange_ts_ns`, `local_ts_ns`, bid/ask px+sz
  - `logs/data/fills-YYYY-MM-DD.jsonl` — one line per fill, flushed immediately
- Wired into `live.rs` as a separate subscriber — zero coupling to strategy path.
- BBO flush interval: every 500 records (bounded crash data loss). Fills always sync-flushed.

*Operations:*
- `RUNBOOK.md` added — full operational guide: pre-flight, monitoring, tuning, exit criteria.
- `cancel_all` per-OID `BatchCancel` confirmed correct approach. `scheduleCancel` removed (required $1M volume, cancelled all instruments — unsafe for multi-strategy). CLAUDE.md + docs updated to reflect this.
- ARCHITECTURE.md updated to current actual state.

**Config additions** (`configs/quoter.toml`):
```
skew_factor_bps_per_unit = "50"
taker_fee_bps            = "0.2"
```

**Observed behavior (US-East server, pre-Tokyo):**
- cancel_ms median ~398ms, place_ms ~345ms, total ~743ms
- Expected from Tokyo: ~40ms total
- Net position drifting short across sessions

**Next:** Deploy Tokyo → validate latency → run 4+ hour sessions → check RUNBOOK.md exit criteria → wire Binance as reference price source.

---

## 2026-04-13 — HL MM Live (Initial)

**What shipped:**
- `HlSpreadQuoter` strategy: two-level symmetric spread MM, order-price-based drift detection
- Always cancel-first: every requote is `[CancelAll, PlaceOrder×N]` as one atomic batch
- `cancel_all` uses per-OID `BatchCancel` — tracks OIDs from place_batch responses, works for all accounts regardless of traded volume
- `place_batch` uses HL `BatchOrder` — all 4 level orders in one REST call
- Startup loads real positions from HL (`clearinghouse_state`) — strategy survives restarts
- Graceful shutdown: `ShutdownHandle` does per-OID `BatchCancel` before process exits
- Persistent JSON line logging to `logs/obird-YYYY-MM-DD.jsonl` — every mid + drift at DEBUG
- `MarketDataSink` trait added to core — distributed transport seam without touching strategies
- `MarketDataBus` made `Arc`-safe — shared between feed task and engine runner
- `EngineRunner` uses `futures::stream::select_all` — all instruments polled fairly (pair-trade ready)
- Action channel changed to `Vec<Action>` batches — enables concurrent cross-exchange leg dispatch
- `OrderRouter` groups PlaceOrder by exchange, fires cross-exchange legs via `join_all`
- L2Book WS subscription for real BBO + exchange timestamps (replaced AllMids)

**Key bug fixes:**
- Price rounding: `tick.normalize().scale()` not `tick.scale()` (was returning 2 for 0.1 tick)
- Fill cancel: use per-OID `BatchCancel` (OIDs tracked via `active_oids` set populated by `place_batch`)
- Multi-receiver: `select_all` replaces first-only polling (silent starvation on second instrument)
- `scheduleCancel` removed: required $1M traded volume, broke on first run with stale orders

**Config:** `configs/quoter.toml` — `level_bps=[5,10]`, `drift_bps=3`, `drift_pause_secs=3`, `fill_pause_secs=10`, `order_size=0.01`, `max_position=0.1`

## 2026-04-11 — Initial Architecture

- Designed tiered messaging architecture (broadcast channels, not NATS on hot path)
- Chose single-binary multi-exchange design with OrderRouter
- Separated fair value service from strategy engine
- Defined core traits: Strategy, ExchangeConnector, RiskCheck
- Established workspace layout with 13 crates
- Created LLM-friendly documentation pattern (CONTEXT.md + ADRs)
