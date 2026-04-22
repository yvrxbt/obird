# obird Tickets

Pre-drafted development tickets organized by phase. One markdown file per ticket with frontmatter (title + labels) and a paste-ready Cursor/agent prompt.

## Layout

```
tickets/
├── README.md              # this file
├── publish.sh             # publishes all tickets in a phase to GitHub Issues
└── phase_1a/              # one directory per phase
    ├── T1-...md
    ├── T2-...md
    └── ...
```

## Why files not issues (yet)

- Review + iterate before filing (rename, re-scope, delete)
- Works offline; Cursor reads from disk
- One command publishes a batch to GitHub when ready
- Tickets stay in git as a living checklist even after issues close

## Usage

### Option A — hand a ticket directly to an agent

```bash
cat tickets/phase_1a/T1-trait-method.md
```

The whole file is the prompt. Copy the "Cursor prompt" block into Cursor, or paste the whole file to any Claude / Codex / Cursor session.

### Option B — publish all tickets in a phase to GitHub Issues

```bash
./tickets/publish.sh phase_1a
```

The script:
1. Creates needed labels if missing (`phase-1a`, `difficulty-*`, `area-*`, `human-only`)
2. For each `T*.md` file, parses the frontmatter title + labels
3. Runs `gh issue create --title "<title>" --body-file <path> --label ... --label ...`

Rerunning the script creates duplicates — check existing issues first (`gh issue list --label phase-1a`).

### Option C — publish one ticket

```bash
./tickets/publish.sh phase_1a T3
```

Matches `T3*.md` in the phase directory.

## Frontmatter format

Each ticket starts with:

```yaml
---
title: "[AGENT] Phase 1a T1: Add instruments() to ExchangeConnector trait"
labels: agent-task,phase-1a,difficulty-trivial,area-core
---
```

- `title` is a single line, goes to `gh issue create --title`
- `labels` is comma-separated, each becomes a `--label` arg
- Body = everything after the second `---`

## Phases

| Dir | Maps to | Scope | Status |
|---|---|---|---|
| `phase_1a/` | `PROJECT_PLAN.md` §1.8 | Engine key change: `HashMap<Exchange>` → `HashMap<InstrumentId>`; single-process multi-market | Ready |
| `phase_1b/` | `PROJECT_PLAN.md` §1.4 | Extract `fair_value_service`: in-process task + `FairValueBus`, strategy consumes external FV | Ready |
| `phase_1c/` | `PROJECT_PLAN.md` §1.3 | Extract md-ingest binaries over UDS + tier-0 NDJSON safety net | Ready |
| `phase_1d/` | `PROJECT_PLAN.md` §1.1, §1.2, §1.5, §1.6 | NATS substrate: schemas, Action/OrderUpdate/FV/MD transports, idempotency | Ready |

More phases will be added as they're scoped. Each phase scoping session produces a new directory under `tickets/`.
