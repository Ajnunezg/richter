#!/bin/bash
set -euo pipefail
BIN="target/release"

cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

echo "╔══════════════════════════════════════════════╗"
echo "║     Richter Mobile — End-to-End Smoke Test   ║"
echo "╚══════════════════════════════════════════════╝"
echo ""

echo "📦 Building release..."
cargo build --release -q 2>&1 | tail -1

echo "🚀 Starting daemon (mobile enabled)..."
RICHTER_MOBILE_ENABLED=true RUST_LOG=error "$BIN/richter-daemon" &
DAEMON_PID=$!
sleep 2
TOKEN=$(cat ~/.richter/auth_token)

echo ""
echo "━━━ 1. Main daemon health ━━━"
curl -s --unix-socket /tmp/richter.sock -H "Authorization: Bearer $TOKEN" http://localhost/health | python3 -m json.tool

echo ""
echo "━━━ 2. Mobile health (TCP) ━━━"
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:9777/mobile/v1/health | python3 -m json.tool

echo ""
echo "━━━ 3. Mobile now ━━━"
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:9777/mobile/v1/now | python3 -m json.tool

echo ""
echo "━━━ 4. Submit command ━━━"
curl -s --unix-socket /tmp/richter.sock -X POST \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"command":"echo hello-mobile","repo":"/tmp","classification":"unknown"}' \
  http://localhost/run_or_join | python3 -m json.tool

echo ""
echo "━━━ 5. Mobile runs ━━━"
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:9777/mobile/v1/runs | python3 -m json.tool

echo ""
echo "━━━ 6. Mobile events ━━━"
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:9777/mobile/v1/events/important | python3 -m json.tool

echo ""
echo "━━━ 7. CLI mobile status ━━━"
"$BIN/richter" mobile status

echo ""
echo "✅ E2E mobile smoke test complete"
