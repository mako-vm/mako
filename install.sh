#!/bin/bash
set -e

REPO="mako-vm/mako"
INSTALL_DIR="/usr/local/bin"

echo "Installing Mako (build from source)"
echo ""

# Check macOS
if [ "$(uname)" != "Darwin" ]; then
    echo "Error: Mako only supports macOS"
    exit 1
fi

# Check macOS version (need 13+)
MACOS_VERSION=$(sw_vers -productVersion | cut -d. -f1)
if [ "$MACOS_VERSION" -lt 13 ]; then
    echo "Error: Mako requires macOS 13 (Ventura) or later"
    exit 1
fi

# Check prerequisites
if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust not found. Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    . "$HOME/.cargo/env"
fi

if ! command -v swiftc >/dev/null 2>&1; then
    echo "Error: Xcode Command Line Tools required. Install with: xcode-select --install"
    exit 1
fi

# Ensure musl target for agent cross-compilation
rustup target add aarch64-unknown-linux-musl 2>/dev/null || true

# Clone or detect existing repo
SRCDIR=""
if [ -f "Cargo.toml" ] && grep -q "mako" Cargo.toml 2>/dev/null; then
    SRCDIR="$(pwd)"
    echo "Using existing source in $SRCDIR"
else
    SRCDIR="$(mktemp -d)/mako"
    echo "Cloning repository..."
    git clone "https://github.com/$REPO.git" "$SRCDIR"
fi

cd "$SRCDIR"

echo "Building host binaries..."
cargo build --release

echo "Cross-compiling VM agent..."
cargo build --release --target aarch64-unknown-linux-musl -p mako-agent 2>/dev/null || \
    echo "  (agent cross-compile skipped — install musl cross toolchain for full setup)"

echo "Codesigning makod..."
codesign --entitlements crates/daemon/entitlements.plist --force -s - target/release/makod

echo "Installing to $INSTALL_DIR (may need sudo)..."
sudo install -m 755 target/release/mako "$INSTALL_DIR/mako"
sudo install -m 755 target/release/makod "$INSTALL_DIR/makod"
sudo codesign --entitlements crates/daemon/entitlements.plist --force -s - "$INSTALL_DIR/makod" 2>/dev/null || true

echo ""
echo "Mako installed successfully!"
echo ""
echo "Quick start:"
echo "  mako setup     # Build VM image (first time only)"
echo "  mako start     # Start the VM and Docker engine"
echo "  export DOCKER_HOST=unix://\$HOME/.mako/docker.sock"
echo "  docker ps      # Use Docker as usual"
