# obird — Agent / Human Handoff Guide

> **Read this first if you are an LLM, coding agent, or human picking up work on obird.**
> This file is the canonical "where am I / what do I do now" document.
> Last meaningful update: 2026-04-22 — Phase 1 ticketing complete.

---

## 1. What obird is (30-second version)

Rust HFT trading workspace. Two verticals in one binary:
- **Hyperliquid ETH perp market-making** — live mainnet (`HlSpreadQuoter`)
- **predict.fun points farming with Polymarket fair-value anchor and delta-neutral hedging** — live mainnet (`PredictionQuoter` + `PredictHedgeStrategy`)

**Key constraint**: the farmer is *not currently running*. The user has paused to re-architect for scale. Phase 1 of the v2 refactor is fully ticketed and ready to execute.

Entry-point architecture docs (ordered most-to-least useful):
1. **`README.md`** — architecture, data flow, code flow, invariants
2. **`PREDICTION_MARKETS.md`** — prediction-market ops/design
3. **`DEX_CEX_MM.md`** — HL MM ops + Binance wiring plan
4. **`PRD_FARMING_PLATFORM.md`** — v2 target architecture (NATS, QuestDB, 3-region AWS)
5. **`PROJECT_PLAN.md`** — phased execution plan with deliverables
6. **`.claude/CLAUDE.md`** — terse invariants + current-state reminders

---

## 2. Current state (as of the last commit touching this file)

### Done
- Doc consolidation: 18 stale/redundant markdown files deleted, replaced by the 5 canonical docs above
- PRD reconciliation: v1→v2 delta table added at `PRD_FARMING_PLATFORM.md §0.1`; stale refs fixed; Appendix C updated
- Phase 1 ticketing: **26 tickets across 4 phases** written to `tickets/phase_1{a,b,c,d}/` and published as GitHub Issues #1–#26 on `yvrxbt/obird`
- `tickets/publish.sh` helper for filing tickets to GitHub Issues as they're added
- `.claude/CLAUDE.md` updated with current idiosyncrasies + pointers

### In flight
- Nothing is actively being written by an agent. Everything is waiting to be picked up.

### Not done (the Phase 1 work)
- No ticket has been implemented yet. Start anywhere in the sequence below.

---

## 3. Phase 1 plan + issue numbers

| Phase | Scope | Issue range | Tickets dir | Maps to PROJECT_PLAN |
|---|---|---|---|---|
| 1a | Engine keying: `HashMap<Exchange>` → `HashMap<InstrumentId>`; multi-market in one process | [#1–7](https://github.com/yvrxbt/obird/issues?q=label%3Aphase-1a) | `tickets/phase_1a/` | §1.8 |
| 1b | Extract `fair_value_service`: in-process first, `FairValueBus` transport | [#8, 9, 15–19](https://github.com/yvrxbt/obird/issues?q=label%3Aphase-1b) | `tickets/phase_1b/` | §1.4 |
| 1c | Extract `md-ingest-{poly,predict,hl}` binaries over UDS + tier-0 NDJSON | [#10–14](https://github.com/yvrxbt/obird/issues?q=label%3Aphase-1c) | `tickets/phase_1c/` | §1.3 |
| 1d | NATS substrate: schemas, Action/OrderUpdate/FV/MD transports, idempotency | [#20–26](https://github.com/yvrxbt/obird/issues?q=label%3Aphase-1d) | `tickets/phase_1d/` | §1.1, 1.2, 1.5, 1.6 |

### Recommended execution order

1. **#1 → #2 → #3 → #4 → #6** (1a refactor, all automatable)
2. **#7** (1a validation, `human-only` — runs live)
3. Then either 1b (#8–19) or 1c (#10–14) — both no-infra, order doesn't matter
4. Finally 1d (#20–26) — requires local NATS (Docker)

Each phase's **final T7** ticket is `human-only`: a live run against mainnet/testnet. Don't let an autonomous agent run those.

---

## 4. How to pick up a ticket

Every ticket is both a markdown file in `tickets/` **and** a GitHub Issue. Same content. Pick the interface that fits your tooling.

### From Cursor / local LLM

```bash
cat tickets/phase_1a/T1-trait-method.md
# Copy the "Cursor prompt" block → paste into Cursor → run
```

### From an agent assigned to a GitHub Issue

The issue body **is** the brief. No extra context needed — each ticket is self-contained (task / context / files / Cursor prompt / acceptance criteria / blockers).

### From `claude -p` in the terminal

```bash
claude -p "$(gh issue view 1 --json body --jq .body)"
```

### Reporting back

When you finish a ticket:
1. Open a PR referencing the issue: `git commit -m "phase 1a T1: Add instruments() to ExchangeConnector (closes #1)"`
2. Tick the acceptance-criteria checkboxes in the issue (or in the PR body — the issue auto-closes on merge)
3. Update `PROJECT_PLAN.md` — check the matching deliverable
4. If you discovered a follow-up, open a new issue and reference it in the PR

---

## 5. How to add MORE tickets (future phases)

1. Write `tickets/phase_<N>/T*.md` files following the existing format (frontmatter: `title`, `labels`; body: task / context / files / Cursor prompt / acceptance criteria / complexity / blockers)
2. Update `tickets/README.md` phases table
3. Add a cross-link in `PROJECT_PLAN.md` §X to `tickets/phase_<N>/`
4. `./tickets/publish.sh -y phase_<N>` — files GitHub Issues and labels automatically
5. Commit + push

The publish script auto-creates labels (`phase-*`, `difficulty-*`, `area-*`, `agent-task`, `human-only`). Run it once for a new phase; idempotent on rerun (creates new issues if you rerun — check for duplicates).

---

## 6. Invariants and don'ts

**Code invariants** (from `.claude/CLAUDE.md`):
- Strategies NEVER import connector crates or call exchange APIs
- `Strategy`, `Action`, `Event`, `ExchangeConnector`, `MarketDataSink` traits are the stability boundary — don't churn
- `rust_decimal::Decimal` for prices/quantities, never `f64`
- `thiserror` for errors, never string errors

**Process don'ts**:
- Don't merge a T7 validation without actually running the live check — they gate each phase
- Don't bypass the ticket system for ad-hoc work — if a task is non-trivial, ticket it first
- Don't push force / rewrite history — this repo has cross-session commits from automated agents
- Don't commit `.env`, secrets, or anything in `logs/`

**Farmer-not-running caveat** (current): You CAN refactor aggressively because no live bot depends on the current code. Verify with `ps aux | grep trading-cli` — should show nothing on the trading box.

---

## 7. Repo map

```
obird/
├── README.md                  # canonical engineering doc
├── PREDICTION_MARKETS.md      # prediction-market domain
├── DEX_CEX_MM.md              # HL + Binance domain
├── PRD_FARMING_PLATFORM.md    # v2 architecture target (w/ §0.1 v1→v2 delta)
├── PROJECT_PLAN.md            # phased execution plan
├── QUICK_START.md             # this file
├── CHANGELOG.md               # historical
├── .claude/CLAUDE.md          # LLM-facing terse invariants
├── tickets/                   # Phase 1 tickets (source of truth for issues)
│   ├── README.md
│   ├── publish.sh             # → GitHub Issues
│   └── phase_1{a,b,c,d}/
├── decisions/                 # ADRs 001–006
├── crates/                    # Rust workspace
│   ├── core/                  # traits, Event/Action enums
│   ├── engine/                # runner, router, risk, MarketDataBus
│   ├── backtest/              # SimConnector + harness
│   ├── cli/                   # trading-cli entrypoint
│   ├── fair_value_service/    # stub; Phase 1b promotes it
│   ├── connectors/            # hyperliquid, polymarket, predict_fun, binance, lighter
│   └── strategies/            # hl_spread_quoter, prediction_quoter, predict_hedger, pair_trader
├── configs/                   # quoter.toml, markets_poly/*.toml
├── scripts/                   # farm.py (legacy), project_status.sh, run_backtest.sh, create_github_issues.py (superseded by tickets/publish.sh)
└── infra/                     # infra-as-code (placeholder)
```

---

## 8. Resetting your session? Resume from here

If you're starting a new agent session on this repo, read in this order:
1. This file (`QUICK_START.md`) — 5 min
2. `README.md` §1–§4 — 5 min (what the system is)
3. The next unclosed ticket in `tickets/phase_1a/` or the lowest-numbered open GitHub issue
4. `.claude/CLAUDE.md` — scan for idiosyncrasies

That's enough context to start executing. Do NOT re-derive any of the scoping — it's all captured in tickets.

If you need to understand a code area, open the relevant crate. Per-crate `CONTEXT.md` files were **deleted** during the doc consolidation (they were stale boilerplate); code is the source of truth.

---

## 9. Escalation

Anything `human-only`: stop, report what's done, ask for the human.
Anything that would require creating a new external account, changing secrets, or touching production (non-testnet): stop and ask.

---

## 10. Superseded / deprecated artifacts

- `scripts/create_github_issues.py` — wrote issues from `PROJECT_PLAN.md` at one item per deliverable. Superseded by `tickets/publish.sh` which has Cursor-ready prompts and finer granularity. Kept for reference; don't run.
- `scripts/farm.py` — multi-process predict.fun farm launcher. Will be retired by Phase 1a T5+T7 (single-process multi-market). Keep running as rollback until T7 validation passes.
- 18 deleted `.md` files (ARCHITECTURE, RUNBOOK, PREDICT_*, POLY_HEDGING_*, all crate CONTEXT.md, LLM_GUIDE) — content merged into the 5 canonical docs above. Do not recreate.
