# Building and Running Userspace Programs

SkyOS userspace is written in Rust. Every program links against `libsarga` (the userspace runtime
in this workspace) and is compiled for the custom `x86_64-sarga.json` target.

## The Runtime Library

`libsarga` provides everything a program needs on SkyOS:

- POSIX syscall wrappers (`read`, `write`, `open`, `mmap`, `close`, …) in `libsarga/src/posix.rs`
- Memory management (`mmap`/`brk`-backed allocator, slab for small objects) in `libsarga/src/mem.rs`
- `net` module (sockets, socketpair IPC), `signal` module, `gui::Window` wrappers, `hash`
  (PBKDF2), and more
- The `_start` entry point, ELF PIE loader, and syscall trampolines

## Writing a Userspace Program

```rust
#![no_std]
#![no_main]
extern crate libsarga;

fn main() {
    libsarga::println!("Hello from SkyOS userspace!");
}
```

Programs are built with `cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json` from the
workspace root (the sarga target has no precompiled sysroot, so `core`/`alloc` are built from
source; `.cargo/config.toml` does not set a global `build-std` because that breaks host builds
such as `cargo test -p libsarga`). The build is orchestrated by `build_disk.py`
(`cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json --release`).

## Loading and Execution

The kernel's ELF loader maps segments into the process address space and jumps to the entry point.
The init process (`init/`) is loaded by the kernel at boot.

## Init System

`init` (a small Rust service supervisor) starts the essential userspace services and respawns them
if they crash:

1. `login-manager` (desktop/authentication entry)
2. ADE services (notifications, clipboard, session, power — see `docs/ade/`)
3. Other system daemons

Each service is defined with a respawn policy (capped at `MAX_RESPAWNS`).

## Environment Variables

The kernel passes a minimal environment to the init process including `PATH` and `HOME`.
