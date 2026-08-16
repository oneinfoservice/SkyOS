# Testing Strategy Overview

The bare-metal targets have no `#[test]`-based harness, but **`libsarga`'s pure-logic `#[cfg(test)]` modules run on the host**: `cargo test -p libsarga` compiles the crate under `cfg(test)` with the std test harness (23 errno/net/semver tests, wired into CI's `host-tests` job). Everything else is tested through real infrastructure:

## Test Layers

| Layer | Mechanism | Scope | Speed |
|-------|-----------|-------|-------|
| libsarga unit tests | `cargo test -p libsarga` (std test harness) | errno/net/semver pure logic | Milliseconds |
| Host-side algorithm suites | `tests/skyos-test` (`skyos-test-core` runner, CI-wired in `host-tests`) | Buddy allocator, mouse decoder | Milliseconds |
| On-OS integration binaries | `tests/thread_test` (`#![no_std]` + `libsarga`) | Real syscalls at the login prompt | Seconds |
| QEMU boot smoke test | `tests/qemu_boot.sh` | Boot to `login:` without panic | Minutes |
| QEMU integration test | `tests/qemu_integration_test.sh` + `expect` | Boot + shell interaction | Minutes |
| Kernel self-tests | kernel `self_test` feature (TAP to serial) | Allocator/FS/net invariants in-kernel | Seconds (during boot) |

## Testing Philosophy

1. **Test at the lowest level possible**: host-side suites validate algorithms fast
2. **Test error paths**: `thread_test` exercises syscall error/edge cases on real hardware
3. **Automate everything**: CI runs fmt, clippy, check-all-targets, and integration-qemu
4. **Boot-gated**: the strongest gate is "does the system boot to a login prompt and pass kernel TAP self-tests"

## Test Infrastructure

- **Host runner**: `skyos-test-core` `TestRunner` + `Test { name, category, run }`
- **Kernel TAP**: the `self_test` feature emits `TAP version 13` / `ok` / `not ok` to serial; CI greps for `not ok`
- **QEMU automation**: `qemu_boot.sh`/`qemu_integration_test.sh` build kernel+userspace+initrd+bootimage+ISO, boot in QEMU, and assert on the serial log

## Running Tests

```bash
# libsarga host unit tests
cargo test -p libsarga

# Host-side algorithm suites
cargo run --manifest-path tests/skyos-test/Cargo.toml -- run

# QEMU boot smoke test (kernel dir defaults to ../SKYIOUS KERNEL)
./tests/qemu_boot.sh

# Full integration (expect-driven)
./tests/qemu_integration_test.sh
```

See `docs/testing/integration.md` for details.
