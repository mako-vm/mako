MAKO_DIR := $(HOME)/.mako
OUTPUT_DIR := $(CURDIR)/output
IMAGE_NAME := mako-vm-builder

.PHONY: all build install clean

all: build install

build: $(OUTPUT_DIR)/vmlinuz $(OUTPUT_DIR)/rootfs.img

$(OUTPUT_DIR)/vmlinuz $(OUTPUT_DIR)/rootfs.img $(OUTPUT_DIR)/initramfs.cpio.gz: Dockerfile rootfs-overlay/sbin/mako-init rootfs-overlay/etc/inittab
	@echo "==> Building VM image using Docker..."
	@echo "    This builds a minimal Alpine Linux rootfs with dockerd."
	docker build --platform linux/$(shell uname -m | sed 's/arm64/aarch64/') -t $(IMAGE_NAME) .
	@mkdir -p $(OUTPUT_DIR)
	@echo "==> Extracting build artifacts..."
	docker create --name mako-vm-extract $(IMAGE_NAME) 2>/dev/null || \
		(docker rm -f mako-vm-extract && docker create --name mako-vm-extract $(IMAGE_NAME))
	docker cp mako-vm-extract:/output/vmlinuz $(OUTPUT_DIR)/vmlinuz
	docker cp mako-vm-extract:/output/rootfs.img $(OUTPUT_DIR)/rootfs.img
	docker cp mako-vm-extract:/output/initramfs.cpio.gz $(OUTPUT_DIR)/initramfs.cpio.gz
	docker rm -f mako-vm-extract
	@echo "==> Build complete:"
	@ls -lh $(OUTPUT_DIR)/

install: $(OUTPUT_DIR)/vmlinuz $(OUTPUT_DIR)/rootfs.img
	@echo "==> Installing VM image to $(MAKO_DIR)..."
	@mkdir -p $(MAKO_DIR)
	cp $(OUTPUT_DIR)/vmlinuz $(MAKO_DIR)/vmlinux
	cp $(OUTPUT_DIR)/rootfs.img $(MAKO_DIR)/rootfs.img
	cp $(OUTPUT_DIR)/initramfs.cpio.gz $(MAKO_DIR)/initramfs.cpio.gz 2>/dev/null || true
	@echo "==> Installed. Kernel and rootfs are at $(MAKO_DIR)/"
	@ls -lh $(MAKO_DIR)/

clean:
	rm -rf $(OUTPUT_DIR)
	docker rmi -f $(IMAGE_NAME) 2>/dev/null || true
	docker rm -f mako-vm-extract 2>/dev/null || true
