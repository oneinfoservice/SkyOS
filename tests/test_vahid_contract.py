#!/usr/bin/env python3
"""Host-runnable unit tests for vahid's exit-code discipline (no QEMU).

Pins the contract between vahid (vahid/src/main.rs) and init's respawn
accounting (init/src/main.rs): a FATAL device-scan failure must exit
NON-ZERO so init counts a crash and eventually gives up after
MAX_RESPAWNS, while a healthy vahid never exits (infinite sleep loop), so
init never sees an exit event and never respawns a working service.

The kernel-side respawn semantics (init/src/main.rs waitpid loop) are:

    status == 0  -> svc.crashes = 0  (clean exit resets the counter)
    svc.crashes += 1
    crashes > MAX_RESPAWNS -> give up (respawn = false)

So a service that exits 0 is treated as "ran its course" and respawns
forever; only a NON-ZERO exit accumulates crashes toward the give-up
threshold. vahid's discipline: exit(1) on fatal scan failure, never exit
on the healthy path. These tests assert the source contract so a future
refactor cannot silently flatten the exit code or drop the status lines.

The end-to-end semantics are pinned as a faithful port of init's
accounting (`RespawnAccounting`) — same order, same conditions — and
validated against the two real services: login-manager's exit(0)
window-failure loop is UNBOUNDED (give-up can never fire), while vahid's
exit(1) fatal path is BOUNDED (5 respawns, then give up). Signal kills
(128+sig, per session.rs's exit_class) accumulate crashes identically to
a bad exit code because init's view of an exit is binary -- pinned
behaviorally in test_signal_killed_service_is_bounded_like_bad_exit.

Run:  python3 tests/test_vahid_contract.py
"""
import os
import re
import unittest

# Single authoritative Python mirror of init/src/main.rs's MAX_RESPAWNS
# (tests/constants.py); test_port_matches_source_max_respawns keeps it in
# lockstep with the source.
from constants import MAX_RESPAWNS

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
VAHID_RS = os.path.join(REPO_ROOT, "vahid", "src", "main.rs")
INIT_RS = os.path.join(REPO_ROOT, "init", "src", "main.rs")
SESSION_RS = os.path.join(REPO_ROOT, "ade", "src", "service", "session.rs")
SYSCALL_RS = os.path.join(REPO_ROOT, "libsarga", "src", "syscall.rs")
LIBSARGA_LIB_RS = os.path.join(REPO_ROOT, "libsarga", "src", "lib.rs")


def _read_kernel(rel):
    """Locate the kernel tree (SKYOS_KERNEL_DIR env override, then
    siblings of this repo: the local 'SKYIOUS KERNEL' and the CI
    'SKYIOUS-KERNEL' checkouts) and read the file at rel inside it.
    Returns None when the tree is absent so the caller can skip with
    a reason instead of failing spuriously."""
    env = os.environ.get("SKYOS_KERNEL_DIR")
    candidates = [env] if env else []
    parent = os.path.dirname(REPO_ROOT)
    candidates += [
        os.path.join(parent, "SKYIOUS KERNEL"),
        os.path.join(parent, "SKYIOUS-KERNEL"),
        os.path.join(parent, "SKYIOUS_KERNEL"),
    ]
    for root in candidates:
        p = os.path.join(root, rel)
        if os.path.isfile(p):
            return _read(p)
    return None


def _read_kernel_numbers():
    """Kernel syscall numbers table (mknod-absence leg)."""
    return _read_kernel(os.path.join("kernel", "src", "syscalls", "numbers.rs"))


def _read(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


def port_exit_class(status):
    """Faithful port of ade session.rs's exit_class -- the fine-grained
    view init deliberately does NOT reuse (init sees status == 0 vs
    everything else):

        status == 0   -> clean
        status < 0    -> killed (kernel-reported)
        status > 128  -> signal N   (128 + N: POSIX 128+sig convention)
        else          -> error N    (non-zero exit code)
    """
    if status == 0:
        return "clean"
    if status < 0:
        return "killed"
    if status > 128:
        return f"signal_{status - 128}"
    return "error_%d" % status


class RespawnAccounting:
    """Faithful port of init/src/main.rs's waitpid-loop accounting.

    Mirrors the exact order and conditions of the source:

        if status == 0 { svc.crashes = 0; }   // reset FIRST
        if svc.respawn {
            svc.crashes += 1;
            if svc.crashes > MAX_RESPAWNS {    // strictly greater
                svc.respawn = false;           // give up (no spawn)
            } else {
                nanosleep(500ms); spawn();
            }
        }

    Consequences the tests rely on: a clean exit always lands at
    crashes == 1 (reset then +1), so give-up can never fire for an
    exit(0) service; a non-zero exit accumulates 1..6, respawning on
    1..5 and giving up on the 6th (exactly MAX_RESPAWNS respawns).

    Two source details are deliberately elided: the 500 ms nanosleep
    between respawns (wall-clock only, irrelevant to the counts) and
    pid tracking (init clears svc.pid on exit; the port models only the
    crash-counter state machine).

    Signal case: a signal-killed service reports 128+sig (130 = SIGINT,
    137 = SIGKILL, 143 = SIGTERM -- the convention session.rs's
    exit_class classifies as `Signal`). init's view is BINARY, so any
    such non-zero status accumulates a crash exactly like a bad exit
    code; pinned behaviorally in
    test_signal_killed_service_is_bounded_like_bad_exit.
    """

    def __init__(self):
        self.crashes = 0
        self.respawn = True
        self.respawns = 0
        self.gave_up = False

    def on_exit(self, status):
        """One service-exit event. Returns 'respawn', 'gave_up', or
        'no_respawn' (respawn already disabled — post-give-up or a
        non-respawnable service; in init, svc.pid was cleared so no
        further exit events arrive anyway)."""
        if status == 0:
            self.crashes = 0
        if not self.respawn:
            return "no_respawn"
        self.crashes += 1
        if self.crashes > MAX_RESPAWNS:
            self.respawn = False
            self.gave_up = True
            return "gave_up"
        self.respawns += 1
        return "respawn"


class VahidExitCodeContract(unittest.TestCase):
    def setUp(self):
        self.vahid = _read(VAHID_RS)
        self.init = _read(INIT_RS)

    # --- vahid side: the exit-code + status-line contract ---

    def test_fatal_exit_code_is_non_zero(self):
        # The whole point: init only accumulates crashes (toward give-up)
        # on a NON-ZERO exit. A zero exit would reset the counter and
        # respawn a broken vahid forever.
        self.assertIn("const EXIT_DEVICE_SCAN_FAILED: i32 = 1;", self.vahid)
        self.assertIn("process::exit(EXIT_DEVICE_SCAN_FAILED)", self.vahid)

    def test_fatal_path_prints_status_line_before_exit(self):
        # The status line must be emitted before the exit so the serial log
        # records WHY vahid died (not just a bare exit code).
        fatal_line = self.vahid.index('[vahid] FATAL: failed to create device nodes')
        exit_call = self.vahid.index("process::exit(EXIT_DEVICE_SCAN_FAILED)")
        self.assertLess(fatal_line, exit_call)

    def test_healthy_path_never_exits(self):
        # After "[vahid] ready", the sleep loop must be the last thing —
        # no process::exit and no fallthrough return on the healthy path.
        ready = self.vahid.index("[vahid] ready")
        tail = self.vahid[ready:]
        self.assertNotIn("process::exit", tail)
        self.assertNotIn("return 0", tail)
        self.assertIn("loop {", tail)
        self.assertIn("nanosleep", tail)

    def test_scan_pci_reports_failure_instead_of_silent_skip(self):
        # scan_pci must return Option (Some(count) / None) so a missing
        # sysfs is an observable degraded state, not a swallowed error.
        self.assertIn("fn scan_pci() -> Option<usize>", self.vahid)
        self.assertIn("None", self.vahid)

    def test_create_devices_reports_success(self):
        # create_devices must return bool so the FATAL branch has a real
        # signal (previously it returned () and swallowed every error).
        self.assertIn("fn create_devices() -> bool", self.vahid)
        self.assertIn("all_ok", self.vahid)

    def test_status_markers_present(self):
        for marker in (
            "[vahid] SkyOS Device Manager",
            "[vahid] Scanning PCI...",
            "[vahid] ready",
            "[vahid] FATAL: failed to create device nodes",
        ):
            self.assertIn(marker, self.vahid, "missing marker: " + marker)

    def test_ready_marker_printed_exactly_once(self):
        # The healthy-path marker the harnesses grep ('[vahid] ready') is
        # vahid's terminal healthy state and MUST print exactly once: the
        # gate's state loop sets saw_vahid on the first occurrence, and a
        # second print would mask a marker-garbling regression behind the
        # duplicate. A future edit that adds an extra announce fails here
        # before any QEMU run.
        self.assertEqual(
            self.vahid.count("[vahid] ready"),
            1,
            "'[vahid] ready' must print exactly once on the healthy path",
        )

    def test_fatal_marker_printed_exactly_once_before_exit(self):
        # The fatal-path marker the gate greps ('[vahid] FATAL:') must print
        # exactly once, BEFORE the non-zero exit init's accounting counts
        # toward MAX_RESPAWNS. The order is the diagnostic contract: the
        # serial log must record WHY vahid died before the exit lands.
        fatal = self.vahid.index("[vahid] FATAL: failed to create device nodes")
        exit_call = self.vahid.index("process::exit(EXIT_DEVICE_SCAN_FAILED)")
        self.assertLess(fatal, exit_call)
        self.assertEqual(
            self.vahid.count("[vahid] FATAL:"),
            1,
            "'[vahid] FATAL:' must print exactly once on the fatal path",
        )

    def test_harness_grep_patterns_match_source_markers(self):
        # Cross-pin: the gate and shell harnesses grep these markers —
        # qemu_gui_gate.exp's state loop and qemu_shell_test.exp's
        # accumulated-buffer regexp. The escaped Tcl patterns must stay in
        # lockstep with the source strings, so a marker rename breaks here
        # before the QEMU jobs go stale.
        gate = _read(os.path.join(REPO_ROOT, "tests", "qemu_gui_gate.exp"))
        shell = _read(os.path.join(REPO_ROOT, "tests", "qemu_shell_test.exp"))
        self.assertIn(r"\[vahid\] ready", gate)
        self.assertIn(r"\[vahid\] FATAL:", gate)
        self.assertIn(r"\[vahid\] ready", shell)
        # The shell harness asserts HEALTHY vahid only; the fatal marker is
        # deliberately a GUI-gate concern. Adding it to the shell harness
        # must be a conscious change, not a silent drift.
        self.assertNotIn(r"\[vahid\] FATAL:", shell)

    def test_shell_harness_vahid_ready_mechanism(self):
        # Host-runnable pin for qemu_shell_test.exp's vahid_ready check
        # (mirrors test_giveup_gate.py's exp-content assertions). Three
        # parts must all stay in the shell harness or the device-manager
        # coverage silently stops being exercised by the shell job:
        #
        # 1. match_max bump: '[vahid] ready' prints EARLY in the boot
        #    (vahid is spawned first), long before the login prompt. The
        #    expect default buffer is 2000 bytes, so without the bump the
        #    marker rolls out of $expect_out(buffer) and the check below
        #    would false-FAIL every boot. A harness edit that drops the
        #    bump (or shrinks it below what the boot log needs) fails here.
        # 2. accumulated-buffer regexp: the check greps $expect_out(buffer)
        #    -- the whole boot from spawn -- not a fresh expect. An edit
        #    that rewrites the check to a sequential expect would lose the
        #    early marker and must fail here.
        # 3. audit entry 14: the header's PER-CHECK AUDIT table documents
        #    the vahid_ready pattern as REAL (vahid/src/main.rs:108). A
        #    marker rename must break the audit pin before the QEMU job.
        shell = _read(os.path.join(REPO_ROOT, "tests", "qemu_shell_test.exp"))
        # 1. match_max bump (any value >= 100_000 keeps the whole boot).
        m = re.search(r"^match_max (\d+)$", shell, re.MULTILINE)
        self.assertIsNotNone(m, "match_max bump missing from the shell harness")
        self.assertGreaterEqual(
            int(m.group(1)), 100_000,
            "match_max must keep the whole boot in the expect buffer",
        )
        # 2. accumulated-buffer regexp grep (the mechanism that sees the
        #    early marker).
        self.assertIn(
            r'regexp -- {\[vahid\] ready} $expect_out(buffer)',
            shell,
            "vahid_ready must grep the ACCUMULATED expect buffer",
        )
        # 3. audit entry 14 in the PER-CHECK AUDIT header.
        self.assertIn("#  14 vahid_ready", shell)
        self.assertIn("vahid/src/main.rs:108", shell)
        # 4. Ordering: the bump must be set BEFORE the vahid_ready regexp
        #    check. A move that puts match_max below the check would leave
        #    the buffer at the 2000-byte default when the marker rolls past
        #    -- every value/pattern assertion above would still pass while
        #    the mechanism silently breaks. The bump is at the top of the
        #    harness (line ~98) and the check after 'login:' (~line 135).
        self.assertLess(
            shell.index("match_max"),
            shell.index("regexp --"),
            "match_max must be set before the vahid_ready buffer grep",
        )

    def test_shell_harness_mem_marker_mechanism(self):
        # Host-runnable pin for qemu_shell_test.exp's mem_marker check
        # (the console getty's boot-time OOM evidence, login/src/main.rs
        # startup). Same three-part shape as the vahid_ready pin:
        # accumulated-buffer grep + fail arm + audit entry. A harness edit
        # that drops the check, turns it into a sequential expect, or
        # renames the marker fails here before the shell QEMU job.
        shell = _read(os.path.join(REPO_ROOT, "tests", "qemu_shell_test.exp"))
        # 1. accumulated-buffer regexp grep (the same mechanism that sees
        #    the early '[vahid] ready' marker; the getty marker also prints
        #    before 'login:').
        self.assertIn(
            r'regexp -- {\[login\] mem free=} $expect_out(buffer)',
            shell,
            "mem_marker must grep the ACCUMULATED expect buffer",
        )
        # 2. hard fail arm (a missing marker fails the shell job). The
        #    needle carries the ESCAPED brackets ('\[login\]') exactly as
        #    the exp writes them - unescaped '[' in a double-quoted Tcl
        #    string is command substitution and would crash the harness
        #    (the bracket-scan pin in test_login_flow.py enforces this).
        self.assertIn(
            r"FAIL: mem_marker - '\[login\] mem free=' missing from accumulated boot log",
            shell,
        )
        # 2b. the fail arm must EXIT (the /dev-probe lesson: a FAIL message
        #     without exit 1 lets a missing marker pass the job silently).
        self.assertIn(
            "send_user \"FAIL: mem_marker - '\\[login\\] mem free=' missing from "
            "accumulated boot log\\n\"\n    exit 1",
            shell,
        )
        # 3. audit entry 15 in the PER-CHECK AUDIT header, citing the
        #    getty source (login/src/main.rs user_main startup). The needle
        #    is the FULL row: the bare '#  15 mem_marker' is a prefix of a
        #    renamed '#  15 mem_markerX' and would stay satisfied.
        self.assertIn(r"#  15 mem_marker    \[login\] mem free=", shell)
        self.assertIn("login/src/main.rs (getty", shell)
        # 4. Ordering: the accumulated-buffer grep must come after the
        #    'login:' expect (the marker is in the buffer by then) and the
        #    match_max bump must precede it (keeps the whole boot).
        self.assertLess(
            shell.index('check "boot_login"'),
            shell.index("regexp --"),
            "mem_marker grep must run after the login prompt is seen",
        )

    def test_shell_ps1_ports_vahid_and_reprompt_checks(self):
        # The local Windows harness tests/qemu_shell_test.ps1 mirrors the
        # exp's two device-manager/getty assertions in PowerShell's
        # line-streaming model. All of these must stay or local runs lose
        # coverage CI still has:
        #
        #  1. vahid-healthy accumulated-log grep: a results-table entry
        #     matching '[vahid] ready' against the whole accumulated serial
        #     log ($fullOutput), the ps1 equivalent of the exp's
        #     regexp-over-buffer.
        #  2. mistype re-prompt probe: the FIRST password prompt gets a
        #     wrong password; 'Login incorrect' arms the probe; a fresh
        #     'login:' must arrive WITHOUT '[init] starting service: getty'
        #     (the MAX_RESPAWNS guard). A ps1 edit that drops the probe or
        #     reverts to the direct correct-password login fails here.
        ps1 = _read(os.path.join(REPO_ROOT, "tests", "qemu_shell_test.ps1"))
        # 1. vahid accumulated-log grep.
        self.assertIn("vahid healthy (device manager)", ps1)
        self.assertIn(r"\[vahid\] ready", ps1)
        # 1b. getty memory-pressure marker grep (the exp's mem_marker
        #     port): the console getty's '[login] mem free=' startup read
        #     must be asserted against the accumulated serial log so local
        #     runs collect the same OOM evidence CI does.
        self.assertIn("getty memory-pressure marker", ps1)
        self.assertIn(r"\[login\] mem free=", ps1)
        # 2a. wrong password first, then the reject announce arms the probe.
        self.assertIn('send = "not-the-password`r"', ps1)
        self.assertIn('after = "Login incorrect"; send = $null; probe = $true', ps1)
        # 2b. the no-respawn ordering check (respawn marker before login:).
        self.assertIn("starting service: getty", ps1)
        self.assertIn("re-prompted in place (no getty respawn)", ps1)
        # 2c. the fresh login: re-submits the real credentials.
        self.assertIn('send = "skyos`r"', ps1)
        # 3. bracket escapes must be SINGLE backslash. In PowerShell -match,
        # backslash is literal: 'sash\[' is an unterminated [] set (runtime
        # error) and '\\[vahid\\]' can never match the text '[vahid] ready'.
        # A double-escaping edit anywhere in the file trips these guards.
        self.assertNotIn(chr(92) * 2 + '[', ps1)
        self.assertNotIn(chr(92) * 2 + ']', ps1)

    def test_no_bogus_mknod_syscall(self):
        # Regression pin for the Aug 8, 2026 removal: the old code called
        # libsarga::syscall::syscall3(0x7d, ...) before the O_CREAT fallback,
        # but 0x7d is SYS_CLIPBOARD (125), not mknod — it could never create
        # a node and its result was discarded anyway. Node creation is the
        # O_CREAT fallback alone and drives all_ok -> the honest exit code.
        # A future edit that re-adds the discarded call fails here. (Pinned
        # on `syscall3`, not `0x7d`, because the removal-decision comment
        # legitimately mentions the number.)
        self.assertNotIn("syscall3", self.vahid)
        self.assertIn("open(&path, 0x41)", self.vahid)
        # Positive leg (Aug 10, 2026): pin the node table a future kernel
        # mknod must serve — exactly these six (name, major, minor) tuples
        # drive the loop, and the mknod contract in session-lifecycle.md
        # (SYS_MKNODAT=259 / SYS_MKNOD=133, dev = (major << 8) | minor)
        # round-trips them verbatim. Extracted from source, so a renamed,
        # added, or dropped node fails here before the doc target drifts.
        nodes = re.findall(r'\("(\w+)", (\d+), (\d+)\),', self.vahid)
        self.assertEqual(
            nodes,
            [
                ("null", "1", "3"),
                ("zero", "1", "5"),
                ("random", "1", "8"),
                ("urandom", "1", "9"),
                ("tty", "5", "0"),
                ("console", "5", "1"),
            ],
            "create_devices node table drifted from the six documented /dev nodes",
        )

    def test_mknod_contract_ready_for_kernel_landing(self):
        # The gated mknod landing (session-lifecycle.md 0x7d note) has three
        # host-side preconditions that must all hold BEFORE the kernel
        # rewrite ships a real SYS_MKNOD, so the landing is a one-place edit.
        #
        # 1. vahid's nodes table carries (major, minor) ready to pass
        #    through: the tuple type is (name, major, minor) and the loop
        #    destructures them into _major/_minor today - un-underscore and
        #    pass to the gated call when the kernel lands. (The six tuple
        #    VALUES are pinned in test_no_bogus_mknod_syscall above.)
        self.assertIn("&[(&str, u32, u32)]", self.vahid)
        self.assertIn("(name, _major, _minor)", self.vahid)
        # Per-node success marker: the QEMU gate greps each of the six
        # '[vahid] created /dev/<name>' prints on the healthy boot, so the
        # node table is observable on real hardware - the success print is
        # the mirror of the FAILED marker (same {name} format string).
        self.assertIn('"[vahid] created /dev/{}\\n"', self.vahid)
        self.assertIn('"[vahid] FAILED to create /dev/{}\\n"', self.vahid)
        #
        # 2. libsarga::syscall3 exists for the gated call (reachable as
        #    libsarga::syscall::syscall3 - lib.rs must keep the module pub).
        syscall_rs = _read(SYSCALL_RS)
        self.assertIn(
            "pub unsafe fn syscall3(n: u64, a1: u64, a2: u64, a3: u64) -> i64",
            syscall_rs,
            "libsarga::syscall3 signature drifted from the gated-call shape",
        )
        # The doc's recommended landing is mknodat (259) - a FOUR-arg call
        # (dirfd, pathname, mode, dev) - so syscall4 must exist too.
        self.assertIn(
            "pub unsafe fn syscall4(n: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64",
            syscall_rs,
            "libsarga::syscall4 signature drifted - mknodat(259) needs 4 args",
        )
        self.assertIn(
            "pub mod syscall;",
            _read(LIBSARGA_LIB_RS),
            "libsarga lib.rs no longer re-exports the syscall module",
        )
        #
        # 3. The kernel still reserves NO mknod number: numbers.rs has no
        #    SYS_MKNOD / SYS_MKNODAT, and 0x7d is SYS_CLIPBOARD (125) - the
        #    reason the old discarded syscall3(0x7d, ...) could never create
        #    a node. When the rewrite adds a constant, this pin trips and
        #    the doc target + gated landing must be updated together.
        # NOTE (cross-repo coupling): this leg reads the kernel tree's
        # numbers.rs at HEAD (host-tests checks out SKYIOUS-KERNEL, no
        # ref pin). The moment the kernel rewrite merges a SYS_MKNOD*
        # constant to main, this pin fires and every PR here goes red
        # until the session-lifecycle.md target AND the gated vahid
        # landing land together. That is the intended tripwire - do not
        # weaken it to unblock a PR; fix the doc + landing in the same
        # change.
        numbers = _read_kernel_numbers()
        if numbers is None:
            self.skipTest(
                "kernel numbers.rs not present (kernel tree is checked out "
                "in the build/QEMU jobs and exists locally as a sibling "
                "repo); mknod-absence leg runs wherever the tree exists"
            )
        # Word-boundary checks (\b): a plain substring "SYS_MKNOD" would
        # false-fire inside "SYS_MKNODAT", reporting the wrong constant -
        # each message below names exactly what the kernel gained.
        self.assertIsNone(
            re.search(r"\bSYS_MKNOD\b", numbers),
            "kernel numbers.rs gained SYS_MKNOD - update the session-"
            "lifecycle.md target and the gated vahid landing together",
        )
        self.assertIsNone(
            re.search(r"\bSYS_MKNODAT\b", numbers),
            "kernel numbers.rs gained SYS_MKNODAT - update the session-"
            "lifecycle.md target and the gated vahid landing together",
        )
        self.assertIn(
            "SYS_CLIPBOARD: u64 = 125",
            numbers,
            "numbers.rs SYS_CLIPBOARD=125 anchor drifted - re-audit the "
            "0x7d removal claim",
        )

    def test_host_tests_checks_out_kernel_for_mknod_leg(self):
        # The mknod-absence leg must not silently skip in CI: host-tests
        # checks out the kernel tree (mirroring the build job) so the pin
        # has teeth on every PR, not just locally.
        ci = _read(os.path.join(REPO_ROOT, ".github", "workflows", "ci.yml"))
        # Scope to the host-tests job block: sibling jobs (integration,
        # gui-login, ...) ALSO check out the kernel, so a whole-file scan
        # would false-PASS if host-tests lost its own checkout.
        m = re.search(r"\n  host-tests:.*?(?=\n  [a-z][a-z-]*:|\Z)", ci, re.S)
        self.assertIsNotNone(m, "host-tests job block not found in ci.yml")
        job = m.group(0)
        scan = job.index("Scan-rust strip regression gate (runs FIRST)")
        pre = job[:scan]
        self.assertIn("Checkout kernel", pre, "host-tests lost the kernel checkout")
        self.assertIn("repository: SKYIOUS/SKYIOUS-KERNEL", pre)
        self.assertIn("path: SKYIOUS-KERNEL", pre)

    def test_kernel_devfs_still_no_create_no_non_native_nodes(self):
        # The doc's ENOENT conclusion (session-lifecycle.md:574-581) rests
        # on two current-kernel facts: devfs implements NO create override
        # (so VfsNode::create's trait default -> Err(()), vfs/mod.rs:
        # 104-106, and sys_open falls through to ENOENT, syscalls/mod.rs:
        # 1379/1400), and the native node list in DevFs::new (devfs.rs:
        # 306-358) carries no random/urandom/console. The absence pin in
        # leg 2 below is keyed to the tree being tested (devfs.rs is read
        # from whatever kernel checkout is present - CI's fresh
        # SKYIOUS-KERNEL default-branch checkout, or a local tree like the
        # in-flight 'SKYIOUS KERNEL'): it asserts absence ONLY while the
        # nodes are absent, i.e. it fires only against the pre-landing CI
        # default branch. The moment random/urandom/console are native in
        # the checked-out kernel (the in-flight devfs work, or the rewrite
        # merged to the CI default branch), leg 2 flips to a positive pin
        # of the new state instead of failing - so a local in-flight tree
        # never trips it. Leg 1 (no create override) stays the
        # unconditional tripwire for the real mknod + create landing,
        # which is when the doc conclusion and the gated vahid landing
        # must update together.
        devfs = _read_kernel(os.path.join("kernel", "src", "vfs", "devfs.rs"))
        vfs_mod = _read_kernel(os.path.join("kernel", "src", "vfs", "mod.rs"))
        if devfs is None:
            self.skipTest(
                "kernel devfs.rs not present (kernel tree is checked out "
                "in the build/QEMU jobs and exists locally as a sibling "
                "repo); devfs-create-absence leg runs wherever the tree "
                "exists"
            )
        # 1. No create override anywhere in devfs.rs: the DevNode impl
        #    (devfs.rs:28) and the FileSystem impl (:387) both fall to the
        #    trait default, which is what makes O_CREAT on a missing name
        #    fail instead of minting a node.
        self.assertNotIn(
            "fn create(",  # "( overrides only - a comment mentioning the name must not trip
            devfs,
            "kernel devfs.rs gained a create override - update the doc "
            "ENOENT conclusion and the gated vahid landing",
        )
        # 2. The native node list pin is state-keyed: while the kernel
        #    tree lacks random/urandom/console, assert their absence (the
        #    doc's "cannot be created" claim holds); once they are native
        #    in the checked-out kernel, assert their presence instead, and
        #    a PARTIAL landing (one of the three missing) still fails.
        #    The create-override leg (#1) remains the unconditional
        #    tripwire, so a nodes-only landing never trips this test but
        #    the full mknod rewrite does. Shape-coupled probe: the regex
        #    matches the current Arc::new(DevNode { name:
        #    String::from("...") ... }) form. A legitimate table-driven
        #    refactor of DevFs::new would break extraction and trip the
        #    positive legs below - re-review, it is not necessarily a
        #    dropped node.
        node_names = re.findall(r'name: String::from\("([^"]+)"\)', devfs)
        if "random" in node_names or "urandom" in node_names:
            # Nodes have landed (in-flight devfs work, or merged to the CI
            # default branch): pin the new native-node state.
            for native in ("random", "urandom", "console"):
                self.assertIn(
                    native,
                    node_names,
                    "kernel devfs landed a partial native node set - '%s' "
                    "missing from DevFs::new" % native,
                )
        else:
            # Pre-landing kernel (current CI default branch): the three
            # names cannot be created - absent natively and no create
            # override, so O_CREAT falls through to ENOENT. Pin absence.
            for missing in ("random", "urandom", "console"):
                self.assertNotIn(
                    missing,
                    node_names,
                    "kernel devfs gained a native node '%s' - update the "
                    "doc ENOENT conclusion" % missing,
                )
        for native in ("null", "zero", "tty"):
            self.assertIn(
                native,
                node_names,
                "native devfs node '%s' vanished from DevFs::new" % native,
            )
        # 3. The mechanism: VfsNode::create's default is still Err(()) so
        #    a missing name can never be created on a no-override fs.
        self.assertIn(
            "fn create(&self, _name: &str) -> Result<Arc<dyn VfsNode>, ()> {\n"
            "        Err(())",
            vfs_mod,
            "VfsNode::create default no longer Err(()) - the ENOENT "
            "conclusion changed",
        )

    # --- init side: the accounting the exit code must feed ---

    def test_init_resets_crashes_on_clean_exit(self):
        # status == 0 resets crashes — a zero exit is therefore a forever-
        # respawn, which is why vahid's FATAL exit MUST be non-zero.
        self.assertIn("if status == 0 {", self.init)
        self.assertIn("svc.crashes = 0;", self.init)

    def test_init_accumulates_crashes_and_gives_up(self):
        self.assertIn("svc.crashes += 1;", self.init)
        self.assertIn("svc.crashes > MAX_RESPAWNS", self.init)
        self.assertIn(f"const MAX_RESPAWNS: u32 = {MAX_RESPAWNS};", self.init)

    # --- end-to-end semantics: faithful port of init's accounting ---

    def test_port_matches_source_max_respawns(self):
        # The port constant must not drift from the source it mirrors.
        self.assertIn(f"const MAX_RESPAWNS: u32 = {MAX_RESPAWNS};", self.init)

    def test_login_manager_clean_exit_loop_is_unbounded(self):
        # login-manager's '[login] failed to create window' path returns 0
        # (the exit(0) window-failure loop). Every clean exit resets crashes
        # BEFORE the increment, so crashes always lands at 1 and give-up can
        # never fire: UNBOUNDED. This is why MAX_RESPAWNS cannot kill the
        # GUI login on a bad window.
        sm = RespawnAccounting()
        for _ in range(1000):
            self.assertEqual(sm.on_exit(0), "respawn")
            self.assertFalse(sm.gave_up)
            self.assertEqual(sm.crashes, 1)  # reset to 0, then +1, every time

    def test_vahid_nonzero_exit_is_bounded(self):
        # vahid's FATAL path exits 1: crashes accumulate 1..6, respawning on
        # exits 1..5 and giving up on the 6th — exactly MAX_RESPAWNS
        # respawns, then dead. The serial log's '[init] giving up on vahid
        # after too many crashes' is the observable proof.
        sm = RespawnAccounting()
        exits = 0
        while not sm.gave_up and exits < 20:
            sm.on_exit(1)  # exits 1..5 respawn; exit 6 flips gave_up
            exits += 1
        self.assertTrue(sm.gave_up)
        self.assertEqual(exits, MAX_RESPAWNS + 1)   # 6 exits total
        self.assertEqual(sm.respawns, MAX_RESPAWNS)  # 5 respawns

    def test_mixed_streak_clean_exit_resets(self):
        # A bad streak followed by a clean exit resets the counter: give-up
        # is about the CURRENT streak, not lifetime (init's stated intent:
        # "a single bad streak doesn't permanently kill a service").
        sm = RespawnAccounting()
        for _ in range(MAX_RESPAWNS - 1):
            sm.on_exit(1)  # crashes 1..=MAX-1
        self.assertFalse(sm.gave_up)
        self.assertEqual(sm.on_exit(0), "respawn")
        self.assertEqual(sm.crashes, 1)  # reset to 0, then +1
        for _ in range(MAX_RESPAWNS - 1):
            sm.on_exit(1)  # crashes 2..=MAX
        self.assertFalse(sm.gave_up)
        self.assertEqual(sm.on_exit(1), "gave_up")  # crashes MAX+1 > MAX

    def test_signal_killed_service_is_bounded_like_bad_exit(self):
        # A signal-killed service reports 128+sig (SIGINT=2 -> 130,
        # SIGKILL=9 -> 137, SIGTERM=15 -> 143 per session.rs's
        # exit_class). init's binary view counts ANY non-zero status as
        # a crash, so the trajectory must be IDENTICAL to exit(1): 5
        # respawns, then give up on the 6th. This is the behavioral pin
        # for the binary-view NOTE in test_login_flow.py.
        for sig in (2, 9, 15):
            sm = RespawnAccounting()
            exits = 0
            while not sm.gave_up and exits < 20:
                sm.on_exit(128 + sig)
                exits += 1
            self.assertTrue(sm.gave_up, f"signal {sig} never gave up")
            self.assertEqual(exits, MAX_RESPAWNS + 1)
            self.assertEqual(sm.respawns, MAX_RESPAWNS)

    def test_signal_status_trajectory_matches_exit_code(self):
        # The on_exit return sequence for a signal status must equal the
        # sequence for a plain bad exit code -- the accounting cannot
        # tell the two apart (that is the binary view).
        def seq(status):
            sm = RespawnAccounting()
            out = []
            for _ in range(MAX_RESPAWNS + 1):
                out.append(sm.on_exit(status))
            return out

        baseline = seq(1)  # ['respawn']*5 + ['gave_up']
        for sig in (2, 9, 15):
            self.assertEqual(seq(128 + sig), baseline, f"signal {sig}")

    def test_signal_killed_mixed_with_clean_exit_resets(self):
        # A clean exit mid signal-streak still resets the counter: the
        # reset is keyed on status == 0 alone, regardless of how the
        # crash-streak statuses were produced.
        sm = RespawnAccounting()
        for _ in range(MAX_RESPAWNS - 1):
            sm.on_exit(137)  # SIGKILL: crashes 1..=MAX-1
        self.assertFalse(sm.gave_up)
        self.assertEqual(sm.on_exit(0), "respawn")
        self.assertEqual(sm.crashes, 1)  # reset to 0, then +1
        for _ in range(MAX_RESPAWNS - 1):
            sm.on_exit(137)  # crashes 2..=MAX
        self.assertFalse(sm.gave_up)
        self.assertEqual(sm.on_exit(137), "gave_up")  # crashes MAX+1 > MAX

    def test_binary_view_agrees_with_exit_class(self):
        # Cross-source agreement: init's binary view (status == 0 is the
        # ONLY clean status) and session.rs's exit_class must classify
        # every status identically on the clean/crash boundary, so no
        # status can be Clean-but-counted or counted-but-Clean. The
        # source branches are pinned here too, so the port cannot drift
        # from session.rs while the kernel rewrite is in flight.
        session = _read(SESSION_RS)
        self.assertIn("if status == 0 {", session)
        # The 128+sig threshold itself is the convention -- a future edit
        # that rekeys the Signal arm (e.g. status > 999) fails here even
        # though the ExitClass::Signal body string survives.
        self.assertIn("} else if status > 128 {", session)
        self.assertIn("ExitClass::Killed", session)
        self.assertIn("ExitClass::Signal((status - 128) as u32)", session)
        for status in (0, 1, 42, 126, 127, 128, 129, 130, 137, 143,
                      254, 255, -1, -9, -15):
            is_clean = status == 0
            self.assertEqual(
                port_exit_class(status) == "clean", is_clean,
                f"status {status}: exit_class vs init binary view disagree",
            )
            if not is_clean:
                # Non-clean statuses ALWAYS accumulate: TWO calls on a
                # fresh machine leave crashes == 2. A signal-aware
                # "reset on 128+sig" regression would reset before each
                # increment and stay at 1, so it fails here.
                sm = RespawnAccounting()
                self.assertEqual(sm.on_exit(status), "respawn")
                self.assertEqual(sm.on_exit(status), "respawn")
                self.assertEqual(sm.crashes, 2)



    def test_init_resets_then_increments_order(self):
        # The reset-on-clean-exit MUST happen BEFORE the unconditional
        # increment. If these were swapped (increment then reset), a clean
        # exit would accumulate to MAX_RESPAWNS and eventually give up —
        # defeating the purpose of "a single bad streak doesn't permanently
        # kill a service". The source order is the invariant.
        code = self.init
        reset = code.index("svc.crashes = 0;")
        incr = code.index("svc.crashes += 1;")
        self.assertLess(
            reset, incr,
            "svc.crashes = 0 (reset) must come BEFORE svc.crashes += 1 "
            "(increment), so a clean exit never accumulates",
        )
        # The reset is inside `if status == 0 { ... }`, the increment is
        # inside `if svc.respawn { ... }`. The outer condition order is:
        # status check first, then respawn check. Swapping either the
        # conditions or the bodies would break the contract.
        status_idx = code.index("if status == 0 {")
        respawn_idx = code.index("if svc.respawn {")
        self.assertLess(status_idx, respawn_idx)

    def test_login_manager_window_failure_end_to_end(self):
        # Traces the full chain: login-manager's window-creation failure
        # return 0 -> init's waitpid sees status==0 -> crashes reset to 0
        # -> then +1 = 1 -> never reaches > MAX_RESPAWNS -> respawns forever.
        # This test reads login-manager source AND init source AND the
        # RespawnAccounting port and cross-checks all three.
        # Step 1: login-manager returns 0 on window-creation failure.
        lm_rs = os.path.join(REPO_ROOT, "login-manager", "src", "main.rs")
        with open(lm_rs, encoding="utf-8") as fh:
            lmsrc = fh.read()
        self.assertIn('[login] failed to create window: Out of memory (errno 12)', lmsrc)
        self.assertIn("return 0;", lmsrc[lmsrc.index("failed to create window"):])
        # Step 2: init's accounting resets on status==0 (not on status!=0).
        self.assertIn("if status == 0 {", self.init)
        self.assertIn("svc.crashes = 0;", self.init)
        # Step 3: the ported accounting (same order + conditions as init)
        # confirms unbounded respawn for exit(0): MAX_RESPAWNS can never fire.
        sm = RespawnAccounting()
        for _ in range(1000):
            self.assertEqual(sm.on_exit(0), "respawn")
            self.assertFalse(sm.gave_up)
        self.assertEqual(sm.crashes, 1)  # reset to 0, then +1, every time


if __name__ == "__main__":
    unittest.main(verbosity=2)
