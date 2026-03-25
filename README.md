# Mako

A fast, lightweight, open-source Docker Desktop alternative for macOS.

Built on Apple's Virtualization.framework with stock dockerd running inside a minimal Alpine Linux VM. Uses reverse vsock for near-native Docker API performance, VirtioFS for file sharing, and Rosetta 2 for seamless x86_64 container support on Apple Silicon.

## Architecture

```
┌───────────────────────────────────────────────┐
│ macOS Host                                    │
│                                               │
│    mako CLI ──┐    ┌── GUI (menu bar)         │
│               ▼    ▼                          │
│         ┌─────────────────────┐               │
│         │      makod          │               │
│         │  socket proxy       │               │
│         │  port forward       │               │
│         │  DNS proxy          │               │
│         │  HTTP CONNECT proxy │               │
│         │  VM manager         │               │
│         └──────┬──────────────┘               │
│                │ vsock                        │
│         ┌──────▼──────────────┐               │
│         │  Linux VM           │               │
│         │  mako-agent         │               │
│         │  dockerd            │               │
│         │  containerd / runc  │               │
│         │  Rosetta (binfmt)   │               │
│         └─────────────────────┘               │
└───────────────────────────────────────────────┘
```

## How It Works

Mako runs a lightweight Linux VM using Apple's Virtualization.framework.
Inside the VM, stock dockerd (from the Moby project) handles all container
operations. The Docker socket is forwarded from the VM to macOS over vsock,
so the standard `docker` CLI and Docker Compose work out of the box.

**What Mako builds from scratch:**
- VM lifecycle management via Virtualization.framework
- Docker socket forwarding over reverse vsock (async I/O)
- Port forwarding with event-driven discovery (Docker events API)
- VirtioFS file sharing (macOS home directory mounted in VM)
- Rosetta 2 integration for x86_64 containers on Apple Silicon (opt-in)
- VPN-aware DNS proxy (resolves corporate/internal domains from inside VM)
- HTTP CONNECT proxy (routes Docker pulls through host VPN)
- VM suspend/resume for fast startup
- Disk write-back caching and noatime for I/O performance
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

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/mako-vm/mako/main/install.sh | bash
```

This clones the repo, builds from source, codesigns the daemon, and installs to `/usr/local/bin`.

## Quick Start

```bash
# Build everything (host binaries + agent + codesign)
make

# Build VM image (downloads Alpine, Docker, kernel → ~/.mako/)
make setup

# Start
./target/release/mako start -f    # foreground (or omit -f for background)

# Use Docker
export DOCKER_HOST=unix://$HOME/.mako/docker.sock
docker run hello-world
docker ps

# Stop
./target/release/mako stop
```

### Make Targets

| Target | Description |
|--------|-------------|
| `make` | Build host binaries, cross-compile agent, codesign daemon |
| `make build` | Build host binaries only (mako CLI + makod) |
| `make agent` | Cross-compile mako-agent for Linux (musl) |
| `make gui` | Build the macOS menu bar app |
| `make codesign` | Codesign makod for Virtualization.framework |
| `make install` | Build + install to `/usr/local/bin` (sudo) |
| `make setup` | Build + run `mako setup` to create VM image |
| `make test` | Run all tests |
| `make check` | Full CI check (clippy, fmt, test, agent check) |
| `make clean` | Remove build artifacts |

### GUI

```bash
cd gui/MakoApp && swift build -c release
.build/release/MakoApp
```

Appears as a cube icon in the menu bar with container list, start/stop controls, and Docker info.

### Rosetta (x86_64 containers)

To run `linux/amd64` images on Apple Silicon, enable Rosetta:

```bash
mako config set vm.rosetta true
mako stop && mako start
```

Requires Rosetta 2 on the host (`softwareupdate --install-rosetta`).

### Shell Completions

```bash
# Generate and install zsh completions
./target/release/mako completions zsh > ~/.zfunc/_mako
```

## Project Structure

```
crates/
  cli/        mako CLI (start, stop, status, setup, info, config, completions)
  daemon/     makod host daemon (VM management, socket proxy, port forwarding,
              DNS proxy, HTTP proxy, memory monitor)
  agent/      mako-agent (runs inside VM, relays Docker API over vsock)
  common/     shared config, types, protocol definitions
swift-ffi/    thin Swift bridge for Apple Virtualization.framework
vm-image/     Linux kernel, rootfs, and initramfs build scripts
gui/MakoApp/  macOS menu bar application (SwiftUI)
tests/        integration tests (CLI smoke tests, VM save/restore)
```

## Features

- **Docker**: full Docker CLI and Compose support via socket forwarding
- **Port forwarding**: container `-p` ports accessible on `localhost` (event-driven, near-instant)
- **File sharing**: VirtioFS mounts (configurable, defaults to home directory)
- **Kubernetes**: built-in K3s via `mako kubernetes enable`
- **DNS**: resolve container names from macOS (`<name>.mako.local`)
- **VPN passthrough**: DNS proxy + HTTP CONNECT proxy for corporate network access
- **Rosetta** (opt-in): x86_64 container support on Apple Silicon
- **Suspend/resume**: fast VM startup via state save/restore
- **GUI**: native macOS menu bar app with container management
- **CLI**: `mako ps`, `mako images`, `mako logs`, `mako exec`, `mako run`
- **Launch at login**: launchd integration
- **Shell completions**: zsh, bash, fish

## Performance

Mako is optimized for speed:
- **Async I/O**: all proxy paths (socket, port forward, HTTP) use tokio async I/O
- **TCP_NODELAY**: disabled Nagle's algorithm on all forwarding connections
- **Event-driven ports**: Docker events API triggers instant port forward setup
- **DNS caching**: 30s TTL cache for VM-facing DNS proxy
- **Disk caching**: write-back caching and noatime mount for lower I/O latency
- **256KB buffers**: large buffers for high-throughput data transfer
- **32 vsock workers**: high concurrency for Docker API relay

## License

Apache-2.0
