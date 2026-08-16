# SkyOS Verified Audit Report

**Date:** July 31, 2026  
**Scope:** Userspace repository (SkyOS)  
**Methodology:** Cross-check of SKYOS_DEV_REPORT.md claims against source code, verification of newly identified issues

---

## Executive Summary

This report provides a verified, corrected inventory of issues in the SkyOS userspace repository. It corrects false claims from the previous audit (SKYOS_DEV_REPORT.md) and documents newly verified issues with accurate descriptions, file references, severity ratings, and verification status.

**Total Issues Verified:** 14  
- **Correctness:** 3  
- **Security:** 3  
- **Dead Code & Duplication:** 4  
- **Architecture:** 1  
- **Build Tooling:** 2  
- **Testing:** 1

---

## Resolution Update (July 31, 2026)

Commit `3216775` ("fix(userspace): audit-verified bug fixes, ADE refactor, and crate modernization") addressed the Phase 2/3 items in this report. Current status:

- **C1 (umount syscall number)** — **RESOLVED.** `libsarga/src/fs.rs:194` now calls `crate::syscall::SYS_UMOUNT2` instead of a hardcoded `166`.
- **S1 (fixed password salt)** — **RESOLVED.** `passwd/src/main.rs` reads `/dev/urandom` for the salt first; the fixed constant `0x9E3779B97F4A7C15` remains only as a PID-seeded fallback when no entropy source is available.
- **S2 (login-manager auth weaknesses)** — **RESOLVED.** `login-manager` returns `false` on an unreadable `/etc/shadow` (no more root auto-accept) and delegates to the shared `libsarga::hash::verify_password`, which rejects non-`PBKDF2-` shadow entries.
- **D1 (unused ADE scaffold modules)** — **RESOLVED.** The `ade/src/sys/{session,session_service,login_session,notification,power}.rs` files and `ade/src/util/clipboard_service.rs` were deleted.
- **D2 (permission constant table collision)** — **RESOLVED.** The duplicate `PERM_*` table in `ade/src/sec/perms.rs` was removed; the live constants live only in `ade/src/ipc/permission.rs`.
- **D3 (password verification duplication)** — **RESOLVED.** `verify_password`/`hex_decode` are consolidated in `libsarga/src/hash.rs` and used by both `login` and `login-manager`. `hex_decode` now wraps the `hex` crate.
- **T1 (zero unit tests)** — **RESOLVED.** `libsarga`'s `#[cfg(test)]` modules (`errno.rs`, `net.rs`, `semver.rs`) compile and run on the host: `cargo test -p libsarga` runs 23 tests, wired into the CI `host-tests` job. `libsarga/src/lib.rs` gates its `no_std`/lang items with `cfg_attr(not(test), ..)`/`#[cfg(not(test))]`, and `.cargo/config.toml` scopes `build-std`/`panic=abort` to the sarga target so the std test harness works.
- **B2 (x86_64-vahi "legacy" naming)** — **STALE/FALSE.** `x86_64-vahi` is the kernel crate's real build target (`kernel/target/x86_64-vahi` exists); scripts referencing it are not stale. The `velox` references were removed.

---

## False Claims from SKYOS_DEV_REPORT.md

The following claims in SKYOS_DEV_REPORT.md Section 4 and 6 were verified as **FALSE** and should not be acted upon:

| ID | Claim | Correction | Verification Status |
|----|-------|------------|---------------------|
| C6 | **64-bit pointer truncated to 32 bits** at `libsarga/src/args.rs:9` | ARGV is stored as `AtomicUsize`, not i32. Line 9 shows `ARGV.store(stack as usize + 8, ...)` which is correct for x86_64. | **FALSE** |
| U1 | **Lifetime transmute** at `libsarga/src/gui.rs:206` | No transmute at line 206. The real unsound pattern is `Vec::leak` at line 231 combined with `unsafe impl Send for TtfFont` at line 227. | **FALSE** (corrected below) |
| U3 | **Mutable statics** at `libsarga/src/stdio.rs:21-23` | No `static mut`. STDIN/STDOUT/STDERR are wrapped in `Mutex<FILE>`. The real hazard is `Box::leak` in `fopen` (line 92) and `Box::from_raw` in `fclose` (line 107). | **FALSE** (corrected below) |
| 4.6 | **Proper password hashing (djb2 used)** | Password hashing uses PBKDF2-HMAC-SHA256 via syscall 401 (SYS_HASH), not djb2. See `libsarga/src/hash.rs`. | **FALSE** |

---

## Verified Issues by Category

### Correctness

#### C1: SYS_UMOUNT2 Syscall Number Mismatch
- **File:** `libsarga/src/fs.rs:194`
- **Description:** `umount()` uses hardcoded syscall number `166` instead of `SYS_UMOUNT2` (`167`) defined in `libsarga/src/syscall.rs:86`. This causes the wrong syscall to be invoked.
- **Reference:** `libsarga/src/io.rs` and `libsarga/src/syscall.rs` correctly use `SYS_UMOUNT2 = 167`.
- **Severity:** HIGH
- **Verification Status:** **RESOLVED** — see Resolution Update.
- **Remediation Phase:** Phase 2

#### C2: Error-Handling Inconsistency
- **Files:** `libsarga/src/syscall.rs`, `libsarga/src/fs.rs`, `libsarga/src/io.rs`, `libsarga/src/posix.rs`
- **Description:** Four different error conventions across the codebase:
  - `syscall.rs`: Returns raw `i64` (negative for errors)
  - `fs.rs`: Returns `Result<T, i64>` (negative errno in Err)
  - `io.rs`: Returns `Result<T, Error>` (enum-based)
  - `posix.rs`: Returns `Result<T, Error>` (enum-based)
  This inconsistency makes error handling fragile and error-prone.
- **Severity:** MEDIUM
- **Verification Status:** CONFIRMED
- **Remediation Phase:** Phase 3 (Library Unification)

#### C3: Vec::leak Memory Leak in TtfFont
- **File:** `libsarga/src/gui.rs:231`
- **Description:** `TtfFont::from_bytes()` uses `Vec::leak()` to convert data to `'static` lifetime, which never frees the memory. Combined with `unsafe impl Send for TtfFont` (line 227), this allows unsound cross-thread access to leaked memory.
- **Severity:** MEDIUM
- **Verification Status:** CONFIRMED (corrected from false transmute claim)
- **Remediation Phase:** Phase 2

---

### Security

#### S1: Fixed Salt in Password Generation
- **File:** `passwd/src/main.rs:62-68`
- **Description:** `generate_salt()` uses a fixed constant salt (`0x9E3779B97F4A7C15u64.to_le_bytes()`) duplicated across both halves of the 16-byte salt array. This makes all passwords vulnerable to rainbow table attacks.
- **Severity:** HIGH
- **Verification Status:** **RESOLVED** — see Resolution Update.
- **Remediation Phase:** Phase 2

#### S2: login-manager Authentication Weaknesses
- **File:** `login-manager/src/main.rs:24-74`
- **Description:** `verify_password()` has two critical weaknesses:
  1. Auto-accepts `username == "root"` when `/etc/shadow` is unreadable (line 27)
  2. Falls back to plaintext comparison for non-`PBKDF2-` shadow entries (line 71)
- **Note:** `login/src/main.rs` does NOT have these weaknesses - it correctly returns `false` on file read error (line 87) and rejects non-PBKDF2 entries (line 132).
- **Severity:** HIGH
- **Verification Status:** **RESOLVED** — see Resolution Update.
- **Remediation Phase:** Phase 2

#### S3: FILE Use-After-Free / Double-Free Risk
- **File:** `libsarga/src/stdio.rs:92, 107`
- **Description:** `fopen()` uses `Box::leak(f)` to return a `'static mut FILE` reference (line 92). `fclose()` attempts to reclaim it with `Box::from_raw(file as *mut FILE)` (line 107). This pattern is fragile:
  - Calling `fclose()` twice causes double-free
  - Calling `fclose()` on a stack FILE causes use-after-free
  - No tracking prevents these errors
- **Severity:** MEDIUM
- **Verification Status:** CONFIRMED (corrected from false static mut claim)
- **Remediation Phase:** Phase 2

---

### Dead Code & Duplication

#### D1: Unused Scaffold Modules in ADE
- **Files:** 
  - `ade/src/sys/session.rs`
  - `ade/src/sys/session_service.rs`
  - `ade/src/sys/login_session.rs`
  - `ade/src/util/clipboard_service.rs`
  - `ade/src/sys/notification.rs`
  - `ade/src/sys/power.rs`
- **Description:** These scaffold modules are declared in `ade/src/sys/mod.rs` but are not used. The live implementations are in `ade/src/service/` (clipboard, notification, power, session) which are wired into `ServiceManager`.
- **Note:** Keep `ade/src/util/crash_manager.rs` - it is used.
- **Severity:** LOW
- **Verification Status:** **RESOLVED** (deleted) — see Resolution Update.
- **Remediation Phase:** Phase 3

#### D2: Permission Constant Table Collision
- **Files:** `ade/src/sec/perms.rs`, `ade/src/ipc/permission.rs`
- **Description:** Two separate `PERM_*` constant tables exist:
  - `ade/src/ipc/permission.rs:47-58` - Live constants used by IPC system
  - `ade/src/sec/perms.rs` - Unused constants (file only contains PermissionManager store)
  Both define constants against the same `AppPermission` bitflags, creating confusion and potential drift.
- **Severity:** MEDIUM
- **Verification Status:** **RESOLVED** — see Resolution Update.
- **Remediation Phase:** Phase 3

#### D3: Password Verification Logic Duplication
- **Files:** `login/src/main.rs:71-82, 84-136`, `login-manager/src/main.rs:11-22, 24-74`
- **Description:** `hex_decode()` and `verify_password()` functions are duplicated between `login` and `login-manager` with identical logic. This doubles maintenance burden and risks divergence.
- **Severity:** MEDIUM
- **Verification Status:** **RESOLVED** — see Resolution Update.
- **Remediation Phase:** Phase 2

#### D4: Binary Naming Duplication
- **Files:** Multiple references across codebase
- **Description:** Two shell names and two package manager names are referenced:
  - Shells: `sash` (exists as crate) vs `sargash` (referenced in scripts, CI, docs but no crate exists)
  - Package managers: `spkg` (exists as crate) vs `skypkg` (referenced in skybuild, ade/app_db.rs, scripts but no crate exists)
- **References:** `.github/workflows/system-updates.yml`, `skybuild/src/main.rs`, `ade/src/util/app_db.rs`, `scripts/build_self.sh`, `SARGA_OS_DESIGN_QA.md`, `README.md`
- **Severity:** LOW
- **Verification Status:** CONFIRMED
- **Remediation Phase:** Phase 4

---

### Architecture

#### A1: Desktop Struct Monolith
- **File:** `ade/src/core/desktop.rs:100-142`
- **Description:** The `Desktop` struct has 64 fields and owns approximately 35 subsystems (window manager, service manager, IPC server, permission manager, lifecycle manager, etc.). This violates single responsibility and makes the desktop coordinator difficult to test, maintain, and reason about.
- **Severity:** MEDIUM
- **Verification Status:** CONFIRMED
- **Remediation Phase:** Phase 3 (future refactoring)

---

### Build Tooling

#### B1: Hardcoded Developer Paths
- **Files:** 
  - `build_disk.py:11, 61, 91` - References to `~/.cargo`, `~/.rustup`
  - `Makefile:5` - `KERNEL_DIR ?= ../SKYIOUS\ KERNEL`
  - `tests/test_panic.ps1:2`, `tests/test_login.ps1:2`, `tests/qemu_shell_test.ps1:4` - `C:\Users\nanda\Desktop\Github\SKYIOUS KERNEL`
  - `run.ps1:6`, `run_qemu_display.ps1:2`, `run_ade_test.bat:5` - Hardcoded kernel paths
- **Description:** Build and test scripts contain hardcoded absolute paths specific to the developer's machine, preventing reproducible builds on other systems.
- **Severity:** HIGH
- **Verification Status:** CONFIRMED
- **Remediation Phase:** Phase 4

#### B2: Legacy Naming Inconsistency
- **Files:** Multiple build scripts and test files
- **Description:** Legacy target triple and naming inconsistent with current `x86_64-sarga`:
  - `x86_64-vahi` - Referenced in `build_disk.py:91`, test scripts, and various `.ps1` files
  - `velox` - Referenced in `make_bootimage.sh:4, 21`
  - `bootimage-vahi_kernel.bin` - Referenced throughout scripts
  Current target is `x86_64-sarga.json` but many scripts reference the old names.
- **Severity:** MEDIUM
- **Verification Status:** **STALE/FALSE** — see Resolution Update. `x86_64-vahi` is the kernel crate's real target, not a stale alias.
- **Remediation Phase:** Phase 4

---

### Testing

#### T1: Zero Unit Tests in Workspace
- **File:** `Cargo.toml:3-45`
- **Description:** Zero `#[test]` functions exist in any workspace crate (libsarga, ade, coreutils, sash, etc.). The only `#[test]` matches are in `target/x86_64-sarga/doc/` which are from the `ttf_parser` dependency, not project code.
- **Workspace Exclusion:** `tests/skyos-test` and `tests/skyos-test-core` crates exist but are excluded from workspace members in `Cargo.toml`.
- **CI:** `.github/workflows/ci.yml` runs `fmt`, `clippy`, build, and the `host-tests` job's `cargo test -p libsarga` (libsarga's errno/net/semver unit tests).
- **Severity:** HIGH
- **Verification Status:** **RESOLVED** — `cargo test -p libsarga` runs 23 host tests; see Resolution Update.
- **Remediation Phase:** Phase 4

---

## Prioritized Remediation Plan

> Status of the plan below as of July 31, 2026: Phase 2 and Phase 3 items marked **DONE** were
> implemented in commit `3216775`. Phase 4 items remain open. See Resolution Update.

### Phase 2: Correctness & Security Fixes (Low-Risk, High-Value)

1. **Fix umount syscall number** (C1) — **DONE**
   - Replace hardcoded `166` with `SYS_UMOUNT2` constant in `libsarga/src/fs.rs:194`
   - Use named constant for consistency with `io.rs`

2. **Fix password salt generation** (S1) — **DONE**
   - Replace fixed constant salt in `passwd/src/main.rs:62-68` with random 16-byte salt
   - Use available entropy source or document dependency

3. **Remove login-manager authentication weaknesses** (S2) — **DONE**
   - Return `false` when `/etc/shadow` unreadable (line 27)
   - Remove plaintext fallback for non-PBKDF2 entries (line 71)

4. **Deduplicate password verification logic** (D3) — **DONE**
   - Add shared `verify_password` and `hex_decode` to `libsarga` (e.g., `libsarga/src/hash.rs` or new auth module)
   - Update `login/src/main.rs` and `login-manager/src/main.rs` to use shared implementation
   - Ensure behavior matches corrected `login` (no auto-accept root, no plaintext fallback)

### Phase 3: Dead Code & Architecture Cleanup

1. **Remove unused scaffold modules** (D1) — **DONE**
   - Delete: `ade/src/sys/session.rs`, `session_service.rs`, `login_session.rs`, `notification.rs`, `power.rs`
   - Delete: `ade/src/util/clipboard_service.rs`
   - Keep: `ade/src/util/crash_manager.rs` (used)
   - Update `ade/src/sys/mod.rs` and `ade/src/util/mod.rs`

2. **Unify permission constant tables** (D2) — **DONE**
   - Choose `ade/src/ipc/permission.rs` as single source of truth (live call sites)
   - Remove unused constants from `ade/src/sec/perms.rs`
   - Keep `PermissionManager` store in `ade/src/sec/perms.rs`
   - Ensure all permission checks use unified constants

### Phase 4: Build Tooling & Testing

1. **Remove hardcoded paths** (B1) — **OPEN**
   - Replace absolute paths with script-relative or environment-variable-based paths
   - Fix references to nonexistent `kernel/`, `builder/`, `userspace/` directories
   - Use `$PSScriptRoot` or equivalent for script locations

2. **Resolve legacy naming** (B2) — **OBSOLETE** (see Resolution Update: `x86_64-vahi` is the kernel's real target; only `velox`/`bootimage-velox` references were stale and have been removed)
   - Align all references to `x86_64-sarga`
   - Remove `x86_64-vahi` and `velox` references
   - Update bootimage naming to be consistent

3. **Resolve binary naming duplication** (D4) — **OPEN**
   - Adopt `sash` as canonical shell name
   - Adopt `spkg` as canonical package manager name
   - Update references to `sargash` and `skypkg` in CI, scripts, app registries

4. **Wire unit-test path or document gap** (T1) — **RESOLVED** — `libsarga`'s errno/net/semver `#[cfg(test)]` modules compile and run on the host via `cargo test -p libsarga` (23 tests), with a matching step in the CI `host-tests` job. `tests/skyos-test`/`skyos-test-core` remain excluded from the workspace (host-side tools with their own `[workspace]`).

---

## Summary

This audit corrects 4 false claims from the previous report and documents 14 verified issues across 6 categories. The most critical issues were:

1. **Security:** Fixed salt in password generation (S1) and login-manager authentication weaknesses (S2) — **both resolved** (commit `3216775`)
2. **Correctness:** Syscall number mismatch (C1) and error-handling inconsistency (C2) — **C1 resolved**; C2 open
3. **Build Reproducibility:** Hardcoded developer paths (B1) — open
4. **Testing:** Complete absence of unit tests (T1) — resolved; `cargo test -p libsarga` runs libsarga's errno/net/semver `#[cfg(test)]` modules on the host (23 tests, CI-wired)

Of the 14 verified issues, 6 are resolved, 2 are stale/false, and 6 remain open (C2, C3, S3, A1, B1, D4).

The remediation plan prioritizes low-risk, high-value fixes in Phase 2 (correctness and security), followed by cleanup in Phase 3, and build tooling/testing in Phase 4.

---

*Report generated July 31, 2026 by systematic codebase verification.*
*All claims cross-checked against actual source code.*
