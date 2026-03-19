# Mako

A fast, lightweight, open-source Docker Desktop alternative for macOS.

Built on Apple's Virtualization.framework with stock dockerd running inside a minimal Linux VM. Designed to match OrbStack's performance: 2-second startup, 0.1% idle CPU, dynamic memory, and near-native file sharing.

## Architecture

```
┌──────────────────────────────────────────┐
│ macOS Host                               │
│                                          │
│    mako CLI ──┐    ┌── GUI (menu bar)    │
│              ▼    ▼                      │
│         ┌───────────────┐                │
│         │    makod      │                │
│         │  socket proxy │                │
│         │  VM manager   │                │
│         └──────┬────────┘                │
│                │ vsock                   │
│         ┌──────▼────────┐                │
│         │  Linux VM     │                │
│         │  mako-agent   │                │
│         │  dockerd      │                │
│         │  containerd   │                │
│         │  runc         │                │
│         └───────────────┘                │
└──────────────────────────────────────────┘
```

## How It Works

Mako runs a lightweight Linux VM using Apple's Virtualization.framework.
Inside the VM, stock dockerd (from the Moby project) handles all container
operations. The Docker socket is forwarded from the VM to macOS over vsock,
so the standard `docker` CLI and Docker Compose work out of the box.

**What Mako builds from scratch** (the performance-critical parts):
- VM lifecycle management via Virtualization.framework
- VirtioFS file sharing with macOS
- Docker socket forwarding over vsock
- Networking (NAT, port forwarding, DNS)
- Dynamic memory ballooning
- Rosetta 2 integration for x86 containers on Apple Silicon
- Native macOS menu bar GUI

**What Mako bundles** (proven, open-source components):
- dockerd / Moby (Docker engine)
- containerd (container runtime)
- runc (OCI runtime)
- BuildKit (image builder)
- K3s (lightweight Kubernetes, optional)

## Requirements

- macOS 13 (Ventura) or later
- Apple Silicon or Intel Mac
- Rust 1.75+
- Xcode Command Line Tools (for Swift FFI bridge)

## Quick Start

```bash
# Build
cargo build --release

# Start the VM
mako start

# Use Docker as usual
docker run hello-world
docker compose up

# Stop
mako stop
```

## Project Structure

```
crates/
  cli/        mako CLI binary
  daemon/     makod host daemon (VM management, socket proxy)
  agent/      mako-agent (runs inside VM, manages dockerd)
  common/     shared types and vsock protocol definitions
swift-ffi/    thin Swift bridge for Apple Virtualization.framework
vm-image/     Linux kernel config and rootfs build scripts
gui/          macOS menu bar application (Swift/AppKit)
```

## License

Apache-2.0
