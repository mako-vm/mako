#!/usr/bin/env bash
#
# Downloads and builds a minimal Linux kernel for the Mako VM.
# Configured for Virtualization.framework with virtio drivers,
# VirtioFS, vsock, and minimal footprint.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUTPUT_DIR="${SCRIPT_DIR}/../output"
KERNEL_VERSION="6.12.8"

mkdir -p "${OUTPUT_DIR}"

echo "==> Building Linux kernel ${KERNEL_VERSION} for Mako VM"

# TODO: Implement the actual kernel build:
# 1. Download linux-${KERNEL_VERSION}.tar.xz from kernel.org
# 2. Apply the Mako kernel config (vm-image/kernel/mako_defconfig)
# 3. Build with: make -j$(nproc) ARCH=arm64 Image  (or x86_64)
# 4. Copy arch/arm64/boot/Image to ${OUTPUT_DIR}/vmlinux

echo ""
echo "NOTE: This script is a placeholder."
echo "The kernel config should enable:"
echo "  - CONFIG_VIRTIO_BLK, CONFIG_VIRTIO_NET, CONFIG_VIRTIO_CONSOLE"
echo "  - CONFIG_VIRTIO_FS (VirtioFS for file sharing)"
echo "  - CONFIG_VSOCKETS, CONFIG_VIRTIO_VSOCKETS (host-guest communication)"
echo "  - CONFIG_OVERLAY_FS (for Docker storage driver)"
echo "  - CONFIG_BINFMT_MISC (for Rosetta)"
echo "  - CONFIG_CGROUPS, CONFIG_NAMESPACES (for containers)"
echo "  - Disable unnecessary drivers (USB, GPU, sound, etc.)"
