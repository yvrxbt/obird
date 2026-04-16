# Polymarket Hedging Architecture

> Created: 2026-04-15 (design)
> Updated: 2026-04-16 (Phase 2 implemented)
> Scope: hedge predict.fun fill exposure on Polymarket while preserving points farming.
> Status: **Phase 2 live** — taker hedging on fills. Phase 3 (passive maker + ledger) pending.

---

## 1) Objective

Primary goal:
- Keep predict.fun farming strategy active.
- Reduce directional risk by offsetting fills on Polymarket.
- Prefer hedge fills at neutral or better prices, but allow near-taking behavior when inventory risk is high.

Secondary goals:
- Preserve auditability of every hedge decision.
- Reuse existing obird architecture boundaries (`Strategy → Action → OrderRouter → Connector`).
- Keep compatible with future pair-trader work (Hyperliquid/Binance).

---

## 2) Core Principle

predict.fun strategy remains the **signal source** for inventory changes.
Polymarket hedge is a **risk layer** that reacts to those inventory changes.

Farming and hedging are **decoupled**: if hedge execution fails, farming continues unaffected. Only risk limits tighten.

**Hedge identity (binary market):**
```
predict_fill_YES + hedge_buy_NO = $1 certain payout at resolution (delta neutral)
predict_fill_NO  + hedge_buy_YES = $1 certain payout at resolution (delta neutral)
```

If `P_fill_yes + P_hedge_no < $1` → locked-in profit.
If `P_fill_yes + P_hedge_no = $1` → break-even hedge (pure risk reduction).
If `P_fill_yes + P_hedge_no > $1` → paying a hedge cost (acceptable up to `max_slippage_cents`).

---

## 3) High-level Topology (as implemented)

```
PredictFun WS feed
    │  Event::Fill (predict YES or NO)
    ▼
MarketDataBus  ←──────────────────────────────────┐
    │                                              │
    ├── PredictionQuoter (farming, unchanged)      │
    │                                              │
    └── PredictHedgeStrategy                      │
            │  checks poly_bbo cache               │
            │                                      │
            ▼                                      │
     Action::PlaceOrder(Polymarket, NO token)      │
            │                                      │
     OrderRouter → PolymarketExecutionClient       │
                         │ POST /order (CLOB)       │
                         │ GTC limit at best_ask    │
                         ▼                          │
               Polymarket CLOB execution           │
                                                   │
PolymarketMarketDataFeed (1 WS, YES+NO tokens) ───┘
    Event::BookUpdate → poly_bbo cache in strategy
```

**Key architectural point**: `PredictHedgeStrategy` and `PredictionQuoter` are two separate `StrategyInstance` entries in the same `EngineRunner`. Both receive all events from the `MarketDataBus` for their subscribed instruments. No special plumbing — the engine's `select_all` merged stream handles it.

---

## 4) Market Mapping (per market)

Each predict.fun market that has poly token IDs configured gets a `MarketMapping`:

```rust
MarketMapping {
    predict_yes: InstrumentId(PredictFun, Binary, "143028-Yes"),
    predict_no:  InstrumentId(PredictFun, Binary, "143028-No"),
    poly_yes:    InstrumentId(Polymarket, Binary, "8501497159..."),   // YES token
    poly_no:     InstrumentId(Polymarket, Binary, "2527312495..."),   // NO token
}
```

Internal hedge_map (built in constructor):
```
predict_yes → poly_no   (YES fill → buy NO to flatten)
predict_no  → poly_yes  (NO fill  → buy YES to flatten)
```

**Token ID convention**: Polymarket CLOB token IDs are large unsigned integers stored as decimal strings. They are used directly as `InstrumentId.symbol`. `U256::from_str` handles decimal parsing (no `0x` prefix needed). Index 0 = YES, index 1 = NO in the Gamma API `clobTokenIds` array.

**To find token IDs** for any market:
```bash
curl "https://gamma-api.polymarket.com/markets?clob_token_ids={yes_or_no_token_id}" \
  | python3 -c "import sys,json; m=json.load(sys.stdin)[0]; print(json.loads(m['clobTokenIds']))"
# Returns: ['<yes_token_id>', '<no_token_id>']
```

Or run `trading-cli predict-markets --write-configs` — now writes both `polymarket_yes_token_id` and `polymarket_no_token_id`.

---

## 5) Data and State Model

### 5.1 Per poly-instrument unhedged state (`UnhedgedState`)

```rust
struct UnhedgedState {
    qty: Decimal,                       // accumulated unhedged outcome tokens
    notional: Decimal,                  // USDC notional (qty × predict_fill_price sum)
    first_unhedged_ts: Option<Instant>, // when first unhedged fill arrived (for urgency)
}
```

- `add_fill(qty, price)` — called on predict fill
- `consume_all()` — called optimistically when hedge order is emitted
- `restore(qty, avg_price)` — called on `PlaceFailed` to re-add the qty
- `avg_predict_fill_price()` = `notional / qty` — used for slippage reference

### 5.2 Optimistic position accounting

**Why optimistic?** The architecture doesn't return order IDs synchronously to the strategy — strategies emit `Action::PlaceOrder` and the engine executes it. There's no way to hook the confirmation back to the strategy without adding new plumbing.

**Why it's safe**: GTC orders at `best_ask` are taker orders — they match immediately. The `post_order` response's `status = "matched"` confirms fill. Order failure surfaces as `Event::PlaceFailed` within the same event loop iteration.

**Failure path**: `PlaceFailed` → `on_place_failed(instrument)` → `restore(qty, avg_price)` → next tick re-evaluates and re-hedges.

### 5.3 BBO cache

```rust
poly_bbo: HashMap<InstrumentId, (Price, Price)>  // (best_bid, best_ask)
```

Populated on every `Event::BookUpdate` from `Exchange::Polymarket`. Used in `try_hedge()` for price selection and notional estimation.

### 5.4 Pending hedge map

```rust
pending_hedge: HashMap<InstrumentId, (Decimal, Decimal)>  // (qty, avg_predict_fill_price)
```

Set when hedge order is emitted (optimistically consumed). Cleared on `PlaceFailed` (qty restored) or on `Event::Fill` from Polymarket (confirmed fill logged).

---

## 6) Hedge Decision Logic (`try_hedge`)

Called after each predict fill and on urgency-check ticks.

```
1. params.enabled? No → return (kill-switch)
2. poly_bbo available? No → log HEDGE_SKIP, return
3. hedge_notional = state.qty × poly_ask
4. urgent = first_unhedged_ts.elapsed() >= max_unhedged_duration_secs
5. (hedge_notional < hedge_min_notional) AND NOT urgent → log HEDGE_BATCH debug, return
6. poly_ask > 0.99 → log HEDGE_SKIP, return (market sanity guard)
7. Slippage check (vs Polymarket spread, NOT vs predict fill price):
   poly_mid     = (poly_bid + poly_ask) / 2
   spread_cross = poly_ask - poly_mid        # half the bid-ask spread
   spread_cross > max_slippage_cents → log HEDGE_SKIP "Polymarket spread too wide", return
   NOTE: venue divergence (predict price vs poly price) does NOT block the hedge.
         We hedge for risk reduction, not break-even arbitrage. If predict filled YES
         at 0.67 and poly NO is at 0.46, we accept the 0.13/share hedge cost.
         The combined cost is logged as HEDGE_COST_INFO for auditing only.
8. qty_rounded = state.qty.round_dp(2)
9. qty_rounded < 5 → return (below Polymarket min order size of 5 shares)
10. price = poly_ask.round_dp(2)
11. Optimistic consume: state.consume_all()
12. pending_hedge.insert(poly_inst, (qty, avg_price))
13. emit Action::PlaceOrder(poly_inst, Buy, price, qty, GTC)
```

**Log events emitted:**
- `HEDGE_TRIGGER` — predict fill received, hedge accumulation started
- `HEDGE_BATCH` — (debug) fill accumulated but below min_notional, waiting for more
- `HEDGE_COST_INFO` — informational: combined cost of predict fill + poly hedge vs $1 payout
- `HEDGE_PLAN` — hedge order about to be emitted
- `HEDGE_SKIP` — hedge skipped with reason
- `HEDGE_FILL` — Polymarket fill confirmed (informational)
- `HEDGE_REJECT` — PlaceFailed, qty restored
- `HEDGE_URGENT` — time threshold breached, escalating

---

## 7) Authentication / Execution Details

### 7.1 PolymarketExecutionClient construction

```rust
PolymarketExecutionClient::from_env("PREDICT_PRIVATE_KEY")
```

Only `PREDICT_PRIVATE_KEY` is needed — no separate `POLY_API_KEY` / `POLY_SECRET` / `POLY_PASSPHRASE`.

Flow:
1. Parse `PREDICT_PRIVATE_KEY` as `PrivateKeySigner` (alloy secp256k1)
2. `.with_chain_id(Some(POLYGON))` — chain ID 137 (Polygon mainnet)
3. `Client::new(CLOB_HOST, config).authentication_builder(&signer).authenticate().await`
   - **No `.credentials()` call** — SDK calls `create_or_derive_api_key` instead
   - This creates the API key if it doesn't exist, or retrieves the existing one deterministically
   - The derived key is tied to the wallet address (e.g. `0xA27D22701Bf0f222467673F563e59aA0E38df847`)
4. Signer stored directly in struct (`PrivateKeySigner` field)

Note: `POLY_API_KEY` / `POLY_SECRET` / `POLY_PASSPHRASE` in `.env` are intentionally NOT used. Tested and working as of 2026-04-16.

### 7.2 Order placement details

```
GTC limit order:
  token_id   = U256::from_str(instrument.symbol)  // decimal parse
  price      = req.price.inner()                  // Decimal, already rounded to 0.01
  size       = req.quantity.inner()               // outcome tokens (e.g. 50.0)
  side       = Side::Buy (always for hedge)
  order_type = OrderType::GTC

Signing:
  client.sign(&signer, signable_order).await
  → internally fetches neg_risk flag from /neg-risk/{token_id} (cached by SDK)
  → EIP-712 typed-data signing on Polygon CTFExchange contract
  → signatureType = Eoa (default) or Proxy depending on account setup

Posting:
  client.post_order(signed_order).await
  → PostOrderResponse { success, order_id, status (live/matched/delayed), ... }
  → success=true → track order_id in active_orders map
  → success=false → ConnectorError::OrderRejected
```

### 7.3 Tick size and price bounds

`decimal_precision()` returns `Some(2)` (0.01 tick). Strategy rounds prices to 2dp before emitting. CLOB rejects prices outside `[0.01, 0.99]` — these are checked in `place_order`.

### 7.4 cancel_all on shutdown

`PolymarketExecutionClient::cancel_all` issues `DELETE /orders` with our tracked order IDs (not account-wide cancel). This is called by the engine's shutdown sequence for `Exchange::Polymarket` after the router drains.

---

## 8) Config and Wiring

### 8.1 Enabling hedge in a market config

Two fields required in `[exchanges.params]`:
```toml
polymarket_yes_token_id = "8501497159083948713316135768103773293754490207922884688769443031624417212426"
polymarket_no_token_id  = "2527312495175492857904889758552137141356236738032676480522356889996545113869"
```

**No additional env vars needed** — `PREDICT_PRIVATE_KEY` (already in `.env` for predict.fun) is the only signing key. The SDK derives the Polymarket API key from it automatically.

If `PREDICT_PRIVATE_KEY` is missing: `from_env` returns `Err`, `live.rs` logs a warning and continues in farming-only mode. No crash.

### 8.2 HedgeParams defaults (no TOML required)

Hedge runs on defaults when both token IDs are present. To override, `HedgeParams` would need to be loaded from a `[hedge]` TOML section — this is **not yet wired** (Phase 3 work). Current defaults:

```
hedge_min_notional      = 5 USDC
max_unhedged_notional   = 100 USDC
max_unhedged_duration   = 60 seconds
max_slippage_cents      = 0.05
enabled                 = true
```

### 8.3 Strategy instance naming

The hedge strategy is named `"{quoter_name}_hedge"` (e.g., `"predict_points_v1_hedge"`). Log output includes `strategy=predict_points_v1_hedge` in tracing fields.

---

## 9) Polymarket API Notes

### CLOB endpoints used

| Endpoint         | Method | Usage |
|------------------|--------|-------|
| `/order`         | POST   | Place single order |
| `/order`         | DELETE | Cancel single order |
| `/orders`        | DELETE | Cancel multiple orders (shutdown) |
| `/neg-risk/{id}` | GET    | Negated internally by SDK before signing |

### WebSocket feed

```
wss://ws-subscriptions-clob.polymarket.com/ws/market
```

Subscribe message (BOTH tokens in one connection):
```json
{"type": "market", "assets_ids": ["<yes_token_id>", "<no_token_id>"]}
```

Events handled:
- `book` — full orderbook snapshot (on subscribe + after fills)
- `price_change` — incremental level updates (`size="0"` = remove level)
- `PONG` — heartbeat response (re-publishes last known book state)

**Critical**: `type` must be lowercase `"market"`. Uppercase is silently ignored by server (no error, no data).

### SDK version and features

```toml
polymarket-client-sdk = { version = "0.4.4", features = ["clob"] }
alloy = { version = "1.6", features = ["signer-local"], default-features = false }
```

The `clob` feature gate is required — without it, `polymarket_client_sdk::clob` does not exist. `alloy` is a direct dependency for `PrivateKeySigner` concrete type (SDK re-exports `LocalSigner` but we need the concrete type for struct storage).

---

## 10) Risk Controls (Phase 2 implemented)

| Control | Mechanism |
|---------|-----------|
| Kill-switch | `params.enabled = false` (in-code default; TOML wiring pending) |
| Min notional gate | `hedge_min_notional` — batches fills below threshold |
| Slippage gate | `max_slippage_cents` — skips hedge if ask too far above reference |
| Market sanity | Skips if `poly_ask > 0.99` |
| Urgency escalation | `max_unhedged_duration_secs` — bypasses min_notional gate |
| Qty min | Skips if `qty_rounded < 1` share |
| Shutdown cancel | `cancel_all` on hedge connector at engine exit |

**Not yet implemented (Phase 3):**
- `max_unhedged_notional` threshold for price escalation
- Per-minute order rate limit
- Passive maker pricing tier (Tier A from design doc)
- Structured hedge ledger (append-only audit trail)
- HEDGE_SUMMARY daily log

---

## 11) Fill Event Routing (How Predict Fills Reach the Hedge Strategy)

predict.fun fills flow through the **MarketDataBus**, not the order router's `strategy_txs` channel:

1. `PredictFunMarketDataFeed::run()` receives WS wallet event (order filled)
2. Looks up `instrument` in `placed_instruments` map (YES or NO instrument)
3. Calls `sink.publish(instrument, Event::Fill { instrument, fill })`
4. `MarketDataBus` broadcasts to all subscribers of that instrument
5. `PredictHedgeStrategy` is subscribed to `predict_yes` and `predict_no` instruments
6. Its `merged_md` stream delivers the fill → `on_event(Event::Fill)` → `on_predict_fill()`

**Important**: `Event::Fill` carries `fill.order_id` (the order hash). The hedge strategy does not use this for routing — it routes by `instrument.exchange == Exchange::PredictFun`.

---

## 12) Implementation Phases

### Phase 0 — Design freeze ✅ (2026-04-15)
- API semantics confirmed from Polymarket docs
- SDK selected (`polymarket-client-sdk 0.4.4`, `clob` feature)
- Hedge direction confirmed: fill YES → buy NO; fill NO → buy YES

### Phase 1 — Paper hedge
**Status**: Skipped. Went directly to Phase 2 per user decision.

### Phase 2 — Live taker hedge ✅ (2026-04-16)
- `PolymarketExecutionClient` implementing `ExchangeConnector`
- `PredictHedgeStrategy` implementing `Strategy`
- GTC orders at `best_ask` (taker, immediate fill)
- Slippage guard, min-notional batching, urgency escalation
- Wired into `run_predict` alongside existing `PredictionQuoter`
- Market 143028 config updated with NO token ID
- `predict-markets --write-configs` updated to write both token IDs

### Phase 3 — Passive maker hedge (next)
- **Price tiers**: Tier A = best_bid+1tick (passive); Tier B = join; Tier C = best_ask (current)
- **Tier selection**: by `unhedged_notional / max_unhedged_notional` ratio and urgency
- **Hedge ledger**: append-only JSONL log with full decision context per hedge attempt
- **TOML config wiring**: load `HedgeParams` from `[hedge]` section
- **Polymarket user WS**: subscribe to `/ws/user` for real-time fill confirmation (replaces optimistic tracking)
- **Rate limiting**: `max_hedge_orders_per_min` enforced in strategy

### Phase 4 — Unified risk core
- Shared `HedgeController` / `RiskManager` across predict/poly pair and HL/Binance pair
- Per-portfolio delta tracking across all venues
- Integrates with future `fair_value_service` binary

---

## 13) Next Steps for Continuity

If picking this up in a new session, check the following in order:

1. **Read this file** (done) + `CHANGELOG.md` 2026-04-16 entry
2. **Check current state**: `cargo build --release` — should compile clean
3. **Verify token IDs are correct** for whichever markets are live: `configs/markets_poly/*.toml`
4. **Confirm POLY_* env vars** are in `.env`
5. **Confirm USDC balance** on Polygon at the predict.fun wallet address
6. **Test run**: `source .env && RUST_LOG=quoter=info cargo run --release --bin trading-cli -- live --config configs/markets_poly/143028.toml`
7. Look for `PolymarketExecutionClient ready` and `PredictHedgeStrategy initialized` in logs
8. After first predict fill: look for `HEDGE_TRIGGER` → `HEDGE_PLAN` → `POLY_PLACE` sequence

**If hedge is not triggering**: check that `polymarket_no_token_id` is set in the market config AND that `PREDICT_PRIVATE_KEY` is exported (it always should be — required for predict.fun too).

**If `HEDGE_SKIP slippage exceeds limit`**: poly ask has moved far from predict fill price. Either widen `max_slippage_cents` or accept the position.

**If `HEDGE_REJECT placement failed`**: check Polymarket account USDC balance and that the API key is valid. `POLY_PLACE` log errors will have SDK error details.
