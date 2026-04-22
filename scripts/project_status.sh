#!/bin/bash
# Quick project status check
# Usage: ./scripts/project_status.sh

set -euo pipefail

echo "📊 obird v2 — Project Status"
echo "================================"
echo ""

# Check if gh CLI is available
if command -v gh &> /dev/null; then
    echo "GitHub Issues:"
    echo "  Phase 1: $(gh issue list --label phase-1 --json state --jq '[.[] | select(.state=="OPEN")] | length') open, $(gh issue list --label phase-1 --json state --jq '[.[] | select(.state=="CLOSED")] | length') closed"
    echo "  Phase 2: $(gh issue list --label phase-2 --json state --jq '[.[] | select(.state=="OPEN")] | length') open, $(gh issue list --label phase-2 --json state --jq '[.[] | select(.state=="CLOSED")] | length') closed"
    echo "  Phase 3: $(gh issue list --label phase-3 --json state --jq '[.[] | select(.state=="OPEN")] | length') open, $(gh issue list --label phase-3 --json state --jq '[.[] | select(.state=="CLOSED")] | length') closed"
    echo ""
else
    echo "⚠️  gh CLI not found — install with: brew install gh"
    echo ""
fi

# Count checkboxes in PROJECT_PLAN.md
if [ -f PROJECT_PLAN.md ]; then
    total_tasks=$(grep -c '^\s*- \[ \]' PROJECT_PLAN.md || true)
    done_tasks=$(grep -c '^\s*- \[x\]' PROJECT_PLAN.md || true)
    echo "PROJECT_PLAN.md:"
    echo "  Total tasks: $total_tasks"
    echo "  Completed: $done_tasks"
    if [ "$total_tasks" -gt 0 ]; then
        pct=$((done_tasks * 100 / total_tasks))
        echo "  Progress: ${pct}%"
    fi
    echo ""
fi

# Check recent commits
echo "Recent Activity (last 7 days):"
git log --since="7 days ago" --oneline --no-merges | head -5
echo ""

# Check current branch
echo "Current Branch:"
git branch --show-current
echo ""

# Check if any services are running (if deployed)
echo "Deployment Status:"
if systemctl is-active --quiet obird-engine 2>/dev/null; then
    echo "  ✅ obird-engine: running"
else
    echo "  ⚪ obird-engine: not deployed"
fi

if systemctl is-active --quiet nats-server 2>/dev/null; then
    echo "  ✅ NATS: running"
else
    echo "  ⚪ NATS: not deployed"
fi

if systemctl is-active --quiet fair-value-service 2>/dev/null; then
    echo "  ✅ fair-value-service: running"
else
    echo "  ⚪ fair-value-service: not deployed"
fi

echo ""
echo "Next steps: see QUICK_START.md"
