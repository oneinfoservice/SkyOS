#!/usr/bin/env python3
"""Host-runnable contract test for Vahi's stride scheduler
(kernel/src/task/scheduler.rs + thread.rs + syscalls/mod.rs), pinning
what is ACTUALLY implemented vs scaffolding at the committed kernel HEAD.

Traced against kernel HEAD (a18848f, Aug 14 2026) — this is the contract
the kernel rewrite must preserve or deliberately break:

IMPLEMENTED (real code paths):
  1. pick_next() three-source order: local stride heap (min-pass via the
     PassOrd reverse-ordering on BinaryHeap) -> global pending queue
     (try_lock: IRQ-context-safe) -> work stealing from other CPUs'
     stride heaps (also try_lock).
  2. Stride accounting: the switched-away thread's pass advances by its
     stride (`old.pass = old.pass.wrapping_add(old.stride)`) so the
     min-pass thread wins the heap.
  3. route_outgoing() routing: sleep_until -> sleep_queue, futex ->
     futex_queue, pipe -> block_queue, else back to ready_queues[p] with
     the dirty flag; Exited threads destroy their address space + stack.
  4. tick() wakes sleepers when current_ticks >= wake_time (or an
     unmasked signal arrived) — rotate in place, try_lock only, because
     tick runs in IRQ context where allocation is forbidden.
  5. wake_blocked_threads / wake_futex rotate the block/futex queues,
     waking up to max_wake matching threads into ready_queues[priority],
     and mark the dirty flag.
  6. sys_sched_setattr accepts SCHED_OTHER only (policy != 0 -> EINVAL)
     and maps nice to priority [0..7]; sys_sched_getattr maps back to
     representative nice values.

SCAFFOLDING / NOT IMPLEMENTED (documented, must not be mistaken for real):
  A. boost_thread_priority() always returns false — priority inheritance
     is a hard stub. futex.rs:138 calls it (FUTEX_WAITERS path) but
     discards the result, so a high-priority waiter never boosts the
     owner. The ponytail comment says exactly why.
  B. Thread::set_tickets() is dead code (#[allow(dead_code)], zero call
     sites) — tickets/stride are frozen at DEFAULT_TICKETS=20, so the
     proportional-share knob is not runtime-settable.
  C. add_sleeping_thread / add_futex_thread (module fns) are
     selftest-only (dead_code); the wake_* paths use the GlobalScheduler
     methods directly.
  D. SCHED_QUIESCE gates BOTH schedule() and try_schedule() — a
     selftest-only toggle, not a runtime feature.
  E. tick()'s ITIMER_REAL pass is capped at 64 pids ("ponytail: >64 pids
     defer to next tick") — the fixed stack array is the documented cap.

The tests are two kinds: source pins (strip_rust scans assert the exact
tokens above survive) and behavioral ports (the wake-rotate semantics and
the nice<->priority mapping are ported as pure functions and exercised
against boundary values, mirroring the RespawnAccounting port pattern).

Run:  python3 tests/test_scheduler_contract.py
"""
import io
import os
import re
import sys
import unittest

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from scan_rust import strip_rust  # noqa: E402


def _read(p):
    with io.open(p, encoding="utf-8") as fh:
        return fh.read()


def _read_kernel(rel):
    """Locate the kernel tree (SKYOS_KERNEL_DIR env override, then sibling
    checkouts) and read the file at rel inside it, or None."""
    env = os.environ.get("SKYOS_KERNEL_DIR")
    cands = ([env] if env else [])
    parent = os.path.dirname(REPO_ROOT)
    cands += [os.path.join(parent, "SKYIOUS KERNEL"),
              os.path.join(parent, "SKYIOUS-KERNEL"),
              os.path.join(parent, "SKYIOUS_KERNEL")]
    for root in cands:
        p = os.path.join(root, rel)
        if p and os.path.isfile(p):
            return _read(p).replace("\r\n", "\n")
    return None


def _kernel_files():
    """All kernel/src/**/*.rs contents (for whole-tree call-site scans)."""
    env = os.environ.get("SKYOS_KERNEL_DIR")
    cands = ([env] if env else [])
    parent = os.path.dirname(REPO_ROOT)
    cands += [os.path.join(parent, "SKYIOUS KERNEL"),
              os.path.join(parent, "SKYIOUS-KERNEL"),
              os.path.join(parent, "SKYIOUS_KERNEL")]
    for root in cands:
        base = os.path.join(root, "kernel", "src")
        if root and os.path.isdir(base):
            out = []
            for dirpath, _dirs, names in os.walk(base):
                for n in names:
                    if n.endswith(".rs"):
                        out.append(_read(os.path.join(dirpath, n)).replace("\r\n", "\n"))
            return out
    return None


# ── Behavioral ports ────────────────────────────────────────────────────

def port_wake_rotate(queue, key, max_wake):
    """Faithful port of GlobalScheduler::wake_blocked_threads /
    wake_futex: rotate the queue, removing (waking) up to max_wake
    threads whose block key matches, preserving FIFO order of the rest.

    Source (scheduler.rs):

        while let Some(mut thread) = block.pop_front() {
            if woken < max_wake && thread.pipe_block_key == Some(key) {
                ... Ready; ready_queues[p].push_back(thread); woken += 1;
            } else { still_waiting.push_back(thread); }
        }
        *block = still_waiting;
    """
    remaining = []
    woken = 0
    for thread in queue:
        if woken < max_wake and thread["key"] == key:
            woken += 1
        else:
            remaining.append(thread)
    return woken, remaining


def port_nice_to_priority(nice):
    """Faithful port of sys_sched_setattr's nice->priority ladder."""
    if nice <= -15:
        return 7
    if nice <= -10:
        return 6
    if nice <= -5:
        return 5
    if nice <= 0:
        return 4
    if nice <= 5:
        return 3
    if nice <= 10:
        return 2
    if nice <= 15:
        return 1
    return 0


def port_priority_to_nice(priority):
    """Faithful port of sys_sched_getattr's priority->nice table."""
    return {7: -20, 6: -10, 5: -5, 4: 0, 3: 5, 2: 10, 1: 15}.get(priority, 19)


class TestPickNextImplemented(unittest.TestCase):
    """The three-source pick order is real: stride heap -> pending -> steal."""

    @classmethod
    def setUpClass(cls):
        cls.sched = _read_kernel(os.path.join("kernel", "src", "task", "scheduler.rs"))
        if cls.sched is None:
            cls.skip_why = "kernel tree not found (SKYOS_KERNEL_DIR or a " \
                           "SKYIOUS KERNEL sibling); CI checks it out, so CI has teeth"
        else:
            cls.skip_why = None
            cls.code = strip_rust(cls.sched)

    def _require(self):
        if self.skip_why:
            self.skipTest(self.skip_why)

    def test_pick_next_flushes_then_pops_stride_heap_first(self):
        self._require()
        self.assertIn("self.flush_ready_queues();", self.code,
                      "pick_next must flush legacy wake queues into the heap first")
        self.assertIn("if let Some(PassOrd(t)) = self.stride_heap.pop() {", self.code,
                      "pick_next lost its primary stride-heap source")

    def test_global_pending_is_second_source_try_lock(self):
        self._require()
        self.assertIn(
            "if let Some(t) = GLOBAL.pending_queue.try_lock().and_then(|mut q| q.pop_front()) {",
            self.code,
            "pick_next must drain the global pending queue via try_lock "
            "(IRQ-context safe; a blocking lock here would deadlock the CPU)",
        )

    def test_work_stealing_third_source(self):
        self._require()
        # The work-steal arm: skip self, try_lock the peer, flush its wake
        # queues, then take its min-pass thread.
        self.assertIn("for i in 0..MAX_CPUS {", self.code)
        self.assertIn("if i == current_cpu { continue; }", self.code)
        self.assertIn("if let Some(mut other) = PER_CPU[i].try_lock() {", self.code,
                      "work stealing must use try_lock (non-blocking in IRQ context)")
        self.assertIn("other.flush_ready_queues();", self.code)
        self.assertIn("if let Some(PassOrd(t)) = other.stride_heap.pop() {", self.code,
                      "work steal must take the peer's MIN-PASS thread")

    def test_pass_ord_reverse_ordering(self):
        self._require()
        # BinaryHeap is a max-heap; PassOrd reverses cmp so the MIN-pass
        # thread sits on top.
        self.assertIn("other.0.pass.cmp(&self.0.pass)", self.code,
                      "PassOrd reverse ordering is the whole stride-heap "
                      "min-pass mechanism - removing it breaks fair scheduling")

    def test_flush_ready_queues_dirty_gate(self):
        self._require()
        self.assertIn("fn flush_ready_queues(&mut self) {", self.code)
        self.assertIn("self.stride_heap.push(PassOrd(t));", self.code)
        self.assertIn("self.ready_queues_dirty = false;", self.code)


class TestStrideAccounting(unittest.TestCase):
    """Pass-advance and the frozen tickets/stride defaults."""

    @classmethod
    def setUpClass(cls):
        cls.sched = _read_kernel(os.path.join("kernel", "src", "task", "scheduler.rs"))
        cls.thread_rs = _read_kernel(os.path.join("kernel", "src", "task", "thread.rs"))
        cls.skip_why = None
        if cls.sched is None or cls.thread_rs is None:
            cls.skip_why = "kernel tree not found (SKYOS_KERNEL_DIR or a " \
                           "SKYIOUS KERNEL sibling)"
        else:
            cls.sched_code = strip_rust(cls.sched)
            cls.thread_code = strip_rust(cls.thread_rs)

    def _require(self):
        if self.skip_why:
            self.skipTest(self.skip_why)

    def test_pass_advances_by_stride_on_switch_away(self):
        self._require()
        self.assertIn(
            "old.pass = old.pass.wrapping_add(old.stride);",
            self.sched_code,
            "stride accounting (pass += stride on switch-away) must survive - "
            "it is what makes min-pass scheduling proportional to tickets",
        )

    def test_stride_max_and_default_tickets(self):
        self._require()
        self.assertIn("pub const STRIDE_MAX: u64 = 1 << 20;", self.thread_code)
        self.assertIn("pub const DEFAULT_TICKETS: u32 = 20;", self.thread_code)
        # Thread::new computes stride from the fixed default.
        self.assertIn(
            "let stride = if DEFAULT_TICKETS > 0 { STRIDE_MAX / DEFAULT_TICKETS as u64 } "
            "else { STRIDE_MAX };",
            self.thread_code,
            "Thread::new must derive stride from DEFAULT_TICKETS",
        )

    def test_boost_thread_priority_is_a_stub(self):
        self._require()
        start = self.sched_code.index("pub fn boost_thread_priority")
        end = self.sched_code.index("pub fn tick", start)
        body = self.sched_code[start:end]
        # The stub: parameter names are underscore-prefixed (unused) and the
        # body is a single bare `false`.
        self.assertIn("pub fn boost_thread_priority(_pid: u64, _target_priority: u8) -> bool {",
                      body, "boost_thread_priority signature changed")
        self.assertNotIn("true", body,
                         "boost_thread_priority must stay a false-stub - priority "
                         "inheritance is NOT implemented (ponytail comment)")
        self.assertIn("false", body)

    def test_boost_stub_rationale_documented(self):
        self._require()
        # The raw (unstripped) comment is the documented reason.
        self.assertIn("ponytail: single-thread-per-process model", self.sched,
                      "the boost stub's rationale comment is the contract - "
                      "if shared-memory threading lands, this pin must be "
                      "rewritten WITH the implementation")
        self.assertIn("full priority inheritance", self.sched)

    def test_set_tickets_is_dead_code(self):
        self._require()
        # Adjacency-anchored: the dead_code attribute must sit IMMEDIATELY
        # above set_tickets' definition. thread.rs has many #[allow(dead_code)]
        # markers, so a bare assertIn cannot detect dropping it here.
        self.assertIn("#[allow(dead_code)]\n    pub fn set_tickets(&mut self, tickets: u32) {",
                      self.thread_code,
                      "set_tickets lost its dead_code marker - it must stay "
                      "annotated until a runtime call site exists")
        # No runtime call site anywhere in the kernel: the only occurrences of
        # `set_tickets` must be the definition line (plus the doc attribute).
        all_files = _kernel_files() or []
        calls = []
        for src in all_files:
            code = strip_rust(src)
            for m in re.finditer(r"\.set_tickets\(|\bset_tickets\(", code):
                line = code[:m.start()].count("\n")
                # Definition is `pub fn set_tickets(`; any OTHER occurrence
                # is a call site.
                seg = code[max(0, m.start() - 40):m.end()]
                if "pub fn" not in seg:
                    calls.append(m.group(0))
        self.assertEqual(calls, [],
                         "set_tickets has runtime call sites - tickets are "
                         "frozen at DEFAULT_TICKETS=20; if the kernel now "
                         "supports runtime tickets, update this pin + the "
                         "stride doc")

    def test_selftest_only_queue_helpers_are_dead_code(self):
        self._require()
        self.assertIn("#[allow(dead_code)] // selftest injects threads via "
                      "GLOBAL.add_sleeping_thread directly", self.sched)
        self.assertIn("#[allow(dead_code)] // only used by selftest "
                      "(tests/futex_test.rs)", self.sched)


class TestSleepWakePaths(unittest.TestCase):
    """route_outgoing routing + tick's IRQ-safe wake + rotate-wake."""

    @classmethod
    def setUpClass(cls):
        cls.sched = _read_kernel(os.path.join("kernel", "src", "task", "scheduler.rs"))
        cls.skip_why = None
        if cls.sched is None:
            cls.skip_why = "kernel tree not found (SKYOS_KERNEL_DIR or a " \
                           "SKYIOUS KERNEL sibling)"
        else:
            cls.code = strip_rust(cls.sched)

    def _require(self):
        if self.skip_why:
            self.skipTest(self.skip_why)

    def test_route_outgoing_routing_priority(self):
        self._require()
        # sleep > futex > pipe > ready, in that order.
        i_sleep = self.code.index("if switching.sleep_until.is_some() {")
        i_futex = self.code.index("} else if switching.futex_wake_addr.is_some() {")
        i_pipe = self.code.index("} else if switching.pipe_block_key.is_some() {")
        self.assertLess(i_sleep, i_futex)
        self.assertLess(i_futex, i_pipe)
        self.assertIn("GLOBAL.sleep_queue.lock().push_back(switching);", self.code)
        self.assertIn("GLOBAL.futex_queue.lock().push_back(switching);", self.code)
        self.assertIn("GLOBAL.block_queue.lock().push_back(switching);", self.code)

    def test_route_outgoing_ready_fallback_and_exited(self):
        self._require()
        self.assertIn(
            "} else if switching.status != crate::task::thread::ThreadStatus::Exited {",
            self.code)
        self.assertIn("s.ready_queues[p_idx].push_back(switching);", self.code)
        self.assertIn("s.mark_ready_queues_dirty();", self.code)
        # Exited: reclaim AS + stack (the leak-free path).
        self.assertIn("AddressSpace::destroy(&proc.address_space);", self.code)
        self.assertIn("crate::memory::stack::free_stack(&switching.stack);", self.code)

    def test_tick_wakes_at_wake_time(self):
        self._require()
        self.assertIn("if current_ticks >= wake_time { wake = true; }", self.code,
                      "tick() must wake sleepers at sleep_until expiry")
        self.assertIn("thread.sleep_until = None;", self.code)

    def test_tick_irq_safe_no_allocation(self):
        self._require()
        # tick runs in IRQ context (IF=0): every lock is try_lock and the
        # wake scan rotates in place (no new VecDeque).
        self.assertIn("let mut sched = match this_cpu_sched().try_lock() {", self.code)
        self.assertIn("GLOBAL.sleep_queue.try_lock()", self.code)
        self.assertIn("let n = sleep.len();", self.code)

    def test_tick_itimer_64_pid_cap_documented(self):
        self._require()
        self.assertIn("let mut itimer_pids: [u64; 64] = [0; 64];", self.code)
        self.assertIn("ponytail: >64 pids defer to next tick.", self.sched,
                      "the 64-pid ITIMER cap is a documented limitation - "
                      "raising it must update this pin")

    def test_wake_rotate_guards(self):
        self._require()
        # Both wake_blocked_threads and wake_futex use the same guarded
        # rotate: max_wake cap + key match + ready_queues[priority] + dirty.
        self.assertIn("if woken < max_wake && thread.pipe_block_key == Some(key) {",
                      self.code)
        self.assertIn("if woken < max_wake && thread.futex_wake_addr == Some(uaddr) {",
                      self.code)
        self.assertIn("target_ready.ready_queues[p as usize].push_back(thread);", self.code)
        self.assertIn("if woken > 0 { target_ready.mark_ready_queues_dirty(); }", self.code)

    def test_wake_broadcasts_reschedule_ipi(self):
        self._require()
        self.assertIn("if woken > 0 { broadcast_reschedule_ipi(); }", self.code,
                      "a wake must IPI other CPUs so a stolen-into thread can run")

    def test_sched_quiesce_gates_both_loops(self):
        self._require()
        self.assertEqual(
            self.code.count("if SCHED_QUIESCE.load(Ordering::Relaxed) {"), 2,
            "SCHED_QUIESCE must gate BOTH schedule() and try_schedule() - "
            "it is the selftest isolation toggle")

    def test_try_schedule_boot_stack_guard(self):
        self._require()
        self.assertIn("if sched.current_thread.is_none() {", self.code,
                      "try_schedule must refuse to run before schedule() set up "
                      "the initial idle thread (boot-stack hijack guard)")


class TestSchedSyscalls(unittest.TestCase):
    """sys_sched_setattr/getattr: SCHED_OTHER-only + the nice ladder."""

    @classmethod
    def setUpClass(cls):
        cls.mod = _read_kernel(os.path.join("kernel", "src", "syscalls", "mod.rs"))
        cls.skip_why = None
        if cls.mod is None:
            cls.skip_why = "kernel tree not found (SKYOS_KERNEL_DIR or a " \
                           "SKYIOUS KERNEL sibling)"
        else:
            cls.code = strip_rust(cls.mod)

    def _require(self):
        if self.skip_why:
            self.skipTest(self.skip_why)

    def test_sched_setattr_sched_other_only(self):
        self._require()
        self.assertIn("if policy != 0 { return errno::Errno::EINVAL as u64; }", self.code,
                      "sched_setattr must reject non-SCHED_OTHER policies")

    def test_nice_ladder_source_matches_port(self):
        self._require()
        # Boundary values of the source ladder, exercised through the port.
        cases = [(-20, 7), (-15, 7), (-14, 6), (-10, 6), (-9, 5), (-5, 5),
                 (-4, 4), (0, 4), (1, 3), (5, 3), (6, 2), (10, 2),
                 (11, 1), (15, 1), (16, 0), (19, 0)]
        for nice, want in cases:
            self.assertEqual(port_nice_to_priority(nice), want,
                             "nice=%d maps wrong in the port" % nice)
        # Pin the exact ladder tokens (they appear as chained else-ifs).
        self.assertIn("if nice <= -15 { 7u8 }", self.code)
        self.assertIn("else if nice <= -10 { 6u8 }", self.code)
        self.assertIn("else if nice <= -5  { 5u8 }", self.code)
        self.assertIn("else if nice <= 0   { 4u8 }", self.code)
        self.assertIn("else if nice <= 5   { 3u8 }", self.code)
        self.assertIn("else if nice <= 10  { 2u8 }", self.code)
        self.assertIn("else if nice <= 15  { 1u8 }", self.code)
        self.assertIn("else { 0u8 };", self.code)

    def test_getattr_nice_table_matches_port(self):
        self._require()
        for priority in range(8):
            self.assertEqual(port_priority_to_nice(priority),
                             {7: -20, 6: -10, 5: -5, 4: 0, 3: 5, 2: 10, 1: 15,
                              }.get(priority, 19))
        self.assertIn("7 => -20, 6 => -10, 5 => -5, 4 => 0,", self.code)
        self.assertIn("3 => 5, 2 => 10, 1 => 15, _ => 19,", self.code)


class TestWakeRotateBehavior(unittest.TestCase):
    """The rotate-wake state machine, exercised behaviorally (port)."""

    def test_max_wake_caps_woken_count(self):
        q = [{"key": 0xF0} for _ in range(5)]
        woken, remaining = port_wake_rotate(q, 0xF0, 3)
        self.assertEqual(woken, 3)
        self.assertEqual(len(remaining), 2)

    def test_wrong_key_never_woken(self):
        q = [{"key": 0x42}, {"key": 0x43}, {"key": 0x42}]
        woken, remaining = port_wake_rotate(q, 0x43, 10)
        self.assertEqual(woken, 1)
        self.assertEqual([t["key"] for t in remaining], [0x42, 0x42],
                         "non-matching threads must survive in FIFO order")

    def test_zero_max_wake_wakes_none(self):
        q = [{"key": 0xAA}]
        woken, remaining = port_wake_rotate(q, 0xAA, 0)
        self.assertEqual(woken, 0)
        self.assertEqual(len(remaining), 1)

    def test_empty_queue(self):
        woken, remaining = port_wake_rotate([], 0xAA, 10)
        self.assertEqual((woken, remaining), (0, []))


class TestFutexPriorityInheritanceDiscarded(unittest.TestCase):
    """The PI call site exists but its result is thrown away (scaffolding)."""

    @classmethod
    def setUpClass(cls):
        cls.futex = _read_kernel(os.path.join("kernel", "src", "syscalls", "futex.rs"))
        cls.skip_why = None
        if cls.futex is None:
            cls.skip_why = "kernel tree not found (SKYOS_KERNEL_DIR or a " \
                           "SKYIOUS KERNEL sibling)"
        else:
            cls.code = strip_rust(cls.futex)

    def _require(self):
        if self.skip_why:
            self.skipTest(self.skip_why)

    def test_boost_call_site_ignores_result(self):
        self._require()
        self.assertIn("scheduler::boost_thread_priority(owner_pid as u64, our_prio);",
                      self.code,
                      "the FUTEX_WAITERS priority-inheritance call site vanished")
        # The result is a statement, not a condition: no `if` wraps it, so a
        # failed boost is silent.
        start = self.code.index("scheduler::boost_thread_priority(owner_pid as u64, our_prio);")
        pre = self.code[max(0, start - 200):start]
        self.assertNotIn("if ", pre.split("\n")[-1],
                         "boost result must stay discarded (PI is a stub)")


if __name__ == "__main__":
    unittest.main()
