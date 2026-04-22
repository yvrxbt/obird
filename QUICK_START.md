# obird v2 — Quick Start Guide

> **You are here**: PRD written → Project plan created → Ready to execute

---

## What Just Happened

I've broken down your `PRD_FARMING_PLATFORM.md` into:

1. **`PROJECT_PLAN.md`** — Master tracking document
   - 3 phases with detailed task breakdown
   - Dependencies mapped
   - Open questions flagged
   - Acceptance criteria for each task

2. **GitHub Issue Templates** — in `.github/ISSUE_TEMPLATE/`
   - `phase1_task.md`
   - `phase2_task.md`
   - `phase3_task.md`

3. **Issue Creation Script** — `scripts/create_github_issues.py`
   - Bulk-create GitHub issues from the plan
   - Supports dry-run, phase-specific, or all-at-once

---

## Next Steps (Pick Your Path)

### Option A: Use Markdown Tracking (Simplest)

Just edit `PROJECT_PLAN.md` and check off boxes as you go:

```bash
# Open in your editor
code PROJECT_PLAN.md

# Track progress by checking boxes:
- [x] 1.1.1 Deploy 3x t4g.small NATS cluster
- [ ] 1.1.2 Configure JetStream streams
```

**Pros**: Zero overhead, works offline, lives in repo  
**Cons**: No GitHub integration, no notifications, manual only

---

### Option B: Use GitHub Issues (Recommended)

Create issues from the plan for better project management:

```bash
# Preview what would be created (dry run)
python3 scripts/create_github_issues.py --dry-run --phase 1

# Create all Phase 1 issues
python3 scripts/create_github_issues.py --phase 1

# Or create all phases at once
python3 scripts/create_github_issues.py --all

# Or create a single task
python3 scripts/create_github_issues.py --task 1.1
```

**Pros**: GitHub tracking, notifications, team collaboration, project boards  
**Cons**: Requires `gh` CLI setup (see below)

---

### Option C: Hybrid (Best of Both)

- Use `PROJECT_PLAN.md` as the master reference
- Create GitHub issues for active tasks only (current phase)
- Link issues back to `PROJECT_PLAN.md` sections

---

## GitHub CLI Setup (for Option B)

If you don't have `gh` CLI:

```bash
# Install (macOS)
brew install gh

# Install (Ubuntu/Debian)
sudo apt install gh

# Authenticate
gh auth login

# Test
gh issue list --repo yvrxbt/obird
```

---

## Open Questions to Resolve (High Priority)

Before starting Phase 1, clarify these:

### 1. Schema Format (blocks 1.2)
**Question**: msgpack vs protobuf for NATS message schemas?  
**Recommendation**: msgpack (simpler, Rust `serde` native, no codegen)  
**Your call**: ____________

### 2. FV Service Colocation (blocks 1.4)
**Question**: Run FV service in us-east-1 (central) or eu-west-2 (colocated)?  
**Recommendation**: Start central, measure latency, move if >5ms overhead  
**Your call**: ____________

### 3. Transport Mode Default (blocks 1.5)
**Question**: Default strategies to in-process or network mode?  
**Recommendation**: Network mode default, opt-in co-located for latency-critical  
**Your call**: ____________

### 4. NATS Auth (blocks 1.1)
**Question**: Static JWTs rotated monthly or Vault integration?  
**Recommendation**: Static JWTs (simpler ops)  
**Your call**: ____________

---

## Critical Path (What Blocks What)

**Week 1**: 1.1 (NATS setup) + 1.2 (contracts) can run in parallel  
**Week 2**: 1.3 (md-ingest) needs 1.2 done  
**Week 2-3**: 1.4 (FV service) needs 1.2 + 1.3  
**Week 3-4**: 1.5 (engine refactor) needs 1.2 + 1.4 (CRITICAL PATH)  
**Week 4**: 1.6 (strategy controller) needs 1.5 (CRITICAL PATH)  
**Week 5-6**: 1.7 (deploy + migration) needs everything above  

**Parallel work**: 1.8 (multi-market fix) can run anytime in Week 5

---

## Budget Quick Check

### Phase 1 (single region)
- 3x t4g.small NATS: ~$45/mo
- 3x c7g.large engines/services: ~$180/mo
- 2x c7g.medium md-ingest: ~$60/mo
- NAT gateway: ~$32/mo
- **Total: ~$320/mo**

### Phase 2 (multi-region + quant lake)
- Add Tokyo region: +$150/mo
- QuestDB r7g.xlarge: +$250/mo
- RDS Postgres: +$25/mo
- Grafana stack: +$30/mo
- Cross-region egress: +$100/mo
- **Total: ~$875/mo**

### Phase 3 (full scale)
- Add Binance/Lighter/Kalshi: +$200/mo
- More egress: +$100/mo
- **Total: ~$1.2k/mo**

Aligns with PRD target: $600-900/mo MVP → $1.5-2.5k at scale ✅

---

## Risks to Watch

| Risk | Mitigation |
|---|---|
| NATS JetStream unproven at >500k msg/sec | Benchmark in Phase 1 week 1, fallback to Redpanda if needed |
| Cross-region latency > 20ms | VPC peering + measure early, adjust topology if needed |
| Migration breaks live farming | Tag monolith build, test rollback before Phase 1.7 |
| QuestDB scales poorly | 90d retention + S3 archive keeps it bounded, can swap to ClickHouse later |
| Key custody (PREDICT_PRIVATE_KEY shared) | Phase 3: migrate to per-venue KMS keys |

---

## Suggested Workflow

### Daily
1. Pick 1-2 tasks from current phase
2. Work → commit → push
3. Update `PROJECT_PLAN.md` checkboxes or close GitHub issues
4. Log any blockers or decisions in `decisions/` if architectural

### Weekly
- Review: what's done, what's blocked, what's slipping
- Adjust estimates if >20% variance
- Update `PROJECT_PLAN.md` with new learnings

### Phase Gates
- **Phase 1 exit**: Code review + 7d P&L comparison before/after migration
- **Phase 2 exit**: QuestDB 7d retention proven + multi-region stable
- **Phase 3 exit**: Continuous (no hard gate, per-venue)

---

## Quick Commands Reference

```bash
# Preview GitHub issues
python3 scripts/create_github_issues.py --dry-run --phase 1

# Create Phase 1 issues
python3 scripts/create_github_issues.py --phase 1

# View issues
gh issue list --label phase-1

# Close an issue
gh issue close <number>

# View PRD
cat PRD_FARMING_PLATFORM.md | less

# View full project plan
cat PROJECT_PLAN.md | less

# Check NATS status (once deployed)
kubectl get pods -n nats  # if k8s, or:
systemctl status nats-server
```

---

## Tools You'll Need

### Phase 1
- Terraform (infra as code)
- AWS CLI (secrets, S3, etc.)
- `gh` CLI (optional, for GitHub issues)
- Rust toolchain (already have)

### Phase 2
- Docker (for QuestDB/Grafana local testing)
- Node.js (for dashboard)
- Python 3.9+ (for compaction script)

### Phase 3
- Same as above

---

## When You're Ready

1. **Review this plan** — flag anything wrong/risky
2. **Answer open questions** above (or accept recommendations)
3. **Choose tracking method** (Option A/B/C)
4. **Start Phase 1 Week 1**: NATS setup (task 1.1)

Questions? Check `PROJECT_PLAN.md` for details or ask.

---

**Let's ship it.** ⚡
