#!/usr/bin/env python3
"""Host-runnable unit tests for the console login flow (no QEMU required).

Validates the initrd credential content built by build_initrd.py against the
exact parsing and verification semantics of the userspace login stack:

  /bin/login  (login/src/main.rs)
    lookup_user       -> parses /etc/passwd  (name:x:uid:gid:gecos:home:shell)
    verify_password   -> /etc/shadow via libsarga::hash::verify_password
                         (PBKDF2-HMAC-SHA256; salt hex after "PBKDF2-",
                          dk hex, optional ":<iterations>")

The shadow verifier in libsarga runs the PBKDF2 inside the kernel (SYS_HASH),
so it cannot execute on the host; this test ports its exact parse/verify shape
to hashlib and checks the *real* initrd constants (imported from build_initrd.py
so the test cannot drift), plus source-contract pins for login's execve argv[0]
change.

Run:  python3 tests/test_login_flow.py
"""
import hashlib
import io
import os
import re
import subprocess
import sys
import unittest

from scan_rust import strip_rust
from constants import MAX_RESPAWNS

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, REPO_ROOT)
import build_initrd  # noqa: E402 -- real constants, must come after sys.path

LOGIN_RS = os.path.join(REPO_ROOT, "login", "src", "main.rs")
LOGIN_MANAGER_RS = os.path.join(REPO_ROOT, "login-manager", "src", "main.rs")
AUTH_RS = os.path.join(REPO_ROOT, "libsarga", "src", "auth.rs")
LIBSARGA_IO_RS = os.path.join(REPO_ROOT, "libsarga", "src", "io.rs")
GUI_LOGIN_EXP = os.path.join(REPO_ROOT, "tests", "qemu_gui_login.exp")
LIBSARGA_LIB_RS = os.path.join(REPO_ROOT, "libsarga", "src", "lib.rs")
LIBSARGA_HASH_RS = os.path.join(REPO_ROOT, "libsarga", "src", "hash.rs")
INIT_RS = os.path.join(REPO_ROOT, "init", "src", "main.rs")
SESSION_RS = os.path.join(REPO_ROOT, "ade", "src", "service", "session.rs")
ADE_MAIN_RS = os.path.join(REPO_ROOT, "ade", "src", "main.rs")
CARGO_CONFIG = os.path.join(REPO_ROOT, ".cargo", "config.toml")
SARGA_JSON = os.path.join(REPO_ROOT, "x86_64-sarga.json")


def _hex(s):
    """hex::decode equivalent: None on invalid/odd input."""
    try:
        return bytes.fromhex(s.decode("ascii"))
    except (ValueError, UnicodeDecodeError):
        return None


def verify_password(shadow_data, username, password):
    """Byte-exact port of libsarga/src/hash.rs::verify_password.

    Accepts only PBKDF2-HMAC-SHA256 entries (other schemes are rejected for
    security); returns False for a missing user or a malformed entry.
    """
    for line in shadow_data.split(b"\n"):
        if not line:
            continue
        name, _, rest = line.partition(b":")
        if name != username.encode("utf-8"):
            continue
        if not rest.startswith(b"PBKDF2-"):
            return False
        salt_hex, _, rest3 = rest[len(b"PBKDF2-"):].partition(b":")
        salt = _hex(salt_hex)
        if salt is None or len(salt) != 16:
            return False
        if b":" in rest3:
            dk_hex, _, iter_str = rest3.partition(b":")
            try:
                iterations = int(iter_str.decode("ascii"))
            except (ValueError, UnicodeDecodeError):
                iterations = 10000
        else:
            dk_hex, iterations = rest3, 10000
            # NOTE (port divergence, crafted inputs only): Rust parses with
            # `iter_str.parse::<u32>().unwrap_or(10000)`; Python int() differs on
            # overflow ("99999999999999999999" -> Rust falls back to 10000) and
            # leading whitespace. Real shadow aging fields contain ':', so both
            # fall back to 10000; divergence is only reachable on crafted data.
        stored = _hex(dk_hex)
        if stored is None or len(stored) != 32:
            return False
        dk = hashlib.pbkdf2_hmac("sha256", password.encode("utf-8"), salt, iterations)
        return dk == stored
    return False


def lookup_shell(passwd_data, username):
    """Port of login/src/main.rs::lookup_user -- returns the shell field."""
    for line in passwd_data.split(b"\n"):
        if not line:
            continue
        parts = line.split(b":")
        if len(parts) == 7 and parts[0] == username.encode("utf-8"):
            return parts[6].decode("utf-8")
    return None


class TestLoginFlow(unittest.TestCase):
    SHADOW = build_initrd.SHADOW_CONTENT.encode("utf-8")
    PASSWD = build_initrd.PASSWD_CONTENT.encode("utf-8")

    # --- credential verification (the QEMU console login depends on this) ---
    def test_root_skyos_verifies(self):
        self.assertTrue(
            verify_password(self.SHADOW, "root", "skyos"),
            "root/skyos must verify against the initrd shadow",
        )

    def test_wrong_passwords_fail(self):
        for bad in ("root", "SKYOS", "skyos1", "skyos ", ""):
            with self.subTest(password=bad):
                self.assertFalse(verify_password(self.SHADOW, "root", bad))

    def test_unknown_user_fails(self):
        self.assertFalse(verify_password(self.SHADOW, "nobody", "skyos"))

    def test_non_pbkdf2_entry_rejected(self):
        self.assertFalse(verify_password(b"root:des-pw:hash\n", "root", "x"))
        self.assertFalse(verify_password(b"root:$6$salt$hash\n", "root", "x"))

    def test_malformed_entries_fail(self):
        self.assertFalse(verify_password(b"root:PBKDF2-0000\n", "root", "x"))
        self.assertFalse(
            verify_password(
                # (15-byte salt = the old SKYOSDESTOPSALT shape; rejected by the 16-byte
                # guard - doubles as a regression pin for the salt-length fix)
                b"root:PBKDF2-534b594f53444553544f5053414c54:zz\n", "root", "x"
            )
        )

    # --- the stored hash is literally PBKDF2("skyos", SKYOSDESKTOPSALT, 10000) ---
    # (16-byte salt is REQUIRED: libsarga verify_password enforces s.len() == 16)
    def test_stored_hash_is_pbkdf2_of_skyos(self):
        fields = self.SHADOW.strip().split(b":")
        self.assertEqual(fields[0], b"root")
        self.assertTrue(fields[1].startswith(b"PBKDF2-"))
        salt = bytes.fromhex(fields[1][len(b"PBKDF2-"):].decode("ascii"))
        self.assertEqual(salt, b"SKYOSDESKTOPSALT")
        stored = bytes.fromhex(fields[2].decode("ascii"))
        self.assertEqual(len(stored), 32)
        iterations = int(fields[3].decode("ascii"))
        self.assertEqual(iterations, 10000)
        self.assertEqual(
            hashlib.pbkdf2_hmac("sha256", b"skyos", salt, iterations), stored
        )

    # --- passwd -> shell (login execs this as argv[0]) ---
    def test_passwd_root_shell_is_sash(self):
        self.assertEqual(lookup_shell(self.PASSWD, "root"), "/bin/sash")

    def test_passwd_root_fields(self):
        parts = self.PASSWD.strip().split(b":")
        self.assertEqual(len(parts), 7)
        self.assertEqual((parts[2], parts[3]), (b"0", b"0"))  # uid, gid

    # --- source-contract pins for login's execve argv[0] change ---
    def test_getty_prints_memory_pressure_marker(self):
        # The console getty must print the same boot-time memory-pressure
        # marker at startup that login-manager prints before Window::create
        # (login-manager/src/main.rs:53-59): the same ctlFS node
        # (/ctl/sys/mem/free), the same '[login] mem free={} pages' shape,
        # and the same unavailable arm. The shell-interaction harness
        # (qemu_shell_test.exp) greps this marker from the accumulated boot
        # log, so the shell job collects the same per-boot OOM evidence the
        # GUI gate does (kernel-gui-window-fix.md Option 1 vs 2). The getty
        # never exits (mistype re-prompt loop), so it prints once per boot.
        src = open(LOGIN_RS, encoding="utf-8").read()
        # 1. The ctlFS read + marker shape (matches login-manager exactly).
        self.assertIn('libsarga::fs::read_to_string("/ctl/sys/mem/free")', src)
        self.assertIn('[login] mem free={} pages\\n', src)
        self.assertIn('"[login] mem free=unavailable\\n"', src)
        # 2. Placement: at getty startup, BEFORE the interactive login loop
        #    (so it prints once per boot before the first 'login: ' prompt,
        #    and is inside the accumulated buffer the harness greps after
        #    the prompt). The needle is the EXECUTABLE form 'mem free={}
        #    pages' (only in the print_str call) - the bare 'mem free' is
        #    also in the marker's own comment block, so it would keep the
        #    ordering assert satisfied after a move of only the match.
        self.assertLess(
            src.index('mem free={} pages'),
            src.index("let mut failures: u32 = 0;"),
            "getty mem marker must print before the interactive login loop",
        )

    def test_login_execve_argv0_is_the_shell(self):
        with open(LOGIN_RS, encoding="utf-8") as fh:
            src = fh.read()
        # argv[0] must be the shell from /etc/passwd (the change that stopped
        # empty-argv argv/opt scans from misbehaving).
        self.assertIn("execve(shell_name, &[shell_name], &env_refs)", src)
        self.assertIn(
            'let shell_name = core::str::from_utf8(&_shell).unwrap_or("/bin/sash");',
            src,
        )

    def test_login_reads_etc_passwd_and_shadow(self):
        with open(LOGIN_RS, encoding="utf-8") as fh:
            src = fh.read()
        self.assertIn('const PASSWD_PATH: &str = "/etc/passwd";', src)
        self.assertIn('const SHADOW_PATH: &str = "/etc/shadow";', src)
        self.assertIn("verify_password(&username, &password)", src)

    # --- source-contract pins for login-manager's GUI failure path ---
    # A bad password must re-prompt in the window, never exit: the GUI session
    # is init service "login-manager" with respawn: true, so an auth-failure
    # exit would burn MAX_RESPAWNS the same way the console getty's would
    # have. (The console getty got the same treatment: it loops in place.)
    def test_gui_login_bad_password_reprompts_not_exits(self):
        with open(LOGIN_MANAGER_RS, encoding="utf-8") as fh:
            src = fh.read()
        # The Enter arm on verify failure must set the error + clear the
        # password buffer and fall through to re-render — no return/exit.
        self.assertIn('"Invalid username or password"', src)
        self.assertIn("password_buf.clear()", src)
        # Exit inventory (verified Aug 8, 2026) — exactly 3 `return`s:
        #   1. verify_password helper: `Err(_) => return false` (returns bool,
        #      not an exit from user_main)
        #   2. user_main: window-create failure -> `return 0`
        #   3. user_main: successful execve -> `return 0` (never returns)
        # plus no process::exit / panic! / .unwrap(), so NO auth-failure path
        # can exit user_main and burn init's MAX_RESPAWNS. Counting on
        # comment/string-stripped code catches `return 1`/`return -1`/`return
        # code` that a literal "return 0" search would miss, and comment/string
        # stripping stops innocent prose from false-tripping the count.
        code = strip_rust(src)
        self.assertEqual(
            len(re.findall(r"\breturn\b", code)),
            3,
            "login-manager exit inventory changed — update this pin deliberately",
        )
        self.assertNotIn("process::exit", src)  # covers libsarga::process::exit
        self.assertNotIn("panic!", code)
        self.assertNotIn(".unwrap()", code)



class TestAttemptCapContract(unittest.TestCase):
    """Source-contract pins for the getty attempt cap / backoff.

    login/src/main.rs throttles failed logins so a brute-forcer or stuck
    terminal cannot hammer the PBKDF2 verify (10k iterations each) at full
    speed, while the getty itself never exits (the MAX_RESPAWNS fix depends
    on that). These pins guard the constants and the exact call topology so
    a change to the throttle surfaces in CI before any QEMU boot.
    """

    @classmethod
    def setUpClass(cls):
        with open(LOGIN_RS, encoding="utf-8") as fh:
            cls.src = fh.read()
        with open(AUTH_RS, encoding="utf-8") as fh:
            cls.auth = fh.read()
        cls.code = strip_rust(cls.src)

    def test_max_failed_attempts_is_10(self):
        self.assertIn("pub const MAX_FAILED_ATTEMPTS: u32 = 10;", self.auth)

    def test_backoff_ns_is_30_seconds(self):
        # 30_000_000_000 ns == 30 s. The literal itself is the pin; the
        # arithmetic below just documents the unit conversion for the reader.
        self.assertIn("pub const BACKOFF_NS: u64 = 30_000_000_000;", self.auth)
        self.assertEqual(30_000_000_000, 30 * 1_000_000_000)

    def test_note_failed_attempt_increments_then_resets_after_pause(self):
        # Counter increments; only disarmed when the pause actually happened
        # (EINTR must not skip the backoff and re-arm the next burst).
        self.assertIn("*failures += 1;", self.code)
        self.assertIn("if *failures >= MAX_FAILED_ATTEMPTS {", self.code)
        self.assertIn("Too many failed attempts - pausing 30s", self.src)
        self.assertIn("if io::nanosleep(BACKOFF_NS).is_ok() {", self.code)
        self.assertIn("*failures = 0;", self.code)

    def test_note_failed_attempt_called_on_exactly_three_paths(self):
        # Exactly the three interactive failure paths count an attempt:
        #   (1) unknown user, (2) invalid password encoding, (3) bad password.
        # The getty NEVER exits on these — it counts, then `continue`s.
        calls = re.findall(r"note_failed_attempt\(&mut failures\);", self.code)
        self.assertEqual(len(calls), 3, "note_failed_attempt call count changed")

    def test_three_paths_are_the_expected_ones(self):
        # Pin each failure path's message next to its call site: unknown user,
        # invalid encoding, bad password. The messages are multi-line Rust
        # strings (real newlines), so match them tolerantly inside print_str.
        for msg in ("login: unknown user", "Invalid password encoding", "Login incorrect"):
            self.assertRegex(self.src, r'print_str\(\s*"[\n]*' + re.escape(msg) + r'[\n]*"')
        # Every call is an INTERACTIVE-only path: guarded by the
        # fixed_user.is_some() early-return (scripted `login <user>` exits
        # without counting) and followed by `continue;` (re-prompt, never a
        # return). Both the guard and the re-prompt must sit with each call,
        # or getty-vs-scripted semantics silently change.
        code = self.code
        for _ in range(3):
            i = code.index("note_failed_attempt(&mut failures);")
            before = code[:i]
            # The guard immediately precedes the call (closing brace + guard).
            m = re.search(r"if fixed_user\.is_some\(\) \{\s*return 1;\s*\}\s*$", before)
            self.assertIsNotNone(
                m,
                "note_failed_attempt must be guarded by fixed_user.is_some() "
                "early-return (interactive-only path)",
            )
            tail = code[i + len("note_failed_attempt(&mut failures);"):]
            self.assertTrue(
                tail.lstrip().startswith("continue;"),
                "note_failed_attempt must be followed by continue; (re-prompt)",
            )
            code = tail
        # Exactly three interactive-only guards total (no stray early-returns).
        self.assertEqual(
            len(re.findall(r"if fixed_user\.is_some\(\) \{\s*return 1;", self.code)),
            3,
            "fixed_user.is_some() early-return count changed",
        )



    def test_fixed_user_guard_exits_non_zero(self):
        # The fixed_user (scripted `login <user>`) path exits with return 1
        # on every authentication failure — unknown user, invalid password
        # encoding, bad password. Exit code 1 means init's waitpid sees
        # status != 0, so crashes accumulate (svc.crashes += 1) and
        # eventually MAX_RESPAWNS gives up. If any guard used return 0
        # instead, init would reset crashes and respawn forever — masking
        # the failure. (Interactive getty: no argv, fixed_user is None, so
        # these guards are never reached; note_failed_attempt runs instead.)
        # Count on stripped code (self.code) so the regex is clean.
        guards = re.findall(
            r"if fixed_user\.is_some\(\) \{\s*return 1;\s*\}",
            self.code,
        )
        self.assertEqual(
            len(guards), 3,
            "must be exactly 3 fixed_user.is_some() -> return 1 guards "
            "(one per failure path: unknown user, invalid encoding, bad pw)",
        )
        # No alternative exit code (return 0) appears in any guard.
        self.assertNotIn("return 0;", self.code[self.code.index("fixed_user"):])

    def test_fixed_user_guard_before_note_failed_attempt(self):
        # Each `if fixed_user.is_some() { return 1; }` guard must appear
        # BEFORE its corresponding `note_failed_attempt(&mut failures);`
        # call — a scripted login exits without counting an attempt.
        # Check this on stripped code (self.code) so we compare against
        # the CALL SITE, not the function definition (which comes first).
        code = self.code
        for _ in range(3):
            i_note = code.index("note_failed_attempt(&mut failures);")
            before = code[:i_note]
            i_guard = before.rfind("if fixed_user.is_some()")
            self.assertGreaterEqual(
                i_guard, 0,
                "each note_failed_attempt call must be preceded by a "
                "fixed_user guard",
            )
            i_return = before.rfind("return 1;")
            self.assertGreaterEqual(
                i_return, 0,
                "each note_failed_attempt call must be preceded by return 1",
            )
            # The guard and return must come before the call.
            self.assertLess(i_guard, i_note)
            self.assertLess(i_return, i_note)
            code = code[i_note + len("note_failed_attempt(&mut failures);"):]

    def test_bare_enter_does_not_count(self):
        # A bare Enter re-prompts WITHOUT consuming an attempt: the empty-name
        # arm must `continue` without calling note_failed_attempt.
        code = self.code
        start = code.index("if name_bytes.is_empty() {")
        arm = code[start:]
        cut = arm.index("continue;")  # raises if the arm has no re-prompt
        self.assertNotIn("note_failed_attempt", arm[:cut])



class TestGuiAttemptCapContract(unittest.TestCase):
    """Source-contract pins for login-manager's GUI attempt cap / backoff.

    The GUI password field now has the same failed-attempt accounting as the
    console getty (login/src/main.rs): 10 bad logins -> a 30 s window message
    + serial announce + backoff, then re-prompt. The backoff runs AFTER the
    frame with the message is flushed (so it stays visible for the whole
    pause), and only disarms when the pause actually ran (EINTR must not
    skip it). A successful disarm also clears the pause message from the
    window, so the next attempt shows the plain "Invalid username or
    password" instead of the stale cap message. A stray Enter with an empty
    username re-prompts without
    counting, matching the getty. The session NEVER exits on bad creds —
    init service "login-manager" has respawn: true, so an exit would burn
    MAX_RESPAWNS. These pins keep the throttle and the re-prompt semantics
    visible in CI before any QEMU boot. The console getty (login/src/main.rs)
    has the SAME throttle constants (10/30 s) — pinned in lockstep by
    test_gui_and_console_throttle_constants_agree, so a brute-forcer cannot
    switch to the weaker path; the only asymmetry left is call topology
    (console counts 3 failure paths, GUI counts 1).
    """

    @classmethod
    def setUpClass(cls):
        with open(LOGIN_MANAGER_RS, encoding="utf-8") as fh:
            cls.src = fh.read()
        with open(GUI_LOGIN_EXP, encoding="utf-8") as fh:
            cls.gui_login_exp = fh.read()
        with open(AUTH_RS, encoding="utf-8") as fh:
            cls.auth = fh.read()
        cls.code = strip_rust(cls.src)

    def test_gui_max_failed_attempts_is_10(self):
        self.assertIn("pub const MAX_FAILED_ATTEMPTS: u32 = 10;", self.auth)

    def test_gui_backoff_ns_is_30_seconds(self):
        self.assertIn("pub const BACKOFF_NS: u64 = 30_000_000_000;", self.auth)

    def test_gui_note_failed_attempt_increments_then_resets_after_pause(self):
        self.assertIn("*failures += 1;", self.code)
        self.assertIn("if *failures >= MAX_FAILED_ATTEMPTS {", self.code)
        self.assertIn('*error_msg = String::from("Too many failed attempts - pausing 30s");', self.src)
        self.assertIn("io::nanosleep(BACKOFF_NS).is_ok() {", self.code)
        # The reset moved to the main-loop local (post-flush pause);
        # `failures = 0;` is a substring of both the old and new forms.
        self.assertIn("failures = 0;", self.code)

    def test_gui_note_failed_attempt_called_exactly_once_on_bad_creds(self):
        # The cap counts ONLY the bad-creds branch. The execve-failure path
        # after a SUCCESSFUL auth must NOT count (that is a correct login).
        calls = re.findall(r"note_failed_attempt\(&mut failures, &mut error_msg\);", self.code)
        self.assertEqual(len(calls), 1, "note_failed_attempt call count changed")
        # In the RAW source the call sits inside the bad-creds else branch:
        # the error message, then the password clear, then the call — as an
        # exact adjacent sequence (checked on src, not the string-masked code).
        self.assertIn(
            'error_msg = String::from("Invalid username or password");\n'
            "                        password_buf.clear();\n"
            "                        note_failed_attempt(&mut failures, &mut error_msg);",
            self.src,
        )
        # The execve-failure arm (after a successful auth) has no call: in
        # the raw source, the first closing brace after execve ends that arm.
        ev = self.src.index('process::execve("/bin/ade"')
        tail = self.src[ev:]
        cut = tail.index("}")
        self.assertNotIn("note_failed_attempt", tail[:cut])

    def test_gui_pause_sets_window_message_and_announces(self):
        self.assertIn("Too many failed attempts - pausing 30s", self.src)
        self.assertIn('io::print_str("\\nToo many failed attempts - pausing 30s\\n");', self.src)

    def test_gui_pause_runs_after_flush_so_message_shows(self):
        # The 30 s sleep must run AFTER win.flush(), so the pause message is
        # drawn before the window freezes — otherwise the user stares at a
        # stale frame for the whole backoff with no feedback.
        idx_flush = self.code.index("let _ = win.flush();")
        idx_sleep = self.code.index("io::nanosleep(BACKOFF_NS).is_ok() {")
        self.assertLess(idx_flush, idx_sleep)
        self.assertIn("failures >= MAX_FAILED_ATTEMPTS", self.code)

    def test_gui_empty_username_does_not_count(self):
        # Parity with the console getty's bare-Enter guard
        # (`if name_bytes.is_empty() { continue; }` in login/src/main.rs): a
        # stray Enter with no username re-prompts WITHOUT burning an attempt.
        code = self.code
        start = code.index("if user.is_empty() {")
        arm = code[start:]
        cut = arm.index("continue;")
        self.assertNotIn("note_failed_attempt", arm[:cut])
        self.assertIn("continue;", arm)

    def test_gui_bad_creds_announces_re_prompt_on_serial(self):
        # Parity with the console getty's "Login incorrect" pin: a bad GUI
        # password must print a serial announce so the QEMU harness
        # (tests/qemu_gui_login.exp) can assert the re-prompt on real
        # hardware - window stays up, no exit, no respawn. The announce sits
        # in the bad-creds else branch right after the note_failed_attempt
        # call (the adjacency pin anchors that branch), so the pinned
        # 3-line sequence is preserved.
        self.assertIn(
            'io::print_str("\\n[login] invalid credentials - re-prompting\\n");',
            self.src,
        )
        # Cross-source consistency: the harness must assert the same marker
        # (drift in either side fails this test).
        self.assertIn("invalid credentials - re-prompting", self.gui_login_exp)

    def test_gui_harness_drives_cap_positive(self):
        # The 'pausing 30s' cap marker is a POSITIVE contract in the GUI
        # harness (audit #17): it drives MAX_FAILED_ATTEMPTS=10 on real
        # hardware. Wrong passwords 1-9 (the step-3a loop) must show only
        # the bad-creds announce (a cap marker there is a FAIL arm); the
        # 10th triggers the marker (PASS arm, exactly once); the window
        # must survive the 30s backoff with no login-manager respawn (FAIL
        # arms on the respawn markers); and an 11th wrong password after
        # the backoff proves the counter was reset (announce only, no cap
        # marker).
        self.assertIn(
            '"Too many failed attempts - pausing 30s" {',
            self.gui_login_exp,
            "harness must have a cap-marker expect arm",
        )
        self.assertIn(
            "PASS: attempt cap activated on 10th failure",
            self.gui_login_exp,
            "the cap marker must be a PASS on the 10th attempt, not a FAIL",
        )
        # The 9-iteration below-cap loop: each wrong password re-prompts in
        # place with the serial announce.
        self.assertIn(
            "for {set i 1} {$i <= 9} {incr i}",
            self.gui_login_exp,
            "harness must loop 9 wrong passwords below the cap",
        )
        self.assertIn(
            "PASS: 11th wrong password re-prompts fresh",
            self.gui_login_exp,
            "harness must prove the counter reset after the 30s backoff",
        )
        self.assertIn(
            "FAIL: login-manager respawned during the 30s backoff",
            self.gui_login_exp,
            "no-respawn must be asserted through the backoff pause",
        )
        # A cap marker on attempts 1-9 (below MAX_FAILED_ATTEMPTS) must be
        # a fail-fast arm in the loop, not a silent tolerance.
        self.assertIn(
            "FAIL: attempt cap fired on attempt $i",
            self.gui_login_exp,
            "a cap marker on attempts 1-9 must be a fail-fast arm "
            "(the cap must not fire below 10)",
        )
        # Literal 'w r o n g' submits: 1 in the loop body + the 10th + the
        # 11th = 3 sites = 11 runtime wrong passwords (9 + 1 + 1).
        wrong = self.gui_login_exp.count('sendkey_seq "w r o n g"')
        self.assertEqual(
            wrong, 3,
            "harness must submit 9 loop + 10th + 11th wrong passwords "
            "(3 literal sendkey sites, 11 runtime attempts)",
        )

    def test_gui_backoff_disarm_clears_pause_message(self):
        # The disarm path (successful nanosleep) must also clear the pause
        # message: after the 30s backoff the window shows no error, so the
        # next wrong attempt displays 'Invalid username or password' (the
        # else-branch message, pinned separately) instead of the stale
        # 'Too many failed attempts - pausing 30s'.
        m = re.search(
            r"nanosleep\(BACKOFF_NS\)\.is_ok\(\) \{\s*failures = 0;\s*"
            r"error_msg\.clear\(\);\s*\}",
            self.code,
        )
        self.assertIsNotNone(
            m,
            "the successful-backoff disarm must reset the counter AND clear "
            "the pause message (EINTR keeps both)",
        )

    def test_gui_exit_inventory_still_three_returns_no_exit(self):
        # The throttle must not change the auth-failure exit behavior: still
        # exactly 3 returns (verify helper, window-create failure, successful
        # execve), no process::exit / panic! / .unwrap() — so NO auth-failure
        # path can exit user_main and burn init's MAX_RESPAWNS.
        code = strip_rust(self.src)
        self.assertEqual(len(re.findall(r"\breturn\b", code)), 3)
        self.assertNotIn("process::exit", self.src)
        self.assertNotIn("panic!", code)
        self.assertNotIn(".unwrap()", code)




    def test_gui_execve_error_keeps_window_alive(self):
        # When execve("/bin/ade") fails (binary missing or corrupt), the
        # window stays alive: the Err arm clears both buffers and falls
        # through to re-render with no return, no exit, no panic, and no
        # note_failed_attempt (a correct login is not a failure).
        code = strip_rust(self.src)
        # The serial announce proves this path is reached on real hardware.
        self.assertIn(
            'io::write_all(1, b"[login] execve failed, continuing\\n")',
            self.src,
        )
        # Both error_msg and password_buf are cleared before re-render.
        self.assertIn("error_msg.clear();", code)
        self.assertIn("password_buf.clear();", code)
        # The execve match body must not contain `return` (Err falls
        # through) or `note_failed_attempt` (correct login = not a failure).
        ev = code.index("process::execve(")
        tail = code[ev:]
        m_open = tail.index("{")          # match opening brace
        depth = 0
        match_end = None
        for i, ch in enumerate(tail[m_open:]):
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    match_end = m_open + i
                    break
        assert match_end is not None, "could not find execve match closing brace"
        err_arm = tail[m_open:match_end + 1]
        # The Err arm must contain no `return` (fallthrough to re-render)
        # and no `note_failed_attempt` (correct login is not a failure).
        err_start = err_arm.index("Err(_) => {")
        eb = err_arm[err_start:]
        depth = 0
        err_end = None
        for i, ch in enumerate(eb):
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    err_end = i
                    break
        assert err_end is not None, "could not find Err arm closing brace"
        err_body = eb[:err_end + 1]
        self.assertNotIn("return", err_body)
        self.assertNotIn("note_failed_attempt", err_body)
        # The exit inventory must still be exactly 3 (this fix does not
        # introduce any new return path).
        self.assertEqual(len(re.findall(r"\breturn\b", code)), 3)
        self.assertNotIn("process::exit", self.src)
        self.assertNotIn("panic!", code)




    def test_gui_window_create_failure_returns_zero(self):
        # The window-creation failure arm returns 0. Init's waitpid sees
        # status == 0, resets crashes (svc.crashes = 0), then increments
        # (svc.crashes = 1). Result: crashes is always 1 — never reaches
        # MAX_RESPAWNS — so login-manager respawns forever even when the
        # window cannot be created. (Compare vahid's exit(1): crashes
        # accumulates to 6 and eventually gives up.)
        code = strip_rust(self.src)
        # The WHY marker (paired with the mem free=N line) distinguishes
        # the known ENOMEM path from a fresh cause: ENOMEM -> Out of
        # memory, any other errno -> errno {e}. Losing either branch or
        # the errno comparison means the next GUI-hang boot cannot settle
        # known-vs-new from the serial log alone.
        self.assertIn(
            "[login] failed to create window: Out of memory (errno 12)", self.src,
            "ENOMEM branch marker lost",
        )
        self.assertIn("[login] failed to create window: errno {}", self.src,
                      "other-errno branch marker lost")
        self.assertIn("libsarga::errno::ENOMEM as i64", self.src,
                      "errno distinction mechanism lost")
        # The return 0 follows the failed-to-create print: in stripped code,
        # the Err arm is `Err(_) => { let _ = ...; return 0; }`.
        self.assertIn("return 0", code)
        # The exit inventory must still be exactly 3 (this is the same
        # window-failure return already counted by test_gui_exit_inventory).
        self.assertEqual(len(re.findall(r"\breturn\b", code)), 3)
        # Confirm the specific window-failure block is return 0 (not 1 or
        # process::exit). Find "failed to create window" then scan to next
        # "return" — it must be "return 0;".
        idx = self.src.index("failed to create window")
        tail = self.src[idx:]
        ret = tail.index("return") if "return" in tail else None
        self.assertIsNotNone(ret, "no return after failed to create window")
        self.assertTrue(tail[ret:].lstrip().startswith("return 0;"))

    def test_gui_and_console_throttle_constants_agree(self):
        # Cross-path reference: the GUI password field and the console getty
        # must stay throttled identically (10 attempts / 30 s backoff), so a
        # brute-forcer cannot switch to the weaker path. A drift in either
        # side's constants fails here before any boot.
        with open(LOGIN_RS, encoding="utf-8") as fh:
            console = fh.read()
        with open(AUTH_RS, encoding="utf-8") as fh:
            auth_src = fh.read()
        console_code = strip_rust(console)
        # The throttle numbers now live in ONE place (libsarga::auth);
        # both binaries import them, so a drift in either side's wiring
        # (or in the shared values) fails here before any boot.
        for lit in (
            "pub const MAX_FAILED_ATTEMPTS: u32 = 10;",
            "pub const BACKOFF_NS: u64 = 30_000_000_000;",
        ):
            self.assertIn(lit, auth_src, "libsarga::auth throttle constant missing")
        self.assertIn(
            "use libsarga::auth::{BACKOFF_NS, MAX_FAILED_ATTEMPTS};",
            console,
            "console getty must import the shared throttle constants",
        )
        self.assertIn(
            "use libsarga::auth::{BACKOFF_NS, MAX_FAILED_ATTEMPTS};",
            self.src,
            "GUI login-manager must import the shared throttle constants",
        )
        # The remaining asymmetry is call TOPOLOGY, not throttling: the
        # console counts all three interactive failure paths; the GUI counts
        # only the bad-creds branch (execve failure after a correct login
        # must not count). Both still never exit on bad creds, so neither
        # can burn init's MAX_RESPAWNS. Pin the split so a future edit that
        # un-throttles one path (or adds a GUI exit) fails CI.
        # The two counts duplicate (on purpose) the single-side pins
        # test_note_failed_attempt_called_on_exactly_three_paths (console)
        # and test_gui_note_failed_attempt_called_exactly_once_on_bad_creds
        # (GUI) — here they are asserted in one place so the asymmetry
        # relationship itself is the contract, not just each side alone.
        console_calls = re.findall(r"note_failed_attempt\(&mut failures\);", console_code)
        gui_calls = re.findall(r"note_failed_attempt\(&mut failures, &mut error_msg\);", self.code)
        self.assertEqual(len(console_calls), 3,
                         "console getty must count exactly 3 failure paths")
        self.assertEqual(len(gui_calls), 1,
                         "GUI must count exactly 1 failure path (bad creds)")
        # The GUI side must never exit on any auth path (an exit would burn
        # init's MAX_RESPAWNS). The console getty's process::exit(1) on
        # serial EOF / read errors is intentional and documented — it is
        # NOT on the mistype paths (those count + re-prompt via
        # note_failed_attempt + continue, pinned above).
        self.assertNotIn("process::exit", self.code)



MAX_FAILED_ATTEMPTS = 10
BACKOFF_NS = 30_000_000_000


def note_failed_attempt_console(failures, announce, sleep_fn):
    """Port of login/src/main.rs::note_failed_attempt.

    *failures += 1; if >= MAX: print + sleep(BACKOFF_NS); reset to 0 ONLY
    if the sleep succeeded (EINTR must not disarm the counter).
    """
    failures[0] += 1
    if failures[0] >= MAX_FAILED_ATTEMPTS:
        announce()
        if sleep_fn(BACKOFF_NS):
            failures[0] = 0
    return failures[0]


def note_failed_attempt_gui(failures, error_msg, announce):
    """Port of login-manager/src/main.rs::note_failed_attempt.

    Count + announce + set the window message ONLY; the BACKOFF_NS sleep
    lives in the main loop after win.flush() (see gui_backoff_step).
    """
    failures[0] += 1
    if failures[0] >= MAX_FAILED_ATTEMPTS:
        error_msg[0] = "Too many failed attempts - pausing 30s"
        announce()
    return failures[0]


def gui_backoff_step(failures, error_msg, sleep_fn):
    """Port of login-manager's main-loop backoff (post-flush).

    `if failures >= MAX_FAILED_ATTEMPTS && nanosleep(BACKOFF_NS).is_ok()
    { failures = 0; error_msg.clear(); }` — the && short-circuits, so
    below the cap nanosleep never runs; a successful sleep disarms the
    counter AND clears the lingering pause message so the next attempt
    shows the plain 'Invalid username or password'.
    """
    if failures[0] >= MAX_FAILED_ATTEMPTS and sleep_fn(BACKOFF_NS):
        failures[0] = 0
        error_msg[0] = ""
    return failures[0]


class TestNoteFailedAttemptStateMachine(unittest.TestCase):
    """Behavioral port of the attempt-cap state machine (no QEMU).

    The source pins (TestAttemptCapContract / TestGuiAttemptCapContract)
    assert the constants and call topology; this class executes the actual
    increment / cap / reset / EINTR-skip logic through faithful Python
    ports with injectable announce + nanosleep fakes.
    """

    def test_console_increments_below_cap(self):
        f = [0]
        calls = []

        def announce():
            calls.append("announce")

        def sleep(_ns):
            calls.append("sleep")
            return True

        for _ in range(9):
            note_failed_attempt_console(f, announce, sleep)
        self.assertEqual(f[0], 9)
        self.assertEqual(calls, [])  # no announce, no sleep below the cap

    def test_console_10th_attempt_hits_cap_and_resets(self):
        f = [0]
        calls = []

        def announce():
            calls.append("announce")

        def sleep(_ns):
            calls.append("sleep")
            return True

        for _ in range(10):
            note_failed_attempt_console(f, announce, sleep)
        self.assertEqual(calls, ["announce", "sleep"])
        self.assertEqual(f[0], 0, "successful pause must reset the counter")

    def test_console_eintr_skips_reset(self):
        # A failed nanosleep (EINTR -> Err) must NOT disarm the counter:
        # the next attempt re-hits the pause instead of bursting at speed.
        f = [0]
        calls = []

        def announce():
            calls.append("announce")

        def sleep(_ns):
            return False  # nanosleep Err (EINTR)

        for _ in range(10):
            note_failed_attempt_console(f, announce, sleep)
        self.assertEqual(f[0], 10, "EINTR must keep the counter armed")
        # 11th attempt: the counter keeps incrementing (the reset is
        # conditional on a successful sleep), but the pause still fires —
        # every attempt re-announces + re-pauses, so the throttle holds.
        note_failed_attempt_console(f, announce, sleep)
        self.assertEqual(f[0], 11, "counter increments; throttle holds")
        self.assertEqual(calls.count("announce"), 2)

    def test_console_reset_only_on_successful_pause(self):
        f = [0]
        results = [False, True]  # EINTR once, then success
        announces = []

        def announce():
            announces.append(1)

        def sleep(_ns):
            return results.pop(0)

        for _ in range(10):
            note_failed_attempt_console(f, announce, sleep)
        self.assertEqual(f[0], 10, "EINTR: still armed")
        note_failed_attempt_console(f, announce, sleep)
        self.assertEqual(f[0], 0, "successful pause resets")
        self.assertEqual(len(announces), 2)

    def test_gui_counts_and_sets_message_but_does_not_sleep(self):
        # The GUI note_failed_attempt never sleeps — the backoff runs in the
        # main loop after flush. Behavioral split pin.
        f = [0]
        msg = [""]
        calls = []

        def announce():
            calls.append("announce")

        for _ in range(10):
            note_failed_attempt_gui(f, msg, announce)
        self.assertEqual(f[0], 10)
        self.assertEqual(msg[0], "Too many failed attempts - pausing 30s")
        self.assertEqual(calls, ["announce"])
        # The counter is NOT reset here — the backoff step owns that.
        self.assertEqual(f[0], 10)

    def test_gui_backoff_step_resets_on_success(self):
        f = [10]
        msg = ["Too many failed attempts - pausing 30s"]
        slept = []

        def sleep(_ns):
            slept.append(1)
            return True

        gui_backoff_step(f, msg, sleep)
        self.assertEqual(f[0], 0)
        self.assertEqual(msg[0], "", "successful backoff must clear the pause message")
        self.assertEqual(slept, [1])

    def test_gui_backoff_step_eintr_keeps_armed(self):
        f = [10]
        msg = ["Too many failed attempts - pausing 30s"]
        slept = []

        def sleep(_ns):
            slept.append(1)
            return False  # EINTR

        gui_backoff_step(f, msg, sleep)
        self.assertEqual(f[0], 10, "EINTR must not disarm")
        self.assertEqual(
            msg[0], "Too many failed attempts - pausing 30s",
            "while still armed the pause message must stay (no clear)",
        )
        self.assertEqual(slept, [1])

    def test_gui_backoff_step_short_circuits_below_cap(self):
        # && short-circuit: below the cap nanosleep never runs.
        f = [9]
        msg = ["Too many failed attempts - pausing 30s"]
        slept = []

        def sleep(_ns):
            slept.append(1)
            return True

        gui_backoff_step(f, msg, sleep)
        self.assertEqual(f[0], 9)
        self.assertEqual(
            msg[0], "Too many failed attempts - pausing 30s",
            "below the cap nothing is cleared",
        )
        self.assertEqual(slept, [], "nanosleep must not run below the cap")

    def test_console_and_gui_share_the_same_constants(self):
        # The lockstep contract (pinned in source by
        # test_gui_and_console_throttle_constants_agree) is also true of the
        # behavioral ports. STRONGEST form: derive the port constants from
        # the live Rust source — the single libsarga::auth home both
        # binaries import — so a Rust-side drift fails the behavioral
        # layer too, not just the layer-2 grep pins.
        def rust_const(path, name):
            with open(os.path.join(REPO_ROOT, path), encoding="utf-8") as fh:
                m = re.search(r"pub const " + name + r": u\w+ = (\d[\d_]*)", fh.read())
            self.assertIsNotNone(m, name + " not found in " + path)
            return int(m.group(1))

        console_max = rust_const("libsarga/src/auth.rs", "MAX_FAILED_ATTEMPTS")
        console_ns = rust_const("libsarga/src/auth.rs", "BACKOFF_NS")
        gui_max = rust_const("libsarga/src/auth.rs", "MAX_FAILED_ATTEMPTS")
        gui_ns = rust_const("libsarga/src/auth.rs", "BACKOFF_NS")
        # All four agree with each other AND with the port constants.
        self.assertEqual({console_max, gui_max}, {MAX_FAILED_ATTEMPTS})
        self.assertEqual({console_ns, gui_ns}, {BACKOFF_NS})
        self.assertEqual(30_000_000_000, 30 * 1_000_000_000)

    def test_gui_combined_flow_end_to_end(self):
        # The GUI split (count+announce in note_failed_attempt_gui, reset in
        # gui_backoff_step) must work TOGETHER: 10 failures set the window
        # message + announce, the post-flush backoff resets on success, and
        # the 11th attempt counts fresh from 1 (not from 10).
        f = [0]
        msg = [""]
        announces = []

        def announce():
            announces.append(1)

        def sleep(_ns):
            return True

        for _ in range(10):
            note_failed_attempt_gui(f, msg, announce)
        self.assertEqual(f[0], 10)
        self.assertEqual(msg[0], "Too many failed attempts - pausing 30s")
        self.assertEqual(announces, [1])
        gui_backoff_step(f, msg, sleep)
        self.assertEqual(f[0], 0, "backoff resets after a successful pause")
        self.assertEqual(msg[0], "", "successful backoff clears the pause message")
        note_failed_attempt_gui(f, msg, announce)
        self.assertEqual(f[0], 1, "11th attempt counts fresh from 1")
        self.assertEqual(
            msg[0], "",
            "below the cap the note leaves the message alone (the plain "
            "'Invalid username or password' is set by the Enter handler, "
            "pinned separately in source)",
        )


class TestPanicContract(unittest.TestCase):
    """Panic contract for the auth binaries (login, login-manager).

    Traced Aug 10, 2026: libsarga/src/lib.rs defines the ONLY panic handler
    in the userspace stack (both login and login-manager link libsarga):

        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            crate::println!("SARGA OS PANIC: {}", info);
            process::exit(1);
        }

    and sarga_main! is a thin wrapper:

        pub extern "Rust" fn main() -> i32 { $main_fn() }

    — no catch, no unwind. The build forces -C panic=abort
    (.cargo/config.toml, x86_64-sarga.json), so core::panic::catch_unwind
    is not usable: a panic reaches the handler and calls process::exit(1),
    which init's waitpid sees as a NON-ZERO status. That accumulates
    svc.crashes and burns a MAX_RESPAWNS respawn exactly like a bad exit
    code. Therefore the auth paths must be TOTAL (never panic) — a hash
    failure must return false and re-prompt, never exit(1) via the panic
    handler.

    KNOWN BOUNDARY: the credential logic is panic-free (no .unwrap(),
    no panic!, no unchecked slicing), but both auth binaries allocate on
    the auth path (libsarga's read_to_end / tty's read_line Vec growth,
    login-manager's read_to_string / String::from / buffer push). An
    allocation failure routes through libsarga's #[alloc_error_handler]
    (mem.rs:150), which panics and exits 1 — the one vector the source scans cannot see. This is accepted as a
    genuine system-level OOM (not a credential failure): a corrupt or
    oversized shadow on an out-of-memory box burns a respawn the same way
    any OOM would. If that ever becomes reachable, the fix is a hard cap
    on the shadow read, not a catch_unwind.

    These pins guard the contract: the handler stays exit(1) (a panic is
    a crash, never a silent exit 0), the build stays panic=abort (so
    nobody can rely on catch_unwind), the auth/hash code stays panic-free
    in its credential logic, and the OOM boundary is pinned explicitly so
    it cannot be silently forgotten.
    """

    @classmethod
    def setUpClass(cls):
        with open(LIBSARGA_LIB_RS, encoding="utf-8") as fh:
            cls.lib = fh.read()
        with open(LIBSARGA_HASH_RS, encoding="utf-8") as fh:
            cls.hash = fh.read()
        with open(LIBSARGA_IO_RS, encoding="utf-8") as fh:
            cls.io = fh.read()
        with open(LOGIN_RS, encoding="utf-8") as fh:
            cls.login = fh.read()
        with open(LOGIN_MANAGER_RS, encoding="utf-8") as fh:
            cls.gui = fh.read()
        cls.lib_code = strip_rust(cls.lib)
        cls.hash_code = strip_rust(cls.hash)
        cls.login_code = strip_rust(cls.login)
        cls.gui_code = strip_rust(cls.gui)

    def test_panic_handler_exits_non_zero(self):
        # A panic must exit NON-ZERO: init's waitpid sees status != 0 and
        # accumulates crashes. If someone changed this to exit(0), a panic
        # would reset crashes and respawn forever — hiding the bug — so the
        # exit(1) is itself the contract.
        self.assertIn("#[panic_handler]", self.lib)
        self.assertIn("fn panic(", self.lib)
        self.assertIn("process::exit(1);", self.lib_code)
        self.assertNotIn("process::exit(0);", self.lib_code)

    def test_build_is_panic_abort(self):
        # panic=abort everywhere: catch_unwind is impossible, so the ONLY
        # defense against a panic burning a respawn is making the auth code
        # total. A future switch to panic=unwind would still need a custom
        # hook (catch_unwind + a fallback path); until that exists, the
        # abort strategy is pinned.
        with open(CARGO_CONFIG, encoding="utf-8") as fh:
            cfg = fh.read()
        with open(SARGA_JSON, encoding="utf-8") as fh:
            target = fh.read()
        self.assertIn("panic=abort", cfg)
        self.assertIn('"panic-strategy": "abort"', target)

    def test_alloc_error_handler_is_known_oom_boundary(self):
        # The ONLY panic vector the credential-logic scans cannot see is
        # allocation failure: Vec growth in read_to_end / read_line
        # hits libsarga's #[alloc_error_handler], which panics and exits
        # 1 (mem.rs:150: "allocation error"). Pin that boundary explicitly
        # so it stays visible — an OOM is a system-level failure (accepted),
        # not a silent exit 0 that would hide the bug.
        with open(os.path.join(REPO_ROOT, "libsarga", "src", "mem.rs"), encoding="utf-8") as fh:
            mem = fh.read()
        # alloc_error_handler panics -> the panic handler (lib.rs, pinned by
        # test_panic_handler_exits_non_zero) turns that into exit(1).
        self.assertIn("#[alloc_error_handler]", mem)
        self.assertIn("allocation error", mem)
        self.assertIn("panic!", strip_rust(mem))

    def test_sarga_main_is_thin_no_catch(self):
        # sarga_main! is `fn main() -> i32 { $main_fn() }` — no try/catch,
        # no conversion. A panic propagates straight to the panic handler.
        self.assertIn("pub extern \"Rust\" fn main() -> i32", self.lib)
        self.assertIn("$main_fn()", self.lib)
        self.assertNotIn("catch_unwind", self.lib)

    def test_hash_verify_path_is_total(self):
        # libsarga/src/hash.rs verify_password must never panic: every
        # fallible step is unwrap_or / early-return-false. A malformed
        # shadow entry (bad hex, wrong salt length, non-PBKDF2 scheme)
        # must produce `false`, not a panic -> exit(1) -> respawn burn.
        self.assertIn("fn verify_password(", self.hash)
        self.assertNotIn(".unwrap()", self.hash_code)
        self.assertNotIn(".expect(", self.hash_code)
        self.assertNotIn("panic!", self.hash_code)
        # The one slice into shadow data (`rest[7..]` after starts_with
        # b"PBKDF2-" and `rest3[..pos]` after position()) is length-
        # guarded by construction: starts_with implies len >= 7, position()
        # returns a valid index. Pin that both guards sit with the slices.
        self.assertIn('rest.starts_with(b"PBKDF2-")', self.hash)
        self.assertIn("let rest2 = &rest[7..];", self.hash)
        self.assertIn(".position(|&b| b == b':')", self.hash)
        self.assertIn("dk_hex = &rest3[..pos];", self.hash)

    def test_login_auth_path_has_no_panic_vectors(self):
        # The console getty's auth path (verify_password wrapper, read_line,
        # read_whole_file, note_failed_attempt) must be panic-free so a
        # shadow/hash failure re-prompts instead of exiting via panic.
        self.assertNotIn(".unwrap()", self.login_code)
        self.assertNotIn(".expect(", self.login_code)
        self.assertNotIn("panic!", self.login_code)
        # The only slice in the whole-file read path is `&tmp[..n]` in the
        # shared libsarga::io::read_to_end (login/passwd's read_whole_file
        # consolidated there), where n comes from read(fd, &mut tmp) — the
        # kernel guarantees n <= 512, so it is safe by construction. Pin
        # that it stays a read-buffer slice in the shared home.
        self.assertIn("&tmp[..n]", self.io)
        # No hand-computed slice indexes (a `&x[expr..]` where expr is not
        # a read-count) anywhere in login — those would be panic vectors.
        self.assertNotIn("[7..]", self.login)
        self.assertNotIn("[pos", self.login)
        self.assertNotIn("[..pos", self.login)

    def test_login_manager_auth_path_has_no_panic_vectors(self):
        # The GUI login's auth path must be panic-free for the same reason:
        # a bad creds or execve failure re-renders; it must never exit via
        # the panic handler's process::exit(1).
        self.assertNotIn(".unwrap()", self.gui_code)
        self.assertNotIn(".expect(", self.gui_code)
        self.assertNotIn("panic!", self.gui_code)
        # login-manager's existing pins already assert no process::exit /
        # panic! / .unwrap() on the raw source; this pin covers the
        # stripped-code view so comments/strings can't hide a new panic.


def _parse_bindings(input_code):
    """Extract the BINDINGS rows from stripped ade/src/input/mod.rs as dicts."""
    block = input_code.split("const BINDINGS:")[1].split("];")[0]
    rows = []

    def field(chunk, name):
        m = re.search(r"\b" + name + r":\s*([^,]+),", chunk)
        return m.group(1).strip() if m else None

    for chunk in block.split("Binding {"):
        if "code:" not in chunk:
            continue
        rows.append({
            "code": field(chunk, "code"),
            "ctrl": field(chunk, "ctrl") == "true",
            "alt": field(chunk, "alt") == "true",
            "shift": field(chunk, "shift") == "true",
            "desktop": field(chunk, "desktop") == "true",
            "action": field(chunk, "action"),
        })
    return rows

def _binding_code(literal):
    """Resolve a BINDINGS `code:` literal to its u8 value."""
    lit = literal.strip().replace("keys::", "")
    if lit.startswith("b'"):
        return ord(lit[2])
    table = {
        "KEY_ESC": 0x1B,
        "KEY_ENTER": 0x0D,
        "KEY_TAB": 0x09,
        "KEY_BACKSPACE": 0x7F,
        "KEY_X": ord("X"),
        "SCAN_F11": 0x57,
    }
    assert lit in table, "unmapped BINDINGS code literal: %r" % lit
    return table[lit]


def _resolve_action(rows, ev):
    """First-match table resolve mirroring the Rust resolve()."""
    for r in rows:
        if (_binding_code(r["code"]) == ev[0] and r["ctrl"] == ev[1]
                and r["alt"] == ev[2] and r["shift"] == ev[3]):
            return r["action"]
    return None


class TestKernelKeyContract(unittest.TestCase):
    """Pins the Phase C packed-key contract while the kernel rewrite is in flight.

    Source: ade/docs/kernel-gui-modifier-delivery.md, Design A. The kernel is
    mid-major-change, so this test holds the *userspace half* — libsarga
    get_key() -> Option<u16> (A5) and ade KeyEvent::from_raw (A6) — plus a
    Python port of the decode, so the libsarga/input changes cannot drift
    while the kernel side (A1-A4) is being rewritten.

    Bit layout (pinned): low byte = char; bit8 = alt, bit9 = ctrl,
    bit10 = shift, bit11 = super (ignored by ade). The chord
    Ctrl+Alt+Backspace = 0x08 | (1<<8) | (1<<9) = 0x0308.
    The same values are pinned at boot by the ade selftest test_from_raw.
    """

    @classmethod
    def setUpClass(cls):
        cls.gui = open(os.path.join(REPO_ROOT, "libsarga", "src", "gui.rs"), encoding="utf-8").read()
        cls.input = open(os.path.join(REPO_ROOT, "ade", "src", "input", "mod.rs"), encoding="utf-8").read()
        cls.event = open(os.path.join(REPO_ROOT, "ade", "src", "core", "event.rs"), encoding="utf-8").read()
        cls.desktop = open(os.path.join(REPO_ROOT, "ade", "src", "core", "desktop.rs"), encoding="utf-8").read()
        cls.main = open(os.path.join(REPO_ROOT, "ade", "src", "main.rs"), encoding="utf-8").read()
        cls.input_code = strip_rust(cls.input)
        cls.desktop_code = strip_rust(cls.desktop)

    # --- Python port of the decode (the contract mirror) -----------------
    @staticmethod
    def from_byte(b):
        """Mirror of ade/src/input/mod.rs KeyEvent::from_byte."""
        if b == 0x1B:
            return (0x1B, False, False, False)        # Esc
        if b in (0x0D, 0x0A, 28):
            return (0x0D, False, False, False)        # Enter
        if b == 0x09:
            return (0x09, False, False, False)        # Tab
        if b in (0x7F, 0x08):
            return (0x7F, False, False, False)        # Backspace
        if 1 <= b <= 26:
            return (ord("a") - 1 + b, True, False, False)  # Ctrl+letter
        return (b, False, False, False)

    @staticmethod
    def from_raw(raw):
        """Mirror of ade/src/input/mod.rs KeyEvent::from_raw (Design A)."""
        code, ctrl, alt, shift = TestKernelKeyContract.from_byte(raw & 0xFF)
        if raw & (1 << 8):
            alt = True
        if raw & (1 << 9):
            ctrl = True
        if raw & (1 << 10):
            shift = True
        return (code, ctrl, alt, shift)

    def test_libsarga_get_key_is_u16(self):
        # A5 landed: the producer no longer truncates to u8; the u64 syscall
        # result widens losslessly to u16 so the modifier bits can arrive.
        self.assertIn("pub fn get_key(&mut self) -> Option<u16>", self.gui)
        self.assertIn("Some(k as u16)", self.gui)
        self.assertNotIn("Some(k as u8)", self.gui)

    def test_ade_from_raw_exists_with_bit_layout(self):
        # A6 landed: low byte decoded via from_byte, bits 8/9/10 OR'd in.
        self.assertIn("pub fn from_raw(raw: u16) -> KeyEvent", self.input_code)
        self.assertIn("(raw & 0xFF) as u8", self.input_code)
        for mask in ("1 << 8", "1 << 9", "1 << 10"):
            self.assertIn(mask, self.input_code, f"missing {mask}")

    def test_event_key_payload_is_u16_and_wired(self):
        # The packed value must reach handle_key un-truncated.
        self.assertIn("Key(u16)", self.event)
        self.assertIn("desktop_win.get_key()", self.main)
        self.assertIn("Event::Key(key)", self.main)

    def test_desktop_routes_via_from_raw_and_guards_a11y(self):
        # handle_key decodes the packed value; the pty path sends the low byte.
        self.assertIn("KeyEvent::from_raw(key)", self.desktop_code)
        self.assertIn("(key & 0xFF) as u8", self.desktop_code)
        # The a11y pre-handler must skip modified events so the chord (0x308)
        # falls through to the keymap instead of being swallowed.
        self.assertIn("key & 0xFF00 != 0", self.desktop_code)

    def test_python_mirror_spec_values(self):
        table = [
            (0x0063, (ord("c"), False, False, False)),    # plain 'c'
            (0x000D, (0x0D, False, False, False)),        # Enter
            (0x0003, (ord("c"), True, False, False)),     # Ctrl+C fold
            (0x0308, (0x7F, True, True, False)),          # chord: backspace+ctrl+alt
            (0x0108, (0x7F, False, True, False)),         # bit8 = alt (alt-only)
            (0x0208, (0x7F, True, False, False)),         # bit9 = ctrl (ctrl-only)
            (0x0808, (0x7F, False, False, False)),        # bit11 super ignored
            (0x0103, (ord("c"), True, True, False)),      # Ctrl+C + alt bit
        ]
        for raw, expected in table:
            self.assertEqual(self.from_raw(raw), expected, f"from_raw(0x{raw:04x})")

    def test_inert_until_kernel_sends_bits(self):
        # Zero high bits: from_raw must be byte-identical to from_byte —
        # the property that makes the u16 path safe to land now.
        for b in range(256):
            self.assertEqual(self.from_raw(b), self.from_byte(b), f"byte 0x{b:02x}")

    def test_chord_resolves_to_quit_in_table(self):
        rows = _parse_bindings(self.input_code)
        quit_rows = [r for r in rows if r["action"] == "KeyAction::Quit"]
        self.assertEqual(len(quit_rows), 1, "exactly one Quit binding")
        q = quit_rows[0]
        self.assertEqual(q["code"], "keys::KEY_BACKSPACE")
        self.assertTrue(q["ctrl"] and q["alt"] and not q["shift"] and q["desktop"])
        # The packed chord decodes to exactly that row's (code, mods) tuple.
        code, ctrl, alt, shift = self.from_raw(0x0308)
        self.assertEqual((code, ctrl, alt, shift), (0x7F, True, True, False))
        # Ctrl+Q must stay unbound: neither plain 'q' nor Ctrl+Q matches a
        # Quit row (the old session-end gates are gone).
        self.assertFalse(any(r["code"] in ("b'q'", "keys::KEY_Q") for r in rows))

    def test_from_raw_wired_into_run_all(self):
        # Boot coverage of the pin must not silently vanish: the selftest
        # runs only if run_all calls it, and the [input] serial marker
        # covers only the keymap dump, not from_raw.
        mod_rs = strip_rust(
            open(os.path.join(REPO_ROOT, "ade", "src", "util", "testing", "mod.rs"), encoding="utf-8").read()
        )
        self.assertIn("ok &= input::test_from_raw();", mod_rs)
    def test_full_table_reachable_via_from_raw(self):
        # Port of the test_keymap packed-stream sweep: EVERY BINDINGS row
        # -- including the Ctrl+Alt+Backspace chord -- must be reachable
        # from some (byte, mods) pair via KeyEvent::from_raw, and the
        # found pair must resolve to that row's action. 16 rows are
        # byte-deliverable (from_byte); the chord needs the packed bits
        # (0x0308). Note: alt/ctrl/shift can always be OR'd into any
        # byte, so the reachability half is near-vacuous -- the real
        # tripwires are the (16, 1) count, an unmapped code literal
        # (diagnosed by _binding_code), and the resolve-through
        # shadowing check.
        rows = _parse_bindings(self.input_code)
        self.assertEqual(len(rows), 17, "BINDINGS row count")

        deliverable = synthetic = 0
        for r in rows:
            code = _binding_code(r["code"])
            if r["alt"] or r["shift"]:
                synthetic += 1
                continue
            ok = any(
                self.from_byte(x) == (code, r["ctrl"], False, False)
                for x in range(256)
            )
            self.assertTrue(ok, "no from_byte input for %r" % r)
            deliverable += 1
        self.assertEqual((deliverable, synthetic), (16, 1),
                         "16 byte-deliverable + 1 synthetic chord")

        for r in rows:
            target = (_binding_code(r["code"]), r["ctrl"], r["alt"], r["shift"])
            found = None
            for byte in range(256):
                for alt in (False, True):
                    for ctrl in (False, True):
                        for shift in (False, True):
                            raw = byte
                            if alt:
                                raw |= 1 << 8
                            if ctrl:
                                raw |= 1 << 9
                            if shift:
                                raw |= 1 << 10
                            if self.from_raw(raw) == target:
                                found = raw
                                break
                        if found is not None:
                            break
                    if found is not None:
                        break
                if found is not None:
                    break
            self.assertIsNotNone(found,
                                 "no from_raw pair for %r" % (target,))
            # resolve-through: the pair must map to THIS row's action.
            action = _resolve_action(rows, self.from_raw(found))
            self.assertEqual(action, r["action"],
                             "packed 0x%04x resolves to a different action" % found)



class TestScancodeConstants(unittest.TestCase):
    """Pins the userspace set-1 scancode constants against the standard.

    Values sourced from pc-keyboard 0.5.1's ScancodeSet1
    (src/scancodes.rs, `map_scancode` / `map_extended_scancode`): Escape=0x01,
    Enter=0x1C, F11=0x57, and the arrow keys as the SECOND byte of the E0
    extended sequence (ArrowUp=E0 48, ArrowLeft=E0 4B, ArrowRight=E0 4D,
    ArrowDown=E0 50) — see ade/docs/kernel-keyboard-gate.md §2.1. The
    constants are what the future RawKey path would push into the byte
    stream, so a typo here (e.g. SCAN_UP=71) would break a11y arrow
    navigation / F11 only at boot — this pin catches it host-side.
    """

    INPUT_RS = os.path.join(REPO_ROOT, "ade", "src", "input", "mod.rs")

    EXPECTED = {
        "SCAN_ESC": 0x01,     # set-1 Escape make
        "SCAN_ENTER": 0x1C,   # set-1 Enter make
        "SCAN_UP": 0x48,      # E0 48 (ArrowUp)
        "SCAN_LEFT": 0x4B,    # E0 4B (ArrowLeft)
        "SCAN_RIGHT": 0x4D,   # E0 4D (ArrowRight)
        "SCAN_DOWN": 0x50,    # E0 50 (ArrowDown)
        "SCAN_F11": 0x57,     # set-1 F11 make
    }

    @classmethod
    def setUpClass(cls):
        cls.input_code = strip_rust(open(cls.INPUT_RS, encoding="utf-8").read())

    def test_constant_values_match_set1(self):
        for name, expected in self.EXPECTED.items():
            m = re.search(
                r"pub const %s: u8 = (0x[0-9A-Fa-f]+|[0-9]+);" % name,
                self.input_code,
            )
            self.assertIsNotNone(m, f"{name} missing from input/mod.rs keys module")
            self.assertEqual(
                int(m.group(1), 0),
                expected,
                f"{name} drifted from its set-1 scancode value (see docs/kernel-keyboard-gate.md §2.1)",
            )

    def test_arrow_constants_are_e0_extended(self):
        # The arrows are E0-extended keys: their values are the E0 second
        # bytes, which collide with the single-byte numpad block
        # (0x47..=0x53). This is a latent collision (numpad keys decode to
        # Unicode digits or RawKey(Numpad*) and never reach the a11y byte
        # path today), pinned here so nobody "fixes" SCAN_UP=0x48 to a
        # numpad-adjacent value.
        e0 = {self.EXPECTED[k] for k in ("SCAN_UP", "SCAN_LEFT", "SCAN_RIGHT", "SCAN_DOWN")}
        self.assertTrue(e0 <= set(range(0x47, 0x54)), "arrow constants must sit in the E0 second-byte range")


class TestKeyboardGateDocTable(unittest.TestCase):
    """Cross-pins the section 2.1 doc table against the userspace constants.

    kernel-keyboard-gate.md section 2.1 is the spec for the future RawKey
    forwarding path: it maps pc_keyboard::KeyCode -> set-1 make code ->
    userspace SCAN_* constant. The scancode constants in input/mod.rs and
    the table must never disagree -- a doc edit that changes a value without
    touching the code (or vice versa) would silently invalidate the RawKey
    path the kernel rewrite will build. This test parses BOTH sources and
    asserts they agree, so neither can drift in isolation.

    The pinned rows are the E0 second-byte arrows (SCAN_UP/LEFT/RIGHT/DOWN)
    and the single-byte ESC/ENTER/F11 -- exactly the rows that carry a
    userspace name. Rows the doc marks "(none...)" (Backspace, Home block,
    Numpad block, F1..F10) carry no constant and are deliberately excluded:
    a future RawKey arm that names one needs BOTH a new SCAN_* constant and
    a table row, and this test forces that update.
    """

    DOC = os.path.join(REPO_ROOT, "ade", "docs", "kernel-keyboard-gate.md")
    INPUT_RS = os.path.join(REPO_ROOT, "ade", "src", "input", "mod.rs")

    # The rows userspace names today: 4 E0 second-byte arrows + 3
    # single-byte keys. Any change to which rows carry a constant fails the
    # set equality below and forces a deliberate update here.
    PINNED = ["SCAN_ESC", "SCAN_ENTER", "SCAN_UP", "SCAN_LEFT",
              "SCAN_RIGHT", "SCAN_DOWN", "SCAN_F11"]

    def setUp(self):
        self.doc = open(self.DOC, encoding="utf-8").read()
        self.input_code = strip_rust(open(self.INPUT_RS, encoding="utf-8").read())

    def _rust_scan(self, name):
        m = re.search(
            r"pub const %s: u8 = (0x[0-9A-Fa-f]+|[0-9]+);" % name,
            self.input_code,
        )
        self.assertIsNotNone(m, "%s missing from input/mod.rs keys module" % name)
        return int(m.group(1), 0)

    def _doc_rows(self):
        # The section 2.1 markdown table: from the '### 2.1' heading to the
        # next heading, every '| ... |' line with 4+ cells (skips the header
        # and the '---' separator, which also starts with '|').
        sec = re.search(r"### 2\.1.*?(?=\n#{1,3} )", self.doc, re.S)
        self.assertIsNotNone(sec, "section 2.1 not found in kernel-keyboard-gate.md")
        rows = []
        for line in sec.group(0).splitlines():
            line = line.strip()
            if not line.startswith("|"):
                continue
            cells = [c.strip() for c in line.strip("|").split("|")]
            if len(cells) < 4 or "pc_keyboard::KeyCode" in cells[0]:
                continue
            rows.append(cells)
        self.assertGreaterEqual(len(rows), 8, "section 2.1 table rows not parsed")
        return rows

    def test_doc_table_matches_input_module(self):
        doc_by_name = {}
        e0_rows = set()
        for cells in self._doc_rows():
            const_cell = cells[3]
            m = re.search(r"`(SCAN_\w+)\s*=\s*(0x[0-9A-Fa-f]+|[0-9]+)`", const_cell)
            if m is None:
                # '(none ...)' rows carry no userspace constant -- the doc's
                # marker for keys the code does not name yet.
                continue
            name, val = m.group(1), int(m.group(2), 0)
            doc_by_name[name] = val
            if "E0-extended" in cells[2]:
                e0_rows.add(name)
            # The set-1 make code column and the constant column are the same
            # scancode written twice -- they must agree inside the doc too.
            code = re.search(r"`0x([0-9A-Fa-f]+)`", cells[1])
            self.assertIsNotNone(code, "set-1 code cell missing in row %s" % name)
            self.assertEqual(int(code.group(1), 16), val,
                             "row %s: set-1 code != constant value in the doc itself" % name)
        # Exactly the pinned names appear in the table: a renamed/removed
        # row, or a newly named row, fails here and forces a deliberate
        # PINNED update.
        self.assertEqual(sorted(doc_by_name), sorted(self.PINNED),
                         "section 2.1 userspace-constant rows changed -- update PINNED deliberately")
        # The four arrows must be exactly the E0-extended rows (a11y arrow
        # navigation consumes these constants).
        self.assertEqual(sorted(e0_rows),
                         sorted(["SCAN_UP", "SCAN_LEFT", "SCAN_RIGHT", "SCAN_DOWN"]),
                         "section 2.1 E0-extended rows must be exactly the four arrows")
        # Doc table vs code: the scancode the doc names must equal the
        # constant the code defines -- neither can drift alone.
        for name in self.PINNED:
            self.assertEqual(self._rust_scan(name), doc_by_name[name],
                             "%s: doc table != input/mod.rs (kernel-keyboard-gate.md section 2.1)" % name)


class TestPhaseBRoutingHarness(unittest.TestCase):
    """Pins the sendkey routing probes added to the QEMU harnesses + CI gates.

    The Phase B open question (kernel-keyboard-gate.md section 6) is answered
    on every CI run via the one-shot [KBD] IRQ1 fired! marker; these pins keep
    the probes wired so a future edit cannot silently delete them. Also pins
    the Tcl bracket-escaping fix: an unescaped [KBD] inside a double-quoted
    send_user string is Tcl command substitution and would crash the harness
    with "invalid command name KBD".
    """

    @classmethod
    def setUpClass(cls):
        cls.login = open(os.path.join(REPO_ROOT, "tests", "qemu_gui_login.exp"), encoding="utf-8").read()
        cls.probe = open(os.path.join(REPO_ROOT, "tests", "probe_console_login.exp"), encoding="utf-8").read()
        cls.gate = open(os.path.join(REPO_ROOT, "tests", "qemu_gui_gate.exp"), encoding="utf-8").read()
        cls.lm_src = open(os.path.join(REPO_ROOT, "login-manager", "src", "main.rs"), encoding="utf-8").read()
        cls.ci = open(os.path.join(REPO_ROOT, ".github", "workflows", "ci.yml"), encoding="utf-8").read()

    def test_login_harness_has_chord_and_irq_probe(self):
        self.assertIn("proc sendkey_chord {mods key}", self.login)
        self.assertIn("sendkey_chord {ctrl alt} backspace", self.login)
        self.assertIn("proc monitor_cmd {cmd}", self.login)

    def test_gate_harness_has_irq_probe(self):
        self.assertIn("proc monitor_cmd {cmd}", self.gate)
        self.assertIn('monitor_cmd "sendkey shift"', self.gate)

    def test_irq_probe_absent_check_precedes_sendkey(self):
        # Airtight ordering (Aug 12, 2026): the one-shot IRQ1 marker can
        # false-PASS the routing probe by matching a STALE buffered copy
        # from early boot (gate.exp's match_max 1000000 keeps everything
        # in the expect buffer). Both harnesses must prove the marker is
        # ABSENT (expect -timeout 0) before the probe sendkey, so it is
        # demonstrated to appear only after the probe key.
        for name, text, anchor in (
            ("login", self.login, 'sendkey_seq "shift"'),
            ("gate", self.gate, 'monitor_cmd "sendkey shift"'),
            ("probe", self.probe, 'sendkey_seq "shift"'),
        ):
            check = text.index("expect -timeout 0 {")
            send = text.index(anchor)
            self.assertLess(
                check, send,
                "%s: IRQ1 absent-check must precede the probe sendkey" % name,
            )
            self.assertIn("stray IRQ1 marker already present", text)

    def test_gate_harness_has_tab_probe(self):
        # Phase B Tab/Enter (kernel-keyboard-gate.md section 3) is closed
        # on real input: one sendkey Tab must advance login-manager's
        # active field with the serial announce '[login] tab: focus ->
        # password'. Pin the exp probe, its PASS verdict, and the ci.yml
        # grep of that verdict (same phase-evidence pattern as vahid).
        self.assertIn('monitor_cmd "sendkey tab"', self.gate)
        self.assertIn("PASS: sendkey Tab advanced the login-manager field", self.gate)
        self.assertIn(r"\[login\] tab: focus -> password", self.gate)
        self.assertIn(
            'grep -q "PASS: sendkey Tab advanced the login-manager field"', self.ci
        )

    def test_gate_harness_has_dev_probe(self):
        # The /dev usability probe (Aug 10, 2026) proves the O_CREAT
        # fallback's six nodes exist AND work on a real boot: console
        # login (root/skyos), 'ls /dev' with six \\b-anchored regexps over
        # the accumulated buffer, and 'dd if=/dev/zero of=/dev/null' for
        # the usable-char-device check. A future edit that silently drops
        # any part (login, a node regexp, the dd, or the match_max bump
        # that keeps the whole boot in the buffer) fails here before any
        # QEMU run.
        gate = self.gate
        # \nmatch_max 1000000\n is the EXECUTABLE line: the bare string
        # also appears in two header comments, so a comment alone could
        # satisfy a looser needle after the real bump is dropped.
        self.assertIn(
            "\nmatch_max 1000000\n",
            gate,
            "match_max bump dropped - buffer no longer holds the boot markers",
        )
        # Console login flow (same root/skyos path the shell harness proves).
        self.assertIn('send "root\\r"', gate, "login flow root send dropped")
        self.assertIn("{Password:}", gate, "Password prompt arm dropped")
        self.assertIn('send "skyos\\r"', gate, "password send dropped")
        self.assertIn("{sash\\[}", gate, "sash prompt arm dropped")
        # Six \\b-anchored regexps over the accumulated buffer (ls /dev).
        for name in ("null", "zero", "random", "urandom", "tty", "console"):
            self.assertIn(
                "{\\b%s\\b}" % name,
                gate,
                "ls /dev regexp for %s dropped from the probe" % name,
            )
        self.assertIn(
            "$expect_out(buffer)",
            gate,
            "node regexps no longer scan the accumulated buffer",
        )
        # The regexps scan $expect_out(buffer) right after 'ls /dev' -
        # pin the SEND itself so they cannot be left scanning a stale buffer.
        self.assertIn('send "ls /dev\\r"', gate, "ls /dev send dropped")
        # Password discipline (credential-leak class): the password send
        # must sit between log_user 0 and log_user 1 so root's password
        # never lands in the CI log (same discipline as the shell harness).
        self.assertIn("log_user 0", gate)
        self.assertIn("log_user 1", gate)
        log0 = gate.index("log_user 0")
        sky = gate.index('send "skyos\\r"')
        log1 = gate.index("log_user 1")
        self.assertLess(log0, sky, "log_user 0 must precede the password send")
        self.assertLess(sky, log1, "log_user 1 must follow the password send")
        # FAIL arms must keep their exit 1: a probe failure silently
        # downgraded to PASS would let a broken /dev path through. Each
        # arm is pinned by its EXACT shape (an arm-scoped window could
        # reach the next arm's exit 1): login/ls arms are inline
        # '; exit 1 }', the dd arm is a block with exit 1 on the line
        # after the message.
        self.assertIn(
            'send_user "FAIL: no console login prompt for the /dev probe\\n"; exit 1',
            gate,
            "login-prompt FAIL arm lost its exit 1",
        )
        self.assertIn(
            "send_user \"FAIL: 'ls /dev' timed out\\n\"; exit 1",
            gate,
            "ls /dev FAIL arm lost its exit 1",
        )
        self.assertIn(
            "send_user \"FAIL: dd on /dev/zero -> /dev/null timed out "
            "(device not usable)\\n\"\n        exit 1",
            gate,
            "dd FAIL arm lost its exit 1",
        )
        # dd usability check: reads 16 bytes from /dev/zero into /dev/null.
        # Pin the SEND line - the bare string also appears in two header
        # audit comments, so a comment alone could satisfy a looser needle.
        self.assertIn(
            'send "dd if=/dev/zero of=/dev/null bs=16 count=1\\r"',
            gate,
            "dd zero->null usability probe dropped",
        )
        self.assertIn(
            "PASS: ls /dev shows all six nodes",
            gate,
            "six-node PASS verdict dropped",
        )

    def test_login_manager_tab_arm_announces_marker(self):
        # The Tab probe's observable is login-manager's serial announce;
        # if the Tab arm ever stops printing it, the gate would timeout
        # forever. Pin the source marker so the harness evidence cannot
        # drift.
        lm = open(
            os.path.join(REPO_ROOT, "login-manager", "src", "main.rs"),
            encoding="utf-8",
        ).read()
        self.assertIn("[login] tab: focus -> ", lm)

    def test_ci_verify_step_has_explicit_vahid_assertions(self):
        # The gate Verify step must assert vahid positively - both the
        # guest marker ('[vahid] ready') and the exp's phase PASS line
        # ('PASS: vahid device manager healthy') - not just the aggregate
        # verdict string. The guest marker alone cannot catch an exp edit
        # that drops the vahid phase (the marker is real guest output and
        # is present either way); only the exp PASS line proves the phase
        # ran. Dropping either grep must fail this host test.
        anchor = self.ci.index("Verify GUI gate verdict")
        # Slice at the next step boundary so the pin covers exactly this
        # step - a future edit that moves the greps into a later step (the
        # give-up Verify step follows) must fail here, not pass because the
        # greps are still somewhere downstream in the file.
        nxt = self.ci.find("\n      - name: ", anchor)
        block = self.ci[anchor:nxt] if nxt != -1 else self.ci[anchor:]
        self.assertIn('grep -q "\\[vahid\\] ready"', block)
        self.assertIn('grep -q "PASS: vahid device manager healthy"', block)

    def test_gate_exp_emits_vahid_phase_verdict(self):
        # Lockstep with the ci.yml grep #2: the gate exp must actually emit
        # the exact 'PASS: vahid device manager healthy' verdict line the
        # Verify step greps for. If the exp's verdict string ever changes,
        # every healthy boot fails the gate with a misleading message - this
        # host pin catches the drift first (same pattern as
        # test_vahid_contract's marker cross-pin).
        self.assertIn('send_user "PASS: vahid device manager healthy', self.gate)

    def test_bracket_escaping_in_tcl_strings(self):
        # Unescaped [KBD] in a double-quoted Tcl string = command substitution
        # = "invalid command name KBD" crash at runtime.
        self.assertIn('send_user "=== routing probe: \[KBD\] IRQ1 fired! ===\\n"', self.login)
        self.assertIn('send_user "=== routing probe: \[KBD\] IRQ1 fired! ===\\n"', self.gate)
        self.assertNotIn('send_user "=== routing probe: [KBD]', self.login)
        self.assertNotIn('send_user "=== routing probe: [KBD]', self.gate)

    def test_gate_state_tracked_loop_is_order_tolerant(self):
        # The GUI gate's boot-verdict phase is ONE order-tolerant loop
        # (Aug 10, 2026) collecting the vahid-health and login-manager
        # window markers: saw_vahid/saw_window flags, a single 240s
        # deadline, and an adaptive per-iteration timeout (min(30, rem)).
        # FATAL / init-give-up / panic fail immediately at any position.
        # A future edit that regresses the order-tolerance (back to two
        # sequential expects, a fixed timeout, or a dropped fail arm) is
        # caught here host-side, before any QEMU run.
        g = self.gate
        # 1. Flags + the order-tolerant loop condition.
        self.assertIn("set saw_vahid 0", g)
        self.assertIn("set saw_window 0", g)
        self.assertIn("while {!$saw_vahid || !$saw_window}", g)
        # 2. Single 240s deadline (replaces the old 120+120 sequential
        #    budget; a slow-but-healthy boot needs the headroom).
        self.assertIn("[clock seconds] + 240", g)
        # 3. Adaptive per-iteration timeout: the final wait expires exactly
        #    at the deadline (a fixed 30s would fail up to 30s late).
        self.assertIn("set rem [expr {$verdict_deadline - [clock seconds]}]", g)
        self.assertIn("expect -timeout [expr {$rem < 30 ? $rem : 30}]", g)
        # 4. Fail arms: vahid FATAL + init give-up, immediate exit 1 at any
        #    position.
        self.assertIn("{\\[vahid\\] FATAL:} {", g)
        self.assertIn("FAIL: vahid fatal device-scan failure", g)
        self.assertIn("-re {(?s)\\[init\\] giving up on .*?vahid}", g)
        self.assertIn("FAIL: init gave up on vahid after too many crashes", g)
        # 5. The window verdict arm sets the second flag (the order partner
        #    of vahid's ready).
        self.assertIn("{\\[login\\] window created} {", g)
        self.assertIn("set saw_window 1", g)
        # 6. Structure: flags init -> deadline -> loop -> adaptive timeout
        #    -> fail arms, so the loop really is one state-tracked body and
        #    not two sequential expects.
        self.assertLess(
            g.index("set saw_vahid 0"),
            g.index("[clock seconds] + 240"),
        )
        self.assertLess(
            g.index("[clock seconds] + 240"),
            g.index("while {!$saw_vahid || !$saw_window}"),
        )
        self.assertLess(
            g.index("while {!$saw_vahid || !$saw_window}"),
            g.index("expect -timeout [expr {$rem < 30 ? $rem : 30}]"),
        )
        self.assertLess(
            g.index("expect -timeout [expr {$rem < 30 ? $rem : 30}]"),
            g.index("{\\[vahid\\] FATAL:} {"),
        )




    def test_login_manager_window_markers_match_gate_patterns(self):
        # Source-to-harness cross-pin (mirrors test_giveup_gate.py's
        # marker cross-pin): the GUI gate's state loop greps the login
        # window verdict markers EXACTLY as login-manager prints them
        # (login-manager/src/main.rs:62 and :66 - io::print_str with the
        # literal \n), and the fail arm reads the '[login] mem free=N
        # pages' memory marker the same source prints right before
        # Window::create (:56). A rename or reformat in either file breaks
        # here before the QEMU job goes stale.
        g = self.gate
        lm = self.lm_src
        # 1. The two window verdict markers, escaped for Tcl braces.
        self.assertIn('io::print_str("[login] window created\\n")', lm)
        self.assertIn("{\\[login\\] window created} {", g)
        # The WHY marker (paired with [login] mem free=N): ENOMEM -> Out
        # of memory, any other errno -> errno {e}. Both keep the "failed
        # to create window" prefix the gate's unanchored pattern greps.
        self.assertIn('[login] failed to create window: Out of memory (errno 12)', lm)
        self.assertIn('[login] failed to create window: errno {}', lm)
        self.assertIn("{\\[login\\] failed to create window} {", g)
        # 2. The window-created arm must set the order partner of vahid's
        #    ready (saw_window), and the failed arm must FAIL immediately
        #    (the respawn loop is a hard gate, not a soft timeout).
        self.assertIn("set saw_window 1", g)
        self.assertIn("respawn loop", g)
        # 3. The memory-pressure marker the FAIL arm's evidence reads must
        #    match login-manager's print before Window::create (a bare
        #    "[login] mem free" without the "=N pages" shape would silently
        #    stop feeding the verdict's memory evidence).
        self.assertIn('[login] mem free={} pages\\n', lm)
        self.assertIn(r"\[login\] mem free=(\d+) pages", g)
        # 4. The audit table's source line refs must not drift from the
        #    markers they cite (currently :62/:66 - corrected from a stale
        #    :23/:27). Pin the refs' SHAPE (win_created/win_failed rows
        #    exist) so an editor updating them is forced to keep the
        #    markers in view.
        self.assertIn("#   5 win_created", g)
        self.assertIn("#   6 win_failed", g)

    def test_no_unescaped_brackets_in_executable_strings(self):
        # Scan-based companion to test_bracket_escaping_in_tcl_strings: EVERY
        # double-quoted Tcl string on an executable (non-comment) line must
        # have its brackets escaped. '[' inside "..." is Tcl command
        # substitution ("invalid command name X" crash), while '{...}' is
        # substitution-free, so only the double-quoted form is scanned.
        import re

        def unescaped_bracket_lines(text):
            hits = []
            for i, line in enumerate(text.splitlines(), 1):
                if line.lstrip().startswith("#"):
                    continue
                for m in re.finditer(r'"(?:[^"\\]|\\.)*"', line):
                    body = m.group(0)[1:-1]
                    j = 0
                    while j < len(body):
                        if body[j] == "\\":
                            j += 2
                            continue
                        if body[j] == "[":
                            hits.append(i)
                            break
                        j += 1
            return hits

        # Scan EVERY Tcl harness in tests/ (login/gate today; any new .exp
        # added later is covered automatically). Verified clean Aug 10, 2026:
        # qemu_shell_test.exp and qemu_ade_selftest.exp have zero hits; the
        # three login FAIL arms were the last survivors.
        import glob
        import os

        exp_dir = os.path.join(REPO_ROOT, "tests")
        for path in sorted(glob.glob(os.path.join(exp_dir, "*.exp"))):
            content = open(path, encoding="utf-8").read()
            name = os.path.basename(path)
            hits = unescaped_bracket_lines(content)
            self.assertEqual(
                hits, [],
                f"{name}: unescaped '[' in double-quoted executable string(s) "
                "at line(s) {hits} — Tcl command substitution crash",
            )

    def test_second_session_leg_present(self):
        # Step 7: after init respawns login-manager (which execs ade directly
        # on the GUI path - there is NO getty 'login:' prompt on respawn),
        # the harness waits for the SECOND '[ade] session established' marker
        # so the negative leg runs on a live desktop. This pins the marker
        # wait so a future edit can't silently drop the leg.
        self.assertIn('send_user "=== second session ===\\n"', self.login)
        self.assertIn("PASS: second session established", self.login)
        # No getty login may appear in the leg: the second session comes from
        # login-manager's exec of ade, not a console login.
        self.assertNotIn("PASS: second login prompt", self.login)

    def test_keyboard_window_open_chain_present(self):
        # Step 8: the ONLY path that opens a window on today's kernel
        # (arrows are E0-dropped, Ctrl+letter folds don't arrive) is the
        # a11y chain Tab (ring on Taskbar) -> Enter (start menu) -> type
        # "term" -> Enter (launch). The harness must drive exactly that
        # sequence and wait for the positive [ade] launched Terminal marker
        # (desktop.rs launch_app) before the negative probe.
        self.assertIn('sendkey_seq "tab"', self.login)
        self.assertIn('sendkey_seq "term"', self.login)
        self.assertIn('\\[ade\\] launched Terminal', self.login)
        self.assertIn("PASS: window opened via keyboard", self.login)

    def test_logout_leg_conditional_chord_with_esc_fallback(self):
        # Step 5 is KERNEL-DEPENDENT: the chord is tried first; the esc
        # fallback fires only when the chord is inert (kernel without
        # Design A A1-A4). The CI-visible $KERNEL_DESIGN_A toggle flips
        # the timeout arm: =1 asserts the chord MUST end the session (no
        # esc fallback), unset/0 keeps the esc fallback so the harness
        # stays green on the current kernel AND the day A1-A4 lands.
        leg = self.login[self.login.index("ending session (design_a="):]
        self.assertIn("sendkey_chord {ctrl alt} backspace", leg)
        self.assertIn('sendkey_seq "esc"', leg)
        self.assertLess(
            leg.index("sendkey_chord {ctrl alt} backspace"),
            leg.index('sendkey_seq "esc"'),
        )
        self.assertIn("PASS: session ended via ctrl+alt+backspace", leg)
        self.assertIn("PASS: session ended via esc fallback", leg)
        # The positive arms must expect the RICH unwind marker (code +
        # ending state printed by main.rs at unwind), cross-pinned against
        # the emit below so harness and main can't drift: exactly two
        # positive arms (chord + esc fallback) in this leg.
        self.assertEqual(
            leg.count("ending=true} {"), 2,
        )
        # (main.rs emit cross-pinned in TestInitRespawnContract.)
        # The gui-login Verify step greps the rich marker as
        # belt-and-suspenders after the verdict + IRQ1 check.
        self.assertIn(r"\[ade\] session ended code=0 ending=true", self.ci)
        self.assertIn("set chord_ended 0", leg)
        self.assertIn("if {!$chord_ended} {", leg)
        # The toggle must gate the fallback: in Design A mode the timeout
        # arm FAILs with a named message instead of falling back to esc.
        self.assertIn("FAIL: KERNEL_DESIGN_A=1 but the chord never ended the session", leg)
        # CI-visible plumbing: the login job must export the toggle.
        self.assertIn("export KERNEL_DESIGN_A=", self.ci)

    def test_negative_leg_chord_with_window_open(self):
        # Step 9: with a window open, the true ctrl+alt+backspace chord must
        # NOT end the session (wm.is_empty() guard holds on real input, the
        # QEMU counterpart of test_session_end_gate's synthetic window case).
        # The leg must FAIL on the marker, not accept it.
        leg = self.login[self.login.index("negative leg: chord with window open"):]
        self.assertIn("sendkey_chord {ctrl alt} backspace", leg)
        self.assertIn("FAIL: chord ended the session with a window open!", leg)
        self.assertIn("guard_holds - chord did not end session", leg)
        self.assertIn("PASS: guard_holds", leg)
        # The leg sits BEFORE the final PASS banner, so the suite verdict
        # only prints if the negative probe held.
        self.assertLess(
            self.login.index("negative leg: chord with window open"),
            self.login.index("GUI login integration: PASS"),
        )

    def test_ci_a11y_gate_includes_keyboard_window_open(self):
        # The selftests test_a11y_keyboard_window_open,
        # test_a11y_start_menu_rows and test_a11y_overlay_mouse_keyboard_parity
        # must be counted in the ci.yml PASS-name gate (20 names now); the
        # gate test (test_ade_selftest_gate.py) already pins set-equality
        # with run_all.
        self.assertIn("a11y_keyboard_window_open", self.ci)
        self.assertIn("a11y_start_menu_rows", self.ci)
        self.assertIn("a11y_overlay_mouse_keyboard_parity", self.ci)
        self.assertIn('"$a11y_passes" -lt 20', self.ci)
        self.assertIn("found $a11y_passes/20", self.ci)

    def test_ci_verify_steps_grep_irq_marker_after_verdict(self):
        # Both Verify steps grep the marker (escaped for BRE) and only after
        # the verdict PASS check, so an early boot death reports the verdict
        # failure rather than a misleading routing diagnosis.
        self.assertEqual(self.ci.count(r"\[KBD\] IRQ1 fired!"), 2, "both Verify steps grep the marker")
        self.assertEqual(self.ci.count(r'! grep -q "\[KBD\] IRQ1 fired!"'), 2)
        pass_idx = self.ci.find("GUI login integration: PASS")
        irq_idx = self.ci.find(r"\[KBD\] IRQ1 fired!", pass_idx)
        self.assertGreater(irq_idx, pass_idx, "login IRQ1 grep must come after the PASS check")


def port_reap_classification(status):
    """Faithful port of ade session.rs reap()'s classification of a reaped
    child: 0 -> ("terminated", None) (Clean, no crash notification);
    < 0 -> ("crashed", "killed"); > 128 -> ("crashed", "signal N") using
    the POSIX 128+sig convention; else -> ("crashed", "exit N"). Mirrors
    init's binary view: only status 0 avoids the crash path.
    """
    if status == 0:
        return "terminated", None
    if status < 0:
        return "crashed", "killed"
    if status > 128:
        return "crashed", f"signal {status - 128}"
    return "crashed", f"exit {status}"


class TestInitRespawnContract(unittest.TestCase):
    """Pins the session-end side of init's respawn accounting.

    ade's logout contract (EXIT_LOGOUT = 0, ade/src/service/session.rs) only
    holds because init's waitpid loop (init/src/main.rs) resets a service's
    crash counter on a CLEAN exit and accumulates crashes only toward
    MAX_RESPAWNS. These pins hold BOTH sides so a change to either cannot
    silently break the logout loop (session end -> exit 0 -> init sees a
    clean exit -> respawns login-manager -> ade again).

    Source contract (verified Aug 10, 2026, init/src/main.rs:132-145):

        // Clean exit (status == 0) means the service ran its course ...
        if status == 0 { svc.crashes = 0; }        // reset FIRST
        if svc.respawn {
            svc.crashes += 1;
            if svc.crashes > MAX_RESPAWNS {        // strictly greater
                svc.respawn = false;               // give up
            } else { nanosleep(500ms); spawn(); }
        }

    Consequences the pins rely on: a clean exit always lands at
    crashes == 1 (reset then +1), so give-up can never fire for an
    exit(0) service; only a non-zero (crash) exit accumulates toward the
    threshold. session.rs's EXIT_LOGOUT comment states the same contract
    in ade's terms ("init resets its crash counter and respawns the login
    service; non-zero = crash (init counts it toward MAX_RESPAWNS)") --
    these tests assert that cross-source agreement so the two sides can't
    drift apart.

    NOTE: init's view of an exit is BINARY (status == 0 vs everything
    else). It does not reuse session.rs's finer-grained exit_class
    (0 = clean, 1..=127 = exit code, 128+sig = killed by signal): a
    signal-killed service still has a non-zero waitpid status, so it is
    counted toward MAX_RESPAWNS just like a bad exit code. That matches
    the "non-zero (or signal) exits count" framing -- there is no status
    init treats as a crash that is not non-zero. Behaviorally pinned in
    test_vahid_contract.py::test_signal_killed_service_is_bounded_like_bad_exit
    and test_binary_view_agrees_with_exit_class.
    """

    @classmethod
    def setUpClass(cls):
        cls.init = open(INIT_RS, encoding="utf-8").read()
        cls.init_code = strip_rust(cls.init)
        cls.session = open(SESSION_RS, encoding="utf-8").read()
        cls.session_code = strip_rust(cls.session)
        cls.ade_main = open(ADE_MAIN_RS, encoding="utf-8").read()
        cls.ade_main_code = strip_rust(cls.ade_main)

    # --- init side (init/src/main.rs) ---
    def test_max_respawns_matches_source(self):
        # Deliberately pinned here too (not just in test_vahid_contract.py,
        # which ports the same state machine): this file holds the SESSION
        # side of the contract, and a MAX_RESPAWNS bump must fail loudly in
        # both places so neither side of the agreement drifts silently.
        self.assertIn(f"const MAX_RESPAWNS: u32 = {MAX_RESPAWNS};", self.init)

    def test_zero_status_resets_crash_counter_first(self):
        # The reset must run BEFORE the increment: a clean exit always
        # lands at crashes == 1, so give-up can never fire for an exit(0)
        # service -- the property that makes the logout loop unbounded
        # on purpose (login-manager's window-failure exit(0) loop).
        self.assertIn("// Clean exit (status == 0)", self.init)
        self.assertIn("if status == 0 {", self.init_code)
        self.assertIn("svc.crashes = 0;", self.init_code)
        reset = self.init_code.index("if status == 0 {")
        increment = self.init_code.index("svc.crashes += 1;")
        self.assertLess(
            reset, increment,
            "crash reset (status == 0) must come before the increment",
        )

    def test_crash_count_is_respawn_guarded_and_strictly_gt(self):
        # Counting happens only for respawnable services, and give-up
        # fires only when crashes STRICTLY exceeds MAX_RESPAWNS (the
        # counter holds 1..=5 while respawning; the 6th event gives up).
        self.assertIn("if svc.respawn {", self.init_code)
        self.assertIn("svc.crashes += 1;", self.init_code)
        self.assertIn("if svc.crashes > MAX_RESPAWNS {", self.init_code)
        self.assertIn("svc.respawn = false;", self.init_code)
        respawn = self.init_code.index("if svc.respawn {")
        increment = self.init_code.index("svc.crashes += 1;")
        give_up = self.init_code.index("if svc.crashes > MAX_RESPAWNS {")
        self.assertLess(respawn, increment)
        self.assertLess(increment, give_up)

    def test_give_up_disables_respawn_so_no_further_counting(self):
        # Structural pins for "give-up stops the loop": exactly ONE
        # increment site exists in init (there is no second counter to
        # advance after respawn is disabled), and `svc.respawn = false;`
        # sits INSIDE the strictly-greater branch, not in the else
        # (respawn) branch. (Behaviorally: init clears svc.pid on every
        # exit, so a post-give-up exit is orphaned, never recounted.)
        code = self.init_code
        self.assertEqual(
            code.count("svc.crashes += 1;"), 1,
            "init must have exactly one crash-count site",
        )
        give_up = code.index("svc.respawn = false;")
        branch_open = code.index("if svc.crashes > MAX_RESPAWNS {")
        self.assertGreater(give_up, branch_open)
        # respawn = false must not be inside the else/respawn branch: the
        # text between it and the give-up branch close must not reach the
        # else keyword that precedes the respawn call.
        between = code[branch_open:give_up]
        self.assertNotIn("} else {", between)

    # --- session side (ade/src/service/session.rs) ---
    def test_exit_logout_is_zero_matching_init_reset(self):
        # The session-side of the contract: a deliberate logout is exit
        # code 0, which init's waitpid loop treats as the clean-exit reset
        # (test_zero_status_resets_crash_counter_first). A non-zero code
        # would accumulate toward MAX_RESPAWNS and eventually kill the
        # login service until reboot.
        self.assertIn("const EXIT_LOGOUT: i32 = 0;", self.session)
        # The doc comment must state init's side accurately -- it is the
        # cross-source agreement these pins enforce.
        self.assertIn("0 = clean logout", self.session)
        self.assertIn("init resets its crash counter and respawns the login service", self.session)
        self.assertIn("init counts it toward", self.session)
        self.assertIn("pub fn exit_code(&self) -> i32", self.session)
        # exit_code returns EXIT_LOGOUT (the const), not a hardcoded 0.
        fn = self.session_code.split("pub fn exit_code", 1)[1]
        self.assertIn("EXIT_LOGOUT", fn)

    # --- the wiring between them (ade/src/main.rs) ---
    def test_ade_passes_session_exit_code_to_init(self):
        # main.rs's end-of-session path: the unwind marker carries the
        # session's exit code AND ending state to serial, so the CI grep
        # gates can assert the idempotent-unwind contract on real input --
        # exactly the value init's waitpid sees (EXIT_LOGOUT 0).
        self.assertIn('"[ade] session ended code={} ending={}\\n"', self.ade_main)
        self.assertIn("desktop.session.exit_code()", self.ade_main_code)
        self.assertIn("desktop.session.is_ending()", self.ade_main_code)
        # The comment on that line documents the contract in init terms.
        self.assertIn("init treats 0 as a clean exit and respawns", self.ade_main)

    def test_exit_code_const_matches_comment(self):
        # The const in session.rs and the comment in main.rs must agree on
        # 0: session.rs writes `const EXIT_LOGOUT: i32 = 0;` (colon before
        # the type), and main.rs's end-of-session comment spells out the
        # same value in init terms.
        self.assertIn("const EXIT_LOGOUT: i32 = 0;", self.session)
        self.assertIn("EXIT_LOGOUT = 0; init treats 0 as a clean exit", self.ade_main)


    def test_reap_dispatches_on_exit_class(self):
        # reap() classifies every reaped child through session.rs's
        # exit_class -- the same 128+sig convention init's binary view
        # counts as a crash. Clean children are terminated in place;
        # everything else falls into the crash branch.
        code = self.session_code
        self.assertIn("match exit_class(status) {", code)
        self.assertIn("ExitClass::Clean => self.lifecycle.mark_terminated(pid),", code)
        self.assertIn("cls => {", code)

    def test_only_non_clean_reap_notifies_crashed(self):
        # The "Application Crashed" notification sits ONLY in the non-Clean
        # branch: exactly one notify site, reachable only through
        # mark_crashed, never through the Clean arm's mark_terminated. A
        # clean exit is silent -- the session-side mirror of init's
        # status == 0 reset (only non-zero exits count toward give-up).
        code = self.session_code
        raw = self.session
        # strip_rust blanks string literals, so literal-bearing pins
        # (the notify title, the reason payloads) read the RAW source;
        # structural pins read the stripped code.
        self.assertEqual(raw.count('services.notify("Application Crashed"'), 1)
        clean = code.index("ExitClass::Clean => self.lifecycle.mark_terminated(pid),")
        crashed = code.index("self.lifecycle.mark_crashed(pid);")
        notify = code.index("services.notify(")  # survives stripping
        self.assertGreater(crashed, clean)
        self.assertGreater(notify, crashed)
        # The notify sits inside the `cls => {` crash branch, which
        # never touches mark_terminated (the Clean arm does). Slice from
        # the branch open to the notify so the Clean arm's own
        # mark_terminated is excluded.
        branch = code[code.index("cls => {"):notify]
        self.assertIn("mark_crashed", branch)
        self.assertNotIn("mark_terminated", branch)
        # Brace-balance the slice: the notify must sit inside at least one
        # open block. This closes the hoist-after-match hole -- moving the
        # notify out of the match (called unconditionally, clean exits
        # included) zeroes the balance and trips here, where the ordering
        # assertions above would pass vacuously (mark_terminated only lives
        # in the Clean arm, before the slice start).
        self.assertGreater(
            branch.count("{"), branch.count("}"),
            "notify hoisted outside the crash branch",
        )
        # The reason strings map the exit_class arms onto the notification
        # payload: Killed / Signal(128+sig) / Error(exit code).
        self.assertIn('alloc::string::String::from("killed")', raw)
        self.assertIn('alloc::format!("signal {}", sig)', raw)
        self.assertIn('alloc::format!("exit {}", code)', raw)

    def test_reap_classification_matches_behavior(self):
        # Behavioral sweep of the reap classification: only status 0 is
        # Clean (terminated, no notification); every other status -- a bad
        # exit code, a 128+sig signal kill, or a negative kill -- is a
        # crash with the documented reason, exactly the set init's binary
        # view counts toward MAX_RESPAWNS.
        cases = [
            (0, "terminated", None),
            (1, "crashed", "exit 1"),
            (42, "crashed", "exit 42"),
            (127, "crashed", "exit 127"),
            (128, "crashed", "exit 128"),
            (130, "crashed", "signal 2"),    # SIGINT
            (137, "crashed", "signal 9"),    # SIGKILL
            (143, "crashed", "signal 15"),   # SIGTERM
            (-1, "crashed", "killed"),
            (-9, "crashed", "killed"),
        ]
        for status, outcome, reason in cases:
            self.assertEqual(
                port_reap_classification(status), (outcome, reason),
                f"status {status}",
            )
        # Binary-view agreement with init: Clean iff status == 0.
        for status in (0, 1, 130, 137, -9):
            is_clean = status == 0
            self.assertEqual(
                port_reap_classification(status)[0] == "terminated", is_clean,
                f"status {status}: reap classification vs init binary view disagree",
            )


class TestOption2bDocDiff(unittest.TestCase):
    """Pins that the Option 2b draft diff in kernel-gui-window-fix.md
    still applies cleanly to login-manager/src/main.rs, so the doc patch
    cannot drift from the live source. (Draft patch, NOT applied - it is
    the userspace companion to kernel Option 2, mutually exclusive with
    the recommended Option 1.)"""

    DOC = os.path.join(REPO_ROOT, "ade", "docs", "kernel-gui-window-fix.md")

    def _extract_diff(self, header, doc=None):
        """Extract the 4-space-indented diff block under a '## <header>'
        section of a doc (default kernel-gui-window-fix.md). Anchors are
        pinned with clean FAILs: a renamed header or dropped fence fails
        with a message, not a ValueError ERROR."""
        path = doc or self.DOC
        name = os.path.basename(path)
        with io.open(path, encoding="utf-8") as fh:
            s = fh.read()
        self.assertIn(header, s,
                        "%s section header renamed in %s" % (header, name))
        head = s.index(header)
        self.assertIn("```diff", s[head:],
                        "no diff fence after the %s header" % header)
        fence = s.index("```diff", head)
        self.assertIn("```", s[fence + 8 :],
                        "%s diff fence never closed" % header)
        close = s.index("```", fence + 8)
        block = s[fence + len("```diff\n") : close]
        lines = []
        for ln in block.splitlines():
            if not ln.startswith("    "):
                raise AssertionError(
                    "%s diff line not 4-space markdown-indented: %r" % (header, ln)
                )
            lines.append(ln[4:])
        return "\n".join(lines) + "\n"

    def _extract_2b_diff(self):
        return self._extract_diff("## Patch Option 2b")

    def test_option2b_diff_applies_cleanly_to_login_manager(self):
        diff = self._extract_2b_diff()
        # Guard the extraction: the block must target login-manager.
        self.assertIn("--- a/login-manager/src/main.rs", diff)
        self.assertIn("+++ b/login-manager/src/main.rs", diff)
        # Name the drift point in the live source (git apply would fail
        # anyway; these make the diagnosis specific).
        lm = open(
            os.path.join(REPO_ROOT, "login-manager", "src", "main.rs"), encoding="utf-8"
        ).read()
        self.assertIn("use libsarga::auth::{BACKOFF_NS, MAX_FAILED_ATTEMPTS};", lm)
        self.assertIn('[login] failed to create window: Out of memory (errno 12)', lm)
        self.assertIn("return 0;", lm)
        # The doc's hunk context must apply to the working tree.
        # input=bytes, NOT text: on Windows the text-mode pipe translates
        # \\n -> \\r\\n and corrupts the patch (git apply then reports
        # 'patch failed' on hunk 1 despite the diff being valid).
        r = subprocess.run(
            ["git", "apply", "--check", "--whitespace=nowarn", "-"],
            cwd=REPO_ROOT,
            input=diff.encode("utf-8"),
            capture_output=True,
        )
        self.assertEqual(
            r.returncode,
            0,
            "Option 2b diff in kernel-gui-window-fix.md no longer applies to "
            "login-manager/src/main.rs - the doc drifted from the live source: %s"
            % r.stderr.decode("utf-8", "replace").strip()[:300],
        )

    def test_option2_diff_applies_cleanly_to_kernel(self):
        """The Option 2 kernel hunk (sys_gui_create_window -> -ENOMEM) must
        keep applying to the kernel tree. Same difflib-verified treatment as
        Option 2b: the doc block is generated from the live source, so any
        drift (doc context edited, or the kernel region rewritten) trips here
        before the rewrite can pick up a stale patch."""
        # Full header, NOT the short '## Patch Option 2': that string is a
        # prefix of '## Patch Option 2b', so a deleted or reordered Option 2
        # section would pass the header assert while silently extracting the
        # 2b block (same substring false-fire class as the mknod SYS_MKNOD /
        # SYS_MKNODAT fix).
        diff = self._extract_diff("## Patch Option 2 (alternative)")
        # Guard the extraction: the block must target the kernel syscall.
        self.assertIn("--- a/kernel/src/syscalls/mod.rs", diff)
        self.assertIn("+++ b/kernel/src/syscalls/mod.rs", diff)
        # Name the drift point (git apply would fail anyway; this makes the
        # diagnosis specific).
        self.assertIn("return errno::Errno::ENOMEM as u64;", diff)
        self.assertIn("-        win.content = Some(alloc::vec![0; content_len].into_boxed_slice());", diff)
        # Locate the kernel tree: env override, then the same siblings the
        # mknod/devfs pins in test_vahid_contract.py use.
        env = os.environ.get("SKYOS_KERNEL_DIR")
        candidates = [env] if env else []
        parent = os.path.dirname(REPO_ROOT)
        candidates += [
            os.path.join(parent, "SKYIOUS KERNEL"),
            os.path.join(parent, "SKYIOUS-KERNEL"),
            os.path.join(parent, "SKYIOUS_KERNEL"),
        ]
        root = next((c for c in candidates if os.path.isfile(os.path.join(c, "kernel", "src", "syscalls", "mod.rs"))), None)
        if root is None:
            self.skipTest("kernel tree not found (SKYOS_KERNEL_DIR or a "
                          "SKYIOUS KERNEL sibling); CI checks it out, so CI has teeth")
            return
        # input=bytes, NOT text: on Windows the text-mode pipe translates
        # \n -> \r\n and corrupts the patch (see the Option 2b test).
        r = subprocess.run(
            ["git", "apply", "--check", "--whitespace=nowarn", "-"],
            cwd=root,
            input=diff.encode("utf-8"),
            capture_output=True,
        )
        self.assertEqual(
            r.returncode,
            0,
            "Option 2 diff in kernel-gui-window-fix.md no longer applies to "
            "kernel/src/syscalls/mod.rs - the doc drifted from the kernel "
            "source: %s"
            % r.stderr.decode("utf-8", "replace").strip()[:300],
        )

    def test_lowwater_diff_applies_cleanly_to_kernel(self):
        """The /ctl/sys/mem/lowwater draft diff in kernel-mem-lowwater.md
        must keep applying to the kernel tree (buddy.rs + ctlfs.rs). Same
        difflib-verified treatment as Option 2/2b: the doc block is
        generated from the live source, so any drift (doc context edited, or
        the kernel region rewritten) trips here before the rewrite can pick
        up a stale patch."""
        doc = os.path.join(REPO_ROOT, "ade", "docs", "kernel-mem-lowwater.md")
        # Full section header, NOT the short '## 3. Patch': assert the
        # generated marker so a renumbered header trips cleanly.
        diff = self._extract_diff(
            "## 3. Patch (difflib-generated, `git apply --check` verified)",
            doc=doc,
        )
        # Guard the extraction: the block must target the two kernel files.
        self.assertIn("--- a/kernel/src/memory/buddy.rs", diff)
        self.assertIn("+++ b/kernel/src/memory/buddy.rs", diff)
        self.assertIn("--- a/kernel/src/vfs/ctlfs.rs", diff)
        self.assertIn("+++ b/kernel/src/vfs/ctlfs.rs", diff)
        # Name the drift points (git apply would fail anyway; these make the
        # diagnosis specific).
        self.assertIn("+    min_free_pages: usize,", diff)
        self.assertIn('add_child(&mem_dir, "lowwater", file_fn(|| {', diff)
        # Locate the kernel tree: env override, then the same siblings the
        # mknod/devfs pins in test_vahid_contract.py use.
        env = os.environ.get("SKYOS_KERNEL_DIR")
        candidates = [env] if env else []
        parent = os.path.dirname(REPO_ROOT)
        candidates += [
            os.path.join(parent, "SKYIOUS KERNEL"),
            os.path.join(parent, "SKYIOUS-KERNEL"),
            os.path.join(parent, "SKYIOUS_KERNEL"),
        ]
        root = next((c for c in candidates if os.path.isfile(
            os.path.join(c, "kernel", "src", "memory", "buddy.rs"))), None)
        if root is None:
            self.skipTest("kernel tree not found (SKYOS_KERNEL_DIR or a "
                          "SKYIOUS KERNEL sibling); CI checks it out, so CI has teeth")
            return
        # input=bytes, NOT text: on Windows the text-mode pipe translates
        # \n -> \r\n and corrupts the patch (see the Option 2b test).
        r = subprocess.run(
            ["git", "apply", "--check", "--whitespace=nowarn", "-"],
            cwd=root,
            input=diff.encode("utf-8"),
            capture_output=True,
        )
        self.assertEqual(
            r.returncode,
            0,
            "lowwater diff in kernel-mem-lowwater.md no longer applies to "
            "kernel/src/memory/buddy.rs + kernel/src/vfs/ctlfs.rs - the doc "
            "drifted from the kernel source: %s"
            % r.stderr.decode("utf-8", "replace").strip()[:300],
        )

    def test_option2b_draft_build_job_pinned(self):
        """The CI job that proves Option 2b COMPILES (not just applies) must
        stay wired: the job runs tests/build_option2b_draft.py, which must
        extract via the pinned extractor, scratch-copy the workspace, apply
        the diff, and cargo build -p login-manager with the same flags the
        build job uses. A future edit that drops the build job, retargets
        the script, or skips the cargo step fails here before any CI run."""
        ci = io.open(
            os.path.join(REPO_ROOT, ".github", "workflows", "ci.yml"),
            encoding="utf-8").read()
        # Guard the script's existence with a named clean FAIL: an io.open
        # on a deleted file would ERROR (unhandled FileNotFoundError), and
        # this session's anchor convention is named FAILs, not ERRORs.
        script_path = os.path.join(REPO_ROOT, "tests", "build_option2b_draft.py")
        self.assertTrue(os.path.isfile(script_path),
                        "tests/build_option2b_draft.py deleted")
        script = io.open(script_path, encoding="utf-8").read()

        # The job exists and runs the script.
        self.assertIn("option2b-draft-build:", ci,
                      "CI option2b-draft-build job removed")
        self.assertIn("python3 tests/build_option2b_draft.py", ci,
                      "CI job no longer runs tests/build_option2b_draft.py")

        # The script is wired to the pieces that give it teeth:
        # 1. extract via the same pinned extractor (no drift from the apply
        #    pin). Needles are the exact code forms, NOT the bare class name:
        #    the docstring mentions TestOption2bDocDiff, so a bare assertIn
        #    would be satisfied by the comment alone after the import died.
        self.assertIn("import test_login_flow as tlf", script,
                      "build script no longer imports the pinned extractor")
        self.assertIn("tlf.TestOption2bDocDiff()", script,
                      "build script no longer instantiates the pinned extractor")
        self.assertIn('extractor._extract_diff("## Patch Option 2b")', script,
                      "build script no longer extracts the Option 2b block")
        # 2. scratch copy + apply. The needle is the CALL site, not the
        #    bare name: `def make_scratch` would satisfy the bare string
        #    even after the call was dropped (silently re-copying nothing).
        self.assertIn("make_scratch(scratch)", script,
                      "build script no longer scratch-copies the workspace")
        self.assertIn('["git", "apply", "--whitespace=nowarn", "-"]', script,
                      "build script no longer applies the diff")
        # 3. cargo build -p login-manager with the build job's flags
        self.assertIn('"cargo", "build", "-p", "login-manager"', script,
                      "build script no longer cargo-builds login-manager")
        self.assertIn("-Zbuild-std=core,alloc", script,
                      "build script dropped the build-std flag")
        # Code form, not the bare file name: the docstring also mentions
        # x86_64-sarga.json, so the bare string stays satisfied after the
        # build command was retargeted to another spec.
        self.assertIn('"--target", TARGET_JSON', script,
                      "build script no longer targets the sarga JSON spec")
        # 4. failing build must fail the script (non-zero exit). The needle
        #    is the message+exit BLOCK, not a bare sys.exit(1): the script
        #    legitimately exits 1 on the apply-fail path too, so a bare
        #    needle would stay satisfied if the compile-fail path lost its
        #    exit (silently turning a non-compiling draft into exit 0).
        self.assertIn('print("=== Option 2b draft does NOT compile ===")\n        sys.exit(1)',
                      script,
                      "build script no longer exits non-zero on a failed build")

        # The script must live next to the other host tests so CI's working
        # directory resolves it (job uses working-directory: SkyOS).
        self.assertIn("tests/build_option2b_draft.py", ci,
                      "CI no longer references the script by that path")


if __name__ == "__main__":
    unittest.main()
