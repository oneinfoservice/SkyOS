use crate::Test;

// ---------------------------------------------------------------------------
// Port 1: the PI futex word protocol (`kernel/src/syscalls/futex.rs`).
//
// The kernel stores the lock owner's PID in the low 31 bits of the futex word
// and the FUTEX_WAITERS bit (0x8000_0000) in bit 31. futex_lock_pi's fast
// path cmpxchg's 0 -> pid; a contended lock sets the WAITERS bit; unlock
// checks the owner (EPERM on mismatch) and wakes one waiter iff WAITERS was
// set. This port runs the same decision tree on an in-memory u32.
// ---------------------------------------------------------------------------

const FUTEX_WAITERS: u32 = 0x8000_0000;
const PID_MASK: u32 = 0x7FFF_FFFF;

#[derive(Debug, PartialEq)]
enum LockOutcome {
    Acquired,
    Contended { owner: u64 },
}

struct FutexWord {
    word: u32,
}

impl FutexWord {
    fn new() -> Self {
        FutexWord { word: 0 }
    }

    /// Mirrors futex_lock_pi's fast-path + contend logic (sans scheduler
    /// blocking, which is exercised by the queue port below).
    fn lock(&mut self, pid: u64) -> LockOutcome {
        let pid32 = pid as u32;
        if self.word == 0 {
            self.word = pid32;
            LockOutcome::Acquired
        } else {
            if self.word & FUTEX_WAITERS == 0 {
                self.word |= FUTEX_WAITERS;
            }
            LockOutcome::Contended {
                owner: (self.word & PID_MASK) as u64,
            }
        }
    }

    /// Mirrors futex_unlock_pi: EPERM unless the caller owns the lock;
    /// clears the word and reports whether a waiter must be woken.
    fn unlock(&mut self, pid: u64) -> Result<u32, &'static str> {
        let prev = self.word;
        if prev & PID_MASK != pid as u32 {
            return Err("EPERM");
        }
        let wakes = if prev & FUTEX_WAITERS != 0 { 1 } else { 0 };
        self.word = 0;
        Ok(wakes)
    }

    fn owner(&self) -> u64 {
        (self.word & PID_MASK) as u64
    }

    fn has_waiters(&self) -> bool {
        self.word & FUTEX_WAITERS != 0
    }
}

// ---------------------------------------------------------------------------
// Port 2: the scheduler futex waiter queue (`kernel/src/task/scheduler.rs`).
//
// wake_futex pops the FIFO queue and wakes up to max_wake threads whose
// futex_wake_addr matches, preserving non-matching waiters in order.
// wake_process_futex rotates the queue and wakes EVERY thread of a given pid
// (used when a process exits). Woken threads enter the ready queue at
// ready_queue_index(priority) = min(priority, 7).
// ---------------------------------------------------------------------------

fn ready_queue_index(priority: u8) -> usize {
    if priority > 7 { 7 } else { priority as usize }
}

struct FutexWaiter {
    addr: u64,
    pid: u64,
    priority: u8,
}

struct FutexQueue {
    waiters: std::collections::VecDeque<FutexWaiter>,
}

impl FutexQueue {
    fn new() -> Self {
        FutexQueue {
            waiters: std::collections::VecDeque::new(),
        }
    }

    fn add(&mut self, addr: u64, pid: u64, priority: u8) {
        self.waiters.push_back(FutexWaiter { addr, pid, priority });
    }

    fn len(&self) -> usize {
        self.waiters.len()
    }

    /// Mirrors scheduler::wake_futex: FIFO scan, wake up to max_wake matches.
    fn wake_futex(&mut self, uaddr: u64, max_wake: u32) -> u32 {
        let mut woken = 0u32;
        let mut still = std::collections::VecDeque::new();
        while let Some(waiter) = self.waiters.pop_front() {
            if woken < max_wake && waiter.addr == uaddr {
                let _idx = ready_queue_index(waiter.priority);
                woken += 1;
            } else {
                still.push_back(waiter);
            }
        }
        self.waiters = still;
        woken
    }

    /// Mirrors scheduler::wake_process_futex: wake every waiter of `pid`
    /// (no cap), rotating the queue in place.
    fn wake_process(&mut self, pid: u64) -> u32 {
        let mut woken = 0u32;
        let n = self.waiters.len();
        for _ in 0..n {
            let Some(waiter) = self.waiters.pop_front() else { break };
            if waiter.pid == pid {
                woken += 1;
            } else {
                self.waiters.push_back(waiter);
            }
        }
        woken
    }
}

pub fn tests() -> Vec<Test> {
    vec![
        // ---- PI futex word protocol ----
        Test {
            name: "futex_pi_acquire_fast_path",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut f = FutexWord::new();
                assert_eq_result!(f.lock(5), LockOutcome::Acquired);
                assert_eq_result!(f.word, 5);
                assert_eq_result!(f.owner(), 5);
                assert_result!(!f.has_waiters(), "uncontended acquire has no WAITERS bit");
                Ok(())
            }),
        },
        Test {
            name: "futex_pi_contend_sets_waiters",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut f = FutexWord::new();
                f.lock(5);
                let outcome = f.lock(7);
                assert_eq_result!(outcome, LockOutcome::Contended { owner: 5 });
                assert_result!(f.has_waiters(), "contended lock sets WAITERS bit");
                assert_eq_result!(f.owner(), 5, "WAITERS bit must not corrupt the owner PID");
                Ok(())
            }),
        },
        Test {
            name: "futex_pi_unlock_wrong_owner_eperm",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut f = FutexWord::new();
                f.lock(5);
                let r = f.unlock(9);
                assert_result!(r.is_err(), "non-owner unlock must be rejected");
                assert_eq_result!(f.word & PID_MASK, 5, "word unchanged on EPERM");
                Ok(())
            }),
        },
        Test {
            name: "futex_pi_unlock_contended_wakes",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut f = FutexWord::new();
                f.lock(5);
                f.lock(7); // sets WAITERS
                let wakes = f.unlock(5);
                assert_result!(wakes.is_ok(), "owner unlock succeeds");
                assert_eq_result!(wakes.unwrap(), 1, "contended unlock wakes one waiter");
                assert_eq_result!(f.word, 0, "word cleared");
                Ok(())
            }),
        },
        Test {
            name: "futex_pi_unlock_uncontended_no_wake",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut f = FutexWord::new();
                f.lock(5);
                let wakes = f.unlock(5);
                assert_result!(wakes.is_ok(), "owner unlock succeeds");
                assert_eq_result!(wakes.unwrap(), 0, "no WAITERS bit -> no wake");
                assert_eq_result!(f.word, 0, "word cleared");
                Ok(())
            }),
        },
        Test {
            name: "futex_pi_word_owner_pid_mask",
            category: "kernel::futex",
            run: Box::new(|| {
                // PIDs up to 0x7FFF_FFFF must survive the WAITERS bit.
                let mut f = FutexWord::new();
                f.lock(0x1234_5678);
                f.lock(1);
                assert_eq_result!(f.owner(), 0x1234_5678);
                assert_eq_result!(f.word, 0x9234_5678, "WAITERS (bit 31) OR'd over owner");
                Ok(())
            }),
        },
        // ---- Scheduler futex waiter queue ----
        Test {
            name: "futex_queue_wake_matching",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut q = FutexQueue::new();
                q.add(0x1000, 1, 3);
                q.add(0x1000, 2, 3);
                q.add(0x1000, 3, 3);
                let woken = q.wake_futex(0x1000, 1);
                assert_eq_result!(woken, 1, "wake up to max_wake");
                assert_eq_result!(q.len(), 2, "non-woken waiters stay queued");
                Ok(())
            }),
        },
        Test {
            name: "futex_queue_wake_fifo",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut q = FutexQueue::new();
                q.add(0x1000, 1, 3);
                q.add(0x1000, 2, 3);
                q.add(0x1000, 3, 3);
                let woken = q.wake_futex(0x1000, 2);
                assert_eq_result!(woken, 2);
                // FIFO: pids 1 and 2 woke; pid 3 (latest) remains first in line.
                let remaining: Vec<u64> = q.waiters.iter().map(|w| w.pid).collect();
                assert_eq_result!(remaining, vec![3]);
                Ok(())
            }),
        },
        Test {
            name: "futex_queue_wake_max_cap",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut q = FutexQueue::new();
                for pid in 1..=3u64 {
                    q.add(0x2000, pid, 3);
                }
                let woken = q.wake_futex(0x2000, 10);
                assert_eq_result!(woken, 3, "max_wake above queue length wakes all");
                assert_eq_result!(q.len(), 0);
                Ok(())
            }),
        },
        Test {
            name: "futex_queue_wake_addr_mismatch_preserved",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut q = FutexQueue::new();
                q.add(0x1000, 1, 3);
                q.add(0x1000, 2, 3);
                q.add(0x2000, 3, 3);
                let woken = q.wake_futex(0x1000, 10);
                assert_eq_result!(woken, 2, "only matching-addr waiters wake");
                let remaining: Vec<u64> = q.waiters.iter().map(|w| w.pid).collect();
                assert_eq_result!(remaining, vec![3]);
                Ok(())
            }),
        },
        Test {
            name: "futex_queue_wake_process_selects_pid",
            category: "kernel::futex",
            run: Box::new(|| {
                let mut q = FutexQueue::new();
                q.add(0x1000, 3, 3);
                q.add(0x1000, 9, 3);
                q.add(0x1000, 3, 3);
                let woken = q.wake_process(3);
                assert_eq_result!(woken, 2, "all waiters of pid 3 wake");
                let remaining: Vec<u64> = q.waiters.iter().map(|w| w.pid).collect();
                assert_eq_result!(remaining, vec![9]);
                Ok(())
            }),
        },
        Test {
            name: "futex_queue_priority_ready_index_cap",
            category: "kernel::futex",
            run: Box::new(|| {
                // Kernel: `let p = if thread.priority > 7 { 7 } else { priority }`
                // then indexes ready_queues[p].
                assert_eq_result!(ready_queue_index(0), 0);
                assert_eq_result!(ready_queue_index(7), 7);
                assert_eq_result!(ready_queue_index(9), 7, "priority capped at top queue");
                assert_eq_result!(ready_queue_index(255), 7);
                Ok(())
            }),
        },
    ]
}
