# SkyOS Testing Guide

## Overview

SkyOS uses a multi-tiered testing strategy due to its bare-metal target architecture. This document explains the current testing infrastructure and gaps.

## Test Infrastructure

### Kernel Self-Tests (Primary Path)

The kernel includes built-in self-tests that run at boot time. These are the primary verification mechanism for kernel functionality.

- **Location:** Kernel crate (separate repository)
- **Trigger:** Boot with appropriate flags or feature `self_test`
- **Output:** TAP (Test Anything Protocol) format via serial console
- **CI Integration:** `.github/workflows/ci.yml` checks for TAP output and `not ok` failures

### Host-Side Test Frameworks

Two test crates exist for host-side testing but are **excluded from the workspace**:

- **`tests/skyos-test-core`** - Test framework library (serde-based)
- **`tests/skyos-test`** - CLI tool for running tests and generating reports

These are designed for testing logic that can run on the host machine, not on the bare-metal target.

### CI Testing

Current CI (`.github/workflows/ci.yml`) includes:
- `fmt` - Code formatting check
- `clippy` - Lint checks
- `build` - Compilation verification
- `integration-qemu` - Boot test and kernel self-test verification

**Included:** `cargo test -p libsarga` + `cargo test -p ade --lib` (host unit tests for pure logic)

## Testing Gap: No CI-Wired Unit Tests

### Current State

- **`libsarga`'s `#[cfg(test)]` modules run on the host.** `cargo test -p libsarga` compiles the crate for the host with the std test harness and runs the pure-logic tests in `errno.rs, fs.rs, gui.rs, hash.rs, net.rs, png.rs, semver.rs, theme.rs, and toml.rs` (75 tests). No QEMU, no kernel, no syscalls. Its lang items (panic handler, global allocator, alloc error handler) are gated on the sarga targets (`os = "none"`), so the crate also builds as a host dependency — the prerequisite for `ade`'s host tests.
- **`libsarga`'s host-test coverage is gated per module.** `tests/test_libsarga_host_coverage.py` (same CI job) runs `cargo test -p libsarga --lib -- --list`, prints per-module counts, and fails if any pure-logic module (errno/fs/gui/hash/net/png/semver/theme/toml) has zero tests — so a `#[cfg(test)]` module that stops running, or a new pure-logic module added without tests, cannot silently shrink the suite.
- **The `skyos-test` host framework runs in CI** (buddy allocator, PS/2 mouse, VFS page cache, futex, page-table/COW, pipe/sleep wait-queue, and ext2/tarfs inode-table suites, 85 tests): `cargo run --manifest-path tests/skyos-test/Cargo.toml -- run`, wired into the `host-tests` job. Its runner exits non-zero on any FAIL or an empty run, so a broken suite cannot silently pass. The same CI step also pins each suite's count individually (a `category:count` loop over `kernel::alloc` … `kernel::fs`), so a suite can't silently lose or gain tests while the total stays put. Every test runs in its own subprocess (the runner re-execs the binary's hidden `exec` subcommand) under a per-test timeout (default 30 s, `--timeout-ms` to change, 0 to disable), so a hung test is KILLED at the timeout instead of stalling or leaking into CI, and a total-run watchdog (default 120 s, `--total-timeout-ms` to change, 0 to disable) caps the SUM of all tests, so N tests each hanging at their per-test limit can't multiply the stall; the runner's own unit tests (`cargo test --manifest-path tests/skyos-test-core/Cargo.toml`) and the CLI-contract tests (`cargo test --manifest-path tests/skyos-test/Cargo.toml`, unknown args must exit non-zero) run in the same job.
- **`ade`'s `#[cfg(test)]` modules run on the host.** `ade` gained a lib target (`lib.rs`) with the same `cfg_attr(not(test), no_std)` treatment as libsarga; the binary (`main.rs`) is the bare-metal entrypoint only. `cargo test -p ade --lib` runs 36 tests across `sys/{audio,display,input,network}.rs` (mixer volume/balance math, display mode/DPI/pitch math, Ctrl-folding byte classification, IPv4/CIDR/SSID/RSSI) and `util/{app_catalog,explorer}.rs` (launch tracking/filtering, entry sorting/size formatting). The input pipeline's `from_byte`/`text` consume the `sys::input` helpers, so the host tests pin real producer behavior.
- Test crates (`skyos-test`, `skyos-test-core`) are excluded from `Cargo.toml` workspace members (host-only deps; they declare their own `[workspace]` roots).

### How `libsarga`'s Host Tests Work (and Why the Others Can't)

The `#![no_std]` + `#![feature(alloc_error_handler)]` crate-level attributes in `libsarga/src/lib.rs` are gated with `#![cfg_attr(not(test), ...)]`, and the kernel-only lang items (`#[panic_handler]`, `#[global_allocator]`, `#[alloc_error_handler]`, the `#[no_mangle]` kernel ABI exports) are `#[cfg(not(test))]`. Under `cfg(test)` the crate compiles as a plain std library, so the std test harness works and `catch_unwind` reports per-test failures.

1. **Bare-Metal Target:** The userspace target `x86_64-sarga` is a bare-metal target with no precompiled sysroot libs, so `core`/`alloc` must be built from source with `-Zbuild-std=core,alloc` (passed explicitly on sarga-target commands, as CI does). The `.cargo/config.toml` no longer sets a global `build-std` — that made cargo rebuild `core` for *every* target and broke host builds with "duplicate lang item in crate `core`".

2. **No Test Harness:** The kernel has `panic = "abort"` and no test harness configured. `[profile.dev] panic = "abort"` covers sarga-target builds; `panic=abort` is scoped to the sarga target (not the host), so host tests unwind normally.

3. **Platform-Specific Code:** Code that touches syscalls/hardware is not exercised on the host: the errno TLS probe is `#[cfg(not(test))]`, and the net/semver tests only reach pure parsing logic.

### Why Test Crates Are Excluded

The `tests/skyos-test` and `tests/skyos-test-core` crates are excluded from the workspace because:

- They are **host-side tools** designed to run on the development machine, not on SkyOS
- They depend on `serde` and other standard library features not available in the bare-metal target
- Including them would complicate the build for the bare-metal target

## Remediation Options

### Option 1: Add Host-Side Unit Tests (Recommended for Pure Logic)

For pure logic that doesn't depend on syscalls or hardware:

1. Add `#[cfg(test)]` modules with `#[test]` functions
2. Use `#[cfg(target_os = "linux")]` or similar to only run tests on host
3. Add `cargo test` step to CI for host-only tests

**Example:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_decode() {
        assert_eq!(hex_decode(b"48656c6c6f"), Some(b"Hello".to_vec()));
    }
}
```

### Option 2: Enable Test Crates in Workspace

If `skyos-test` and `skyos-test-core` are meant to be actively used:

1. Add them to `Cargo.toml` workspace members
2. Mark them with `[[bin]]` configuration if needed
3. Add build/test steps to CI

### Option 3: Continue with Kernel Self-Tests (Current Path)

Accept that kernel self-tests are the primary verification mechanism and document this as the intended testing strategy.

## Current Recommendation

**Run `cargo test -p libsarga` and `cargo test -p ade --lib` (wired into the CI `host-tests` job), and keep adding `#[cfg(test)]` modules for pure logic.**

The `#[cfg(test)]` modules in `libsarga` and `ade` cover pure logic (parsing, errno, semver, mixer/display/input/network math, catalog/explorer state) that does not need the kernel runtime. New pure-logic code should follow the same pattern: keep it syscall-free, and let the crate's `cfg(test)` mode compile it for the host (a lib target when the crate is a binary). Code that touches syscalls/hardware stays covered by kernel self-tests.

## Adding Unit Tests for Pure Logic

To add unit tests for pure logic (e.g., `libsarga/src/hash.rs::hex_decode`):

1. Add test module with `#[cfg(test)]` guard
2. Keep the logic syscall-free (pure parsing/encoding/comparison)
3. Run with `cargo test -p libsarga` (or `cargo test -p ade --lib` for ade) — the crate compiles for the host under `cfg(test)`; no `--target` needed

Example (matches the existing `libsarga/src/errno.rs` test module):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_decode_valid() {
        assert_eq!(hex_decode(b"48656c6c6f"), Some(vec![0x48, 0x65, 0x6c, 0x6c, 0x6f]));
    }

    #[test]
    fn test_hex_decode_odd_length() {
        assert_eq!(hex_decode(b"486"), None);
    }
}
```

## Summary

- **Primary test path:** Kernel self-tests (TAP format, verified in CI)
- **Unit tests:** `libsarga`'s `#[cfg(test)]` modules (errno/fs/gui/hash/net/png/semver/theme/toml) and `ade`'s (sys/{audio,display,input,network} + util/{app_catalog,explorer}) run on the host via `cargo test -p libsarga` / `cargo test -p ade --lib` and are wired into the CI `host-tests` job
- **Host-side test framework:** Exists but excluded from workspace
- **Recommendation:** Keep adding syscall-free `#[cfg(test)]` modules to `libsarga` and `ade` (the lib target exists); rely on kernel self-tests for system verification
