# Mako

A fast, lightweight, open-source Docker Desktop alternative for macOS.

Built on Apple's Virtualization.framework with stock dockerd running inside a minimal Alpine Linux VM. Uses reverse vsock for near-native Docker API performance and VirtioFS for file sharing.

## Architecture

```
┌───────────────────────────────────────────┐
│ macOS Host                                │
│                                           │
│    mako CLI ──┐    ┌── GUI (menu bar)     │
│               ▼    ▼                      │
│         ┌────────────────┐                │
│         │    makod       │                │
│         │  socket proxy  │                │
│         │  port forward  │                │
│         │  VM manager    │                │
│         └──────┬─────────┘                │
│                │ vsock                    │
│         ┌──────▼─────────┐                │
│         │  Linux VM      │                │
│         │  mako-agent    │                │
│         │  dockerd       │                │
│         │  containerd    │                │
│         │  runc          │                │
│         └────────────────┘                │
└───────────────────────────────────────────┘
```

## How It Works

Mako runs a lightweight Linux VM using Apple's Virtualization.framework.
Inside the VM, stock dockerd (from the Moby project) handles all container
operations. The Docker socket is forwarded from the VM to macOS over vsock,
so the standard `docker` CLI and Docker Compose work out of the box.

**What Mako builds from scratch:**
- VM lifecycle management via Virtualization.framework
- Docker socket forwarding over reverse vsock
- Port forwarding (container `-p` ports accessible on localhost)
- VirtioFS file sharing (macOS home directory mounted in VM)
- Rosetta 2 integration for x86 containers on Apple Silicon
- Graceful shutdown with signal handling and PID management
- Native macOS menu bar GUI with container management
- Launch at login via launchd

**What Mako bundles** (proven, open-source components):
- dockerd / Moby (Docker engine)
- containerd (container runtime)
- runc (OCI runtime)

## Requirements

- macOS 13 (Ventura) or later
- Apple Silicon or Intel Mac
- Rust 1.75+
- Xcode Command Line Tools (for Swift FFI bridge)
- e2fsprogs (`brew install e2fsprogs`) for VM image build
- `aarch64-unknown-linux-musl` Rust target (`rustup target add aarch64-unknown-linux-musl`)

## Quick Start

```bash
# Build host binaries
cargo build --release

# Cross-compile the in-VM agent for Linux
cargo build --release --target aarch64-unknown-linux-musl -p mako-agent

# Build VM image (downloads Alpine, Docker, kernel → ~/.mako/)
./target/release/mako setup

# Codesign daemon (required for Virtualization.framework)
codesign --entitlements crates/daemon/entitlements.plist --force -s - target/release/makod

# Start
./target/release/mako start -f    # foreground (or omit -f for background)

# Use Docker
export DOCKER_HOST=unix://$HOME/.mako/docker.sock
docker run hello-world
docker ps

# Stop
./target/release/mako stop
```

### GUI

```bash
cd gui/MakoApp && swift build -c release
.build/release/MakoApp
```

Appears as a cube icon in the menu bar with container list, start/stop controls, and Docker info.

### Shell Completions

```bash
# Generate and install zsh completions
./target/release/mako completions zsh > ~/.zfunc/_mako
```

## Project Structure

```
crates/
  cli/        mako CLI (start, stop, status, setup, info, config, completions)
  daemon/     makod host daemon (VM management, socket proxy, port forwarding)
  agent/      mako-agent (runs inside VM, relays Docker API over vsock)
  common/     shared config, types, protocol definitions
swift-ffi/    thin Swift bridge for Apple Virtualization.framework
vm-image/     Linux kernel, rootfs, and initramfs build scripts
gui/MakoApp/  macOS menu bar application (SwiftUI)
```

## Status

Mako is under active development. The core Docker workflow is functional:
containers, images, volumes, networks, and Docker Compose all work through
the standard `docker` CLI.

**Working:**
- VM boot/stop, Docker engine, all Docker CLI commands
- Port forwarding (`-p` flags work on localhost)
- VirtioFS file sharing (home directory)
- Rosetta x86 emulation (Apple Silicon)
- Menu bar GUI, launch at login, shell completions

**Planned:**
- Kubernetes (K3s) integration
- DNS forwarding (container name resolution from macOS)
- Dynamic memory ballooning
- Configurable VirtioFS share paths
- CI/CD and Homebrew distribution

## License

Apache-2.0
