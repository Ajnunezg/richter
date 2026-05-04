#!/usr/bin/env bash
#
# test.sh — Run Richter test suite (unit + integration + clippy + fmt).
#
# Usage:
#   bash scripts/test.sh                    # Run all checks
#   bash scripts/test.sh --crate richter-core  # Single crate
#   bash scripts/test.sh --nextest            # Use cargo-nextest (faster)
#   bash scripts/test.sh --no-clippy          # Skip clippy
#   bash scripts/test.sh --no-fmt             # Skip format check
#   bash scripts/test.sh --help               # Show help
#
# Requirements:
#   - Rust toolchain 1.80+ (rustup)
#   - Optional: cargo-nextest (for --nextest)

set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ────────────────────────────────────────────────────────────
# Defaults
# ────────────────────────────────────────────────────────────
RUN_TESTS=true
RUN_CLIPPY=true
RUN_FMT=true
USE_NEXTEST=false
TARGET_CRATE=""
FAILURES=0

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# ────────────────────────────────────────────────────────────
# Helpers
# ────────────────────────────────────────────────────────────
pass_msg() { echo -e "${GREEN}✓${NC} $1"; }
fail_msg() { echo -e "${RED}✗${NC} $1"; FAILURES=$((FAILURES + 1)); }
warn_msg() { echo -e "${YELLOW}⚠${NC} $1"; }

# ────────────────────────────────────────────────────────────
# Parse arguments
# ────────────────────────────────────────────────────────────
usage() {
    cat << 'EOF'
Usage: test.sh [FLAGS]

Flags:
  --crate <NAME>    Test only a specific crate (e.g., richter-core)
  --nextest         Use cargo-nextest test runner (faster, requires install)
  --no-clippy       Skip clippy lint checks
  --no-fmt          Skip rustfmt format check
  --help            Show this help message
EOF
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --crate)
            TARGET_CRATE="$2"
            shift 2
            ;;
        --nextest)
            USE_NEXTEST=true
            shift
            ;;
        --no-clippy)
            RUN_CLIPPY=false
            shift
            ;;
        --no-fmt)
            RUN_FMT=false
            shift
            ;;
        --help)
            usage
            ;;
        *)
            echo "Unknown flag: $1"
            echo "Use --help for usage."
            exit 1
            ;;
    esac
done

cd "$PROJECT_ROOT"

# ────────────────────────────────────────────────────────────
# Check prerequisites
# ────────────────────────────────────────────────────────────
echo "═══ Richter Test Suite ═══"
echo ""

if ! command -v rustc &>/dev/null; then
    fail_msg "Rust toolchain not found. Install via https://rustup.rs"
    exit 1
fi

echo "Rust: $(rustc --version)"
echo "Cargo: $(cargo --version)"
if [ "$USE_NEXTEST" = true ]; then
    if command -v cargo-nextest &>/dev/null; then
        echo "Nextest: $(cargo nextest --version 2>&1 | head -1)"
    else
        fail_msg "cargo-nextest not found. Install: curl -LsSf https://get.nexte.st/latest/mac | tar zxf - -C ~/.cargo/bin"
        exit 1
    fi
fi
if [ -n "$TARGET_CRATE" ]; then
    echo "Crate:  $TARGET_CRATE"
fi
echo ""

# ────────────────────────────────────────────────────────────
# Format check
# ────────────────────────────────────────────────────────────
if [ "$RUN_FMT" = true ]; then
    echo "── rustfmt check ──"
    if cargo fmt --all -- --check 2>/dev/null; then
        pass_msg "Formatting OK"
    else
        fail_msg "Formatting issues found. Run: cargo fmt --all"
    fi
    echo ""
fi

# ────────────────────────────────────────────────────────────
# Clippy
# ────────────────────────────────────────────────────────────
if [ "$RUN_CLIPPY" = true ]; then
    echo "── clippy ──"

    local clippy_args=("clippy" "--all-targets" "--all-features" "--" "-D" "warnings")
    if [ -n "$TARGET_CRATE" ]; then
        clippy_args=("clippy" "-p" "$TARGET_CRATE" "--all-targets" "--all-features" "--" "-D" "warnings")
    fi

    if cargo "${clippy_args[@]}" 2>&1; then
        pass_msg "Clippy OK"
    else
        fail_msg "Clippy warnings/errors found"
    fi
    echo ""
fi

# ────────────────────────────────────────────────────────────
# Tests
# ────────────────────────────────────────────────────────────
if [ "$RUN_TESTS" = true ]; then
    echo "── tests ──"

    if [ "$USE_NEXTEST" = true ]; then
        local test_args=("nextest" "run" "--workspace")
        if [ -n "$TARGET_CRATE" ]; then
            test_args=("nextest" "run" "-p" "$TARGET_CRATE")
        fi
    else
        local test_args=("test" "--workspace")
        if [ -n "$TARGET_CRATE" ]; then
            test_args=("test" "-p" "$TARGET_CRATE")
        fi
    fi

    if cargo "${test_args[@]}" 2>&1; then
        pass_msg "All tests passed"
    else
        fail_msg "Test failures"
    fi
    echo ""
fi

# ────────────────────────────────────────────────────────────
# Summary
# ────────────────────────────────────────────────────────────
echo "═══ Results ═══"
if [ "$FAILURES" -eq 0 ]; then
    echo -e "${GREEN}All checks passed.${NC}"
else
    echo -e "${RED}$FAILURES check(s) failed.${NC}"
    exit 1
fi
