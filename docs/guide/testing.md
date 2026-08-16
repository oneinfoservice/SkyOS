# How to Write and Run Tests

The bare-metal targets have no `#[test]`-based harness, but **`libsarga`'s pure-logic `#[cfg(test)]` modules run on the host**: `cargo test -p libsarga` compiles the crate under `cfg(test)` with the std test harness (23 errno/net/semver tests). Testing is otherwise split across host-side suites, on-OS integration binaries, and QEMU boot tests. See `docs/testing/integration.md` for the full picture.

## Host-Side Suites (`tests/skyos-test`)

Algorithms are validated host-side against mocks (e.g. the buddy allocator). Run with:

```bash
cargo run --manifest-path tests/skyos-test/Cargo.toml -- run
cargo run --manifest-path tests/skyos-test/Cargo.toml -- run --category kernel::alloc
```

Suites are Rust `Test { name, category, run: Fn() -> Result<(), String> }` entries in `tests/skyos-test-core/src/suites/`.

## On-OS Integration Binaries (`tests/thread_test`)

`tests/thread_test` is a `#![no_std]`/`#![no_main]` userspace crate that runs real syscalls at the login prompt (futex, DAC/perms, signals, pipe+signal interplay):

```rust
use libsarga::{println, sarga_main};

fn user_main() -> i32 {
    // ... syscall scenarios ...
    0
}
sarga_main!(user_main);
```

## QEMU Boot/Integration Tests

```bash
./tests/qemu_boot.sh              # build everything, boot, assert login prompt
./tests/qemu_integration_test.sh  # + expect-driven shell interaction
```

These build the kernel and userspace, assemble the initrd/bootimage/ISO, and boot in QEMU (OVMF, 512M, 2 cpus, e1000). PASS = `login:` prompt in the log; FAIL = panic or timeout.

## Kernel Self-Test Feature

The kernel `self_test` feature emits TAP output (`ok`/`not ok`) to serial during boot. The CI `integration-qemu` job scans for `not ok` and fails on any. There is no `kernel_test!` macro.

## Running in CI

The GitHub Actions workflow (`.github/workflows/ci.yml`) runs `fmt`, `clippy`, `check-all-targets` (debug+release), and `integration-qemu`. The `host-tests` job also runs `cargo test -p libsarga` for libsarga's host unit tests.
