#!/usr/bin/env python3
"""Host-runnable regression gate for scan_rust.strip_rust itself.

Every source-contract test in tests/ scans Rust through strip_rust; a bug
in the stripping pipeline (the string-mask running after the comment pass,
an escaped quote breaking the mask, block comments leaking through) would
silently corrupt ALL of them at once. This file pins the stripping
behavior on synthetic snippets and on one real source file, so a stripping
regression fails HERE with a focused diagnosis instead of a confusing
downstream mismatch.

Run:  python3 tests/test_scan_rust.py
"""
import os
import re
import unittest

from scan_rust import strip_definition_lines, strip_rust

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TESTS_DIR = os.path.join(REPO_ROOT, "tests")
LOGIN_MANAGER_RS = os.path.join(REPO_ROOT, "login-manager", "src", "main.rs")


class StripRustBehaviorTest(unittest.TestCase):
    """Synthetic pattern pins for the three-pass pipeline."""

    def test_string_literals_masked(self):
        self.assertEqual(strip_rust('let s = "hello world";'), 'let s = "";')

    def test_escaped_quotes_inside_strings(self):
        # The mask must consume \" inside a literal without ending early.
        bs = chr(92)
        self.assertEqual(
            strip_rust('let s = "a ' + bs + '" b";'),
            'let s = "";',
        )

    def test_line_comments_stripped(self):
        out = strip_rust("let a = 1; // note\nlet b = 2;")
        self.assertNotIn("note", out)
        self.assertIn("let a = 1;", out)
        self.assertIn("let b = 2;", out)

    def test_block_comments_stripped_multiline(self):
        out = strip_rust("let a = 1; /* one\n two */ let b = 2;")
        self.assertNotIn("one", out)
        self.assertNotIn("two", out)
        self.assertIn("let a = 1;", out)
        self.assertIn("let b = 2;", out)

    def test_doc_comments_stripped(self):
        out = strip_rust("/// doc line\nfn f() {}")
        self.assertNotIn("doc", out)
        self.assertIn("fn f() {}", out)

    def test_string_mask_precedes_comment_pass(self):
        # A // inside a string literal must NOT become a comment start:
        # the string is masked first, so the comment pass cannot eat it.
        out = strip_rust('let s = "// not a comment";')
        self.assertNotIn("not a comment", out)
        self.assertIn('""', out)

    def test_block_comment_markers_inside_strings(self):
        out = strip_rust('let s = "/* still a string */";')
        self.assertNotIn("still a string", out)
        self.assertIn('""', out)

    def test_real_login_manager_invariants(self):
        # The downstream pins this helper feeds (return count, call
        # topology) must hold through the stripped REAL source.
        with open(LOGIN_MANAGER_RS, encoding="utf-8") as fh:
            code = strip_rust(fh.read())
        returns = re.findall(r"\breturn\b", code)
        self.assertEqual(
            len(returns), 3,
            "expected exactly 3 return sites (verify_password, window-create"
            " failure, successful execve) in login-manager/src/main.rs;"
            " found %d: %r" % (len(returns), returns),
        )
        self.assertEqual(code.count("note_failed_attempt("), 2)  # def + 1 call
        self.assertNotIn("process::exit", code)


    def test_strip_definition_lines_removes_only_definition_lines(self):
        src = (
            "pub fn read_line(history: &mut History, prompt: &str) -> String {\n"
            "    // interactive editor\n"
            "    String::new()\n"
            "}\n"
            "let called = read_line(&mut h, \"$ \");\n"
        )
        out = strip_definition_lines(strip_rust(src), [r"\bfn\s+read_line\b"])
        self.assertNotIn("fn read_line", out,
                         "definition line must be blanked")
        self.assertIn("read_line(&mut h", out,
                      "a genuine call site must survive the strip")
        self.assertNotIn("interactive", out)


class StripRustSingleHomeTest(unittest.TestCase):
    """The stripping logic must live in exactly one file: scan_rust.py."""

    def test_no_other_test_file_inlines_re_sub_stripping(self):
        offenders = []
        # Self-scan: this file's own "re.sub(" probe literal would flag
        # itself, so skip it like scan_rust.py.
        this = os.path.basename(__file__)
        for fn in sorted(os.listdir(TESTS_DIR)):
            if not fn.endswith(".py") or fn in ("scan_rust.py", this):
                continue
            with open(os.path.join(TESTS_DIR, fn), encoding="utf-8") as fh:
                if "re.sub(" in fh.read():
                    offenders.append(fn)
        self.assertEqual(
            offenders, [],
            "no test file may use re.sub() for comment/string stripping;"
            " move it into scan_rust.py: %s" % offenders,
        )

    def test_scan_rust_owns_the_mask_regex(self):
        with open(
            os.path.join(TESTS_DIR, "scan_rust.py"), encoding="utf-8"
        ) as fh:
            src = fh.read()
        self.assertIn("re.sub", src)


if __name__ == "__main__":
    unittest.main()
