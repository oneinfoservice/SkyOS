#!/usr/bin/env python3
"""Single-home gate for the auth tty / echo / attempt-cap primitives.

The consolidation moved the termios echo discipline and line reads into
libsarga::tty, the attempt-cap constants into libsarga::auth, and the
whole-file read loop into libsarga::io::read_to_end. This gate scans every
userspace .rs file and fails if any of those DEFINITIONS appears outside
its allowed home — a future auth binary (or a reverted copy) reintroducing
the duplication fails here before any QEMU boot, the same way
test_single_userspace_termios_definition pins the Termios layout.

`fn read_line` has one legitimate second home: sash/src/readline.rs — the
interactive line EDITOR (history + prompt, String-returning, a different
purpose from the byte-at-a-time tty reader). It is allowlisted explicitly
so the gate cannot false-fail it.

Also pins the svc decision host-side: svc reads its config through the
shared libsarga::io::read_to_string and defines NO private read loop.

Documented NON-GOAL -- the CString::new FFI idiom: a one-line
`CString::new(x.as_bytes())` match is not a behavior-bearing primitive.
It appears in 17 code sites across 8 files in three crates (sash 11,
libsarga 3, spkg 3). Consolidating it into a libsarga helper would touch
17 call sites to save ~2 lines each, with zero behavioral, safety, or
correctness gain -- pure churn. The gate must NOT grow a requirement
for it; if the count ever grows substantially, revisit the non-goal
deliberately (see TestWorkspaceDupSurveyNonGoals).

Run:  python3 tests/test_single_home_gate.py
"""

import os
import re
import sys
import unittest

from scan_rust import strip_definition_lines, strip_rust

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Definition pattern -> files where it is allowed to appear.
HOMES = {
    "struct Termios": {"libsarga/src/tty.rs"},
    "fn echo_off": {"libsarga/src/tty.rs"},
    "fn echo_on": {"libsarga/src/tty.rs"},
    "fn ensure_echo": {"libsarga/src/tty.rs"},
    "fn read_password": {"libsarga/src/tty.rs"},
    "fn read_line": {"libsarga/src/tty.rs", "sash/src/readline.rs"},
    "const MAX_FAILED_ATTEMPTS": {"libsarga/src/auth.rs"},
    "const BACKOFF_NS": {"libsarga/src/auth.rs"},
    "fn read_stdin_all_bytes": {"libsarga/src/io.rs"},
}

# Shared primitives that must have at least one consumer OUTSIDE libsarga.
# echo_off/echo_on are deliberately NOT listed: their only caller is
# read_password, inside libsarga -- they are read_password's implementation
# (pinned by test_read_password_still_wraps_echo_discipline below), not a
# public surface a future refactor could silently orphan.
#
# value = regex of DEFINITION lines to ignore in the scan (None = any
# reference counts). read_line needs it because sash/src/readline.rs
# legitimately DEFINES its own read_line (the allowlisted second home);
# its definition must not masquerade as a caller of libsarga's reader.
CALLER_REQUIRED = {
    "ensure_echo": None,
    "read_line": r"\bfn\s+read_line\b",
    "read_password": None,
    "MAX_FAILED_ATTEMPTS": None,
    "BACKOFF_NS": None,
    "read_stdin_all_bytes": None,
}

# Directories never scanned: build output, VCS, local kernel copy (separate
# repo), archived trees, and the Freebuff DB.
SKIP_DIRS = {
    "target",
    "target-ws",
    ".git",
    "__pycache__",
    "archive",
    "kernel",
    "fuzz",
    ".freebuff",
}


def _workspace_rs_files():
    for root, dirs, files in os.walk(REPO_ROOT):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for fn in files:
            if not fn.endswith(".rs"):
                continue
            p = os.path.join(root, fn)
            rel = os.path.relpath(p, REPO_ROOT).replace("\\", "/")
            yield rel, p


class TestSingleHomeGate(unittest.TestCase):
    """No auth tty/echo/cap definition outside its libsarga home."""

    def test_auth_primitives_live_only_in_libsarga_homes(self):
        hits = {name: [] for name in HOMES}
        for rel, p in _workspace_rs_files():
            with open(p, encoding="utf-8", errors="replace") as fh:
                src = fh.read()
            for name in HOMES:
                if re.search(r"\b" + re.escape(name), src):
                    hits[name].append(rel)
        for name, allowed in HOMES.items():
            offenders = sorted(set(hits[name]) - allowed)
            self.assertEqual(
                offenders,
                [],
                "%s is defined outside its single home %s: %s"
                % (name, sorted(allowed), offenders),
            )


class TestSvcUsesSharedReader(unittest.TestCase):
    """svc reads config via the shared libsarga reader — no private loop.

    svc was flagged during the read-loop consolidation audit. Verified: it
    has NO private read-whole-file loop (the different-errno concern did
    not materialize — svc calls libsarga::io::read_to_string directly), so
    there is nothing to consolidate; this pin keeps it that way.
    """

    def test_svc_has_no_private_read_loop(self):
        with open(
            os.path.join(REPO_ROOT, "svc", "src", "main.rs"), encoding="utf-8"
        ) as fh:
            svc = fh.read()
        code = strip_rust(svc)
        self.assertIn("io::read_to_string", code,
                      "svc must read config through libsarga's shared reader")
        self.assertNotIn("fn read_", code,
                         "svc must not define a private read loop")


class TestSuEchoDiscipline(unittest.TestCase):
    """su's password read goes through the shared hidden-input reader.

    su was the THIRD auth binary duplicating the read loop — and it read
    the password with NO echo_off, leaking it back to the console/serial
    (the same leak login/passwd were fixed against). It now uses the
    shared libsarga::tty::read_password; this pin keeps the leak closed.
    """

    def test_su_password_read_is_hidden(self):
        with open(
            os.path.join(REPO_ROOT, "coreutils", "src", "su.rs"), encoding="utf-8"
        ) as fh:
            su = fh.read()
        self.assertIn("use libsarga::tty::read_password;", su,
                       "su must import the shared hidden-input reader")
        self.assertEqual(su.count("read_password(0)"), 1,
                         "su must read its single password via read_password(0)")
        self.assertNotIn("read_line(0)", su,
                          "no bare read_line(0) may read su's password")


class TestSashFallbackUsesSharedReader(unittest.TestCase):
    """sash's no-history fallback line reader delegates to the shared tty reader.

    read_raw_line was a THIRD byte-at-a-time console read loop (after
    libsarga::tty::read_line and the readline.rs editor) and its `_raw` name
    silently bypassed the single-home scan. It is now a thin wrapper around
    libsarga::tty::read_line that adds interactive backspace erasure; this pin
    keeps the byte loop out of sash and blocks a renamed reintroduction.
    """

    def test_sash_fallback_delegates_to_shared_reader(self):
        with open(
            os.path.join(REPO_ROOT, "sash", "src", "main.rs"), encoding="utf-8"
        ) as fh:
            sash = fh.read()
        code = strip_rust(sash)
        self.assertIn("libsarga::tty::read_line", code,
                       "sash's fallback must read through the shared tty reader")
        self.assertNotIn("io::read(0, &mut buf)", code,
                         "no private byte-read loop may return under a new name")


class TestSharedPrimitivesHaveLiveCallers(unittest.TestCase):
    """Every shared tty/auth primitive has a real consumer outside libsarga.

    Catches dead-code drift: if a refactor orphans one of the shared
    primitives (no external imports or calls), the single home becomes a
    museum piece and the gate silently stops protecting the duplication
    it exists to prevent. Each symbol below must appear in at least one
    file outside libsarga/ (definitions excluded where a second home is
    legitimate).
    """

    def test_each_primitive_has_an_external_caller(self):
        missing = []
        for sym, def_pat in CALLER_REQUIRED.items():
            callers = []
            for rel, p in _workspace_rs_files():
                if rel.startswith("libsarga/"):
                    continue
                with open(p, encoding="utf-8", errors="replace") as fh:
                    code = strip_rust(fh.read())
                if def_pat:
                    code = strip_definition_lines(code, [def_pat])
                if re.search(r"\b" + re.escape(sym), code):
                    callers.append(rel)
            if not callers:
                missing.append(sym)
        self.assertEqual(
            missing, [],
            "shared primitives with no caller outside libsarga "
            "(dead-code drift): %s" % missing,
        )

    def test_read_password_still_wraps_echo_discipline(self):
        # echo_off/echo_on are read_password's implementation (they have no
        # external callers BY DESIGN); this pins the wiring that keeps them
        # live: the restore-on-every-outcome invariant must not silently
        # drop the disable/restore pair.
        with open(
            os.path.join(REPO_ROOT, "libsarga", "src", "tty.rs"),
            encoding="utf-8",
        ) as fh:
            tty = strip_rust(fh.read())
        self.assertIn("echo_off(fd)", tty,
                       "read_password must disable echo before reading")
        self.assertIn("echo_on(fd, lflag)", tty,
                       "read_password must restore echo after reading")


class TestStdinReadAllConsolidation(unittest.TestCase):
    """The coreutils read-all-stdin loop has one home.

    Twenty tools each carried the identical byte-at-a-time stdin loop;
    they now call libsarga::io::read_stdin_all_bytes -- a raw byte
    reader (no UTF-8 assumption; the contract is in the name, and text
    tools apply from_utf8_lossy themselves). Five tools are
    INTENTIONALLY not consolidated: cat/tee/telnet write inside the loop
    (streaming), head stops after N lines (early exit), wc counts per
    chunk -- reading all of stdin first would change their memory /
    streaming behavior, and a byte-all variant is irrelevant to them
    because they never accumulate the whole input.
    """

    READ_ALL_TOOLS = [
        "awk", "base64", "cut", "fold", "grep", "less", "md5sum", "more",
        "nl", "patch", "sed", "shuf", "split", "sum", "tac", "tail", "tr",
        "tsort", "uniq", "xargs",
    ]
    STREAMING_TOOLS = ["cat", "tee", "telnet", "head", "wc"]

    def test_read_all_tools_use_the_shared_helper(self):
        missing = []
        for name in self.READ_ALL_TOOLS:
            with open(
                os.path.join(REPO_ROOT, "coreutils", "src", name + ".rs"),
                encoding="utf-8",
            ) as fh:
                code = strip_rust(fh.read())
            if "libsarga::io::read_stdin_all_bytes" not in code:
                missing.append(name)
        self.assertEqual(
            missing, [],
            "read-all tools not wired to read_stdin_all_bytes: %s" % missing,
        )


    def test_helper_returns_bytes_not_string(self):
        """The reader's no-UTF-8 contract is explicit and loss-free."""
        with open(
            os.path.join(REPO_ROOT, "libsarga", "src", "io.rs"),
            encoding="utf-8",
        ) as fh:
            io_src = strip_rust(fh.read())
        # Scope to the helper's own body: io.rs also holds read_to_string,
        # which legitimately contains from_utf8 -- that must not trip this.
        start = io_src.index("pub fn read_stdin_all_bytes()")
        end = io_src.index(chr(10) + '}' + chr(10), start) + 3
        body = io_src[start:end]
        self.assertIn("pub fn read_stdin_all_bytes() -> Vec<u8>", body)
        for bad in ("from_utf8", "from_utf8_lossy", "-> String"):
            self.assertNotIn(bad, body,
                             "read_stdin_all_bytes must never lossy-convert: %s" % bad)

    def test_binary_hash_tools_hash_raw_bytes(self):
        """md5sum/sum are binary hash tools: stdin AND file paths must never
        route through a lossy String conversion -- hashing corrupted bytes
        would silently produce a wrong checksum."""
        for name in ("md5sum", "sum"):
            with open(
                os.path.join(REPO_ROOT, "coreutils", "src", name + ".rs"),
                encoding="utf-8",
            ) as fh:
                code = strip_rust(fh.read())
            self.assertIn("libsarga::io::read_stdin_all_bytes()", code,
                          "%s stdin must read via the bytes helper" % name)
            self.assertNotIn("from_utf8", code,
                             "%s must never lossy-convert (binary data)" % name)
            self.assertIn("io::read_to_end", code,
                          "%s file path must read raw bytes" % name)
            self.assertNotIn("read_to_string", code,
                             "%s must not read files lossily" % name)


    def test_streaming_tools_keep_their_chunked_loops(self):
        offenders = []
        for name in self.STREAMING_TOOLS:
            with open(
                os.path.join(REPO_ROOT, "coreutils", "src", name + ".rs"),
                encoding="utf-8",
            ) as fh:
                code = strip_rust(fh.read())
            if "read_stdin_all_bytes" in code:
                offenders.append(name + ":uses-helper")
            if "io::read(0" not in code:
                offenders.append(name + ":no-chunked-loop")
        self.assertEqual(
            offenders, [],
            "streaming tools must keep chunked io::read(0 loops: %s" % offenders,
        )


class TestWorkspaceDupSurveyNonGoals(unittest.TestCase):
    """Patterns deliberately NOT single-homed, so a future editor doesn't
    "fix" them into churn. Each non-goal is documented in the module
    docstring AND pinned here, so deleting the note trips a test."""

    # 17 code sites across sash (11) / libsarga (3) / spkg (3); the raw
    # 18th hit in spkg/src/main.rs lives inside a comment (strip_rust
    # excludes it). If this count grows, revisit the non-goal before
    # bumping the constant.
    CSTRING_COUNT = 17

    def test_cstring_idiom_is_documented_non_goal(self):
        # Assert against the MODULE DOCSTRING OBJECT, not the raw file:
        # a file-text search is self-satisfied by this test's own
        # assertion strings.
        doc = sys.modules[__name__].__doc__ or ""
        self.assertIn("CString::new", doc,
                      "the CString::new non-goal note must stay in the docstring")
        self.assertIn(
            "Documented NON-GOAL -- the CString::new FFI idiom",
            doc,
            "the non-goal note must stay marked in the module docstring",
        )

    def test_cstring_idiom_not_required_to_consolidate(self):
        for key in list(HOMES) + list(CALLER_REQUIRED):
            self.assertNotIn(
                "cstring", key.lower(),
                "no single-home / caller-required key may cover the "
                "CString::new idiom: %r" % key,
            )

    def test_cstring_idiom_count_stable(self):
        count = 0
        for root, dirs, files in os.walk(REPO_ROOT):
            dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
            for f in files:
                if not f.endswith(".rs"):
                    continue
                p = os.path.join(root, f)
                code = strip_rust(
                    open(p, encoding="utf-8", errors="replace").read()
                )
                count += code.count("CString::new")
        self.assertEqual(
            count,
            self.CSTRING_COUNT,
            "CString::new site count drifted; revisit the documented "
            "non-goal before updating CSTRING_COUNT",
        )



if __name__ == "__main__":
    unittest.main()
