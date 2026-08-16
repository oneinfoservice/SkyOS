# SkyOS Build System
# ==================
# Targets are designed for WSL/Linux with QEMU and xorriso.

KERNEL_DIR ?= kernel
SHELL := /bin/bash
PYTHON ?= python3

.PHONY: all clean fmt clippy build build-release \
        kernel userspace initrd bootimage iso \
        run test qemu-test run-nographic

# --- Aggregate targets ---

all: fmt clippy build

fmt:
	cargo fmt --check

# -Zbuild-std is explicit here (not in .cargo/config.toml): the sarga
# target has no precompiled sysroot, and a global build-std breaks host
# builds such as `cargo test -p libsarga`.
clippy:
	cargo clippy -Zbuild-std=core,alloc --target x86_64-sarga.json -- -D warnings

build:
	cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json

build-release:
	cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json --release

# NOTE: Make runs each dep regardless of prior failure. Use && if you need isolation.
test: fmt clippy build-release qemu-test

# --- Full ISO build ---

kernel:
	cd "$(KERNEL_DIR)/kernel" && cargo build --release -Zbuild-std=core,alloc --features net,smp,ai_rule,ext4

userspace: build-release

initrd: userspace
	$(PYTHON) build_initrd.py
	mkdir -p "$(KERNEL_DIR)/SkyOS"
	cp initrd.tar "$(KERNEL_DIR)/SkyOS/"

bootimage: kernel initrd
	cd "$(KERNEL_DIR)" && cargo run --release --manifest-path builder/Cargo.toml

iso: bootimage
	$(PYTHON) scripts/make_iso.py "release.$(shell date +%Y%m%d)"

# --- QEMU targets ---

qemu-test: iso
	ISO=$$(ls release/skyos-*.iso 2>/dev/null | head -1); \
	if [ -z "$$ISO" ]; then echo "No ISO found"; exit 1; fi; \
	qemu-system-x86_64 \
		-bios OVMF.fd \
		-cdrom "$$ISO" \
		-m 512M -smp 2 \
		-nographic -no-reboot \
		-serial mon:stdio \
		-device e1000,netdev=net0 -netdev user,id=net0 \
		2>&1 | tee /tmp/skyos-boot.log; \
	if grep -q "login:" /tmp/skyos-boot.log; then \
		echo "PASS: Boot OK"; \
		if grep -q "PASS" /tmp/skyos-boot.log; then \
			echo "PASS: Integration tests passed"; \
		fi \
	else \
		echo "FAIL: No login prompt"; exit 1; \
	fi

run: iso
	ISO=$$(ls release/skyos-*.iso 2>/dev/null | head -1); \
	qemu-system-x86_64 \
		-bios OVMF.fd \
		-cdrom "$$ISO" \
		-m 512M -smp 2 \
		-serial stdio \
		-device e1000,netdev=net0 -netdev user,id=net0 \
		-vga std -no-reboot

run-nographic: iso
	ISO=$$(ls release/skyos-*.iso 2>/dev/null | head -1); \
	qemu-system-x86_64 \
		-bios OVMF.fd \
		-cdrom "$$ISO" \
		-m 512M -smp 2 \
		-nographic -no-reboot \
		-serial mon:stdio \
		-device e1000,netdev=net0 -netdev user,id=net0

# --- Cleanup ---

clean:
	cargo clean
	cd "$(KERNEL_DIR)" && cargo clean
	rm -rf release/ initrd.tar build/
