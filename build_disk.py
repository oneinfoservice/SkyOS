"""SkyOS Unified Build System.

Single entry point for building the entire SkyOS stack:
userspace -> initrd -> kernel -> UEFI bootimage -> VDI/ISO.

Usage:
    python build_disk.py                        # full build
    python build_disk.py --kernel-only          # kernel + bootimage only
    python build_disk.py --userspace-only       # userspace + initrd only
    python build_disk.py --release --iso        # release build + ISO
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def log(step, msg):
    print(f"  [{step}] {msg}")


def run(cmd, cwd=None, desc=None, check=True, env=None):
    if desc:
        print(f"\n--- {desc} ---")
    full_cmd = " ".join(cmd) if isinstance(cmd, list) else cmd
    if isinstance(cmd, list):
        # Quote args containing spaces so paths like "C:\Program Files\..." survive shell=True
        full_cmd = " ".join(f'"{a}"' if " " in a else a for a in cmd)
    else:
        full_cmd = cmd
    result = subprocess.run(full_cmd, cwd=cwd, shell=True, env=env)
    if check and result.returncode != 0:
        print(f"ERROR: '{full_cmd}' failed with code {result.returncode}")
        sys.exit(result.returncode)
    return result.returncode == 0


def find_nightly_cargo():
    try:
        r = subprocess.run(
            ["rustup", "which", "cargo", "--toolchain", "nightly"],
            capture_output=True, text=True, check=True
        )
        return r.stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        nightly_bin = Path.home() / ".rustup" / "toolchains" / "nightly-x86_64-pc-windows-msvc" / "bin"
        return str(nightly_bin / "cargo.exe")


def ensure_nightly_wrapper(nightly_cargo):
    nightly_rustc = nightly_cargo.replace("cargo.exe", "rustc.exe")
    wrapper = Path(tempfile.gettempdir()) / "opencode" / "cargo-nightly-wrapper.cmd"
    wrapper.parent.mkdir(parents=True, exist_ok=True)
    wrapper.write_text(
        f"@echo off\n"
        f"set RUSTC={nightly_rustc}\n"
        f"set RUSTC_WORKSPACE_WRAPPER=\n"
        f"{nightly_cargo} %*\n"
    )
    return str(wrapper)


def build_userspace(root_dir, release=False):
    log("1/4", "Building userspace")
    target = "x86_64-sarga.json"
    # The sarga target has no precompiled sysroot libs, so core/alloc are
    # built from source; -Zbuild-std is passed explicitly (the workspace
    # .cargo/config.toml must not set a global build-std, which would break
    # host builds like `cargo test -p libsarga`).
    cmd = ["cargo", "build", "-Zbuild-std=core,alloc", "--target", target]
    if release:
        cmd.append("--release")
    run(cmd, cwd=root_dir,
        desc=f"Userspace ({'release' if release else 'debug'})")


def build_initrd(root_dir):
    log("2/4", "Building initrd")
    script = Path(root_dir) / "build_initrd.py"
    if not script.exists():
        log("WARN", "build_initrd.py not found, skipping")
        return None
    run([sys.executable, str(script), str(root_dir)], cwd=root_dir)
    initrd_path = Path(root_dir) / "initrd.tar"
    if initrd_path.exists():
        log("OK", f"initrd.tar ({initrd_path.stat().st_size / 1024:.0f} KB)")
        return str(initrd_path)
    return None


def build_kernel(kernel_dir):
    log("3/4", "Building kernel")
    krate = Path(os.path.realpath(kernel_dir)) / "kernel"
    if not krate.is_dir():
        print("ERROR: kernel/ directory not found at", kernel_dir)
        sys.exit(1)
    run(["cargo", "+nightly", "build"], cwd=str(krate),
        desc="Kernel (nightly, debug)")


def build_bootimage(root_dir, kernel_dir):
    log("4/4", "Creating UEFI bootimage")
    kernel_dir = os.path.realpath(kernel_dir)
    nightly = find_nightly_cargo()
    wrapper = ensure_nightly_wrapper(nightly)
    builder_dir = Path(kernel_dir) / "builder"
    env = {**os.environ, "RUST_BACKTRACE": "1", "CARGO_NIGHTLY": wrapper}

    run(["cargo", "+stable", "build"], cwd=str(builder_dir), env=env,
        desc="Builder (stable)")

    builder_bin = builder_dir / "target" / "debug" / "builder.exe"
    if not builder_bin.exists():
        builder_bin = builder_dir / "target" / "debug" / "builder"
    if not builder_bin.exists():
        print(f"ERROR: builder binary not found at {builder_bin}")
        sys.exit(1)
    run([str(builder_bin)], cwd=str(builder_dir), env=env)

    triple = os.environ.get("VAHI_TARGET_TRIPLE", "x86_64-vahi")
    uefi = Path(kernel_dir) / "target" / triple / "debug" / "bootimage-vahi_kernel.bin"
    if not uefi.exists():
        print(f"ERROR: UEFI image not found at {uefi}")
        sys.exit(1)
    output = Path(root_dir) / "skyos_uefi.img"
    shutil.copy2(str(uefi), str(output))
    log("OK", f"skyos_uefi.img ({output.stat().st_size / 1024:.0f} KB)")
    return str(output)


def build_vdi(uefi_path, root_dir):
    log("OPT", "Creating VDI")
    output = Path(root_dir) / "skyos.vdi"
    output.unlink(missing_ok=True)

    vbox = shutil.which("VBoxManage") or r"C:\Program Files\Oracle\VirtualBox\VBoxManage"
    if not shutil.which(vbox) and not Path(vbox).exists():
        log("SKIP", "VBoxManage not found")
        return None

    try:
        run([vbox, "convertfromraw", uefi_path, str(output), "--format", "VDI"])
        run([vbox, "modifymedium", "disk", str(output), "--resize", "64"])
        log("OK", f"skyos.vdi ({output.stat().st_size / 1024 / 1024:.0f} MB)")
        return str(output)
    except Exception as e:
        log("WARN", f"VDI failed: {e}")
        return None


def build_iso(uefi_path, root_dir, version="0.6.0"):
    log("OPT", "Creating ISO")
    script = Path(root_dir) / "scripts" / "make_iso.py"
    if not script.exists():
        log("SKIP", "scripts/make_iso.py not found")
        return None
    run([sys.executable, str(script), version], cwd=root_dir)
    iso_path = Path(root_dir) / "release" / f"skyos-{version}.iso"
    if iso_path.exists():
        log("OK", f"ISO ({iso_path.stat().st_size / 1024 / 1024:.0f} MB)")
        return str(iso_path)
    return None


def main():
    parser = argparse.ArgumentParser(description="SkyOS Build System")
    parser.add_argument("--kernel-only", action="store_true",
                        help="Build kernel + bootimage only")
    parser.add_argument("--userspace-only", action="store_true",
                        help="Build userspace + initrd only")
    parser.add_argument("--no-vdi", action="store_true",
                        help="Skip VDI conversion")
    parser.add_argument("--iso", action="store_true",
                        help="Create bootable ISO")
    parser.add_argument("--version", default="0.6.0",
                        help="Version string for ISO")
    parser.add_argument("--release", action="store_true",
                        help="Release mode (optimized)")
    args = parser.parse_args()

    root = Path(__file__).parent.resolve()
    kernel = root / "kernel"

    print("=== SkyOS Build System ===")
    print(f"  Root:   {root}")
    print(f"  Kernel: {kernel}")
    print(f"  Mode:   {'release' if args.release else 'debug'}")
    print()

    if not args.kernel_only:
        build_userspace(root, release=args.release)
        initrd = build_initrd(root)
        if initrd:
            dst = kernel / "kernel" / "initrd.tar"
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(initrd, str(dst))

    if not args.userspace_only:
        build_kernel(kernel)
        uefi = build_bootimage(root, kernel)

        if not args.no_vdi:
            build_vdi(uefi, root)

        if args.iso:
            build_iso(uefi, root, args.version)

    print("\n=== Build Complete ===")
    print("  qemu-system-x86_64 -bios OVMF.fd -drive format=raw,file=skyos_uefi.img -m 512M -smp 2")


if __name__ == "__main__":
    main()