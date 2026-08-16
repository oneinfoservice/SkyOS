#!/usr/bin/env python3
"""Guard pins for the clippy fixes in libsarga (no cargo needed).

The pinned CI toolchain (nightly-2026-01-15) is clean on libsarga, but
NEWER clippy (>= 1.9x) flags three patterns that were fixed deliberately:

  - libsarga/src/libskyos.rs getdents loop: `n as usize` where `n` is
    already `usize` (clippy::unnecessary_cast) — the casts were dropped;
    this pins the fixed form so a future edit can't re-add them.
  - libsarga/src/posix.rs: `SHEBANG` written as a char-array slice
    (clippy::byte_char_slices) — rewritten as the byte str `*b"#!"`.

The memcpy/memset/memcmp/memmove runtime-symbol definitions and the slab
allocator machinery in mem.rs are cfg(target_os = "none")-gated (std/libc
provide all of it on the host), so the host clippy leg lints only lib
targets -- the no_std bin crates cannot compile on the host at all. This
gate pins only the fixable patterns above, plus the host-leg scope.

Run:  python3 tests/test_clippy_guard.py
"""

import os
import re
import unittest

from scan_rust import strip_rust

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIBSKYOS_RS = os.path.join(REPO_ROOT, "libsarga", "src", "libskyos.rs")
POSIX_RS = os.path.join(REPO_ROOT, "libsarga", "src", "posix.rs")


class TestClippyGuard(unittest.TestCase):
    """Source pins so the fixed clippy-clean forms cannot regress."""

    @classmethod
    def setUpClass(cls):
        with open(LIBSKYOS_RS, encoding="utf-8") as fh:
            cls.libskyos = fh.read()
        with open(POSIX_RS, encoding="utf-8") as fh:
            cls.posix = fh.read()
        cls.libskyos_code = strip_rust(cls.libskyos)

    def test_libskyos_getdents_loop_has_no_useless_cast(self):
        # getdents64 returns Result<usize, _>, so `n as usize` was a no-op
        # cast (clippy::unnecessary_cast on newer toolchains). The loop must
        # compare n directly; a re-added cast fails here.
        self.assertIn("while off < n {", self.libskyos_code)
        self.assertIn("if off + 18 > n {", self.libskyos_code)
        self.assertNotIn("n as usize", self.libskyos_code,
                         "useless usize cast re-added to the getdents loop")

    def test_posix_shebang_is_byte_str(self):
        # clippy::byte_char_slices: a char-array const [b'#', b'!'] is better
        # written as the byte str *b"#!". The fixed form is pinned.
        self.assertIn('const SHEBANG: [u8; 2] = *b"#!";', self.posix)
        self.assertNotIn("const SHEBANG: [u8; 2] = [b'#', b'!'];", self.posix)


class TestClippyJobCoversWholeWorkspace(unittest.TestCase):
    """The CI clippy job lints ALL workspace members with zero warnings.

    The existing clippy job is workspace-wide, but its coverage was implicit
    (workspace-root semantics). These pins make the contract explicit and
    tamper-proof: a future `-p <crate>` filter, a workspace
    `default-members`, or a dropped host-target leg fails here before any
    GitHub run -- so the local/CI toolchain lint delta can never silently
    widen the gate.
    """

    def _clippy_job(self):
        with open(
            os.path.join(REPO_ROOT, ".github", "workflows", "ci.yml"),
            encoding="utf-8",
        ) as fh:
            ci = fh.read()
        m = re.search(r"^  clippy:\n(.*?)(?=^  [a-z0-9-]+:)", ci, re.M | re.S)
        self.assertIsNotNone(m, "clippy job not found in ci.yml")
        return m.group(1)

    def test_clippy_runs_are_workspace_wide_with_no_filters(self):
        job = self._clippy_job()
        self.assertIn("nightly-2026-01-15", job,
                       "clippy must run on the CI-pinned nightly")
        runs = re.findall(r"cargo clippy[^\n]*", job)
        self.assertGreaterEqual(len(runs), 2,
                                "expected sarga + host clippy runs, got: %r" % runs)
        for run in runs:
            self.assertIn("--workspace", run,
                           "clippy must cover all workspace members: %r" % run)
            self.assertNotIn("-p ", run,
                             "clippy must not filter crates: %r" % run)

    def test_workspace_has_no_default_members(self):
        with open(os.path.join(REPO_ROOT, "Cargo.toml"), encoding="utf-8") as fh:
            root = fh.read()
        self.assertNotIn("default-members", root,
                         "a workspace default-members would silently shrink "
                         "the clippy gate")

    def test_clippy_runs_both_sarga_and_host_targets(self):
        job = self._clippy_job()
        self.assertIn("--target x86_64-sarga.json", job,
                       "sarga-target clippy run missing")
        self.assertIn("cargo clippy --workspace --lib -- -D warnings", job,
                       "host-target clippy run missing")


if __name__ == "__main__":
    unittest.main()
