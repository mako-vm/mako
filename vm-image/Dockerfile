FROM alpine:3.21

ARG DOCKER_VERSION=27.5.1

# Install build tools and kernel
RUN apk add --no-cache \
    linux-virt \
    e2fsprogs \
    mkinitfs \
    curl \
    tar \
    coreutils

# Download Docker static binaries
RUN ARCH=$(uname -m) && \
    curl -fsSL "https://download.docker.com/linux/static/stable/${ARCH}/docker-${DOCKER_VERSION}.tgz" \
    | tar xz -C /tmp && \
    mkdir -p /docker-bin && \
    cp /tmp/docker/* /docker-bin/

# Download Alpine minirootfs
RUN ARCH=$(uname -m) && \
    mkdir -p /rootfs && \
    curl -fsSL "https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/${ARCH}/alpine-minirootfs-3.21.3-${ARCH}.tar.gz" \
    | tar xz -C /rootfs

# Copy Docker binaries into rootfs
RUN mkdir -p /rootfs/usr/bin && \
    cp /docker-bin/* /rootfs/usr/bin/

# Create required directories
RUN mkdir -p \
    /rootfs/dev \
    /rootfs/proc \
    /rootfs/sys \
    /rootfs/tmp \
    /rootfs/run \
    /rootfs/var/log \
    /rootfs/var/run \
    /rootfs/mnt/host \
    /rootfs/sys/fs/cgroup \
    /rootfs/etc/init.d \
    /rootfs/root

# Copy overlay files (init scripts, configs)
COPY rootfs-overlay/ /rootfs/

# Make init scripts executable
RUN chmod +x /rootfs/etc/init.d/* /rootfs/sbin/mako-init 2>/dev/null || true

# Prepare output directory
RUN mkdir -p /output

# Copy kernel
RUN cp /boot/vmlinuz-virt /output/vmlinuz

# Generate initramfs for the virt kernel
RUN KVER=$(ls /lib/modules/ | head -1) && \
    mkinitfs -b / -k -o /output/initramfs.cpio.gz "$KVER"

# Create ext4 rootfs image (2GB sparse)
RUN mke2fs -t ext4 -d /rootfs -L mako-rootfs /output/rootfs.img 2G

CMD ["echo", "Build complete. Copy artifacts from /output/"]
