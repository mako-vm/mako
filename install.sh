#!/bin/bash
set -e

REPO="mako-vm/mako"
INSTALL_DIR="/usr/local/bin"

echo "Installing Mako..."
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

ARCH=$(uname -m)
if [ "$ARCH" = "arm64" ]; then
    TARGET="aarch64-apple-darwin"
elif [ "$ARCH" = "x86_64" ]; then
    TARGET="x86_64-apple-darwin"
else
    echo "Error: Unsupported architecture: $ARCH"
    exit 1
fi

# Get latest release
echo "Fetching latest release..."
LATEST=$(curl -s "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | cut -d'"' -f4)

if [ -z "$LATEST" ]; then
    echo "No releases found. Building from source..."
    echo ""
    echo "Prerequisites:"
    echo "  - Rust 1.75+ (https://rustup.rs)"
    echo "  - Xcode Command Line Tools"
    echo "  - e2fsprogs (brew install e2fsprogs)"
    echo ""
    echo "Build steps:"
    echo "  git clone https://github.com/$REPO.git"
    echo "  cd mako"
    echo "  cargo build --release"
    echo "  cargo build --release --target aarch64-unknown-linux-musl -p mako-agent"
    echo "  ./target/release/mako setup"
    echo "  codesign --entitlements crates/daemon/entitlements.plist --force -s - target/release/makod"
    echo ""
    echo "Then add target/release to your PATH."
    exit 0
fi

echo "Latest version: $LATEST"
DOWNLOAD_URL="https://github.com/$REPO/releases/download/$LATEST/mako-$LATEST-$TARGET.tar.gz"

# Download
TMPDIR=$(mktemp -d)
echo "Downloading $DOWNLOAD_URL..."
if curl -fSL "$DOWNLOAD_URL" -o "$TMPDIR/mako.tar.gz" 2>/dev/null; then
    tar xzf "$TMPDIR/mako.tar.gz" -C "$TMPDIR"

    echo "Installing to $INSTALL_DIR (may need sudo)..."
    sudo install -m 755 "$TMPDIR/mako" "$INSTALL_DIR/mako"
    sudo install -m 755 "$TMPDIR/makod" "$INSTALL_DIR/makod"

    # Codesign makod
    sudo codesign --force -s - "$INSTALL_DIR/makod" 2>/dev/null || true

    rm -rf "$TMPDIR"

    echo ""
    echo "Mako installed successfully!"
    echo ""
    echo "Quick start:"
    echo "  mako setup     # Download VM image (first time only)"
    echo "  mako start     # Start the VM and Docker engine"
    echo "  export DOCKER_HOST=unix://\$HOME/.mako/docker.sock"
    echo "  docker ps      # Use Docker as usual"
else
    rm -rf "$TMPDIR"
    echo ""
    echo "No pre-built binaries available yet. Build from source:"
    echo "  git clone https://github.com/$REPO.git && cd mako"
    echo "  cargo build --release"
    echo "  cargo build --release --target aarch64-unknown-linux-musl -p mako-agent"
    echo "  ./target/release/mako setup"
fi
