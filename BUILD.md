# SkyOS Build System

## Quick Start

```bash
# Full build (userspace → kernel → UEFI image)
python build_disk.py

# Kernel + bootimage only (faster for kernel dev)
python build_disk.py --kernel-only

# Userspace + initrd only
python build_disk.py --userspace-only

# Full build with ISO output
python build_disk.py --iso --version 0.6.0

# Release build (optimized)
python build_disk.py --release
```

## Options

| Option | Description |
|--------|-------------|
| `--kernel-only` | Build kernel + UEFI bootimage only (skips userspace) |
| `--userspace-only` | Build userspace + initrd only (skips kernel) |
| `--no-vdi` | Skip VirtualBox VDI conversion |
| `--iso` | Create bootable ISO image |
| `--version VERSION` | Version for ISO output (default: 0.6.0) |
| `--release` | Optimized release build |

## Pipeline

| Step | Component | Output |
|------|-----------|--------|
| 1 | Userspace (cargo build) | `target/x86_64-sarga/{debug,release}/` |
| 2 | Initrd (build_initrd.py) | `initrd.tar` |
| 3 | Kernel (cargo +nightly build) | Kernel ELF |
| 4 | Bootimage (builder) | `skyos_uefi.img` |
| 5 | VDI (VBoxManage, optional) | `skyos.vdi` |
| 6 | ISO (make_iso.py, optional) | `release/skyos-{version}.iso` |

## Outputs

- `skyos_uefi.img` — UEFI bootable disk image (primary)
- `skyos.vdi` — VirtualBox disk image
- `release/skyos-{version}.iso` — Bootable ISO

## Running

### QEMU

```bash
qemu-system-x86_64 -bios OVMF.fd -drive format=raw,file=skyos_uefi.img -m 512M -smp 2
qemu-system-x86_64 -bios OVMF.fd -cdrom release/skyos-0.6.0.iso -m 512M -smp 2
```

### Makefile (WSL/Linux)

```bash
make run        # build + QEMU with display
make test       # build + QEMU nographic + check login
```

## Prerequisites

- **Rust nightly** with `rust-src` + `llvm-tools-preview` components
- **Python 3** for build scripts
- **QEMU** for testing (recommended)
- **xorriso** for ISO creation (or `pip install pycdlib` as fallback)
- **VirtualBox** with VBoxManage for VDI (optional)

## Development

```bash
# Fast kernel iteration (skips userspace)
python build_disk.py --kernel-only

# Fast userspace iteration (skips kernel)
python build_disk.py --userspace-only

# Specific crate (the sarga target has no precompiled sysroot, so core/alloc
# are built from source; CI and build scripts pass -Zbuild-std explicitly)
cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json -p sash
```