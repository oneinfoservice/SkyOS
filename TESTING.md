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

**Included:** `cargo test -p libsarga` (host unit tests for pure logic)

## Testing Gap: No CI-Wired Unit Tests

### Current State

- **`libsarga`'s `#[cfg(test)]` modules run on the host.** `cargo test -p libsarga` compiles the crate for the host with the std test harness and runs the pure-logic tests in `errno.rs`, `net.rs`, and `semver.rs` (23 tests). No QEMU, no kernel, no syscalls.
- **The `skyos-test` host framework runs in CI** (buddy-allocator + PS/2-mouse suites, 17 tests): `cargo run --manifest-path tests/skyos-test/Cargo.toml -- run`, wired into the `host-tests` job. Its runner exits non-zero on any FAIL or an empty run, so a broken suite cannot silently pass.
- **`ade`'s `#[cfg(test)]` modules** (`sys/{audio,display,input,network}.rs`, `util/{automation,developer,extension,plugin,sdk}.rs`) still cannot run on the host: `ade` is a `#![no_std]` + `#![no_main]` binary with no lib target, so `cargo test -p ade` has no harness to compile.
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

**Run `cargo test -p libsarga` (wired into the CI `host-tests` job), and keep adding `#[cfg(test)]` modules for pure logic.**

The `#[cfg(test)]` modules in `libsarga` cover pure logic (parsing, errno, semver) that does not need the kernel runtime. New pure-logic code should follow the same pattern: keep it syscall-free, and let the crate's `cfg(test)` mode compile it for the host. `ade` should gain a lib target (`ade-core`) before its `#[cfg(test)]` modules can run; code that touches syscalls/hardware stays covered by kernel self-tests.

## Adding Unit Tests for Pure Logic

To add unit tests for pure logic (e.g., `libsarga/src/hash.rs::hex_decode`):

1. Add test module with `#[cfg(test)]` guard
2. Keep the logic syscall-free (pure parsing/encoding/comparison)
3. Run with `cargo test -p libsarga` (the crate compiles for the host under `cfg(test)`; no `--target` needed)

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
- **Unit tests:** `libsarga`'s `#[cfg(test)]` modules (errno/net/semver) run on the host via `cargo test -p libsarga` and are wired into the CI `host-tests` job; `ade`'s modules still need a lib target before they can run
- **Host-side test framework:** Exists but excluded from workspace
- **Recommendation:** Keep adding syscall-free `#[cfg(test)]` modules to `libsarga`; extract an `ade-core` lib for ADE's pure logic; rely on kernel self-tests for system verification
