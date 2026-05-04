#!/usr/bin/env bash
#
# build.sh — Build all Richter crates, binaries, and the Agnos macOS app.
#
# Usage:
#   bash scripts/build.sh              # Release build (default)
#   bash scripts/build.sh --debug       # Debug build
#   bash scripts/build.sh --release     # Explicit release build
#   bash scripts/build.sh --app-only    # Only the SwiftUI app (requires Xcode)
#   bash scripts/build.sh --rust-only   # Only Rust crates
#   bash scripts/build.sh --help        # Show help
#
# Output:
#   target/release/richter              CLI binary
#   target/release/richterd             Daemon binary
#   target/release/richter-mcp          MCP server binary
#   target/release/Richter.app          macOS app bundle (if --app-only or full build)
#
# Requirements:
#   - Rust toolchain 1.80+ (rustup)
#   - Xcode 16+ (for app build)
#   - macOS 14+

set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ────────────────────────────────────────────────────────────
# Defaults
# ────────────────────────────────────────────────────────────
BUILD_MODE="release"
BUILD_RUST=true
BUILD_APP=true
RUST_FLAGS=""
CARGO_CMD="build"

# ────────────────────────────────────────────────────────────
# Parse arguments
# ────────────────────────────────────────────────────────────
usage() {
    cat << 'EOF'
Usage: build.sh [FLAGS]

Flags:
  --debug          Build with debug symbols (default: release)
  --release        Build optimized release binaries (default)
  --app-only       Build only the SwiftUI macOS app
  --rust-only      Build only Rust crates (richter, richterd, richter-mcp)
  --help           Show this help message
EOF
    exit 0
}

for arg in "$@"; do
    case "$arg" in
        --debug)
            BUILD_MODE="debug"
            ;;
        --release)
            BUILD_MODE="release"
            ;;
        --app-only)
            BUILD_RUST=false
            BUILD_APP=true
            ;;
        --rust-only)
            BUILD_RUST=true
            BUILD_APP=false
            ;;
        --help)
            usage
            ;;
        *)
            echo "Unknown flag: $arg"
            echo "Use --help for usage."
            exit 1
            ;;
    esac
done

cd "$PROJECT_ROOT"

# ────────────────────────────────────────────────────────────
# Check prerequisites
# ────────────────────────────────────────────────────────────
check_rust() {
    if ! command -v rustc &>/dev/null; then
        echo "ERROR: Rust toolchain not found. Install via https://rustup.rs"
        exit 1
    fi
    local rust_ver
    rust_ver=$(rustc --version | grep -oE '[0-9]+\.[0-9]+' | head -1)
    echo "  Rust: $(rustc --version)"
}

check_xcode() {
    if ! command -v xcodebuild &>/dev/null; then
        echo "WARN:  Xcode command-line tools not found. App build will be skipped."
        echo "       Install via: xcode-select --install"
        BUILD_APP=false
        return
    fi
    echo "  Xcode: $(xcodebuild -version | head -1)"
}

echo "═══ Richter Build ═══"
echo "Mode:    $BUILD_MODE"
echo "Rust:    $([ "$BUILD_RUST" = true ] && echo 'yes' || echo 'skip')"
echo "App:     $([ "$BUILD_APP" = true ] && echo 'yes' || echo 'skip')"
echo ""
echo "Prerequisites:"
check_rust
check_xcode
echo ""

# ────────────────────────────────────────────────────────────
# Rust build
# ────────────────────────────────────────────────────────────
build_rust() {
    echo "── Building Rust crates ($BUILD_MODE) ──"

    local cargo_args=("$CARGO_CMD" "--workspace")

    if [ "$BUILD_MODE" = "release" ]; then
        cargo_args+=("--release")
    fi

    cargo "${cargo_args[@]}"

    # Determine target directory
    local target_dir="target/$BUILD_MODE"

    echo ""
    echo "Rust binaries:"
    local binaries=("richter" "richterd" "richter-mcp")
    for bin in "${binaries[@]}"; do
        if [ -f "$target_dir/$bin" ]; then
            local size
            size=$(du -h "$target_dir/$bin" | cut -f1)
            echo "  ✓ $target_dir/$bin ($size)"
        else
            echo "  ✗ $target_dir/$bin not found"
        fi
    done

    echo ""
    echo "Rust build OK."
}

# ────────────────────────────────────────────────────────────
# SwiftUI App build
# ────────────────────────────────────────────────────────────
build_app() {
    echo "── Building Richter.app ($BUILD_MODE) ──"

    local apps_dir="$PROJECT_ROOT/apps/macos/RichterApp"

    if [ ! -d "$apps_dir" ]; then
        echo "WARN:  apps/macos/RichterApp/ not found. Skipping app build."
        BUILD_APP=false
        return
    fi

    if [ ! -f "$apps_dir/RichterApp.xcodeproj/project.pbxproj" ] && \
       [ ! -d "$apps_dir/RichterApp.xcodeproj" ]; then
        echo "WARN:  Xcode project not found in $apps_dir. Skipping app build."
        echo "       The SwiftUI app project will be scaffolded during implementation."
        BUILD_APP=false
        return
    fi

    local xcode_config="Release"
    if [ "$BUILD_MODE" = "debug" ]; then
        xcode_config="Debug"
    fi

    # Xcode build
    xcodebuild \
        -project "$apps_dir/RichterApp.xcodeproj" \
        -scheme RichterApp \
        -configuration "$xcode_config" \
        -derivedDataPath "$PROJECT_ROOT/target/xcode-derived" \
        -archivePath "$PROJECT_ROOT/target/Richter" \
        archive \
        ONLY_ACTIVE_ARCH=YES \
        CODE_SIGN_IDENTITY="-" \
        CODE_SIGNING_REQUIRED=NO \
        CODE_SIGNING_ALLOWED=NO

    # Copy app bundle to target directory
    local app_src="$PROJECT_ROOT/target/xcode-derived/Build/Products/$xcode_config/RichterApp.app"
    local app_dst="$PROJECT_ROOT/target/$BUILD_MODE/Richter.app"

    if [ -d "$app_src" ]; then
        rm -rf "$app_dst"
        cp -R "$app_src" "$app_dst"
        echo "  ✓ $app_dst"
    else
        echo "  ✗ App bundle not found at $app_src"
        return 1
    fi

    echo ""
    echo "App build OK."
}

# ────────────────────────────────────────────────────────────
# Main
# ────────────────────────────────────────────────────────────

if [ "$BUILD_RUST" = true ]; then
    build_rust
fi

if [ "$BUILD_APP" = true ]; then
    build_app
fi

echo ""
echo "═══ Build complete ═══"
echo ""
echo "Binaries:"
if [ "$BUILD_RUST" = true ]; then
    echo "  richter:       target/$BUILD_MODE/richter"
    echo "  richterd:      target/$BUILD_MODE/richterd"
    echo "  richter-mcp:   target/$BUILD_MODE/richter-mcp"
fi
if [ "$BUILD_APP" = true ] && [ -d "target/$BUILD_MODE/Richter.app" ]; then
    echo "  Richter.app:   target/$BUILD_MODE/Richter.app"
fi
echo ""
echo "Next steps:"
echo "  bash scripts/test.sh         Run tests"
echo "  bash scripts/install.sh      Install Richter"
echo "  bash scripts/demo.sh         Run demo"
