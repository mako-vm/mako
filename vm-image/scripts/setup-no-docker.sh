#!/usr/bin/env bash
#
# Build the Mako VM image WITHOUT requiring Docker.
# Downloads Alpine Linux, Docker static binaries, and kernel directly,
# then assembles an ext4 rootfs using mke2fs.
#
# Requirements: curl, tar, mke2fs (brew install e2fsprogs)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
VM_IMAGE_DIR="$(dirname "$SCRIPT_DIR")"
OUTPUT_DIR="${VM_IMAGE_DIR}/output"
STAGING_DIR="${VM_IMAGE_DIR}/staging"
MAKO_DIR="${HOME}/.mako"

ALPINE_VERSION="3.21"
ALPINE_MINOR="3.21.3"
DOCKER_VERSION="27.5.1"
ROOTFS_SIZE="2G"

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64)
        ALPINE_ARCH="aarch64"
        DOCKER_ARCH="aarch64"
        ;;
    x86_64)
        ALPINE_ARCH="x86_64"
        DOCKER_ARCH="x86_64"
        ;;
    *)
        echo "Error: Unsupported architecture: $ARCH"
        exit 1
        ;;
esac

# Check for mke2fs
MKE2FS=""
for path in /opt/homebrew/opt/e2fsprogs/sbin/mke2fs /usr/local/opt/e2fsprogs/sbin/mke2fs $(which mke2fs 2>/dev/null); do
    if [ -x "$path" 2>/dev/null ]; then
        MKE2FS="$path"
        break
    fi
done

if [ -z "$MKE2FS" ]; then
    echo "Error: mke2fs not found. Install it with:"
    echo "  brew install e2fsprogs"
    exit 1
fi

echo "==> Mako VM Image Builder (no Docker required)"
echo "    Architecture: ${ALPINE_ARCH}"
echo "    Alpine:       ${ALPINE_MINOR}"
echo "    Docker:       ${DOCKER_VERSION}"
echo ""

mkdir -p "$OUTPUT_DIR" "$STAGING_DIR"

# --- Download Alpine minirootfs ---
ALPINE_ROOTFS_URL="https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/releases/${ALPINE_ARCH}/alpine-minirootfs-${ALPINE_MINOR}-${ALPINE_ARCH}.tar.gz"
ALPINE_ROOTFS_TAR="${OUTPUT_DIR}/alpine-minirootfs.tar.gz"

if [ ! -f "$ALPINE_ROOTFS_TAR" ]; then
    echo "==> Downloading Alpine Linux minirootfs..."
    curl -fSL -o "$ALPINE_ROOTFS_TAR" "$ALPINE_ROOTFS_URL"
else
    echo "==> Alpine minirootfs already downloaded"
fi

# --- Download Docker static binaries ---
DOCKER_URL="https://download.docker.com/linux/static/stable/${DOCKER_ARCH}/docker-${DOCKER_VERSION}.tgz"
DOCKER_TAR="${OUTPUT_DIR}/docker-static.tgz"

if [ ! -f "$DOCKER_TAR" ]; then
    echo "==> Downloading Docker ${DOCKER_VERSION} static binaries..."
    curl -fSL -o "$DOCKER_TAR" "$DOCKER_URL"
else
    echo "==> Docker binaries already downloaded"
fi

# --- Download Alpine kernel + modules ---
# The APK contains boot/vmlinuz-virt (EFI stub, gzip-compressed) and
# lib/modules/<ver>/ with loadable modules.
# Apple's Virtualization.framework requires the raw uncompressed ARM64 Image.
KERNEL_FILE="${OUTPUT_DIR}/vmlinux"
INITRD_FILE="${OUTPUT_DIR}/initramfs.img"
MODULES_DIR="${OUTPUT_DIR}/modules"

if [ ! -f "$KERNEL_FILE" ]; then
    echo "==> Downloading Alpine Linux virt kernel..."
    KERNEL_INDEX_URL="https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/main/${ALPINE_ARCH}/"

    KERNEL_PKG=$(curl -fsSL "$KERNEL_INDEX_URL" | grep -o 'linux-virt-[0-9][^"]*\.apk' | head -1)

    if [ -z "$KERNEL_PKG" ]; then
        echo "Error: Could not find linux-virt package in Alpine repository"
        exit 1
    fi

    echo "    Package: ${KERNEL_PKG}"
    KERNEL_APK="${OUTPUT_DIR}/${KERNEL_PKG}"
    curl -fSL -o "$KERNEL_APK" "${KERNEL_INDEX_URL}${KERNEL_PKG}"

    KEXTRACT="${OUTPUT_DIR}/kernel-extract"
    mkdir -p "$KEXTRACT"
    tar xzf "$KERNEL_APK" -C "$KEXTRACT" 2>/dev/null || true

    VMLINUZ="${KEXTRACT}/boot/vmlinuz-virt"
    if [ ! -f "$VMLINUZ" ]; then
        echo "Error: vmlinuz-virt not found in kernel APK"
        ls -R "$KEXTRACT/"
        exit 1
    fi

    echo "    Decompressing kernel for Virtualization.framework..."
    python3 -c "
import gzip, struct
with open('$VMLINUZ', 'rb') as f:
    data = f.read()
magic = data[4:8]
if magic == b'zimg':
    off = struct.unpack_from('<I', data, 8)[0]
    sz = struct.unpack_from('<I', data, 12)[0]
    raw = gzip.decompress(data[off:off+sz])
else:
    raw = data
with open('$KERNEL_FILE', 'wb') as out:
    out.write(raw)
print(f'Kernel: {len(raw)} bytes ({len(raw)//1024//1024} MB)')
"
    echo "    Kernel ready: $(ls -lh "$KERNEL_FILE" | awk '{print $5}')"

    # Save kernel modules for initramfs and rootfs
    if [ -d "${KEXTRACT}/lib/modules" ]; then
        rm -rf "$MODULES_DIR"
        cp -R "${KEXTRACT}/lib/modules" "$MODULES_DIR"
        KVER=$(ls "$MODULES_DIR" | head -1)
        echo "    Kernel modules: ${KVER}"
    fi

    rm -rf "$KEXTRACT" "$KERNEL_APK"
else
    echo "==> Kernel already present"
    KVER=$(ls "$MODULES_DIR" 2>/dev/null | head -1)
fi

# --- Build initramfs ---
# Minimal initramfs to load virtio + ext4 modules then switch_root
if [ ! -f "$INITRD_FILE" ] || [ ! -f "$KERNEL_FILE" -a -d "$MODULES_DIR" ]; then
    echo "==> Building initramfs..."
    INITRD_ROOT="${OUTPUT_DIR}/initrd-root"
    rm -rf "$INITRD_ROOT"
    mkdir -p "$INITRD_ROOT"/{bin,sbin,dev,proc,sys,mnt/root,lib/modules}

    # Use busybox from the Alpine minirootfs
    ALPINE_ROOTFS_TAR="${OUTPUT_DIR}/alpine-minirootfs.tar.gz"
    if [ -f "$ALPINE_ROOTFS_TAR" ]; then
        tar xzf "$ALPINE_ROOTFS_TAR" -C "$INITRD_ROOT" bin/busybox lib/ usr/lib/ 2>/dev/null || true
    fi

    # Copy and decompress required kernel modules (Alpine ships .ko.gz)
    MODDIR="${INITRD_ROOT}/modules"
    mkdir -p "$MODDIR"
    if [ -n "$KVER" ] && [ -d "${MODULES_DIR}/${KVER}" ]; then
        KMOD_SRC="${MODULES_DIR}/${KVER}/kernel"
        for mod_gz in \
            "${KMOD_SRC}/drivers/virtio/virtio_mmio.ko"* \
            "${KMOD_SRC}/drivers/block/virtio_blk.ko"* \
            "${KMOD_SRC}/drivers/net/virtio_net.ko"* \
            "${KMOD_SRC}/fs/mbcache.ko"* \
            "${KMOD_SRC}/fs/jbd2/jbd2.ko"* \
            "${KMOD_SRC}/fs/ext4/ext4.ko"* \
            "${KMOD_SRC}/lib/crc16.ko"* \
            "${KMOD_SRC}/crypto/crc32c_generic.ko"*; do
            [ -f "$mod_gz" ] || continue
            base=$(basename "$mod_gz")
            if echo "$base" | grep -q '\.gz$'; then
                gunzip -c "$mod_gz" > "$MODDIR/${base%.gz}"
            else
                cp "$mod_gz" "$MODDIR/"
            fi
        done
        echo "    Initrd modules: $(ls "$MODDIR/" | tr '\n' ' ')"
    fi

    # Create init script -- uses insmod with full paths (no modprobe needed)
    cat > "${INITRD_ROOT}/init" << 'INIT_EOF'
#!/bin/busybox sh
/bin/busybox --install -s /bin
/bin/busybox --install -s /sbin

mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev

echo "mako-initrd: loading modules..."
for mod in virtio_mmio virtio_blk crc16 crc32c_generic mbcache jbd2 ext4; do
    if [ -f "/modules/${mod}.ko" ]; then
        insmod "/modules/${mod}.ko" 2>&1 || echo "  warning: ${mod} failed"
    fi
done

# Wait for /dev/vda
count=0
while [ ! -b /dev/vda ] && [ $count -lt 50 ]; do
    sleep 0.1
    count=$((count + 1))
done

if [ ! -b /dev/vda ]; then
    echo "FATAL: /dev/vda not found after 5s"
    ls /dev/vd* 2>&1 || echo "  no /dev/vd* devices"
    exec sh
fi

# Small settle time for block device
sleep 0.2

echo "mako-initrd: mounting rootfs on /dev/vda..."
mount -t ext4 -o rw /dev/vda /mnt/root
if [ $? -ne 0 ]; then
    echo "FATAL: Failed to mount /dev/vda"
    dmesg | tail -5 2>/dev/null
    exec sh
fi

echo "mako-initrd: switching to real root..."
umount /proc /sys /dev 2>/dev/null
exec switch_root /mnt/root /sbin/mako-init
INIT_EOF
    chmod +x "${INITRD_ROOT}/init"

    # Create initramfs cpio archive
    (cd "$INITRD_ROOT" && find . | cpio -o -H newc 2>/dev/null | gzip -9 > "$INITRD_FILE")
    echo "    Initramfs: $(ls -lh "$INITRD_FILE" | awk '{print $5}')"
    rm -rf "$INITRD_ROOT"
fi

# --- Build rootfs ---
echo "==> Building rootfs..."

# Clean staging directory
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"

# Extract Alpine minirootfs
echo "    Extracting Alpine base..."
tar xzf "$ALPINE_ROOTFS_TAR" -C "$STAGING_DIR"

# Create required directories
mkdir -p \
    "$STAGING_DIR/dev" \
    "$STAGING_DIR/proc" \
    "$STAGING_DIR/sys" \
    "$STAGING_DIR/tmp" \
    "$STAGING_DIR/run" \
    "$STAGING_DIR/var/log" \
    "$STAGING_DIR/var/run" \
    "$STAGING_DIR/mnt/host" \
    "$STAGING_DIR/sys/fs/cgroup" \
    "$STAGING_DIR/root"

# Extract Docker binaries
echo "    Installing Docker binaries..."
tar xzf "$DOCKER_TAR" -C "$STAGING_DIR/usr/bin/" --strip-components=1

# Install iptables (required by dockerd bridge networking)
echo "    Installing iptables from Alpine packages..."
IPTABLES_PKGS_DIR="${OUTPUT_DIR}/iptables-pkgs"
mkdir -p "$IPTABLES_PKGS_DIR"
ALPINE_PKG_BASE="https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/main/${ALPINE_ARCH}"
INDEX_HTML=$(curl -fsSL "$ALPINE_PKG_BASE/")
for pkg_pattern in 'iptables-1\.[0-9][^"]*\.apk' 'libxtables-1\.[0-9][^"]*\.apk' 'libnftnl-1\.[0-9][^"]*\.apk' 'libmnl-1\.[0-9][^"]*\.apk'; do
    pkg_file=$(echo "$INDEX_HTML" | grep -oE "$pkg_pattern" | head -1)
    if [ -n "$pkg_file" ] && [ ! -f "${IPTABLES_PKGS_DIR}/${pkg_file}" ]; then
        echo "      Downloading ${pkg_file}..."
        curl -fsSL -o "${IPTABLES_PKGS_DIR}/${pkg_file}" "${ALPINE_PKG_BASE}/${pkg_file}"
    fi
    if [ -n "$pkg_file" ] && [ -f "${IPTABLES_PKGS_DIR}/${pkg_file}" ]; then
        tar xzf "${IPTABLES_PKGS_DIR}/${pkg_file}" -C "$STAGING_DIR" 2>/dev/null || true
    fi
done
echo "    iptables installed: $(ls "$STAGING_DIR/usr/sbin/iptables" 2>/dev/null && echo 'yes' || echo 'MISSING')"

# Install kernel modules in rootfs (needed at runtime for networking, etc.)
if [ -n "$KVER" ] && [ -d "${MODULES_DIR}/${KVER}" ]; then
    echo "    Installing kernel modules (${KVER})..."
    mkdir -p "$STAGING_DIR/lib/modules"
    cp -R "${MODULES_DIR}/${KVER}" "$STAGING_DIR/lib/modules/"
fi

# Copy rootfs overlay files (init scripts, configs)
echo "    Installing Mako init scripts..."
OVERLAY_DIR="${VM_IMAGE_DIR}/rootfs-overlay"
if [ -d "$OVERLAY_DIR" ]; then
    cp -R "$OVERLAY_DIR"/* "$STAGING_DIR/"
fi

# Install mako-agent (vsock relay for Docker socket)
AGENT_BIN="$(dirname "$VM_IMAGE_DIR")/target/aarch64-unknown-linux-musl/release/mako-agent"
if [ -f "$AGENT_BIN" ]; then
    cp "$AGENT_BIN" "$STAGING_DIR/usr/bin/mako-agent"
    chmod +x "$STAGING_DIR/usr/bin/mako-agent"
    echo "    Installed mako-agent (vsock relay)"
else
    echo "    WARNING: mako-agent not found at $AGENT_BIN"
    echo "    Build it with: cargo build --release --target aarch64-unknown-linux-musl -p mako-agent"
fi

# Make init scripts executable
chmod +x "$STAGING_DIR/sbin/mako-init" 2>/dev/null || true
find "$STAGING_DIR/etc/init.d" -type f -exec chmod +x {} \; 2>/dev/null || true

# Configure Alpine package repositories (for future apk add inside VM)
mkdir -p "$STAGING_DIR/etc/apk"
cat > "$STAGING_DIR/etc/apk/repositories" << EOF
https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/main
https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/community
EOF

# Set root password to empty (for serial console login)
sed -i '' 's|^root:.*|root::0:0:root:/root:/bin/sh|' "$STAGING_DIR/etc/passwd" 2>/dev/null || \
sed -i 's|^root:.*|root::0:0:root:/root:/bin/sh|' "$STAGING_DIR/etc/passwd"

# Create ext4 image from staging directory
ROOTFS_IMG="${OUTPUT_DIR}/rootfs.img"
echo "    Creating ext4 filesystem (${ROOTFS_SIZE})..."
rm -f "$ROOTFS_IMG"
"$MKE2FS" -t ext4 -d "$STAGING_DIR" -L mako-rootfs "$ROOTFS_IMG" "$ROOTFS_SIZE" 2>&1 | tail -3

# Clean up staging
rm -rf "$STAGING_DIR"

echo ""
echo "==> Build complete!"
echo "    Kernel:   $(ls -lh "$KERNEL_FILE" | awk '{print $5}')"
echo "    Initrd:   $(ls -lh "$INITRD_FILE" | awk '{print $5}')"
echo "    Rootfs:   $(ls -lh "$ROOTFS_IMG" | awk '{print $5}')"

# --- Install to ~/.mako ---
echo ""
echo "==> Installing to ${MAKO_DIR}..."
mkdir -p "$MAKO_DIR"
cp "$KERNEL_FILE" "$MAKO_DIR/vmlinux"
cp "$INITRD_FILE" "$MAKO_DIR/initramfs.img"
cp "$ROOTFS_IMG" "$MAKO_DIR/rootfs.img"

echo ""
echo "==> Done! VM image installed to ${MAKO_DIR}/"
ls -lh "$MAKO_DIR/"
echo ""
echo "Start Mako with:  mako start -f"
