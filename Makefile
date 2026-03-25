.PHONY: all build agent gui codesign install clean test fmt check setup start stop

INSTALL_DIR ?= /usr/local/bin
AGENT_TARGET := aarch64-unknown-linux-musl
ENTITLEMENTS := crates/daemon/entitlements.plist

all: build agent codesign

# ── Host binaries (mako CLI + makod daemon) ──────────────────────────
build:
	cargo build --release

# ── In-VM agent (cross-compiled for Linux) ───────────────────────────
agent:
	cargo build --release --target $(AGENT_TARGET) -p mako-agent

# ── macOS menu bar GUI ───────────────────────────────────────────────
gui:
	cd gui/MakoApp && swift build -c release

# ── Codesign makod (required for Virtualization.framework) ───────────
codesign:
	codesign --entitlements $(ENTITLEMENTS) --force -s - target/release/makod

# ── Install to system path ───────────────────────────────────────────
install: all
	sudo install -m 755 target/release/mako $(INSTALL_DIR)/mako
	sudo install -m 755 target/release/makod $(INSTALL_DIR)/makod
	sudo codesign --entitlements $(ENTITLEMENTS) --force -s - $(INSTALL_DIR)/makod 2>/dev/null || true
	@echo ""
	@echo "Installed mako and makod to $(INSTALL_DIR)"

# ── Build VM image (downloads Alpine, Docker, kernel → ~/.mako/) ─────
setup: all
	./target/release/mako setup

# ── Start / Stop ─────────────────────────────────────────────────────
start:
	./target/release/mako start

stop:
	./target/release/mako stop

# ── Quality ──────────────────────────────────────────────────────────
test:
	cargo test --workspace

fmt:
	cargo fmt --all

check:
	cargo check --workspace
	cargo clippy --workspace -- -D warnings
	cargo fmt --all -- --check
	cargo test --workspace
	cargo check --target $(AGENT_TARGET) -p mako-agent

# ── Clean ────────────────────────────────────────────────────────────
clean:
	cargo clean
	rm -rf gui/MakoApp/.build
