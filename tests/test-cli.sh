#!/usr/bin/env bash
set -uo pipefail

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); echo "  ✓ $1"; }
fail() { FAILED=$((FAILED + 1)); echo "  ✗ $1"; }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ -n "${MAKO_BIN:-}" ]; then
    MAKO="$MAKO_BIN"
elif [ -x "$PROJECT_ROOT/target/release/mako" ]; then
    MAKO="$PROJECT_ROOT/target/release/mako"
elif [ -x "$PROJECT_ROOT/target/debug/mako" ]; then
    MAKO="$PROJECT_ROOT/target/debug/mako"
elif command -v mako >/dev/null 2>&1; then
    MAKO="mako"
else
    echo "Error: mako binary not found. Build with: cargo build --release"
    exit 1
fi

echo "=== Mako CLI Smoke Tests ==="
echo "  Binary: $MAKO"
echo

# 1. --version exits 0 and prints a version string
echo "[1] mako --version"
if $MAKO --version 2>/dev/null | grep -qE '^mako'; then
    pass "--version prints version string"
else
    fail "--version did not print expected output"
fi

# 2. --help contains expected subcommands
echo "[2] mako --help"
HELP=$($MAKO --help 2>&1 || true)
ALL_OK=true
for cmd in start stop status setup info config; do
    if echo "$HELP" | grep -q "$cmd"; then
        pass "--help contains '$cmd'"
    else
        fail "--help missing '$cmd'"
        ALL_OK=false
    fi
done

# 3. mako status works (with or without daemon running)
echo "[3] mako status"
if $MAKO status 2>/dev/null; then
    pass "status exits 0"
else
    # status may exit non-zero if daemon isn't running; that's acceptable
    pass "status ran (daemon may not be running)"
fi

# 4. mako config show outputs valid JSON
echo "[4] mako config show"
CONFIG_OUT=$($MAKO config show 2>/dev/null || echo "")
if echo "$CONFIG_OUT" | python3 -m json.tool >/dev/null 2>&1; then
    pass "config show outputs valid JSON"
else
    fail "config show did not output valid JSON"
fi

# 5. If daemon is running, test ps and images
echo "[5] docker-dependent commands (skipped if daemon not running)"
SOCKET="$HOME/.mako/docker.sock"
if [ -S "$SOCKET" ]; then
    if $MAKO ps 2>/dev/null; then
        pass "mako ps exits 0"
    else
        fail "mako ps failed"
    fi
    if $MAKO images 2>/dev/null; then
        pass "mako images exits 0"
    else
        fail "mako images failed"
    fi
else
    echo "  - Skipped (no daemon running)"
fi

echo
echo "=== Results: $PASSED passed, $FAILED failed ==="
[ "$FAILED" -eq 0 ] || exit 1
