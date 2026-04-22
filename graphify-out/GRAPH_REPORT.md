# Graph Report - .  (2026-04-16)

## Corpus Check
- 85 files · ~58,783 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 476 nodes · 665 edges · 41 communities detected
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 6 edges (avg confidence: 0.84)
- Token cost: 0 input · 0 output

## God Nodes (most connected - your core abstractions)
1. `PredictFunClient` - 25 edges
2. `BinanceClient` - 23 edges
3. `PredictionQuoter` - 18 edges
4. `HyperliquidClient` - 18 edges
5. `HlSpreadQuoter` - 15 edges
6. `calculate()` - 14 edges
7. `make_book()` - 13 edges
8. `SimConnector` - 12 edges
9. `PolymarketExecutionClient` - 12 edges
10. `PredictHedgeStrategy` - 11 edges

## Surprising Connections (you probably didn't know these)
- `AuditEntry (Structured Decision Record)` --semantically_similar_to--> `FairValueMessage (Wire Format)`  [INFERRED] [semantically similar]
  crates/telemetry/src/audit.rs → crates/fair_value_service/src/publisher.rs
- `Fair Value Service Entry Point` --calls--> `FairValuePublisher (UDS Broadcaster)`  [INFERRED]
  crates/fair_value_service/src/main.rs → crates/fair_value_service/src/publisher.rs
- `Fair Value Service Entry Point` --calls--> `FairValueModel (Probability Estimator)`  [INFERRED]
  crates/fair_value_service/src/main.rs → crates/fair_value_service/src/model.rs
- `Fair Value Service Entry Point` --calls--> `PriceAggregator (Multi-Exchange Feature Extractor)`  [INFERRED]
  crates/fair_value_service/src/main.rs → crates/fair_value_service/src/data_source.rs
- `FairValueModel (Probability Estimator)` --shares_data_with--> `FairValueMessage (Wire Format)`  [INFERRED]
  crates/fair_value_service/src/model.rs → crates/fair_value_service/src/publisher.rs

## Communities

### Community 0 - "Community 0"
Cohesion: 0.05
Nodes (24): Action, ExchangeConnector, Price, Exchange, InstrumentId, InstrumentKind, FillModel, SimOrder (+16 more)

### Community 1 - "Community 1"
Cohesion: 0.06
Nodes (24): BatchOrderResult, BookState, BookTickerMsg, fill(), ListenKeyResponse, now_ns(), open_order_from_rest(), OpenOrderResponse (+16 more)

### Community 2 - "Community 2"
Cohesion: 0.08
Nodes (8): GammaMarket, HyperliquidClient, OrderEntry, PredictFunParams, PredictShutdownHandle, resolve_symbol(), ResolvedMarket, ShutdownHandle

### Community 3 - "Community 3"
Cohesion: 0.1
Nodes (6): MarketMapping, PredictHedgeStrategy, Strategy, StrategyState, UnhedgedState, PairTrader

### Community 4 - "Community 4"
Cohesion: 0.11
Nodes (15): AssetInfo, BinanceMarketDataFeed, create_listen_key(), HlMarketDataFeed, MarketDataSink, OrderbookSnapshot, parse_ts_ms(), PolymarketMarketDataFeed (+7 more)

### Community 5 - "Community 5"
Cohesion: 0.11
Nodes (1): PredictFunClient

### Community 6 - "Community 6"
Cohesion: 0.14
Nodes (2): BinanceClient, urlencoding_simple()

### Community 7 - "Community 7"
Cohesion: 0.23
Nodes (16): calculate(), fill_safety_spread_cents_from_predict_mid(), large_downward_divergence_no_scores_at_spread_cents(), large_downward_divergence_yes_clamped_no_placed(), large_upward_divergence_yes_placed_no_clamped(), make_book(), mid(), moderate_divergence_both_sides_placed() (+8 more)

### Community 8 - "Community 8"
Cohesion: 0.19
Nodes (3): CycleRecord, HlSpreadQuoter, State

### Community 9 - "Community 9"
Cohesion: 0.2
Nodes (2): epoch_to_ymd_hms(), PredictionQuoter

### Community 10 - "Community 10"
Cohesion: 0.14
Nodes (6): default_hedge_min_notional(), default_max_slippage_cents(), default_max_unhedged_duration_secs(), default_max_unhedged_notional(), HedgeParams, QuoterParams

### Community 11 - "Community 11"
Cohesion: 0.23
Nodes (5): main(), MarketProcess, parse_args(), True if we've restarted too many times in CRASH_WINDOW_SECS., write_pid_file()

### Community 12 - "Community 12"
Cohesion: 0.19
Nodes (4): Event, DataRecorder, MarketDataRecorder, SimMarketDataFeed

### Community 13 - "Community 13"
Cohesion: 0.2
Nodes (1): SimConnector

### Community 14 - "Community 14"
Cohesion: 0.21
Nodes (1): PolymarketExecutionClient

### Community 15 - "Community 15"
Cohesion: 0.28
Nodes (1): MatchingEngine

### Community 16 - "Community 16"
Cohesion: 0.5
Nodes (1): PredictFunMarketDataFeed

### Community 17 - "Community 17"
Cohesion: 0.25
Nodes (5): AppConfig, EngineConfig, ExchangeConfig, StrategyConfig, TelemetryConfig

### Community 18 - "Community 18"
Cohesion: 0.38
Nodes (7): PriceAggregator (Multi-Exchange Feature Extractor), Fair Value Service Entry Point, FairValueModel (Probability Estimator), FairValueMessage (Wire Format), FairValuePublisher (UDS Broadcaster), AuditEntry (Structured Decision Record), AuditLogger (Decision Rationale Trail)

### Community 19 - "Community 19"
Cohesion: 0.38
Nodes (1): MarketDataBus

### Community 20 - "Community 20"
Cohesion: 0.33
Nodes (1): Quantity

### Community 21 - "Community 21"
Cohesion: 0.33
Nodes (1): PositionTracker

### Community 22 - "Community 22"
Cohesion: 0.4
Nodes (2): EngineRunner, StrategyInstance

### Community 23 - "Community 23"
Cohesion: 0.4
Nodes (1): SpreadModel

### Community 24 - "Community 24"
Cohesion: 0.4
Nodes (1): TradeLogger

### Community 25 - "Community 25"
Cohesion: 0.6
Nodes (1): BacktestHarness

### Community 26 - "Community 26"
Cohesion: 0.8
Nodes (4): parse_instrument(), run(), run_hl(), run_predict()

### Community 27 - "Community 27"
Cohesion: 0.6
Nodes (1): OrderRouter

### Community 28 - "Community 28"
Cohesion: 0.5
Nodes (4): LighterClient (stub), Lighter Connector Crate Root, Lighter Market Data Module, Lighter Normalize Module

### Community 29 - "Community 29"
Cohesion: 1.0
Nodes (2): flag_value(), main()

### Community 30 - "Community 30"
Cohesion: 1.0
Nodes (0): 

### Community 31 - "Community 31"
Cohesion: 1.0
Nodes (0): 

### Community 32 - "Community 32"
Cohesion: 1.0
Nodes (0): 

### Community 33 - "Community 33"
Cohesion: 1.0
Nodes (1): PairTraderParams

### Community 34 - "Community 34"
Cohesion: 1.0
Nodes (1): TradingMetrics (Prometheus Metrics Schema)

### Community 35 - "Community 35"
Cohesion: 1.0
Nodes (1): ConnectorError

### Community 36 - "Community 36"
Cohesion: 1.0
Nodes (1): RiskRejection

### Community 37 - "Community 37"
Cohesion: 1.0
Nodes (1): Position

### Community 38 - "Community 38"
Cohesion: 1.0
Nodes (1): Fill

### Community 39 - "Community 39"
Cohesion: 1.0
Nodes (1): CLI Backtest Mode (stub)

### Community 40 - "Community 40"
Cohesion: 1.0
Nodes (1): CLI Record Mode (stub)

## Knowledge Gaps
- **54 isolated node(s):** `True if we've restarted too many times in CRASH_WINDOW_SECS.`, `PairTraderParams`, `CycleRecord`, `MarketMapping`, `AuditLogger (Decision Rationale Trail)` (+49 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **Thin community `Community 30`** (2 nodes): `tracing_setup.rs`, `init_tracing()`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 31`** (2 nodes): `poly_check.rs`, `run()`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 32`** (2 nodes): `predict_approve.rs`, `run()`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 33`** (1 nodes): `PairTraderParams`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 34`** (1 nodes): `TradingMetrics (Prometheus Metrics Schema)`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 35`** (1 nodes): `ConnectorError`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 36`** (1 nodes): `RiskRejection`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 37`** (1 nodes): `Position`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 38`** (1 nodes): `Fill`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 39`** (1 nodes): `CLI Backtest Mode (stub)`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 40`** (1 nodes): `CLI Record Mode (stub)`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `PredictFunClient` connect `Community 5` to `Community 2`?**
  _High betweenness centrality (0.078) - this node is a cross-community bridge._
- **Why does `BinanceClient` connect `Community 6` to `Community 2`?**
  _High betweenness centrality (0.070) - this node is a cross-community bridge._
- **Why does `PredictionQuoter` connect `Community 9` to `Community 8`?**
  _High betweenness centrality (0.054) - this node is a cross-community bridge._
- **What connects `True if we've restarted too many times in CRASH_WINDOW_SECS.`, `PairTraderParams`, `CycleRecord` to the rest of the system?**
  _54 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Community 0` be split into smaller, more focused modules?**
  _Cohesion score 0.05 - nodes in this community are weakly interconnected._
- **Should `Community 1` be split into smaller, more focused modules?**
  _Cohesion score 0.06 - nodes in this community are weakly interconnected._
- **Should `Community 2` be split into smaller, more focused modules?**
  _Cohesion score 0.08 - nodes in this community are weakly interconnected._