# Mako — Coding Agent Guide

> Open-source Docker Desktop replacement for macOS, built on Apple's Virtualization.framework.
> Stock `dockerd` (Moby) runs inside a minimal Alpine Linux VM; all Docker CLI/Compose commands work unchanged.

Repository: https://github.com/mako-vm/mako
Platform: macOS 13+ only (Apple Silicon and Intel)
Primary language: Rust, with a thin Swift FFI bridge for macOS framework calls
License: Apache-2.0

---

## Architecture

```
macOS Host
├── mako CLI (Rust, clap)            user-facing commands
├── makod daemon (Rust + Swift FFI)
│   ├── VM manager                   boots/stops Linux VM via Virtualization.framework
│   ├── Docker socket proxy          Unix socket <-> vsock relay
│   └── Port forwarder               exposes container -p ports on localhost
├── GUI (SwiftUI menu bar app)       container list, start/stop controls
│
│  ── vsock (CID 2, port 2375) ──
│
└── Linux VM (Alpine, ext4 rootfs)
    ├── mako-agent                   reverse vsock relay to /var/run/docker.sock
    ├── dockerd + containerd + runc
    └── mako-init                    PID 1 init script (shell)
```

The guest (`mako-agent`) initiates vsock connections to the host — the host never dials
into the guest. This "reverse vsock" pattern avoids CID discovery problems with Apple's
Virtualization.framework.

---

## Project Layout

```
Cargo.toml              workspace root (resolver v2)
crates/
  cli/                  mako CLI binary
    src/main.rs           clap parser, subcommands dispatch
    src/commands.rs       start, stop, status, setup, info, config implementations
  daemon/               makod host daemon
    src/main.rs           entry point — main thread runs CFRunLoopRun(), tokio on worker threads
    src/ffi.rs            extern "C" bindings to Swift MakoVMWrapper
    src/vm.rs             VmManager — VM lifecycle, serial parsing, IP discovery
    src/socket_proxy.rs   DockerSocketProxy — Unix socket <-> vsock relay
    src/port_forward.rs   PortForwarder — polls Docker API, creates TCP listeners
    build.rs              compiles Swift FFI with swiftc, links libmako_vz.a
    entitlements.plist    macOS entitlements for Virtualization.framework
  agent/                mako-agent (runs inside Linux VM)
    src/main.rs           reverse vsock relay, 8 worker threads, raw libc, no async
  common/               shared library
    src/config.rs         MakoConfig, mako_data_dir() -> ~/.mako/
    src/types.rs          VmState, VmInfo, SharedDirectory
    src/protocol.rs       HostMessage, AgentMessage, vsock port constants
    src/error.rs          MakoError enum
swift-ffi/              Swift bridge to Virtualization.framework
  Package.swift           MakoVirtualizationFFI static library
  include/mako_ffi.h      C header for Rust FFI
  Sources/VirtualizationFFI/
    MakoVM.swift           MakoVMWrapper — VZVirtualMachine config and lifecycle
vm-image/               VM image build system
  scripts/
    setup-no-docker.sh    builds Alpine rootfs, downloads Docker/kernel (no Docker needed)
  rootfs-overlay/
    sbin/mako-init        init script (mounts, cgroups, network, dockerd, mako-agent)
    etc/                  fstab, hostname, inittab, resolv.conf
  Makefile                alternative Docker-based build
  Dockerfile              Alpine-based image builder
gui/MakoApp/            macOS menu bar GUI
  Package.swift           Swift Package, macOS 13+
  Sources/MakoApp/
    MakoApp.swift          AppDelegate, NSStatusItem, NSPopover
    MakoViewModel.swift    Docker API polling, daemon detection, start/stop
    MenuBarView.swift      SwiftUI view — containers, status, controls
    LaunchdHelper.swift    launchd plist for auto-start at login
```

---

## Build Instructions

### Prerequisites

- macOS 13 (Ventura) or later
- Rust 1.75+ with `aarch64-unknown-linux-musl` target (`rustup target add aarch64-unknown-linux-musl`)
- Xcode Command Line Tools (for `swiftc`)
- e2fsprogs (`brew install e2fsprogs`) for VM image build

### Build Steps

```bash
# 1. Build all host Rust binaries
cargo build --release

# 2. Cross-compile the in-VM agent for Linux
cargo build --release --target aarch64-unknown-linux-musl -p mako-agent

# 3. Build VM image (Alpine rootfs + kernel + initramfs → ~/.mako/)
./target/release/mako setup

# 4. Codesign daemon (REQUIRED — Virtualization.framework needs entitlements)
codesign --entitlements crates/daemon/entitlements.plist --force -s - target/release/makod

# 5. Run
./target/release/mako start        # background mode
./target/release/mako start -f     # foreground mode

# 6. Use Docker
export DOCKER_HOST=unix://$HOME/.mako/docker.sock
docker ps
```

### Build GUI

```bash
cd gui/MakoApp && swift build -c release
.build/release/MakoApp
```

---

## Key Paths at Runtime

| Path | Purpose |
|------|---------|
| `~/.mako/` | Data directory |
| `~/.mako/config.json` | MakoConfig (CPUs, memory, disk, shares) |
| `~/.mako/docker.sock` | Docker API Unix socket (macOS side) |
| `~/.mako/makod.pid` | Daemon PID file for process management |
| `~/.mako/vmlinux` | Linux kernel binary |
| `~/.mako/rootfs.img` | VM root filesystem (ext4, 2GB) |
| `~/.mako/initramfs.img` | Initramfs with virtio modules |

---

## Coding Conventions

### Rust

- **Edition**: 2021
- **Dependency management**: all versions declared in workspace `[workspace.dependencies]`, crates use `{ workspace = true }`
- **Error handling**: `anyhow::Result` at binary boundaries, `thiserror` for library errors in `mako-common`
- **Logging**: `tracing` crate — use `info!`, `debug!`, `warn!`, `error!` (not `println!`)
- **Async runtime**: `tokio` with `rt-multi-thread` feature in daemon and CLI; mako-agent is fully synchronous (no tokio)
- **Formatting**: `cargo fmt` enforced; `cargo clippy -- -D warnings` must pass

### Swift

- **Minimum deployment**: macOS 13
- **FFI pattern**: Swift functions use `@_cdecl("function_name")` to export C-callable symbols; Rust consumes via `extern "C"` block in `ffi.rs`; C header in `swift-ffi/include/mako_ffi.h` defines the interface
- **GUI framework**: SwiftUI for views, AppKit (`NSStatusItem`, `NSPopover`) for menu bar integration

### VM / Guest

- **Init system**: custom `mako-init` shell script (not systemd/openrc)
- **Health checks**: PID-file based (`/var/run/docker.pid` + `kill -0`), not `pgrep`
- **Agent communication**: reverse vsock (guest-initiated), CID 2, port 2375
- **Half-close propagation**: always call `shutdown(fd, SHUT_WR)` when one side of a relay closes — failure to do this causes connection pool exhaustion with HTTP/1.1 keep-alive

### Pre-commit Hooks

Configured in `.pre-commit-config.yaml`:
- `trailing-whitespace`, `end-of-file-fixer`, `check-yaml`, `check-toml`
- `check-merge-conflict`, `detect-private-key`, `check-added-large-files` (5MB limit)
- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo check --workspace`

---

## Critical Implementation Details

### makod Main Thread Constraint

The `makod` daemon's main thread **must** run `CFRunLoopRun()` because Apple's
Virtualization.framework dispatches VM events on the main thread's run loop.
All async work (socket proxy, port forwarding) runs on tokio worker threads.
The pattern is:

```rust
// Main thread
std::thread::spawn(move || { /* tokio runtime here */ });
unsafe { CFRunLoopRun(); }
```

### Reverse Vsock Design

The host creates a vsock listener. The guest agent connects outward to
`CID 2` (host) on port `2375`. Each worker thread in the agent maintains
one persistent connection. When a Docker client connects to `~/.mako/docker.sock`,
the proxy accepts the next available vsock connection from the pool and relays
bidirectionally.

### Code Signing

After every rebuild of `makod`, it must be re-signed:

```bash
codesign --entitlements crates/daemon/entitlements.plist --force -s - target/release/makod
```

Without this, Virtualization.framework refuses to start (`VZErrorDomain code=1`).

### Cross-Compilation

`mako-agent` runs inside the Linux VM and must be compiled for the guest:

```bash
cargo build --release --target aarch64-unknown-linux-musl -p mako-agent
```

The resulting binary is copied into the rootfs during `mako setup`.

---

## Known Pitfalls

| Problem | Cause | Solution |
|---------|-------|----------|
| `VZErrorDomain code=1` on VM start | `makod` not codesigned | Run `codesign` with entitlements plist |
| `VZErrorDomain code=2` on VM start | Corrupted/locked `rootfs.img` | Kill lingering `com.apple.Virtualization.VirtualMachine.xpc`, rebuild image |
| Daemon dies immediately | stdout piped to `head` or similar | Never pipe `makod` output through tools that close stdin early (SIGPIPE) |
| `docker ps` hangs after several commands | vsock connections not recycled | Ensure `shutdown(fd, SHUT_WR)` is called in all relay threads |
| Compilation error on `svm_family` type | macOS uses `u8`, Linux uses `u16` | Cast with `as _` to let compiler infer the correct type |
| `pgrep -x` unreliable in VM | BusyBox pgrep has different semantics | Use PID file + `kill -0` for process checks |
| `iptables not found` in VM | Missing Alpine packages in rootfs | Ensure `iptables`, `libxtables`, `libnftnl`, `libmnl` are in rootfs build |

---

## Current Feature Status

**Fully working:**
- VM lifecycle (boot, stop, graceful shutdown with SIGTERM/SIGINT, PID file)
- Docker engine (dockerd 27.5.1, containerd, runc inside Alpine VM)
- Docker socket proxy (reverse vsock relay, half-close propagation)
- Port forwarding (container `-p` ports accessible on `localhost`)
- VirtioFS file sharing (configurable via `mako config set vm.share.<tag>=<path>`, defaults to home dir)
- Rosetta x86 emulation on Apple Silicon (VirtioFS share configured)
- CLI: start, stop, status, setup, info, config, completions, images, ps, run, logs, exec
- Kubernetes: `mako kubernetes enable/disable/status/kubeconfig` (runs K3s as a container)
- DNS forwarding: `*.mako.local` resolves container names from macOS (via `/etc/resolver/mako.local`)
- Memory monitoring: daemon tracks per-container memory usage (virtio-balloon device configured)
- macOS menu bar GUI (SwiftUI): daemon detection, container list, start/stop
- Launch at login (launchd plist helper)
- Shell completions (zsh, bash, fish)
- CI/CD: GitHub Actions (check, clippy, fmt, build, cross-compile agent, build GUI)
- Pre-commit hooks (fmt, clippy, check)
- Docker Compose: works via socket passthrough, integration test in `tests/`
- Distribution: install script (`install.sh`), Homebrew formula template (`Formula/mako.rb`)
