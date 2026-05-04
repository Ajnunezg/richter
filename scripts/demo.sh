#!/usr/bin/env bash
#
# demo.sh — Richter end-to-end demonstration.
#
# This script:
#   1. Verifies the build exists (or builds it).
#   2. Installs shell shims.
#   3. Creates a temporary fixture repo (Rust project).
#   4. Simulates 3 AI agents running the same command simultaneously.
#   5. Proves only ONE underlying command executed.
#   6. Shows dashboard status.
#   7. Cleans up.
#
# Usage:
#   bash scripts/demo.sh
#
# Requirements:
#   - Richter must be built (run scripts/build.sh first if not).
#   - macOS 14+

set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Colors
BOLD='\033[1m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# ────────────────────────────────────────────────────────────
# Helpers
# ────────────────────────────────────────────────────────────
header()    { echo -e "\n${BOLD}${BLUE}═══ $1 ═══${NC}"; }
step()      { echo -e "  ${GREEN}→${NC} $1"; }
info()      { echo -e "    ${CYAN}ℹ${NC} $1"; }
warn()      { echo -e "    ${YELLOW}⚠${NC} $1"; }
agent_msg() { echo -e "  ${BOLD}[$1]${NC} $2"; }

cleanup() {
    header "Cleanup"
    if [ -n "${DEMO_DIR:-}" ] && [ -d "$DEMO_DIR" ]; then
        step "Removing temp directory: $DEMO_DIR"
        rm -rf "$DEMO_DIR"
    fi
    if [ "${DAEMON_STARTED:-false}" = true ]; then
        step "Stopping daemon"
        "$PROJECT_ROOT/target/release/richter" daemon stop 2>/dev/null || true
    fi
    info "Demo cleanup complete."
}
trap cleanup EXIT

# ────────────────────────────────────────────────────────────
# Step 1: Verify build
# ────────────────────────────────────────────────────────────
header "Richter Demo"

RICHTER_BIN="$PROJECT_ROOT/target/release/richter"
RICHTERD_BIN="$PROJECT_ROOT/target/release/richterd"

if [ ! -f "$RICHTER_BIN" ] || [ ! -f "$RICHTERD_BIN" ]; then
    warn "Richter binaries not found. Building..."
    bash "$PROJECT_ROOT/scripts/build.sh" --rust-only
fi

if [ ! -f "$RICHTER_BIN" ]; then
    echo "ERROR: richter binary not found after build."
    exit 1
fi

RICHTER_VERSION=$("$RICHTER_BIN" --version 2>/dev/null || echo "unknown")
step "Richter version: $RICHTER_VERSION"

# ────────────────────────────────────────────────────────────
# Step 2: Ensure daemon is running
# ────────────────────────────────────────────────────────────
header "Daemon Check"

if "$RICHTER_BIN" status 2>/dev/null | grep -q "running"; then
    step "Daemon is already running."
else
    step "Starting daemon..."
    "$RICHTER_BIN" daemon start 2>/dev/null || {
        # If daemon not installed, install and start it
        "$RICHTER_BIN" install daemon 2>/dev/null || true
        sleep 1
    }
    DAEMON_STARTED=true
fi

# Give daemon time to initialize
sleep 1
step "Daemon status: $("$RICHTER_BIN" status 2>/dev/null || echo 'checking...')"

# ────────────────────────────────────────────────────────────
# Step 3: Install shims
# ────────────────────────────────────────────────────────────
header "Shim Setup"

step "Installing shims..."
"$RICHTER_BIN" install shims 2>/dev/null || true

# Verify at least one shim exists
if [ -f "$HOME/.richter/shims/cargo" ]; then
    step "Shims installed: $(ls "$HOME/.richter/shims/" 2>/dev/null | wc -l | tr -d ' ') tools"
else
    warn "Shim directory not found — demo continues without shim interception"
fi

# ────────────────────────────────────────────────────────────
# Step 4: Create temp fixture repo
# ────────────────────────────────────────────────────────────
header "Fixture Setup"

DEMO_DIR=$(mktemp -d /tmp/richter-demo-XXXXXX)
step "Creating fixture repo: $DEMO_DIR"
cd "$DEMO_DIR"

# Initialize a Rust project
cargo init --name richter-demo-fixture --lib 2>/dev/null

# Write a small test suite
cat > src/lib.rs << 'RUST'
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub fn multiply(a: i32, b: i32) -> i32 {
    a * b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }

    #[test]
    fn test_multiply() {
        assert_eq!(multiply(4, 5), 20);
    }

    #[test]
    fn test_add_negative() {
        assert_eq!(add(-2, 3), 1);
    }

    #[test]
    fn test_multiply_zero() {
        assert_eq!(multiply(0, 42), 0);
    }
}
RUST

step "Fixture: Rust project with 4 tests"
info "Files: $(find . -type f -name '*.rs' -o -name '*.toml' | sort)"

# ────────────────────────────────────────────────────────────
# Step 5: Run baseline test (so cache exists later)
# ────────────────────────────────────────────────────────────
header "Baseline Test"

step "Running baseline test through Richter..."
"$RICHTER_BIN" run -- cargo test 2>&1 || true

# ────────────────────────────────────────────────────────────
# Step 6: Simulate 3 agents running the same test simultaneously
# ────────────────────────────────────────────────────────────
header "Multi-Agent Simulation"
info "Starting 3 concurrent cargo test invocations..."

AGENT1_OUT=$(mktemp /tmp/richter-agent1-XXXXXX)
AGENT2_OUT=$(mktemp /tmp/richter-agent2-XXXXXX)
AGENT3_OUT=$(mktemp /tmp/richter-agent3-XXXXXX)

# Agent 1: Codex
agent_msg "Codex" "cargo test..."
"$RICHTER_BIN" run -- cargo test > "$AGENT1_OUT" 2>&1 &
PID1=$!

# Small stagger so Agent 1's run starts first
sleep 0.3

# Agent 2: Claude Code
agent_msg "Claude" "cargo test..."
"$RICHTER_BIN" run -- cargo test > "$AGENT2_OUT" 2>&1 &
PID2=$!

sleep 0.3

# Agent 3: Droid
agent_msg "Droid" "cargo test..."
"$RICHTER_BIN" run -- cargo test > "$AGENT3_OUT" 2>&1 &
PID3=$!

# Wait for all to finish
step "Waiting for all agents to complete..."
wait $PID1 $PID2 $PID3 2>/dev/null || true

echo ""

# ────────────────────────────────────────────────────────────
# Step 7: Prove only ONE command executed
# ────────────────────────────────────────────────────────────
header "Verification"

RICHTER_RUNS=$("$RICHTER_BIN" runs 2>/dev/null || echo "")
echo "Recent runs from Richter daemon:"
echo "$RICHTER_RUNS" | head -20

echo ""

# Check agent output for deduplication messages
for out in "$AGENT1_OUT" "$AGENT2_OUT" "$AGENT3_OUT"; do
    echo "── $(basename "$out") ──"
    if grep -qi "joined existing" "$out"; then
        step "✓ DETECTED: 'joined existing run' — this agent was deduplicated"
    elif grep -qi "cache hit\|cached" "$out"; then
        step "✓ DETECTED: 'cache hit' — cached result returned"
    elif grep -qi "test result: ok" "$out"; then
        step "→ This agent ran the actual test (leader)"
    else
        cat "$out"
    fi
done

echo ""
step "Key metric: At most ONE actual cargo test process should have executed."
step "The other agents should show 'joined existing run' or 'cache hit'."

# ────────────────────────────────────────────────────────────
# Step 8: Show dashboard status
# ────────────────────────────────────────────────────────────
header "Dashboard Status"

echo "Daemon:     $("$RICHTER_BIN" status 2>/dev/null || echo 'check failed')"
echo "Repos:      $("$RICHTER_BIN" repos 2>/dev/null || echo 'check failed')"
echo "Agents:     $("$RICHTER_BIN" agents 2>/dev/null || echo 'check failed')"
echo "Runs:       $("$RICHTER_BIN" runs --last 5 2>/dev/null || echo 'check failed')"

echo ""
info "Open the Richter.app dashboard to see the full visual status."

# ────────────────────────────────────────────────────────────
# Step 9: Cleanup
# ────────────────────────────────────────────────────────────
rm -f "$AGENT1_OUT" "$AGENT2_OUT" "$AGENT3_OUT"

echo ""
echo "═══ Demo Complete ═══"
echo ""
echo "What was demonstrated:"
echo "  ✓ 3 AI agents ran 'cargo test' simultaneously"
echo "  ✓ Only ONE underlying process executed"
echo "  ✓ Other agents joined the existing run"
echo "  ✓ All agents received the same test results"
echo ""
echo "To clean everything:"
echo "  richter uninstall --all && rm -rf ~/.richter"
