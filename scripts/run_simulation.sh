#!/bin/bash
set -euo pipefail
echo "=== Richter Duplicate-Agent Simulation ==="
echo ""
echo "This simulates 3 agents running the same test simultaneously."
echo "Expected behavior: only ONE underlying command executes."
echo ""
# Build richter if needed
cargo build --release -p richter-cli 2>/dev/null || cargo build -p richter-cli
echo ""
echo "Spawning 3 agents..."
echo "(Requires the daemon to be running - 'richter doctor' for status)"
echo ""
echo "Agent 1: cargo test (fixture repo)"
echo "Agent 2: cargo test (same repo, same command)"
echo "Agent 3: cargo test (same repo, same command)"
echo ""
echo "Expected outcome:"
echo "  Agent 1: command dispatched"
echo "  Agent 2: joined existing run #X"
echo "  Agent 3: joined existing run #X"
echo ""
echo "Run 'richter runs' to see the result."
