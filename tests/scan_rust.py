#!/usr/bin/env python3
"""Comment/string-stripping helper for Rust source-contract tests.

Exports a single `strip_rust(src)` that masks string literals, then strips
line and block comments, so code-scan patterns (return counting, call-site
topology, guard adjacency) see only the real Rust tokens — not prose in
comments or user-facing strings.

Usage::

    from scan_rust import strip_rust

    src = open("login/src/main.rs").read()
    code = strip_rust(src)
    calls = code.count("note_failed_attempt(")
"""

import re


def strip_rust(src: str) -> str:
    """Strip comments and mask string literals from Rust source.

    Order matters: string literals are masked FIRST so that ``//`` and
    ``/*`` inside strings (e.g. ``\"// not a comment\"``) survive the
    comment pass. Line comments are stripped second, then block comments.
    Returns a string where double-quoted string literals become ``\"\"``
    and comments become empty; everything else is unchanged.  Rust char
    literals / lifetimes (single-quoted) are intentionally left intact.
    """
    code = re.sub(r'"(?:\\.|[^"\\])*"', '""', src)     # mask string literals
    code = re.sub(r"//[^\n]*", "", code)                  # strip line comments
    code = re.sub(r"/\*.*?\*/", "", code, flags=re.S)   # strip block comments
    return code

def strip_definition_lines(code: str, patterns) -> str:
    """Blank whole lines that DEFINE the given items.

    Used by caller/usage scans so a second home's DEFINITION does not count
    as a usage of the item (e.g. sash/src/readline.rs defines its own
    ``read_line``; the caller scan must not treat that line as a caller of
    libsarga's reader). Each pattern is a regex matched against a single
    line (``re.M``); the entire matching line is removed. Patterns must be
    mid-line regexes (no ``^``/``$`` anchors).
    """
    for pat in patterns:
        code = re.sub(r"^.*" + pat + r".*$", "", code, flags=re.M)
    return code
