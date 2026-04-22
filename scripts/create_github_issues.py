#!/usr/bin/env python3
"""
Helper script to bulk-create GitHub issues from PROJECT_PLAN.md

Usage:
    # Dry run (preview only)
    python3 scripts/create_github_issues.py --dry-run

    # Create issues for Phase 1 only
    python3 scripts/create_github_issues.py --phase 1

    # Create all issues
    python3 scripts/create_github_issues.py --all

Requires: gh CLI authenticated (run `gh auth login` first)
"""

import subprocess
import sys
import argparse
from typing import List, Dict, Optional

# Task definitions extracted from PROJECT_PLAN.md
# Format: (task_id, title, labels, body_template)

PHASE_1_TASKS = [
    {
        "id": "1.1",
        "title": "NATS Infrastructure Setup",
        "labels": ["phase-1", "infra", "nats", "p1-high"],
        "deliverables": [
            "Deploy 3x t4g.small NATS cluster in eu-west-2 (Terraform module)",
            "Configure JetStream streams: md_archive, actions, order_updates",
            "Wire mTLS + nkey auth (AWS Secrets Manager for keys)",
            "Test latency: localhost Core publish/subscribe < 200μs",
            "Test JetStream work-queue consumer (action idempotency)",
        ],
        "acceptance": [
            "NATS cluster healthy, 3 replicas, <1ms cross-node latency",
            "Test harness proves idempotent delivery on action.* subjects",
        ],
        "blockers": [],
        "questions": [
            "JetStream retention on md_archive: start with 7d, OK?",
            "Auth: static JWTs rotated monthly or Vault integration? (Lean static for MVP)",
        ],
    },
    {
        "id": "1.2",
        "title": "NATS Subject Contract + Schemas",
        "labels": ["phase-1", "contract", "schema", "p1-high"],
        "deliverables": [
            "Document NATS subject hierarchy in docs/NATS_SUBJECTS.md",
            "Define wire schemas (msgpack or protobuf?) for Action/OrderUpdate/FairValue",
            "Add schema_version field to each message type",
            "Write schema evolution policy (dual-version support for 1 release)",
        ],
        "acceptance": [
            "All subject patterns documented",
            "Schema definition file committed (schemas/action.msgpack.json or .proto)",
        ],
        "blockers": [],
        "questions": [
            "msgpack vs protobuf? (Lean msgpack: simpler, Rust serde support, no codegen)",
            "Schema registry needed or just versioned files in git?",
        ],
    },
    {
        "id": "1.3",
        "title": "Extract Market Data Ingest Binaries",
        "labels": ["phase-1", "md-ingest", "extraction", "p1-high"],
        "deliverables": [
            "Create crates/md-ingest/ with common binary scaffold",
            "Extract Polymarket feed: md-ingest-poly binary",
            "Extract predict.fun feed: md-ingest-predict",
            "Add systemd unit files for both",
            "Test: verify NATS subjects populated, NDJSON rotates daily",
        ],
        "acceptance": [
            "Both feeds run as separate processes, publish to NATS",
            "Tier-0 NDJSON logs written to local SSD, rotated by date",
            "In-process broadcast still works (for co-located strategies during transition)",
        ],
        "blockers": ["1.2"],
        "questions": [
            "Should md-ingest also expose HTTP health endpoint? (Yes, /health + Prometheus /metrics)",
        ],
    },
    {
        "id": "1.4",
        "title": "Fair Value Service Extraction",
        "labels": ["phase-1", "fv-service", "extraction", "p1-high"],
        "deliverables": [
            "Promote crates/fair_value_service from stub to full binary",
            "Implement pluggable FV models: mid, cross_venue_conservative, microprice",
            "Subscribe to md.<venue>.<instrument>.book via NATS",
            "Publish to fv.<symbol> (NATS Core, latest-value)",
            "Config file: map symbols to models + source venues",
            "Add staleness monitoring: emit warning if no FV update in >2s",
        ],
        "acceptance": [
            "FV service runs standalone, publishes Poly + predict FV",
            "PredictionQuoter can subscribe to fv.* instead of computing inline",
            "Config-driven model selection (no hardcoded venue pairs)",
        ],
        "blockers": ["1.2", "1.3"],
        "questions": [
            "Colocation: run FV service in us-east-1 (central) or eu-west-2 (colocated with poly)?",
            "Store FV model state (e.g., EMA) in Redis or in-memory only? (In-memory for MVP)",
        ],
    },
    {
        "id": "1.5",
        "title": "Refactor Engine: NATS Action/Event Transport",
        "labels": ["phase-1", "engine", "nats", "refactor", "p0-critical"],
        "deliverables": [
            "Add ActionTransport trait: in_process(mpsc) vs nats(JetStream)",
            "Add EventTransport trait: in_process(broadcast) vs nats(Core)",
            "Wire OrderRouter to consume action.<venue>.<market> via NATS",
            "Wire OrderManager to publish order.<venue>.<market>.<oid> via NATS",
            "Add idempotency layer in engine: track action_id → order_id map",
            "Test: run strategy in separate process, prove Actions round-trip",
        ],
        "acceptance": [
            "Engine can run with --transport=nats flag",
            "Strategies can run co-located (in-process) OR network mode (separate binary)",
            "Idempotency proven: duplicate Actions don't spam exchange",
        ],
        "blockers": ["1.2", "1.4"],
        "questions": [
            "Backwards compat: keep in-process mode as default during rollout? (Yes, feature-flag it)",
            "Action timeout: if no ack in Xs, re-publish or dead-letter? (Dead-letter after 30s)",
        ],
    },
    {
        "id": "1.6",
        "title": "Extract Strategy Controller Binary",
        "labels": ["phase-1", "strategy", "controller", "extraction", "p0-critical"],
        "deliverables": [
            "Create crates/strategy-controller/ binary scaffold",
            "Move PredictionQuoter logic into controller",
            "Add config hot-reload: watch TOML, re-parse on change",
            "Add kill-switch subscription: control.kill_switch.<venue>",
            "Prove strategy runs in network mode (separate process from engine)",
        ],
        "acceptance": [
            "strategy-controller binary runs standalone, quotes predict.fun",
            "Engine receives Actions over NATS, executes, returns Events",
            "No inline FV computation in strategy (all via fv.* subscription)",
        ],
        "blockers": ["1.5"],
        "questions": [
            "Language: keep Rust or allow Python for network-mode strategies? (Rust for now, revisit in Phase 3)",
            "How to handle position state? Query position-service or track locally? (Track locally for now, Phase 2 adds position-service)",
        ],
    },
    {
        "id": "1.7",
        "title": "Single-Region Deploy + Migration",
        "labels": ["phase-1", "deploy", "migration", "p0-critical"],
        "deliverables": [
            "Deploy to eu-west-2 (London): all services",
            "Terraform module: infra/phase1-single-region/",
            "Systemd units for all services",
            "Secrets via AWS Secrets Manager (no .env in systemd)",
            "Test: run one predict.fun market end-to-end",
            "Migrate all predict.fun farming from monolith",
            "Compare P&L: 7-day window before/after migration",
        ],
        "acceptance": [
            "All predict.fun markets running via new architecture",
            "P&L delta < 2% (allowing for market noise)",
            "No new bugs/crashes vs monolith",
            "scripts/farm.py deprecated (multi-market-single-process working)",
        ],
        "blockers": ["1.3", "1.4", "1.5", "1.6"],
        "questions": [
            "Rollback plan if migration fails? (Keep monolith build tagged, can revert in <5 min)",
            "Monitoring: Grafana dashboard ready or just NATS metrics? (Basic NATS dashboard + engine health checks)",
        ],
    },
    {
        "id": "1.8",
        "title": "Multi-Market Single-Process Fix",
        "labels": ["phase-1", "engine", "multi-market", "p1-high"],
        "deliverables": [
            "Change engine key from HashMap<Exchange, Connector> to HashMap<InstrumentId, Connector>",
            "Update OrderRouter and EngineRunner to support multi-market per exchange",
            "Test: one obird-engine process quotes 3+ predict.fun markets simultaneously",
            "Retire scripts/farm.py crash-loop orchestration",
        ],
        "acceptance": [
            "Single engine process serves all predict.fun markets",
            "One Polymarket WS connection serves all FV subscriptions",
            "Process count drops from N (markets) to 1 per venue",
        ],
        "blockers": [],
        "questions": [
            "Does this break HL? (No, HL is already single-instrument)",
        ],
    },
]

PHASE_2_TASKS = [
    {
        "id": "2.1",
        "title": "QuestDB Deployment + Ingestion",
        "labels": ["phase-2", "questdb", "quant", "p1-high"],
        "deliverables": [
            "Deploy QuestDB on r7g.xlarge in us-east-1",
            "Create quant-tap consumer (Rust or Vector)",
            "Define QuestDB schemas (book_updates, trades, fills, fv_snapshots)",
            "Test: verify 90d retention, query latency <50ms for BBO reconstruction",
            "Add Grafana data source + sample dashboard (book depth viz)",
        ],
        "acceptance": [
            "QuestDB ingesting live MD from all venues",
            "Query: 'Latest BBO per instrument' returns in <50ms",
            "7 days of tick data queryable",
        ],
        "blockers": ["Phase 1 complete"],
        "questions": [
            "Partitioning: daily WAL partitions OK or need hourly? (Daily OK for MVP)",
            "Backup strategy: QuestDB snapshots to S3 daily? (Yes, cron at 02:00 UTC)",
        ],
    },
    {
        "id": "2.2",
        "title": "S3 Parquet Archive",
        "labels": ["phase-2", "s3", "parquet", "archive", "p2-medium"],
        "deliverables": [
            "Create S3 bucket: s3://obird-quant-hot/",
            "Write compaction script (Python + pyarrow)",
            "Deploy as daily cron (02:00 UTC on central host)",
            "Test: query S3 Parquet with DuckDB embedded",
        ],
        "acceptance": [
            "Compaction runs successfully for 1 day",
            "S3 bucket contains Parquet files, queryable via DuckDB",
            "QuestDB disk usage stays <100GB (90d hot retention enforced)",
        ],
        "blockers": ["2.1"],
        "questions": [
            "Glacier Deep Archive after 2y? (Yes, lifecycle policy auto-transitions)",
        ],
    },
    {
        "id": "2.3",
        "title": "Control Plane Dashboard",
        "labels": ["phase-2", "dashboard", "ui", "control-plane", "p1-high"],
        "deliverables": [
            "Scaffold Next.js 15 app in obird/dashboard/",
            "Deploy RDS Postgres db.t4g.small in us-east-1 (control DB)",
            "Wire tRPC or Hono API layer",
            "Build pages: Fleet Health, Positions, Kill Switches, Strategy Params",
            "Add WS subscription to NATS for realtime updates",
            "Deploy to Vercel free tier or EC2 + Caddy",
        ],
        "acceptance": [
            "Dashboard accessible at https://obird-dash.<domain>",
            "Can view live fleet health (engine status, connector WS status)",
            "Can toggle kill switch per venue, see effect in <2s",
        ],
        "blockers": ["2.1"],
        "questions": [
            "Hosting: Vercel vs self-host? (Lean Vercel for speed)",
            "Auth: Clerk ($25/mo) vs Auth.js free? (Auth.js for 2 users)",
        ],
    },
    {
        "id": "2.4",
        "title": "Cross-Region NATS + HL Migration",
        "labels": ["phase-2", "multi-region", "hl", "migration", "p0-critical"],
        "deliverables": [
            "Deploy NATS cluster in ap-northeast-1 (Tokyo)",
            "Deploy md-ingest-hl + obird-hl in Tokyo",
            "Wire NATS subject routing (local vs cross-region)",
            "Test: measure cross-region latency (Tokyo ↔ us-east-1)",
            "Migrate HL trading from monolith to new architecture",
        ],
        "acceptance": [
            "HL spread MM running via new arch",
            "Cross-region Action → Order → Fill latency < 20ms (p95)",
            "No P&L degradation vs monolith",
        ],
        "blockers": ["Phase 1 complete", "2.1"],
        "questions": [
            "HlSpreadQuoter: keep co-located (in-process) or promote to network mode?",
            "VPC peering vs public IPs for NATS gateways? (VPC peering, cheaper + secure)",
        ],
    },
    {
        "id": "2.5",
        "title": "Risk Gate + Position Service",
        "labels": ["phase-2", "risk", "position", "pnl", "p1-high"],
        "deliverables": [
            "Promote UnifiedRiskManager from stub to full implementation",
            "Create position-service (Rust)",
            "Persist position snapshots to Postgres every 1min",
            "Wire dashboard to show live positions + PnL",
        ],
        "acceptance": [
            "Risk gate blocks orders that violate limits",
            "Position service correctly aggregates fills from all venues",
            "Dashboard shows live PnL, updates every 1s",
        ],
        "blockers": ["2.3"],
        "questions": [
            "Limit source: Postgres or hot-reloadable TOML?",
            "Drawdown kill switch: auto-resume or require manual override?",
        ],
    },
    {
        "id": "2.6",
        "title": "Observability Stack",
        "labels": ["phase-2", "observability", "grafana", "prometheus", "p2-medium"],
        "deliverables": [
            "Deploy Grafana + Prometheus + Loki + Tempo on t4g.medium in us-east-1",
            "Wire Prometheus scrape for all services",
            "Wire Loki for structured logs",
            "Wire Tempo for OTLP traces (Action → Order → Fill span tree)",
            "Build Grafana dashboards (fleet, latency, FV freshness, connector health)",
            "Configure alerts → PagerDuty or SNS",
        ],
        "acceptance": [
            "Grafana accessible at https://grafana.<domain>",
            "Can trace an Action from strategy → engine → exchange → fill",
            "Alerts fire when FV goes stale (tested via kill md-ingest)",
        ],
        "blockers": [],
        "questions": [
            "Retention: Prometheus (30d), Loki (30d), Tempo (7d)? (Yes, OK for MVP)",
            "PagerDuty vs Opsgenie vs SNS+SMS? (SNS+SMS for free tier)",
        ],
    },
]

PHASE_3_TASKS = [
    {
        "id": "3.1",
        "title": "Binance Connector Wiring",
        "labels": ["phase-3", "binance", "venue", "p2-medium"],
        "deliverables": [
            "Wire BinanceConnector (already built) into live runner",
            "Create md-ingest-binance binary",
            "Deploy in ap-northeast-1 (Tokyo) or ap-southeast-1 (Singapore)",
            "Add Binance to FV service as ref-price source",
            "Test: Phase A ref-price only (no live quoting)",
            "Phase B: second MM leg (pair-trade or spread arb)",
        ],
        "acceptance": [
            "Binance MD flowing into FV service",
            "Can quote HL using Binance microprice as FV anchor",
            "Phase B: live Binance MM running",
        ],
        "blockers": ["Phase 2 complete"],
        "questions": [
            "Binance API rate limits: need VIP tier? (Monitor in Phase A, upgrade if needed)",
        ],
    },
    {
        "id": "3.2",
        "title": "Lighter + Kalshi Connectors",
        "labels": ["phase-3", "lighter", "kalshi", "venue", "p3-low"],
        "deliverables": [
            "Build LighterConnector (scaffolding exists)",
            "Build KalshiConnector (new)",
            "Deploy Kalshi in us-east-2 (Ohio) or Equinix Chicago",
            "Wire into farming rotation",
        ],
        "acceptance": [
            "Lighter + Kalshi live, farming incentives",
        ],
        "blockers": ["3.1"],
        "questions": [
            "Kalshi regulatory risk? (Legal review needed before Phase 3 starts)",
            "Lighter sequencer location unknown — defer to Phase 3",
        ],
    },
    {
        "id": "3.3",
        "title": "ML Fair Value Models",
        "labels": ["phase-3", "ml", "fv", "quant", "p2-medium"],
        "deliverables": [
            "Extract features from QuestDB (book imbalance, volatility, spread)",
            "Train ML model (XGBoost or simple regression)",
            "Add ml_ensemble model to FV service",
            "Backtest vs simple mid / microprice models",
            "Deploy to production if >10bps edge",
        ],
        "acceptance": [
            "ML FV model live, outperforms baseline",
        ],
        "blockers": ["2.1"],
        "questions": [
            "Feature store: Postgres or separate service? (Postgres for MVP)",
            "Model update cadence: daily retrain or weekly? (Weekly initially)",
        ],
    },
    {
        "id": "3.4",
        "title": "Backtest CI Gate",
        "labels": ["phase-3", "backtest", "ci", "p2-medium"],
        "deliverables": [
            "Wire trading-cli backtest to harness (currently stub)",
            "Record 1 day of live MD as test fixture",
            "Add CI job: replay recorded day, assert P&L within tolerance",
            "Block PR merge if backtest fails",
        ],
        "acceptance": [
            "CI runs backtest on every PR",
            "Can catch regressions before deploy",
        ],
        "blockers": [],
        "questions": [
            "P&L tolerance: ±5% or ±10%? (±10% allowing for sim noise)",
        ],
    },
]


def format_issue_body(task: Dict) -> str:
    """Format task dict into GitHub issue body"""
    parts = [
        f"## Task ID\n{task['id']}\n",
        f"## Description\n{task.get('description', 'See PROJECT_PLAN.md')}\n",
    ]

    if task.get("blockers"):
        blockers_list = "\n".join([f"- [ ] {b}" for b in task["blockers"]])
        parts.append(f"## Blockers\n{blockers_list}\n")

    if task.get("deliverables"):
        deliverables_list = "\n".join([f"- [ ] {d}" for d in task["deliverables"]])
        parts.append(f"## Deliverables\n{deliverables_list}\n")

    if task.get("acceptance"):
        acceptance_list = "\n".join([f"- [ ] {a}" for a in task["acceptance"]])
        parts.append(f"## Acceptance Criteria\n{acceptance_list}\n")

    if task.get("questions"):
        questions_list = "\n".join([f"- {q}" for q in task["questions"]])
        parts.append(f"## Open Questions\n{questions_list}\n")

    parts.append(
        f"\n## Related PRD Section\nSee `PRD_FARMING_PLATFORM.md` and `PROJECT_PLAN.md` section {task['id']}\n"
    )

    return "\n".join(parts)


def create_issue(task: Dict, dry_run: bool = False) -> Optional[str]:
    """Create a GitHub issue using gh CLI"""
    title = f"[{task['id']}] {task['title']}"
    body = format_issue_body(task)
    labels = ",".join(task["labels"])

    cmd = [
        "gh",
        "issue",
        "create",
        "--title",
        title,
        "--body",
        body,
        "--label",
        labels,
    ]

    if dry_run:
        print(f"\n{'='*80}")
        print(f"DRY RUN: Would create issue:")
        print(f"Title: {title}")
        print(f"Labels: {labels}")
        print(f"\nBody:\n{body}")
        print(f"{'='*80}\n")
        return None

    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, check=True, cwd="/home/ubuntu/.openclaw/workspace/obird"
        )
        issue_url = result.stdout.strip()
        print(f"✅ Created: {title} -> {issue_url}")
        return issue_url
    except subprocess.CalledProcessError as e:
        print(f"❌ Failed to create issue: {title}")
        print(f"Error: {e.stderr}")
        return None


def main():
    parser = argparse.ArgumentParser(
        description="Create GitHub issues from PROJECT_PLAN.md"
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="Preview issues without creating"
    )
    parser.add_argument(
        "--phase",
        type=int,
        choices=[1, 2, 3],
        help="Create issues for specific phase only",
    )
    parser.add_argument("--all", action="store_true", help="Create all issues")
    parser.add_argument(
        "--task",
        type=str,
        help="Create single task by ID (e.g., 1.1, 2.3)",
    )

    args = parser.parse_args()

    if not any([args.dry_run, args.all, args.phase, args.task]):
        parser.print_help()
        print("\nℹ️  Use --dry-run to preview before creating")
        sys.exit(1)

    # Collect tasks based on args
    tasks_to_create = []

    if args.task:
        # Find specific task
        all_tasks = PHASE_1_TASKS + PHASE_2_TASKS + PHASE_3_TASKS
        matching = [t for t in all_tasks if t["id"] == args.task]
        if not matching:
            print(f"❌ Task {args.task} not found")
            sys.exit(1)
        tasks_to_create = matching
    elif args.phase:
        if args.phase == 1:
            tasks_to_create = PHASE_1_TASKS
        elif args.phase == 2:
            tasks_to_create = PHASE_2_TASKS
        elif args.phase == 3:
            tasks_to_create = PHASE_3_TASKS
    elif args.all:
        tasks_to_create = PHASE_1_TASKS + PHASE_2_TASKS + PHASE_3_TASKS

    print(f"\n📋 {'Previewing' if args.dry_run else 'Creating'} {len(tasks_to_create)} issues...\n")

    created = []
    for task in tasks_to_create:
        url = create_issue(task, dry_run=args.dry_run)
        if url:
            created.append(url)

    if not args.dry_run:
        print(f"\n✨ Created {len(created)} issues")
    else:
        print(f"\n✨ Would create {len(tasks_to_create)} issues (dry run)")
        print(f"\nTo create them, run:")
        if args.phase:
            print(f"  python3 scripts/create_github_issues.py --phase {args.phase}")
        elif args.all:
            print(f"  python3 scripts/create_github_issues.py --all")


if __name__ == "__main__":
    main()
