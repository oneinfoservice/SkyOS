#!/usr/bin/env python3
"""Host-test coverage gate for libsarga's pure-logic modules (no QEMU).

The libsarga host-test gate (`cargo test -p libsarga --lib` in the CI
host-tests job) proves the `#[cfg(test)]` modules COMPILE and PASS; this
suite proves they are actually THERE. It asks the real host test binary for
its test list (`cargo test -p libsarga --lib -- --list`), groups the tests
by source module, prints the per-module counts, and fails if any
pure-logic module has zero tests — a `#[cfg(test)]` module that silently
stops running, or a new pure-logic module added without tests, fails here
instead of quietly shrinking the suite.

The pure-logic set is the errno/net/semver/hash/toml/png/theme group the
docs pin as host-testable (TESTING.md, docs/testing/unit_tests.md): modules
whose logic is syscall-free and therefore runnable under the std test
harness. Adding a new pure-logic module to libsarga means adding it to
REQUIRED_MODULES and giving it a `#[cfg(test)]` module — the gate then
enforces it.

Run:  python3 tests/test_libsarga_host_coverage.py
"""
import os
import re
import subprocess
import unittest

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CI_YML = os.path.join(REPO_ROOT, ".github", "workflows", "ci.yml")

# Pure-logic modules that must each have >= 1 host test. Keep in sync with
# the module list pinned in TESTING.md / docs/testing/unit_tests.md (the
# two must not drift).
REQUIRED_MODULES = [
    "errno",
    "fs",
    "gui",
    "hash",
    "net",
    "png",
    "semver",
    "theme",
    "toml",
]


def list_host_tests():
    """Ask the actual host test binary for its test list.

    Returns (module -> count, total). Parsing the compiled binary's --list
    (not the source) is deliberate: the gate must reflect what really runs
    on the host, so a #[cfg] that drops a module, or a #[test] that never
    compiles, is caught here rather than by an optimistic source scan.
    """
    proc = subprocess.run(
        ["cargo", "test", "-p", "libsarga", "--lib", "--", "--list"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=600,
    )
    if proc.returncode != 0:
        raise AssertionError(
            "`cargo test -p libsarga --lib -- --list` failed (exit %d) — "
            "the host build itself is broken:\n%s"
            % (proc.returncode, proc.stderr[-3000:])
        )
    counts = {}
    total = 0
    for line in proc.stdout.splitlines():
        # Rust test-harness --list lines look like `errno::tests::test_x: test`.
        m = re.match(r"^(\S+): test$", line)
        if not m:
            continue
        module = m.group(1).split("::")[0]
        counts[module] = counts.get(module, 0) + 1
        total += 1
    return counts, total


class LibsargaHostCoverage(unittest.TestCase):
    def test_every_required_module_has_tests(self):
        counts, total = list_host_tests()
        self.assertGreater(total, 0, "no tests listed — is the host build broken?")
        print("\nper-module host test counts (libsarga):")
        for mod in sorted(counts):
            marker = " REQUIRED" if mod in REQUIRED_MODULES else ""
            print("  %-12s %d%s" % (mod, counts[mod], marker))
        missing = [m for m in REQUIRED_MODULES if counts.get(m, 0) == 0]
        self.assertEqual(
            missing,
            [],
            "pure-logic modules with zero host tests: %s. A #[cfg(test)] "
            "module stopped running or a new pure-logic module was added "
            "without tests — the libsarga host-test gate must cover every "
            "pure-logic module." % (", ".join(missing) if missing else "<none>"),
        )

    def test_wired_into_host_tests_job(self):
        # This file must be run by the host-tests job (the thread pattern:
        # a contract suite that is never wired into CI catches nothing). A
        # future edit that drops the step fails the suite, not just CI.
        with open(CI_YML, encoding="utf-8") as f:
            ci = f.read()
        self.assertIn(
            "python3 tests/test_libsarga_host_coverage.py", ci,
            "host-tests job lost the libsarga host-test coverage step",
        )


if __name__ == "__main__":
    unittest.main()
