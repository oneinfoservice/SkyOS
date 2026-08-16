#!/usr/bin/env python3
"""Host-runnable pins for the DHCP/net probe (K10-gated).

tests/qemu_net_probe.exp settles whether smoltcp actually works on real
hardware: it boots the ISO with the e1000 device, waits for the console
getty, logs in root/skyos, and reads the DHCP outcome from the accumulated
serial buffer. Today the lease is NEVER obtained - '[DHCP] lease lost'
fires at the first poll and no '[DHCP] configured IP:' ever follows - so
the harness dumps the captured net-marker evidence and defers via
KERNEL-GATED (exit 0), the drm-probe pattern; the moment K10 lands
(kernel-net-dhcp.md) the marker becomes a hard PASS. These tests pin the
harness legs, the CI wiring (QEMU step + host-tests step), the K10 queue
row, and - via ReplaySettlementTest - the tri-state settlement logic
against the net markers captured from a real boot of the working ISO.
"""
import io
import os
import re
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BS = chr(92)  # backslash, for escape-containing needles

# The net-marker lines from a real clean boot of release/skyos-probe.iso
# (Aug 14, 2026; reached 'login:', zero SIGSEGV). Captured from the serial
# log; the stack initialized and polled exactly once (Event::Deconfigured)
# but no lease exchange ever completed.
REAL_NET_MARKERS = (
    "[BOOT] net init...\n"
    "[E1000] sts=80080783 icr=00000002 rdh=0 rdt=31 cur=0 dd=0\n"
    "[DHCP] lease lost\n"
)

# The exact regexes the exp uses (single-backslash Tcl escapes -> plain
# Python here: '\[DHCP\]' matches the literal '[DHCP]').
RE_CONFIGURED_IP = re.compile(r"\[DHCP\] configured IP:")
RE_LEASE_LOST = re.compile(r"\[DHCP\] lease lost")


def _settle(buffer_):
    """Port of the exp's stage-2 tri-state, in the same branch order."""
    if RE_CONFIGURED_IP.search(buffer_):
        return "PASS"
    if RE_LEASE_LOST.search(buffer_):
        return "KERNEL-GATED"
    return "FAIL"


def _read(rel):
    with io.open(os.path.join(REPO, rel), encoding="utf-8") as fh:
        return fh.read()


class TestNetProbeContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.exp = _read(os.path.join("tests", "qemu_net_probe.exp"))
        cls.ci = _read(os.path.join(".github", "workflows", "ci.yml"))
        cls.sl = _read(os.path.join("ade", "docs", "session-lifecycle.md"))
        cls.doc = _read(os.path.join("ade", "docs", "kernel-net-dhcp.md"))

    def test_harness_legs_pinned(self):
        e = self.exp
        # Spawn: e1000 + user-mode netdev (the DHCP settlement device).
        self.assertIn("-device e1000,netdev=net0 -netdev user,id=net0", e)
        # Boot + console login flow.
        self.assertIn("=== 1. boot to console getty ===", e)
        self.assertIn('send "root\\r"', e)
        self.assertIn("sash" + BS + "[", e)
        # The net markers, escaped exactly as the executable Tcl uses them
        # (single backslash before '[' - both the braced -re patterns and
        # the double-quoted send_user strings).
        self.assertIn(BS + "[DHCP" + BS + "] configured IP:", e)
        self.assertIn(BS + "[DHCP" + BS + "] lease lost", e)
        self.assertIn(BS + "[(BOOT|E1000|DHCP)" + BS + "]", e)
        # The VGA-only marker must be explicitly documented, not grepped.
        self.assertIn("VGA-ONLY", e)
        self.assertIn("Network: Stack initialized", e)
        # Deferral machinery + evidence dump + verdict.
        self.assertIn("KERNEL-GATED:", e)
        self.assertIn("PASS: DHCP lease obtained on real hardware", e)
        self.assertIn("NET-EVIDENCE", e)
        self.assertIn("dump_net_evidence", e)
        # Tri-state branch order: configured-IP is checked BEFORE the
        # lease-lost deferral arm (a landed K10 must never defer).
        self.assertLess(
            e.index(BS + "[DHCP" + BS + "] configured IP:"),
            e.index(BS + "[DHCP" + BS + "] lease lost"),
        )
        # The TCP-echo leg is documented as blocked, not silently omitted.
        self.assertIn("tcp_echo_leg", e)
        self.assertIn("guest TCP client/server", e)

    def test_ci_step_wired_with_gate(self):
        c = self.ci
        self.assertIn("- name: DHCP/net probe (K10-gated)", c)
        self.assertIn('expect tests/qemu_net_probe.exp "$ISO" 240', c)
        # The KERNEL-GATED conditional must defer (exit 0), not fail.
        self.assertIn(
            'if grep -q "KERNEL-GATED:" qemu_net_probe_log.txt; then', c)
        self.assertIn('exit 0', c)
        self.assertIn(
            'grep -q "PASS: DHCP lease obtained" qemu_net_probe_log.txt', c)
        # This file itself must be wired into the host-tests job.
        self.assertIn("python3 tests/test_net_probe_contract.py", c)

    def test_k10_queue_row_references_probe(self):
        # §6 K10 row: names the doc, the marker, and the harness flip.
        self.assertIn("tests/qemu_net_probe.exp", self.sl)
        self.assertIn("kernel-net-dhcp.md", self.sl)
        self.assertIn("`[DHCP] configured IP:`", self.sl)
        self.assertIn("flips from `KERNEL-GATED:` to hard", self.sl)

    def test_doc_records_settlement_and_scope(self):
        d = self.doc
        # The observed settlement and the misleading fallback-IP finding.
        self.assertIn("[DHCP] lease lost", d)
        self.assertIn("never appears", d)
        self.assertIn("fallback IP", d)
        # The VGA-only routing note (crate::println! -> vga_buffer.rs).
        self.assertIn("VGA-only", d)
        self.assertIn("vga_buffer.rs:164", d)
        # Landing condition + explicit out-of-scope TCP echo.
        self.assertIn("`[DHCP] configured IP:` appears in the serial log", d)
        self.assertIn("Explicitly out of scope", d)
        self.assertIn("TCP echo session", d)


class ReplaySettlementTest(unittest.TestCase):
    """Replay the exp's tri-state against the real captured net markers."""

    def test_current_kernel_defers(self):
        # The working ISO's real boot: stack ran, lease lost, no configured
        # IP -> the exp must take the KERNEL-GATED arm (exit 0), not fail.
        self.assertEqual(_settle(REAL_NET_MARKERS), "KERNEL-GATED")
        self.assertNotIn("configured IP", REAL_NET_MARKERS)

    def test_k10_landed_passes(self):
        # The moment K10 lands, the same boot shows a configured IP -> the
        # first branch wins and the probe hard-PASSes (never defers).
        landed = REAL_NET_MARKERS + "[DHCP] configured IP: 10.0.2.15\n"
        self.assertEqual(_settle(landed), "PASS")

    def test_no_stack_fails(self):
        # Neither marker: the stack never ran / no NIC -> the FAIL arm.
        self.assertEqual(_settle("[BOOT] something else\n"), "FAIL")

    def test_fixture_is_the_real_evidence(self):
        # The fixture must keep carrying all three observed marker kinds so
        # the deferral evidence stays byte-relevant to a real boot.
        self.assertIn("[BOOT] net init...", REAL_NET_MARKERS)
        self.assertIn("[E1000] sts=", REAL_NET_MARKERS)
        self.assertIn("[DHCP] lease lost", REAL_NET_MARKERS)


if __name__ == "__main__":
    unittest.main()
