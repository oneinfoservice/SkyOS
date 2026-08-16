#!/usr/bin/env python3
"""Host-runnable unit tests for login's getty echo contract (no QEMU).

Pins `libsarga::tty` — echo_off / echo_on / ensure_echo / read_password /
read_line, the getty's echo + line-read contract, in its single userspace
home (shared with passwd):

  echo_off(fd)   -> TCGETS the current termios, save c_lflag, clear ECHO
                    (0x8) in c_lflag, TCSETS it back; returns the saved
                    lflag on success, None on ANY ioctl failure (so the
                    caller skips the restore and a bogus 0 can never
                    clobber real termios once the kernel implements TCSETS).
  echo_on(fd, l) -> TCGETS, set c_lflag = l, TCSETS; silently no-ops if
                    TCGETS fails.
  read_password  -> echo_off, read one line, echo_on(restore) on EVERY
                    read_line outcome (line / EOF / Err) — but ONLY if
                    echo_off returned Some. Rust's read_line Err is a
                    value (no `?`), so the restore precedes the Err
                    return: a bare-Enter or dropped-connection password
                    read never leaves the console echo-off.
  ensure_echo(fd)-> TCGETS, OR the ECHO bit into c_lflag, TCSETS; silent
                    no-op on failure. Called before the USERNAME read —
                    the username is read before echo_off runs, so login
                    must not depend on the kernel's default c_lflag.
  read_line(fd)  -> one byte per read (`[u8; 1]`); `\n` OR `\r` terminates
                    (terminator CONSUMED but NOT included); `read()==0` ->
                    Ok(None) (EOF: partial line discarded — distinct from
                    an empty line `Ok(Some([]))`); a read `Err` propagates.

Also pins passwd's echo discipline: passwd/src/main.rs hides its two
new-password reads through the shared libsarga::tty::read_password (no
local Termios copy), so the kernel's TCSETS store has exactly one
userspace mirror to verify against.

Also pins that the GUI login path (login-manager) never relies on
termios echo — its password field draws into a window (win.get_key /
win.draw_string), not the console tty.

The kernel's TCSETS is currently a no-op returning 0 (syscalls/mod.rs),
so this cannot be exercised in QEMU today — the host test is the only
execution of the contract. Two layers of pinning, same as
test_login_flow.py:

  1. A faithful Python port driven through an injectable fake ioctl
     channel, so TCGETS-failure / TCSETS-failure / bit-clear / field-
     preservation semantics are executed on the host.
  2. Source-contract pins that grep libsarga/src/tty.rs, login/src/main.rs
     and libsarga's ioctls module, so a drift in the Rust (const value,
     mask expression, restore-skip) fails CI before any boot.

Run:  python3 tests/test_login_echo.py
"""

import os
import re
import unittest

from scan_rust import strip_rust

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOGIN_RS = os.path.join(REPO_ROOT, "login", "src", "main.rs")
LIBSARGA_IO_RS = os.path.join(REPO_ROOT, "libsarga", "src", "io.rs")
LOGIN_MANAGER_RS = os.path.join(REPO_ROOT, "login-manager", "src", "main.rs")
PASSWD_RS = os.path.join(REPO_ROOT, "passwd", "src", "main.rs")
TTY_RS = os.path.join(REPO_ROOT, "libsarga", "src", "tty.rs")
INSTALLER_RS = os.path.join(REPO_ROOT, "installer", "src", "main.rs")

# ioctl request numbers (mirrors libsarga/src/io.rs ioctls module).
TCGETS = 0x5401
TCSETS = 0x5402

# ECHO bit in termios c_lflag (POSIX; pinned to libsarga/src/tty.rs).
ECHO = 0x8

CC_SIZE = 19


def _termios(**kw):
    """Zeroed Termios mirror (repr(C): 4 u32 + c_cc [u8; 19])."""
    t = {
        "c_iflag": 0,
        "c_oflag": 0,
        "c_cflag": 0,
        "c_lflag": 0,
        "c_cc": [0] * CC_SIZE,
    }
    t.update(kw)
    return t


class FakeTty:
    """In-memory tty device + ioctl channel.

    Behaves like the kernel's sys_ioctl: TCGETS copies the device state
    into the caller's buffer; TCSETS copies the caller's buffer into the
    device. `fail` is a set of request numbers that return Err(None).
    Records every request for call-order assertions.
    """

    def __init__(self, **state):
        self.dev = _termios(**state)
        self.fail = set()
        self.calls = []

    def ioctl(self, _fd, request, argp):
        self.calls.append(request)
        if request in self.fail:
            return None  # Err
        if request == TCGETS:
            argp.update(self.dev)
        elif request == TCSETS:
            self.dev.update(argp)
        return 0


# --- Faithful ports of login/src/main.rs --------------------------------
def echo_off(fd, ioctl):
    """Port of login::echo_off -> Option<u32>."""
    t = _termios()
    if ioctl(fd, TCGETS, t) is None:
        return None
    saved = t["c_lflag"]
    t["c_lflag"] &= ~ECHO
    if ioctl(fd, TCSETS, t) is None:
        return None
    return saved


def echo_on(fd, lflag, ioctl):
    """Port of login::echo_on -> () (silent no-op on TCGETS failure)."""
    t = _termios()
    if ioctl(fd, TCGETS, t) is None:
        return
    t["c_lflag"] = lflag
    ioctl(fd, TCSETS, t)


def ensure_echo(fd, ioctl):
    """Port of login::ensure_echo -> () (silent no-op on TCGETS failure)."""
    t = _termios()
    if ioctl(fd, TCGETS, t) is None:
        return
    t["c_lflag"] |= ECHO
    ioctl(fd, TCSETS, t)


class ByteSource:
    """One-byte-per-read tty model, mirroring login's `[u8; 1]` buffer.

    Each call yields the next byte; raises StopIteration at EOF (the port
    of read() returning 0); raises OSError when `error_at` (1-based read
    number) is hit (the port of read() returning an Err)."""

    def __init__(self, data=b"", error_at=None):
        self.data = data
        self.pos = 0
        self.error_at = error_at
        self.reads = 0

    def __call__(self, fd):
        self.reads += 1
        if self.error_at is not None and self.reads == self.error_at:
            raise OSError("read: EIO")
        if self.pos >= len(self.data):
            raise StopIteration
        b = self.data[self.pos]
        self.pos += 1
        return b


def read_line(fd, read_fn):
    """Port of login::read_line -> Option[bytes].

    `read_fn(fd)` yields the next byte (0..255) like the Rust 1-byte read
    loop; StopIteration = read()==0 (EOF), OSError = read Err (propagates,
    mirroring `?`). Returns None on EOF — the partial line is DISCARDED,
    exactly like Rust returning Ok(None) mid-accumulation; returns
    bytes(line) on '\\n' or '\\r' with the terminator NOT included (it was
    consumed by the read, never pushed).
    """
    buf = bytearray()
    while True:
        try:
            byte = read_fn(fd)
        except StopIteration:
            return None
        if byte == 0x0A or byte == 0x0D:
            return bytes(buf)
        buf.append(byte)


def read_password(fd, ioctl, read_line):
    """Port of login::read_password -> Result<Option<Vec<u8>>, Error>.

    The restore is skipped only when echo_off failed (non-tty fd), so a
    bogus lflag can never clobber real termios. The restore runs on EVERY
    read_line outcome — line, None (EOF), or a raised OSError (Err): Rust's
    read_line Err is a value, so the restore precedes the Err return. A
    bare-Enter or dropped-connection password read must not leave the
    console echo-off.
    """
    saved = echo_off(fd, ioctl)
    try:
        r = read_line(fd)
    except OSError:
        if saved is not None:
            echo_on(fd, saved, ioctl)
        raise
    if saved is not None:
        echo_on(fd, saved, ioctl)
    return r


class TestEchoOff(unittest.TestCase):
    def test_tcgets_failure_returns_none_no_tcsets(self):
        tty = FakeTty(c_lflag=0xB)
        tty.fail = {TCGETS}
        self.assertIsNone(echo_off(0, tty.ioctl))
        self.assertEqual(tty.calls, [TCGETS])  # no TCSETS attempted
        self.assertEqual(tty.dev["c_lflag"], 0xB)  # device untouched

    def test_tcsets_failure_returns_none(self):
        tty = FakeTty(c_lflag=0xB)
        tty.fail = {TCSETS}
        self.assertIsNone(echo_off(0, tty.ioctl))
        self.assertEqual(tty.calls, [TCGETS, TCSETS])
        self.assertEqual(tty.dev["c_lflag"], 0xB)  # store rejected

    def test_clears_echo_bit_and_returns_saved(self):
        tty = FakeTty(c_lflag=0xB)  # ISIG|ICANON|ECHO
        self.assertEqual(echo_off(0, tty.ioctl), 0xB)  # saved lflag
        self.assertEqual(tty.dev["c_lflag"], 0xB & ~ECHO)  # ECHO cleared
        self.assertEqual(tty.dev["c_lflag"] & ECHO, 0)  # ECHO bit cleared

    def test_other_fields_preserved(self):
        tty = FakeTty(
            c_iflag=0x100,
            c_oflag=0x2,
            c_cflag=0xBF,
            c_lflag=0xB,
            c_cc=[7] * CC_SIZE,
        )
        echo_off(0, tty.ioctl)
        self.assertEqual(tty.dev["c_iflag"], 0x100)
        self.assertEqual(tty.dev["c_oflag"], 0x2)
        self.assertEqual(tty.dev["c_cflag"], 0xBF)
        self.assertEqual(tty.dev["c_cc"], [7] * CC_SIZE)
        self.assertEqual(tty.dev["c_lflag"], 0xB & ~ECHO)

    def test_no_echo_bit_noop(self):
        # lflag without ECHO (e.g. kernel's advertised 0x5): clear is a no-op.
        tty = FakeTty(c_lflag=0x5)
        self.assertEqual(echo_off(0, tty.ioctl), 0x5)
        self.assertEqual(tty.dev["c_lflag"], 0x5)


class TestEchoOn(unittest.TestCase):
    def test_restores_saved_lflag_preserves_fields(self):
        tty = FakeTty(c_iflag=0x100, c_oflag=0x2, c_cflag=0xBF, c_lflag=0xB)
        saved = echo_off(0, tty.ioctl)
        echo_on(0, saved, tty.ioctl)
        self.assertEqual(tty.dev["c_lflag"], 0xB)  # fully restored
        self.assertEqual(tty.dev["c_iflag"], 0x100)
        self.assertEqual(tty.dev["c_oflag"], 0x2)
        self.assertEqual(tty.dev["c_cflag"], 0xBF)

    def test_tcgets_failure_silent_no_tcsets(self):
        tty = FakeTty(c_lflag=0xB)
        tty.fail = {TCGETS}
        echo_on(0, 0xB, tty.ioctl)  # must not panic / must not store
        self.assertEqual(tty.calls, [TCGETS])
        self.assertEqual(tty.dev["c_lflag"], 0xB)


class TestEnsureEcho(unittest.TestCase):
    def test_sets_echo_bit_preserves_fields(self):
        tty = FakeTty(c_iflag=0x100, c_oflag=0x2, c_cflag=0xBF, c_lflag=0x5)
        ensure_echo(0, tty.ioctl)
        self.assertEqual(tty.dev["c_lflag"], 0x5 | ECHO)  # ECHO set
        self.assertEqual(tty.dev["c_lflag"] & ECHO, ECHO)
        self.assertEqual(tty.dev["c_iflag"], 0x100)  # other fields preserved
        self.assertEqual(tty.dev["c_oflag"], 0x2)
        self.assertEqual(tty.dev["c_cflag"], 0xBF)

    def test_already_set_is_idempotent(self):
        tty = FakeTty(c_lflag=0xB)  # ECHO already on (kernel default)
        ensure_echo(0, tty.ioctl)
        self.assertEqual(tty.dev["c_lflag"], 0xB)  # unchanged

    def test_tcgets_failure_silent_no_tcsets(self):
        tty = FakeTty(c_lflag=0x5)
        tty.fail = {TCGETS}
        ensure_echo(0, tty.ioctl)
        self.assertEqual(tty.calls, [TCGETS])
        self.assertEqual(tty.dev["c_lflag"], 0x5)  # untouched

    def test_tcsets_failure_silent_noop(self):
        tty = FakeTty(c_lflag=0x5)
        tty.fail = {TCSETS}
        ensure_echo(0, tty.ioctl)
        self.assertEqual(tty.calls, [TCGETS, TCSETS])
        self.assertEqual(tty.dev["c_lflag"], 0x5)  # store rejected


class TestUsernameEchoFlow(unittest.TestCase):
    """The username read happens BEFORE echo_off: its ECHO must be set
    explicitly (ensure_echo), never assumed from the kernel default."""

    def test_username_echoes_then_password_hidden(self):
        tty = FakeTty(c_lflag=0x5)  # hostile default: ECHO NOT set
        ensure_echo(0, tty.ioctl)
        self.assertEqual(tty.calls, [TCGETS, TCSETS])
        seen_user = tty.dev["c_lflag"] & ECHO
        saved = echo_off(0, tty.ioctl)
        self.assertEqual(tty.calls, [TCGETS, TCSETS, TCGETS, TCSETS])
        seen_pw = tty.dev["c_lflag"] & ECHO
        echo_on(0, saved, tty.ioctl)
        self.assertNotEqual(seen_user, 0)  # username echoes
        self.assertEqual(seen_pw, 0)       # password hidden during read
        self.assertEqual(tty.dev["c_lflag"], 0x5 | ECHO)  # restored after


class TestReadLine(unittest.TestCase):
    """Faithful-port behavior of login::read_line (terminator handling)."""

    def test_terminates_on_newline_not_included(self):
        self.assertEqual(read_line(0, ByteSource(b"root\n")), b"root")

    def test_terminates_on_carriage_return_not_included(self):
        self.assertEqual(read_line(0, ByteSource(b"root\r")), b"root")

    def test_empty_line_is_some_empty_not_eof(self):
        # "\r" -> Ok(Some([])): an empty line is NOT EOF.
        self.assertEqual(read_line(0, ByteSource(b"\r")), b"")

    def test_eof_before_any_byte_is_none(self):
        self.assertIsNone(read_line(0, ByteSource(b"")))

    def test_eof_mid_line_discards_partial(self):
        # "roo" then EOF -> Ok(None): the partial line is discarded, not
        # returned as Some("roo").
        self.assertIsNone(read_line(0, ByteSource(b"roo")))

    def test_error_propagates_and_discards_partial(self):
        # A read Err mid-line propagates (mirrors `?`); the partial is lost.
        with self.assertRaises(OSError):
            read_line(0, ByteSource(b"ro", error_at=2))

    def test_line_assembled_across_many_reads(self):
        # The 1-byte loop assembles "root" from five single-byte reads.
        src = ByteSource(b"root\r")
        self.assertEqual(read_line(0, src), b"root")
        self.assertEqual(src.reads, 5)

    def test_terminator_consumed_next_read_continues(self):
        # "\r" is consumed by the first call (never pushed, never re-read):
        # the next read_line starts right after it.
        src = ByteSource(b"abc\rdef\n")
        self.assertEqual(read_line(0, src), b"abc")
        self.assertEqual(read_line(0, src), b"def")
        self.assertEqual(src.pos, 8)

    def test_binary_safe_nul_and_tab_preserved(self):
        # Only 0x0A/0x0D terminate; NUL (0x00) and other bytes are data —
        # EOF is StopIteration, never the byte value.
        self.assertEqual(
            read_line(0, ByteSource(b"a\x00b\tc\r")), b"a\x00b\tc"
        )

    def test_crlf_breaks_on_first_terminator(self):
        # '\r\n' breaks on the FIRST terminator: '\r' ends the line
        # (b"abc") and the '\n' then terminates an EMPTY second line.
        # The kernel's input-time \n->\r conversion makes CRLF
        # unreachable in the getty flow, so this documents the byte
        # loop's exact behavior rather than mandating it.
        src = ByteSource(b"abc\r\n")
        self.assertEqual(read_line(0, src), b"abc")
        self.assertEqual(read_line(0, src), b"")
        self.assertEqual(src.pos, 5)


class TestReadPassword(unittest.TestCase):
    def test_echo_off_during_read_restored_after(self):
        tty = FakeTty(c_lflag=0xB)
        seen_during_read = []

        def read_line(_fd):
            # Snapshot the device lflag while the password is being read:
            # it must be ECHO-cleared at that exact moment.
            seen_during_read.append(tty.dev["c_lflag"])
            return [b"skyos"]

        r = read_password(0, tty.ioctl, read_line)
        self.assertEqual(r, [b"skyos"])
        self.assertEqual(seen_during_read, [0xB & ~ECHO])  # hidden during read
        self.assertEqual(tty.dev["c_lflag"], 0xB)  # restored after

    def test_restore_skipped_when_echo_off_failed(self):
        # Non-tty fd (TCGETS fails): echo_off returns None -> no restore.
        tty = FakeTty(c_lflag=0xB)
        tty.fail = {TCGETS}
        r = read_password(0, tty.ioctl, lambda _fd: [b"x"])
        self.assertEqual(r, [b"x"])
        self.assertEqual(tty.calls, [TCGETS])  # echo_on never ran
        self.assertEqual(tty.dev["c_lflag"], 0xB)

    def test_restore_skipped_when_tcsets_fails(self):
        # TCSETS fails after a successful TCGETS: echo_off still returns
        # None, so read_password must skip echo_on -- the "bogus 0 can
        # never clobber real termios" clause of the Rust comment. The
        # device must be left exactly as it was (all-or-nothing syscall).
        tty = FakeTty(c_lflag=0xB)
        tty.fail = {TCSETS}
        r = read_password(0, tty.ioctl, lambda _fd: [b"x"])
        self.assertEqual(r, [b"x"])
        self.assertEqual(tty.calls, [TCGETS, TCSETS])  # echo_on never ran
        self.assertEqual(tty.dev["c_lflag"], 0xB)  # untouched, no clobber

    def test_restore_runs_on_eof(self):
        # Ok(None) from read_line (EOF / dropped connection): the restore
        # still runs — a bare-Enter password read must not leave the
        # console echo-off.
        tty = FakeTty(c_lflag=0xB)
        r = read_password(0, tty.ioctl, lambda _fd: None)  # None = Ok(None)
        self.assertIsNone(r)
        self.assertEqual(tty.dev["c_lflag"], 0xB)  # ECHO restored
        self.assertEqual(tty.calls, [TCGETS, TCSETS, TCGETS, TCSETS])

    def test_restore_runs_on_read_error(self):
        # Err from read_line: the restore runs BEFORE the error propagates
        # (Rust's read_line Err is a value, not a panic) — a dropped
        # connection mid-password must not leave the console echo-off.
        def boom(_fd):
            raise OSError("read: EIO")

        tty = FakeTty(c_lflag=0xB)
        with self.assertRaises(OSError):
            read_password(0, tty.ioctl, boom)
        self.assertEqual(tty.dev["c_lflag"], 0xB)  # ECHO restored
        self.assertEqual(tty.calls, [TCGETS, TCSETS, TCGETS, TCSETS])

    def test_restore_skipped_on_err_when_echo_off_failed(self):
        # Both failure modes together: echo_off failed (non-tty fd) AND
        # the read errored — the restore must be skipped (nothing was
        # changed, so nothing to restore) while the Err still propagates.
        def boom(_fd):
            raise OSError("read: EIO")

        tty = FakeTty(c_lflag=0xB)
        tty.fail = {TCGETS}
        with self.assertRaises(OSError):
            read_password(0, tty.ioctl, boom)
        self.assertEqual(tty.calls, [TCGETS])  # no echo_on
        self.assertEqual(tty.dev["c_lflag"], 0xB)  # untouched


class TestSourceContract(unittest.TestCase):
    """Grep-pins so a Rust drift fails CI before any boot."""

    @classmethod
    def setUpClass(cls):
        with open(LOGIN_RS, encoding="utf-8") as fh:
            cls.login = fh.read()
        with open(LIBSARGA_IO_RS, encoding="utf-8") as fh:
            cls.io = fh.read()
        with open(LOGIN_MANAGER_RS, encoding="utf-8") as fh:
            cls.login_manager = fh.read()
        with open(PASSWD_RS, encoding="utf-8") as fh:
            cls.passwd = fh.read()
        with open(TTY_RS, encoding="utf-8") as fh:
            cls.tty = fh.read()
        with open(INSTALLER_RS, encoding="utf-8") as fh:
            cls.installer = fh.read()

    def test_echo_const_is_0x8(self):
        self.assertIn("pub const ECHO: u32 = 0x8;", self.tty)

    def test_clear_is_bitwise_and_not(self):
        self.assertIn("t.c_lflag &= !ECHO;", self.tty)
        self.assertIn("let saved = t.c_lflag;", self.tty)
        self.assertIn("Some(saved)", self.tty)

    def test_restore_skipped_on_none(self):
        self.assertIn("if let Some(lflag) = saved {", self.tty)
        self.assertIn("echo_on(fd, lflag);", self.tty)

    def test_echo_on_sets_exact_lflag(self):
        self.assertIn("t.c_lflag = lflag;", self.tty)

    def test_ioctls_match_libsarga(self):
        # login must call through libsarga's ioctls module.
        self.assertIn("ioctls::TCGETS", self.tty)
        self.assertIn("ioctls::TCSETS", self.tty)
        # libsarga must keep the Linux termios numbers.
        self.assertIn("pub const TCGETS: u64 = 0x5401;", self.io)
        self.assertIn("pub const TCSETS: u64 = 0x5402;", self.io)

    def test_termios_layout_mirror(self):
        # repr(C), 4 u32 + c_cc [u8; 19] -- the kernel ABI both sides share.
        # Order is pinned, not just presence: reordering fields (e.g. c_cc
        # before c_lflag) would silently break the shared repr(C) layout
        # while all presence checks still pass.
        m = re.search(
            r"#\[repr\(C\)\]\npub struct Termios \{\n"
            r"\s+pub c_iflag: u32,\n"
            r"\s+pub c_oflag: u32,\n"
            r"\s+pub c_cflag: u32,\n"
            r"\s+pub c_lflag: u32,\n"
            r"\s+pub c_cc: \[u8; 19\],\n\}",
            self.tty,
        )
        self.assertIsNotNone(m, "Termios repr(C) field order/layout drifted")

    def test_termios_layout_size_contract(self):
        # Byte-level contract for the future kernel TCSETS store: the store
        # copies size_of::<Termios>() from the caller's buffer, so login's
        # struct must stay exactly 4 x u32 + [u8; 19]. A field added or a
        # c_cc resized would silently truncate (kernel reads only its own
        # size) or miss the new field — no compiler error, no boot failure.
        # Order itself is pinned by test_termios_layout_mirror; this pins
        # types + count + the computed sizes.
        m = re.search(r"struct Termios \{\n(.+?)\n\}", self.tty, re.S)
        self.assertIsNotNone(m, "Termios struct body not found")
        # Exactly one occurrence in the whole file: a second (drifting)
        # copy below the good one would otherwise evade the mirror pin,
        # the size pin, and the file-counting workspace scan.
        self.assertEqual(self.tty.count("struct Termios"), 1,
                         "libsarga::tty must define exactly one Termios struct")
        fields = re.findall(r"\s+pub (c_[a-z]+): (u32|\[u8; (\d+)\])", m.group(1))
        u32s = [f for f in fields if f[1] == "u32"]
        ccs = [f for f in fields if f[1].startswith("[")]
        self.assertEqual(
            [f[0] for f in u32s],
            ["c_iflag", "c_oflag", "c_cflag", "c_lflag"],
            "exactly four u32 fields in order",
        )
        self.assertEqual(len(ccs), 1, "exactly one c_cc array")
        self.assertEqual(ccs[0][2], "19", "c_cc must stay [u8; 19]")
        data = 4 * 4 + 19  # 35 data bytes
        padded = (data + 3) & ~3  # repr(C) aligns struct size to 4 -> 36
        self.assertEqual((data, padded), (35, 36))

    def test_single_userspace_termios_definition(self):
        # The audit's drift risk: if a new userspace crate defines its own
        # Termios, the kernel TCSETS store can no longer be verified against
        # one mirror. Pin that exactly ONE userspace file defines it
        # (libsarga/src/tty.rs — the shared home for the auth binaries).
        # ./kernel/ is an untracked local kernel copy, not a userspace crate
        # and not in the repo.
        hits = []
        skip = {
            "target",
            "target-ws",
            ".git",
            ".freebuff",
            "__pycache__",
            "archive",
            "1",
            "SkyOS",
            "kernel",
        }
        for root, dirs, files in os.walk(REPO_ROOT):
            dirs[:] = [d for d in dirs if d not in skip]
            for fn in files:
                if not fn.endswith(".rs"):
                    continue
                p = os.path.join(root, fn)
                with open(p, encoding="utf-8", errors="replace") as fh:
                    if "struct Termios" in fh.read():
                        hits.append(os.path.relpath(p, REPO_ROOT).replace("\\", "/"))
        self.assertEqual(
            sorted(hits),
            ["libsarga/src/tty.rs"],
            "exactly one userspace file may define Termios (libsarga::tty)",
        )

    def test_passwd_uses_shared_tty_discipline(self):
        # passwd hides two new-password reads from the console tty with the
        # same echo_off/echo_on discipline as login's getty password read,
        # so the kernel's future ECHO-on-read cannot leak them to serial.
        # passwd hides its two new-password reads through the shared
        # libsarga::tty::read_password — it defines no local echo discipline
        # anymore (single home, pinned by test_single_userspace_termios_definition).
        self.assertIn("use libsarga::tty::read_password;", self.passwd)
        self.assertNotIn("fn echo_off", self.passwd)
        self.assertNotIn("fn echo_on", self.passwd)
        self.assertNotIn("fn read_password", self.passwd)
        self.assertNotIn("fn read_line", self.passwd)
        self.assertNotIn("struct Termios", self.passwd)

    def test_passwd_both_reads_wrapped(self):
        # Both interactive reads ("New password:" / "Retype new password:")
        # must go through read_password — no bare read_line(0) may remain
        # at the call sites (read_password's internal read_line(fd) is fine).
        self.assertEqual(self.passwd.count("read_password(0)"), 2,
                         "exactly the two password reads must use read_password(0)")
        self.assertNotIn("read_line(0)", self.passwd,
                         "no bare read_line(0) may survive in passwd")

    def test_passwd_restores_before_returning(self):
        # The shared restore contract (libsarga::tty::read_password):
        # echo_off -> read_line -> restore on every outcome (only if
        # echo_off succeeded) -> return r. passwd consumes this function.
        m = re.search(
            r"pub fn read_password\(fd: i64\) -> Result<Option<Vec<u8>>, Error> \{\n(.+?)\n\}",
            self.tty,
            re.S,
        )
        self.assertIsNotNone(m, "libsarga::tty read_password body not found")
        body = m.group(1)
        # Presence guards first: a missing step must trip a clear
        # AssertionError, not a ValueError from .index() below.
        self.assertIn("let saved = echo_off(fd);", body)
        self.assertIn("let r = read_line(fd);", body)
        self.assertIn("echo_on(fd, lflag);", body)
        self.assertLess(body.index("let saved = echo_off(fd);"),
                        body.index("let r = read_line(fd);"))
        self.assertLess(body.index("let r = read_line(fd);"),
                        body.index("if let Some(lflag) = saved {"))
        self.assertLess(body.index("if let Some(lflag) = saved {"),
                        body.index("echo_on(fd, lflag);"))
        self.assertLess(body.index("echo_on(fd, lflag);"),
                        body.index("\n    r"))
        # The restore must be conditional on echo_off succeeding, so a bogus
        # 0 can never clobber real termios once the kernel implements TCSETS.
        self.assertIn("if let Some(lflag) = saved {", body)

    def test_auth_binaries_define_no_local_termios(self):
        # The single Termios layout lives in libsarga::tty; neither auth
        # binary defines its own copy anymore, so the byte-identical-across-
        # binaries risk is gone — there is one home, pinned by
        # test_termios_layout_mirror / test_termios_layout_size_contract /
        # test_single_userspace_termios_definition.
        self.assertNotIn("struct Termios", self.login)
        self.assertNotIn("struct Termios", self.passwd)

    def test_ensure_echo_sets_bit_and_precedes_username_read(self):
        # The username read runs BEFORE echo_off (only the password is
        # hidden), so login must set ECHO explicitly rather than depend on
        # the kernel's default c_lflag (0xB). Pin helper + call site: the
        # call sits between the "login: " prompt and the read_line(0) in
        # the interactive arm (comment lines in between are skipped).
        self.assertIn("pub fn ensure_echo(fd: i64) {", self.tty)
        self.assertIn("t.c_lflag |= ECHO;", self.tty)
        m = re.search(
            r'None => \{\n'
            r'\s*io::print_str\("login: "\);\n'
            r'(?:\s*//[^\n]*\n)*'
            r'\s*ensure_echo\(0\);\n'
            r'\s*let name_bytes = match read_line\(0\) \{',
            self.login,
        )
        self.assertIsNotNone(m, "ensure_echo must precede the username read")

    def test_ensure_echo_only_in_interactive_arm(self):
        # Only the interactive (None =>) username arm calls ensure_echo —
        # the fixed_user (Some(u) =>) arm never reads a username, so it
        # must not call it either. (The helper itself lives in
        # libsarga::tty; the "defined before use" ordering moved with it.)
        some_arm = self.login[
            self.login.index("Some(u) =>") : self.login.index("None => {")
        ]
        self.assertNotIn("ensure_echo", some_arm)
        none_arm = self.login[self.login.index("None => {"):]
        self.assertIn("ensure_echo(0);", none_arm)

    def test_read_line_signature_and_terminator_contract(self):
        # Ok(None) on EOF (distinct from an empty Ok(Some([]))) and the
        # 1-byte buffer (lines assemble across many reads).
        self.assertIn("fn read_line(fd: i64) -> Result<Option<Vec<u8>>, Error> {", self.tty)
        self.assertIn("let mut byte = [0u8; 1];", self.tty)
        self.assertIn("let n = read(fd, &mut byte)?;", self.tty)
        self.assertIn("if n == 0 {", self.tty)
        self.assertIn("return Ok(None);", self.tty)
        self.assertIn("if byte[0] == b'\\n' || byte[0] == b'\\r' {", self.tty)
        self.assertIn("break;", self.tty)
        # Exactly one push in the whole file: a buggy read_line that
        # pushes before the terminator check AND again after would
        # otherwise still satisfy the order regex.
        self.assertEqual(
            self.tty.count("buf.push(byte[0]);"),
            1,
            "read_line must contain exactly one buf.push(byte[0]);",
        )

    def test_read_line_order_eof_terminator_push(self):
        # Order is the contract: EOF check, THEN terminator check, THEN
        # push — the terminator must never be pushed and EOF discards.
        m = re.search(
            r"fn read_line\(fd: i64\) -> Result<Option<Vec<u8>>, Error> \{.*?loop \{\n"
            r"\s+let n = read\(fd, &mut byte\)\?;\n"
            r"\s+if n == 0 \{\n"
            r"\s+return Ok\(None\);\n"
            r"\s+\}\n"
            r"\s+if byte\[0\] == b'\\n' \|\| byte\[0\] == b'\\r' \{\n"
            r"\s+break;\n"
            r"\s+\}\n"
            r"\s+buf\.push\(byte\[0\]\);",
            self.tty,
            re.S,
        )
        self.assertIsNotNone(m, "read_line order drifted: EOF -> terminator -> push")

    def test_single_read_line_definition(self):
        self.assertEqual(self.tty.count("fn read_line(fd: i64)"), 1)

    def test_read_password_restores_before_returning(self):
        # The restore runs on EVERY read_line outcome when echo_off
        # succeeded: read_line's Err is a VALUE (no `?`), so echo_on
        # precedes the return of r. A dropped-connection password read
        # must not leave the console echo-off.
        m = re.search(
            r"fn read_password\(fd: i64\) -> Result<Option<Vec<u8>>, Error> \{\n"
            r"\s+let saved = echo_off\(fd\);\n"
            r"\s+let r = read_line\(fd\);\n"
            r"\s+if let Some\(lflag\) = saved \{\n"
            r"\s+echo_on\(fd, lflag\);\n"
            r"\s+\}\n"
            r"\s+r\n"
            r"\}",
            self.tty,
        )
        self.assertIsNotNone(m, "read_password must restore before returning r")
        # A `?` on the read_line call would propagate Err before the
        # restore — banned.
        self.assertNotIn("read_line(fd)?", self.tty)
        self.assertEqual(self.tty.count("echo_on(fd, lflag);"), 1)

    def test_gui_login_does_not_rely_on_termios_echo(self):
        # login-manager's password field draws into a window
        # (win.get_key() input, win.draw_string output) — it must never
        # depend on tty termios echo. No TCGETS/TCSETS/ioctl/ECHO/Termios
        # anywhere and no tty read() at all: a future edit routing its
        # input through the console tty would bring the kernel ECHO
        # contract into play — and this pin fails CI first.
        lm = self.login_manager
        # The banned-word scan runs on comment/string-STRIPPED source
        # (same helper test_login_flow.py uses): a doc comment mentioning
        # "ECHO"/"ioctl"/"Termios" must not false-fail the pin — only
        # real code uses do.
        lm_code = strip_rust(lm)
        self.assertIn("win.get_key()", lm)  # GUI key pipeline, not the tty
        self.assertIn("win.draw_string", lm)  # GUI-drawn, not console echo
        for banned in ("TCGETS", "TCSETS", "ioctl", "ECHO", "echo_off",
                       "echo_on", "Termios"):
            self.assertNotIn(
                banned, lm_code, f"login-manager must not reference {banned}"
            )
        # No tty reads in code: neither idiomatic read(0, ...) nor
        # libsarga's io::read (the import surface permits it).
        self.assertNotIn("read(0", lm_code)
        self.assertNotIn("io::read", lm_code)

    def test_gui_installer_password_uses_window_pipeline(self):
        # installer's password entry is the same GUI contract as
        # login-manager's: win.get_key() input, star-masked draw_string
        # output -- never the console tty. A future edit routing it
        # through read_password/read_line/io::read would bring the
        # kernel ECHO contract into play -- and this pin fails CI first.
        ins = self.installer
        ins_code = strip_rust(ins)
        self.assertIn("win.get_key()", ins)  # GUI key pipeline, not the tty
        self.assertIn('"*".repeat(password.len())', ins)  # masked stars
        self.assertIn("draw_string", ins)  # GUI-drawn, not console echo
        for banned in ("TCGETS", "TCSETS", "ioctl", "ECHO", "echo_off",
                       "echo_on", "Termios", "read_password", "read_line"):
            self.assertNotIn(
                banned, ins_code, f"installer must not reference {banned}"
            )
        # No tty reads in code: neither idiomatic read(0, ...) nor
        # libsarga's io::read (the import surface permits io::MouseState).
        self.assertNotIn("read(0", ins_code)
        self.assertNotIn("io::read", ins_code)

if __name__ == "__main__":
    unittest.main()
