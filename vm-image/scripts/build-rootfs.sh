#!/usr/bin/env bash
#
# Builds a minimal Linux root filesystem for the Mako VM.
# The rootfs includes: init system, dockerd, containerd, runc, mako-agent.
#
# Prerequisites: Docker (for building the rootfs in a container)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUTPUT_DIR="${SCRIPT_DIR}/../output"
ROOTFS_SIZE="2G"
ROOTFS_IMG="${OUTPUT_DIR}/rootfs.img"

DOCKER_VERSION="27.4.1"
CONTAINERD_VERSION="2.0.1"
RUNC_VERSION="1.2.4"

mkdir -p "${OUTPUT_DIR}"

echo "==> Building Mako rootfs"
echo "    Docker: ${DOCKER_VERSION}"
echo "    containerd: ${CONTAINERD_VERSION}"
echo "    runc: ${RUNC_VERSION}"

# Create a sparse disk image
echo "==> Creating sparse disk image (${ROOTFS_SIZE})"
dd if=/dev/zero of="${ROOTFS_IMG}" bs=1 count=0 seek="${ROOTFS_SIZE}" 2>/dev/null
mkfs.ext4 -F -q "${ROOTFS_IMG}"

echo "==> Populating rootfs"
# TODO: Mount the image and install:
# 1. Alpine Linux minimal base (via alpine-minirootfs tarball)
# 2. dockerd from Moby project static binaries
# 3. containerd static binary
# 4. runc static binary
# 5. BuildKit binary
# 6. mako-agent binary (cross-compiled for Linux x86_64/aarch64)
# 7. Init scripts to start mako-agent and dockerd on boot
# 8. Rosetta binfmt_misc registration (for Apple Silicon)

echo "==> rootfs build complete: ${ROOTFS_IMG}"
echo ""
echo "NOTE: This script is a placeholder. The actual rootfs build will"
echo "use a container-based approach to set up the Alpine base system"
echo "and install all required binaries."
