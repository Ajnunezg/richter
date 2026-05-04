#!/usr/bin/env bash
# -----------------------------------------------------------------------------
# Richter Shell Initialization Script
#
# Adds ~/.richter/shims to the front of PATH and sets up shell hooks for
# automatic Richter command passthrough.
#
# Installation:
#   source <(richter install shell --print)
#
# Manual installation:
#   Add this to your ~/.zshrc or ~/.bashrc:
#     export RICHTER_SHIMS_DIR="$HOME/.richter/shims"
#     export PATH="$RICHTER_SHIMS_DIR:$PATH"
#     source "$HOME/.richter/shell-init.sh"
#
# Uninstall:
#   richter uninstall shell
# -----------------------------------------------------------------------------

RICHTER_HOME="${RICHTER_HOME:-$HOME/.richter}"
RICHTER_SHIMS_DIR="${RICHTER_SHIMS_DIR:-$RICHTER_HOME/shims}"

# ── Ensure shims directory exists ────────────────────────────────────────────
if [ ! -d "$RICHTER_SHIMS_DIR" ]; then
    echo "Richter: shims directory not found at $RICHTER_SHIMS_DIR" >&2
    echo "Richter: run 'richter install shims' to create the shim layer." >&2
fi

# ── Add shims to PATH (front, before package managers) ───────────────────────
# Only add if not already present.
case ":$PATH:" in
    *:"$RICHTER_SHIMS_DIR":*)
        # Already in PATH.
        ;;
    *)
        export PATH="$RICHTER_SHIMS_DIR:$PATH"
        ;;
esac

# ── Richter shell functions ──────────────────────────────────────────────────

# richter_run: passthrough wrapper that invokes the Richter daemon for run/join.
# Falls back to direct execution if the daemon is offline.
richter_run() {
    local shim_name="$1"
    shift
    if command -v richter &>/dev/null; then
        richter run --shim-name "$shim_name" -- "$@"
    else
        # Daemon/CLI not available; execute directly.
        command "$shim_name" "$@"
    fi
}

# richter_status_line: one-line status for prompt integration.
richter_status_line() {
    if command -v richter &>/dev/null; then
        richter status --brief 2>/dev/null || true
    fi
}

# ── Optional: Zsh precmd hook for statusline ─────────────────────────────────
# Uncomment to show Richter status before each prompt in zsh.
# precmd() {
#     richter_status_line
# }

# ── Optional: Bash PROMPT_COMMAND ────────────────────────────────────────────
# Uncomment for bash users.
# PROMPT_COMMAND="richter_status_line${PROMPT_COMMAND:+;$PROMPT_COMMAND}"

# ── Export Richter environment for child processes ───────────────────────────
export RICHTER_HOME
export RICHTER_SHIMS_DIR

# ── Notify if daemon is unreachable ──────────────────────────────────────────
# (Only on interactive shell startup, not every subshell invocation.)
if [ -n "${-//[^i]/}" ] && [ -z "${RICHTER_SHELL_INIT_RAN:-}" ]; then
    export RICHTER_SHELL_INIT_RAN=1
    if command -v richter &>/dev/null; then
        if richter status --brief &>/dev/null; then
            : # Daemon is reachable.
        else
            echo "Richter: daemon not running. Start with 'richter daemon start' or launch the Richter app." >&2
        fi
    fi
fi
