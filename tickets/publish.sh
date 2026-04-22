#!/usr/bin/env bash
# Publish ticket markdown files to GitHub Issues via gh CLI.
#
# Usage:
#   ./tickets/publish.sh <phase_dir>           # publishes every T*.md in the phase
#   ./tickets/publish.sh <phase_dir> <prefix>  # publishes only tickets whose filename starts with <prefix>
#
# Examples:
#   ./tickets/publish.sh phase_1a
#   ./tickets/publish.sh phase_1a T3
#
# Reads frontmatter:
#   title:  single line → --title
#   labels: comma-separated → multiple --label args
# Body = everything after the second `---` line.

set -euo pipefail

AUTO_YES=0
ARGS=()
for a in "$@"; do
  case "$a" in
    -y|--yes) AUTO_YES=1 ;;
    *) ARGS+=("$a") ;;
  esac
done
set -- "${ARGS[@]}"

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 [-y] <phase_dir> [ticket_prefix]" >&2
  echo "Example: $0 phase_1a T3" >&2
  echo "         $0 -y phase_1a            # skip confirmation" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PHASE_DIR="$SCRIPT_DIR/$1"
PREFIX="${2:-T}"

if [[ ! -d "$PHASE_DIR" ]]; then
  echo "No such phase directory: $PHASE_DIR" >&2
  exit 1
fi

# Ensure gh is available and authenticated
command -v gh >/dev/null 2>&1 || { echo "gh CLI not found"; exit 1; }
gh auth status >/dev/null 2>&1 || { echo "gh not authenticated; run 'gh auth login'"; exit 1; }

# Ensure labels exist (idempotent — failure to create just means it already exists)
ensure_label() {
  local name="$1"
  local color="$2"
  local desc="$3"
  gh label create "$name" --color "$color" --description "$desc" 2>/dev/null || true
}

ensure_label "agent-task"          "0366d6" "Task drafted for an autonomous coding agent (Cursor, Claude, etc.)"
ensure_label "phase-1a"            "5319e7" "Phase 1a: engine key change (InstrumentId)"
ensure_label "phase-1b"            "5319e7" "Phase 1b: extract fair-value service"
ensure_label "phase-1c"            "5319e7" "Phase 1c: extract md-ingest binaries"
ensure_label "phase-1d"            "5319e7" "Phase 1d: NATS action transport"
ensure_label "difficulty-trivial"  "0e8a16" "Single file, few lines, no judgment"
ensure_label "difficulty-easy"     "7ccc6a" "Mechanical, one crate, clear pattern"
ensure_label "difficulty-medium"   "fbca04" "Multiple files, some design judgment"
ensure_label "difficulty-hard"     "d93f0b" "Judgment-heavy or cross-cutting"
ensure_label "area-core"           "1d76db" "crates/core"
ensure_label "area-engine"         "1d76db" "crates/engine"
ensure_label "area-connectors"     "1d76db" "crates/connectors/*"
ensure_label "area-cli"            "1d76db" "crates/cli"
ensure_label "area-backtest"       "1d76db" "crates/backtest"
ensure_label "area-ops"            "1d76db" "Deploy, runbooks, scripts"
ensure_label "area-fair-value"     "1d76db" "crates/fair_value_service"
ensure_label "area-strategies"     "1d76db" "crates/strategies/*"
ensure_label "area-infra"          "1d76db" "infra/, Docker, Terraform"
ensure_label "human-only"          "b60205" "Requires hands-on human judgment; not for automated agent"

# Walk tickets matching the prefix
shopt -s nullglob
FILES=("$PHASE_DIR"/${PREFIX}*.md)

if [[ ${#FILES[@]} -eq 0 ]]; then
  echo "No tickets match $PHASE_DIR/${PREFIX}*.md" >&2
  exit 1
fi

echo "About to publish ${#FILES[@]} ticket(s) from $PHASE_DIR:"
for f in "${FILES[@]}"; do
  echo "  - $(basename "$f")"
done
if [[ "$AUTO_YES" -ne 1 ]]; then
  read -rp "Proceed? [y/N] " ans
  [[ "$ans" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 0; }
fi

for f in "${FILES[@]}"; do
  # Parse frontmatter
  title="$(awk '/^title:/{ sub(/^title:[[:space:]]*/, ""); gsub(/^"|"$/, ""); print; exit }' "$f")"
  labels_csv="$(awk '/^labels:/{ sub(/^labels:[[:space:]]*/, ""); print; exit }' "$f")"
  # Body: skip to after the second `---`
  body="$(awk 'BEGIN{c=0} /^---$/{c++; next} c>=2{print}' "$f")"

  if [[ -z "$title" ]]; then
    echo "SKIP: $f has no title frontmatter" >&2
    continue
  fi

  # Build --label args
  label_args=()
  IFS=',' read -ra labels <<< "$labels_csv"
  for l in "${labels[@]}"; do
    l="$(echo "$l" | xargs)"  # trim
    [[ -n "$l" ]] && label_args+=(--label "$l")
  done

  echo
  echo "Creating: $title"
  echo "$body" | gh issue create \
    --title "$title" \
    --body-file - \
    "${label_args[@]}"
done

echo
echo "Done. List with: gh issue list --label phase-1a"
