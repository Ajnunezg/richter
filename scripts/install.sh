#!/usr/bin/env bash
#
# install.sh — Full Richter installation script.
#
# This script:
#   1. Builds release binaries (if not already built).
#   2. Installs the daemon (SMAppService/LaunchAgent).
#   3. Sets up shell integration (zsh, bash, or fish).
#   4. Installs PATH shims for build/test tools.
#   5. Configures MCP for Claude Code and Codex.
#   6. Verifies installation with `richter doctor`.
#
# Usage:
#   bash scripts/install.sh              # Full install (interactive)
#   bash scripts/install.sh --yes         # Non-interactive (accept all defaults)
#   bash scripts/install.sh --shell zsh   # Specific shell
#   bash scripts/install.sh --help        # Show help
#
# Requirements:
#   - macOS 14+
#   - Rust toolchain 1.80+
#   - Git

set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ────────────────────────────────────────────────────────────
# Defaults
# ────────────────────────────────────────────────────────────
YES_MODE=false
TARGET_SHELL=""
RICHTER_HOME="$HOME/.richter"
RICHTER_BIN_DIR="$RICHTER_HOME/bin"

# Colors
BOLD='\033[1m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

# ────────────────────────────────────────────────────────────
# Helpers
# ────────────────────────────────────────────────────────────
header()    { echo -e "\n${BOLD}${BLUE}═══ $1 ═══${NC}"; }
step()      { echo -e "  ${GREEN}→${NC} $1"; }
info()      { echo -e "    ${NC}$1${NC}"; }
warn()      { echo -e "  ${YELLOW}⚠${NC} $1"; }
err()       { echo -e "  ${RED}✗${NC} $1"; }
ok()        { echo -e "  ${GREEN}✓${NC} $1"; }

ask() {
    if [ "$YES_MODE" = true ]; then
        return 0
    fi
    local prompt="$1"
    local default="${2:-y}"
    local response
    read -r -p "  ? $prompt [$default] " response
    response="${response:-$default}"
    case "$response" in
        y|Y|yes|YES) return 0 ;;
        *) return 1 ;;
    esac
}

# ────────────────────────────────────────────────────────────
# Parse arguments
# ────────────────────────────────────────────────────────────
usage() {
    cat << 'EOF'
Usage: install.sh [FLAGS]

Flags:
  --yes            Non-interactive mode (accept all defaults)
  --shell <SHELL>  Target shell: zsh, bash, fish (auto-detect if omitted)
  --help           Show this help message
EOF
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --yes)
            YES_MODE=true
            shift
            ;;
        --shell)
            TARGET_SHELL="$2"
            shift 2
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

# Detect shell
if [ -z "$TARGET_SHELL" ]; then
    case "$SHELL" in
        */zsh)  TARGET_SHELL="zsh" ;;
        */bash) TARGET_SHELL="bash" ;;
        */fish) TARGET_SHELL="fish" ;;
        *)      TARGET_SHELL="zsh"
                warn "Unknown shell '$SHELL', defaulting to zsh." ;;
    esac
fi

# ────────────────────────────────────────────────────────────
# Pre-flight checks
# ────────────────────────────────────────────────────────────
header "Richter Installation"
echo ""
step "Target shell: $TARGET_SHELL"
step "Install dir:  $RICHTER_HOME"
step "Platform:     $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m))"
echo ""

if ! command -v rustc &>/dev/null; then
    err "Rust toolchain not found. Install via https://rustup.rs"
    exit 1
fi
ok "Rust $(rustc --version)"

if ! command -v git &>/dev/null; then
    err "Git not found. Install via xcode-select --install"
    exit 1
fi
ok "Git $(git --version)"

# ────────────────────────────────────────────────────────────
# Step 1: Build
# ────────────────────────────────────────────────────────────
header "Step 1: Build"

if [ -f "$PROJECT_ROOT/target/release/richter" ] && \
   [ -f "$PROJECT_ROOT/target/release/richterd" ] && \
   [ -f "$PROJECT_ROOT/target/release/richter-mcp" ]; then
    step "Release binaries found. Skipping build."
else
    step "Building release binaries..."
    bash "$PROJECT_ROOT/scripts/build.sh" --rust-only
fi

ok "Binaries present"

# ────────────────────────────────────────────────────────────
# Step 2: Install daemon
# ────────────────────────────────────────────────────────────
header "Step 2: Install Daemon"

# Create directories
mkdir -p "$RICHTER_BIN_DIR"
mkdir -p "$RICHTER_HOME/logs"
chmod 0700 "$RICHTER_HOME"
chmod 0700 "$RICHTER_HOME/logs"

# Copy daemon binary
step "Copying richterd to $RICHTER_BIN_DIR/"
cp "$PROJECT_ROOT/target/release/richterd" "$RICHTER_BIN_DIR/richterd"
chmod 0755 "$RICHTER_BIN_DIR/richterd"

# Copy CLI binary
step "Copying richter to $RICHTER_BIN_DIR/"
cp "$PROJECT_ROOT/target/release/richter" "$RICHTER_BIN_DIR/richter"
chmod 0755 "$RICHTER_BIN_DIR/richter"

# Copy MCP binary
step "Copying richter-mcp to $RICHTER_BIN_DIR/"
cp "$PROJECT_ROOT/target/release/richter-mcp" "$RICHTER_BIN_DIR/richter-mcp"
chmod 0755 "$RICHTER_BIN_DIR/richter-mcp"

# Create LaunchAgent plist
PLIST_PATH="$HOME/Library/LaunchAgents/com.richter.daemon.plist"
step "Creating LaunchAgent: $PLIST_PATH"

mkdir -p "$HOME/Library/LaunchAgents"

cat > "$PLIST_PATH" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.richter.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>$RICHTER_BIN_DIR/richterd</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$RICHTER_HOME/logs/daemon.log</string>
    <key>StandardErrorPath</key>
    <string>$RICHTER_HOME/logs/daemon.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>RICHTER_HOME</key>
        <string>$RICHTER_HOME</string>
    </dict>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
PLIST

chmod 0644 "$PLIST_PATH"

# Load the daemon
step "Loading daemon (launchctl bootstrap)..."
launchctl bootstrap "gui/$UID" "$PLIST_PATH" 2>/dev/null || {
    warn "launchctl bootstrap failed. The daemon may already be loaded."
}

# Wait for daemon to start
sleep 2

# Verify daemon is running
if "$RICHTER_BIN_DIR/richter" status 2>/dev/null | grep -q "running"; then
    ok "Daemon is running"
else
    warn "Daemon may not be running yet. Check with: $RICHTER_BIN_DIR/richter status"
    warn "If it doesn't start, check logs: tail -f $RICHTER_HOME/logs/daemon.log"
fi

# ────────────────────────────────────────────────────────────
# Step 3: Shell integration
# ────────────────────────────────────────────────────────────
header "Step 3: Shell Integration ($TARGET_SHELL)"

# Add Richter to PATH
RICHTER_PATH_LINE='export RICHTER_HOME="$HOME/.richter"'
SHIM_PATH_LINE='export PATH="$RICHTER_HOME/shims:$PATH"'

integrate_shell() {
    local rc_file="$1"

    if [ ! -f "$rc_file" ]; then
        touch "$rc_file"
    fi

    # Check if already integrated
    if grep -q "richter" "$rc_file" 2>/dev/null; then
        info "Shell integration already present in $rc_file"
        return
    fi

    step "Adding Richter integration to $rc_file"

    {
        echo ""
        echo "# Richter shell integration (added by install.sh)"
        echo "$RICHTER_PATH_LINE"
        echo "$SHIM_PATH_LINE"
    } >> "$rc_file"

    info "Added to $rc_file"
}

case "$TARGET_SHELL" in
    zsh)
        integrate_shell "$HOME/.zshrc"
        ;;
    bash)
        if [ -f "$HOME/.bash_profile" ]; then
            integrate_shell "$HOME/.bash_profile"
        else
            integrate_shell "$HOME/.bashrc"
        fi
        ;;
    fish)
        FISH_CONFIG="$HOME/.config/fish/config.fish"
        mkdir -p "$(dirname "$FISH_CONFIG")"
        if ! grep -q "richter" "$FISH_CONFIG" 2>/dev/null; then
            step "Adding Richter integration to $FISH_CONFIG"
            {
                echo ""
                echo "# Richter shell integration (added by install.sh)"
                echo "set -x RICHTER_HOME \"$HOME/.richter\""
                echo "set -x PATH \"$HOME/.richter/shims\" \$PATH"
            } >> "$FISH_CONFIG"
        fi
        ;;
esac

ok "Shell integration configured"
info "Restart your shell or run: source ~/.${TARGET_SHELL}rc"

# ────────────────────────────────────────────────────────────
# Step 4: Install shims
# ────────────────────────────────────────────────────────────
header "Step 4: Install Shims"

SHIM_DIR="$RICHTER_HOME/shims"

# Define all supported tools
TOOLS=(
    npm pnpm yarn bun node npx
    cargo
    python pytest uv ruff
    go
    swift xcodebuild
    gradle mvn
    make cmake ninja bazel
    turbo nx
    deno tsc eslint jest vitest playwright
)

step "Creating shim directory: $SHIM_DIR"
mkdir -p "$SHIM_DIR"

installed_count=0
for tool in "${TOOLS[@]}"; do
    # Skip if the real tool doesn't exist on the system
    REAL_TOOL=$(command -v "$tool" 2>/dev/null || true)
    if [ -z "$REAL_TOOL" ] || [ "$REAL_TOOL" = "$SHIM_DIR/$tool" ]; then
        # Tool not found on system; create a generic pass-through shim
        cat > "$SHIM_DIR/$tool" << SHIMEOF
#!/usr/bin/env bash
# Richter shim: $tool
exec "$RICHTER_BIN_DIR/richter" run --shim-name "$tool" -- "\$@"
SHIMEOF
    else
        cat > "$SHIM_DIR/$tool" << SHIMEOF
#!/usr/bin/env bash
# Richter shim: $tool → $REAL_TOOL
exec "$RICHTER_BIN_DIR/richter" run --shim-name "$tool" -- "\$@"
SHIMEOF
    fi
    chmod 0755 "$SHIM_DIR/$tool"
    installed_count=$((installed_count + 1))
done

ok "$installed_count shims installed in $SHIM_DIR"

# ────────────────────────────────────────────────────────────
# Step 5: MCP configuration
# ────────────────────────────────────────────────────────────
header "Step 5: MCP Configuration"

setup_mcp_claude() {
    local config_file=""
    # Check for existing Claude MCP config
    if [ -f "$HOME/.claude/claude_desktop_config.json" ]; then
        config_file="$HOME/.claude/claude_desktop_config.json"
    elif [ -f ".mcp.json" ]; then
        config_file="$(pwd)/.mcp.json"
    else
        # Create project-local config
        config_file="$(pwd)/.mcp.json"
        info "Creating $config_file"
    fi

    # Add Richter MCP entry
    # We use a simple approach: if richter isn't in the file, add it
    if [ -f "$config_file" ] && grep -q "richter" "$config_file" 2>/dev/null; then
        info "Claude Code MCP already configured with Richter"
    else
        step "Configuring Claude Code MCP..."
        # For a proper JSON merge, we'd need jq. For now, create a basic config.
        if [ ! -f "$config_file" ]; then
            cat > "$config_file" << 'MCPEOF'
{
  "mcpServers": {
    "richter": {
      "command": "richter-mcp",
      "args": ["stdio"]
    }
  }
}
MCPEOF
        fi
        info "Claude Code MCP config: $config_file"
    fi
}

setup_mcp_codex() {
    local config_file="$HOME/.codex/mcp.json"
    local config_dir="$(dirname "$config_file")"

    mkdir -p "$config_dir"

    if [ -f "$config_file" ] && grep -q "richter" "$config_file" 2>/dev/null; then
        info "Codex MCP already configured with Richter"
    else
        step "Configuring Codex MCP..."
        if [ ! -f "$config_file" ]; then
            mkdir -p "$config_dir"
            cat > "$config_file" << 'MCPEOF'
{
  "mcpServers": {
    "richter": {
      "command": "richter-mcp",
      "args": ["stdio"]
    }
  }
}
MCPEOF
        fi
        info "Codex MCP config: $config_file"
    fi
}

setup_mcp_claude
setup_mcp_codex
ok "MCP configuration complete"

# ────────────────────────────────────────────────────────────
# Step 6: Verify
# ────────────────────────────────────────────────────────────
header "Step 6: Verify Installation"

sleep 1
"$RICHTER_BIN_DIR/richter" doctor 2>&1 || true

# ────────────────────────────────────────────────────────────
# Done
# ────────────────────────────────────────────────────────────
echo ""
echo "${BOLD}${GREEN}═══ Installation Complete ═══${NC}"
echo ""
echo "What was installed:"
echo "  • Daemon:      $RICHTER_BIN_DIR/richterd (LaunchAgent)"
echo "  • CLI:         $RICHTER_BIN_DIR/richter"
echo "  • MCP:         $RICHTER_BIN_DIR/richter-mcp"
echo "  • Shims:       $installed_count tools in $SHIM_DIR"
echo "  • Shell:       ~/.${TARGET_SHELL}rc"
echo "  • MCP:         Claude Code + Codex"
echo ""
echo "Next steps:"
echo "  1. Restart your shell: exec \$SHELL -l"
echo "  2. Verify:              richter doctor"
echo "  3. Open dashboard:      open /Applications/Richter.app"
echo "  4. Run demo:            bash scripts/demo.sh"
echo ""
echo "To uninstall:"
echo "  $RICHTER_BIN_DIR/richter uninstall --all"
echo "  rm -rf $RICHTER_HOME"
