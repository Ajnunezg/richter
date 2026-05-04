#!/usr/bin/env bash
# -----------------------------------------------------------------------------
# Richter Statusline for Claude Code
#
# Produces a one-line, color-coded status summary suitable for the Claude Code
# statusline. Shows: repo name, current branch, active runs, queued runs, and
# the latest cache hit or conflict event.
#
# Usage:
#   Add to Claude Code settings:
#     "statusLine": {
#       "type": "command",
#       "command": "bash ~/.richter/integrations/statusline.sh"
#     }
#
#   Or install automatically with:
#     richter install hooks --agent claude
# -----------------------------------------------------------------------------

set -euo pipefail

# ── Discover current repo ────────────────────────────────────────────────────
if git rev-parse --show-toplevel &>/dev/null; then
    REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || echo "?")
    REPO_NAME=$(basename "$REPO_ROOT")
    BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "?")
else
    REPO_NAME="(no repo)"
    BRANCH="?"
fi

# ── Query Richter daemon ─────────────────────────────────────────────────────
RICHTER_STATUS=$(richter status --json 2>/dev/null || echo '{"error":"daemon_offline"}')

# Parse values with fallbacks.
ACTIVE_RUNS=$(echo "$RICHTER_STATUS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('active_runs',0))" 2>/dev/null || echo "0")
QUEUED_RUNS=$(echo "$RICHTER_STATUS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('queued_runs',0))" 2>/dev/null || echo "0")
CACHE_HITS=$(echo "$RICHTER_STATUS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('cache_hits_today',0))" 2>/dev/null || echo "0")

# ── Color coding ─────────────────────────────────────────────────────────────
# ANSI color codes (may be stripped by Claude Code, but available in terminal).
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Determine status color based on runs.
if [ "$ACTIVE_RUNS" -gt 2 ] || [ "$QUEUED_RUNS" -gt 2 ]; then
    STATUS_COLOR="$YELLOW"
    STATUS_ICON="⚡"
elif [ "$ACTIVE_RUNS" -gt 0 ] || [ "$QUEUED_RUNS" -gt 0 ]; then
    STATUS_COLOR="$CYAN"
    STATUS_ICON="●"
else
    STATUS_COLOR="$GREEN"
    STATUS_ICON="✓"
fi

# ── Build the statusline ─────────────────────────────────────────────────────
# Format: ⚡ repo(branch) active:N queued:N cache:N latest-event
ST="${STATUS_COLOR}${STATUS_ICON}${NC} ${REPO_NAME}(${BRANCH}) active:${ACTIVE_RUNS} queued:${QUEUED_RUNS} cache:${CACHE_HITS}"

# Append latest important event if daemon is online and has events.
if echo "$RICHTER_STATUS" | python3 -c "import sys,json; d=json.load(sys.stdin); sys.exit(0 if 'error' not in d else 1)" 2>/dev/null; then
    LATEST_EVENT=$(richter events --limit 1 --importance-min 50 --format oneline 2>/dev/null || echo "")
    if [ -n "$LATEST_EVENT" ]; then
        ST="${ST} | ${LATEST_EVENT}"
    fi
fi

echo -e "$ST"
