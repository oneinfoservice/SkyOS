use crate::Test;

// ---------------------------------------------------------------------------
// Port 1: the pipe block queue (`kernel/src/task/scheduler.rs`).
//
// `block_on_pipe(key)` marks the current thread Blocked with
// `pipe_block_key = Some(key)` and schedules away; the thread lands in the
// global `block_queue` (FIFO). `wake_pipe(key)` runs
// `wake_blocked_threads(key, u32::MAX)`: a FIFO scan that wakes up to
// max_wake threads whose pipe_block_key matches, clears the key, routes
// them to ready_queues[min(priority, 7)], and preserves every non-woken
// waiter in order. `wake_process_blocked(pid)` (process-exit path) rotates
// the queue in place, waking EVERY thread of the given pid with no cap —
// and must not allocate (tick IRQ context).
// ---------------------------------------------------------------------------

const READY_QUEUES: usize = 8;

fn ready_queue_index(priority: u8) -> usize {
    if priority > 7 {
        7
    } else {
        priority as usize
    }
}

#[derive(Debug, Clone, PartialEq)]
enum TStatus {
    Blocked,
    Ready,
}

/// Ready-queue snapshot: the per-priority FIFO lanes plus the dirty flag
/// the kernel sets (`mark_ready_queues_dirty`) only when something was
/// actually woken.
#[derive(Debug)]
struct ReadyQueues {
    queues: [std::collections::VecDeque<u64>; READY_QUEUES],
    dirty: bool,
}

impl ReadyQueues {
    fn new() -> Self {
        ReadyQueues {
            queues: std::array::from_fn(|_| std::collections::VecDeque::new()),
            dirty: false,
        }
    }

    fn push(&mut self, pid: u64, priority: u8) {
        self.queues[ready_queue_index(priority)].push_back(pid);
        self.dirty = true;
    }

    fn lane(&self, priority: u8) -> Vec<u64> {
        self.queues[ready_queue_index(priority)].iter().copied().collect()
    }

    fn all(&self) -> Vec<u64> {
        self.queues.iter().flatten().copied().collect()
    }
}

struct BlockedThread {
    pid: u64,
    priority: u8,
    pipe_block_key: Option<u64>,
    status: TStatus,
}

struct PipeBlockQueue {
    waiters: std::collections::VecDeque<BlockedThread>,
}

impl PipeBlockQueue {
    fn new() -> Self {
        PipeBlockQueue {
            waiters: std::collections::VecDeque::new(),
        }
    }

    /// Mirrors `scheduler::block_on_pipe`: the caller's thread is marked
    /// Blocked with a pipe_block_key and joins the FIFO queue.
    fn block_on(&mut self, pid: u64, priority: u8, key: u64) {
        self.waiters.push_back(BlockedThread {
            pid,
            priority,
            pipe_block_key: Some(key),
            status: TStatus::Blocked,
        });
    }

    fn len(&self) -> usize {
        self.waiters.len()
    }

    /// Mirrors `GlobalScheduler::wake_blocked_threads`: FIFO scan, wake up
    /// to max_wake matching-key threads into the ready lanes, preserve the
    /// rest in order. Only reports the dirty flag when something woke.
    fn wake_blocked(&mut self, key: u64, max_wake: u32, rq: &mut ReadyQueues) -> u32 {
        let mut woken = 0u32;
        let mut still = std::collections::VecDeque::new();
        while let Some(mut t) = self.waiters.pop_front() {
            if woken < max_wake && t.pipe_block_key == Some(key) {
                t.status = TStatus::Ready;
                t.pipe_block_key = None;
                rq.push(t.pid, t.priority);
                woken += 1;
            } else {
                still.push_back(t);
            }
        }
        self.waiters = still;
        rq.dirty = woken > 0;
        woken
    }

    /// Mirrors `wake_pipe`: every thread blocked on the key wakes.
    fn wake_all(&mut self, key: u64, rq: &mut ReadyQueues) -> u32 {
        self.wake_blocked(key, u32::MAX, rq)
    }

    /// Mirrors `wake_process_blocked`: rotate the queue in place (bounded
    /// loop, no allocation), waking EVERY thread of `pid` regardless of
    /// key, preserving the rest in order.
    fn wake_process(&mut self, pid: u64, rq: &mut ReadyQueues) -> u32 {
        let mut woken = 0u32;
        let n = self.waiters.len();
        for _ in 0..n {
            let Some(mut t) = self.waiters.pop_front() else { break };
            if t.pid == pid {
                t.status = TStatus::Ready;
                t.pipe_block_key = None;
                rq.push(t.pid, t.priority);
                woken += 1;
            } else {
                self.waiters.push_back(t);
            }
        }
        rq.dirty = woken > 0;
        woken
    }

    fn blocked_keys(&self) -> Vec<Option<u64>> {
        self.waiters.iter().map(|t| t.pipe_block_key).collect()
    }
}

// ---------------------------------------------------------------------------
// Port 2: the sleep timer queue (`kernel/src/task/scheduler.rs`, `tick`).
//
// Sleepers carry `sleep_until: Option<u64>` (tick counter deadline). The
// tick handler rotates the sleep_queue IN PLACE with a bounded number of
// iterations (IRQ context: no allocation), waking a thread when its
// deadline has passed OR its process has a pending unmasked signal. Woken
// threads are cleared of sleep_until (so one tick cannot wake them twice),
// marked Ready, and routed to ready_queues[min(priority, 7)].
// ---------------------------------------------------------------------------

struct SleepThread {
    pid: u64,
    priority: u8,
    sleep_until: Option<u64>,
    /// Simulates the process signal state the kernel checks:
    /// `sig.has_unmasked_pending(sig.blocked)`.
    has_unmasked_pending: bool,
    status: TStatus,
}

struct SleepQueue {
    sleepers: std::collections::VecDeque<SleepThread>,
}

impl SleepQueue {
    fn new() -> Self {
        SleepQueue {
            sleepers: std::collections::VecDeque::new(),
        }
    }

    fn add(&mut self, pid: u64, priority: u8, sleep_until: Option<u64>, has_unmasked_pending: bool) {
        self.sleepers.push_back(SleepThread {
            pid,
            priority,
            sleep_until,
            has_unmasked_pending,
            status: TStatus::Blocked,
        });
    }

    fn len(&self) -> usize {
        self.sleepers.len()
    }

    /// Mirrors tick()'s sleep_queue pass: rotate in place (n iterations),
    /// wake due or signal-pending sleepers, keep the rest in FIFO order.
    /// The caller's ready lanes are only marked dirty when something woke.
    fn tick(&mut self, current_ticks: u64, rq: &mut ReadyQueues) -> u32 {
        let mut woken = 0u32;
        let n = self.sleepers.len();
        for _ in 0..n {
            let Some(mut t) = self.sleepers.pop_front() else { break };
            let due = t.sleep_until.is_some_and(|wake_time| current_ticks >= wake_time);
            if due || t.has_unmasked_pending {
                t.status = TStatus::Ready;
                t.sleep_until = None;
                rq.push(t.pid, t.priority);
                woken += 1;
            } else {
                self.sleepers.push_back(t);
            }
        }
        rq.dirty = woken > 0;
        woken
    }

    fn pending_deadlines(&self) -> Vec<Option<u64>> {
        self.sleepers.iter().map(|t| t.sleep_until).collect()
    }
}

pub fn tests() -> Vec<Test> {
    vec![
        // ---- Pipe block queue ----
        Test {
            name: "pipe_block_block_on_marks_blocked",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = PipeBlockQueue::new();
                q.block_on(7, 3, 0xABCD);
                assert_eq_result!(q.len(), 1);
                assert_eq_result!(q.blocked_keys(), vec![Some(0xABCD)]);
                // The blocked thread is not runnable anywhere yet.
                let rq = ReadyQueues::new();
                assert_eq_result!(rq.all(), Vec::<u64>::new());
                Ok(())
            }),
        },
        Test {
            name: "pipe_block_wake_matching_key_only",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = PipeBlockQueue::new();
                q.block_on(1, 3, 10);
                q.block_on(2, 3, 20);
                q.block_on(3, 3, 10);
                let mut rq = ReadyQueues::new();
                let woken = q.wake_blocked(10, u32::MAX, &mut rq);
                assert_eq_result!(woken, 2, "only waiters on key 10 wake");
                assert_eq_result!(rq.all(), vec![1, 3], "FIFO wake order");
                assert_eq_result!(q.blocked_keys(), vec![Some(20)]);
                Ok(())
            }),
        },
        Test {
            name: "pipe_block_wake_fifo_respects_max_wake",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = PipeBlockQueue::new();
                q.block_on(1, 3, 5);
                q.block_on(2, 3, 5);
                q.block_on(3, 3, 5);
                let mut rq = ReadyQueues::new();
                let woken = q.wake_blocked(5, 2, &mut rq);
                assert_eq_result!(woken, 2);
                assert_eq_result!(rq.all(), vec![1, 2], "FIFO: oldest two wake");
                assert_eq_result!(q.len(), 1, "third stays queued");
                assert_eq_result!(q.blocked_keys(), vec![Some(5)], "still blocked on the key");
                Ok(())
            }),
        },
        Test {
            name: "pipe_block_wake_all_drains_queue",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = PipeBlockQueue::new();
                for pid in 1..=3u64 {
                    q.block_on(pid, 3, 9);
                }
                let mut rq = ReadyQueues::new();
                let woken = q.wake_all(9, &mut rq);
                assert_eq_result!(woken, 3, "wake_pipe wakes every waiter");
                assert_eq_result!(q.len(), 0);
                assert_result!(rq.dirty, "waking marks ready queues dirty");
                Ok(())
            }),
        },
        Test {
            name: "pipe_block_nonmatching_preserved_in_order",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = PipeBlockQueue::new();
                q.block_on(1, 3, 10);
                q.block_on(2, 3, 20);
                q.block_on(3, 3, 10);
                q.block_on(4, 3, 20);
                let mut rq = ReadyQueues::new();
                let woken = q.wake_blocked(10, u32::MAX, &mut rq);
                assert_eq_result!(woken, 2);
                // The non-matching waiters keep their relative order.
                assert_eq_result!(q.blocked_keys(), vec![Some(20), Some(20)]);
                let remaining: Vec<u64> = q.waiters.iter().map(|t| t.pid).collect();
                assert_eq_result!(remaining, vec![2, 4]);
                Ok(())
            }),
        },
        Test {
            name: "pipe_block_wake_process_rotates_in_place",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = PipeBlockQueue::new();
                q.block_on(3, 3, 10);
                q.block_on(9, 3, 10);
                q.block_on(3, 3, 20);
                q.block_on(9, 3, 20);
                let mut rq = ReadyQueues::new();
                let woken = q.wake_process(3, &mut rq);
                assert_eq_result!(woken, 2, "both pid-3 waiters wake, regardless of key");
                let remaining: Vec<u64> = q.waiters.iter().map(|t| t.pid).collect();
                assert_eq_result!(remaining, vec![9, 9], "non-matching preserved in order");
                assert_eq_result!(q.blocked_keys(), vec![Some(10), Some(20)]);
                Ok(())
            }),
        },
        Test {
            name: "pipe_block_ready_priority_routing",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = PipeBlockQueue::new();
                q.block_on(1, 2, 7);
                q.block_on(2, 7, 7);
                q.block_on(3, 9, 7); // capped at top lane
                let mut rq = ReadyQueues::new();
                let woken = q.wake_all(7, &mut rq);
                assert_eq_result!(woken, 3);
                assert_eq_result!(rq.lane(2), vec![1]);
                assert_eq_result!(rq.lane(7), vec![2, 3]);
                assert_result!(rq.dirty, "woken > 0 marks ready queues dirty");
                Ok(())
            }),
        },
        Test {
            name: "pipe_block_no_wake_leaves_queues_clean",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = PipeBlockQueue::new();
                q.block_on(1, 3, 10);
                let mut rq = ReadyQueues::new();
                let woken = q.wake_blocked(99, u32::MAX, &mut rq);
                assert_eq_result!(woken, 0);
                assert_result!(!rq.dirty, "no wake -> ready queues not marked dirty");
                assert_eq_result!(q.len(), 1);
                assert_eq_result!(rq.all(), Vec::<u64>::new());
                Ok(())
            }),
        },
        // ---- Sleep timer queue ----
        Test {
            name: "sleep_wake_when_deadline_reached",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = SleepQueue::new();
                q.add(1, 3, Some(100), false);
                let mut rq = ReadyQueues::new();
                let woken = q.tick(100, &mut rq);
                assert_eq_result!(woken, 1, "tick at the deadline wakes");
                assert_eq_result!(q.len(), 0);
                assert_eq_result!(rq.all(), vec![1]);
                assert_result!(rq.dirty, "woken > 0 marks ready queues dirty");
                Ok(())
            }),
        },
        Test {
            name: "sleep_deadline_not_reached_stays",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = SleepQueue::new();
                q.add(1, 3, Some(100), false);
                let mut rq = ReadyQueues::new();
                let woken = q.tick(99, &mut rq);
                assert_eq_result!(woken, 0, "one tick early is still asleep");
                assert_eq_result!(q.len(), 1);
                assert_result!(!rq.dirty, "nothing woken -> ready queues untouched");
                // And it wakes on the next tick.
                let woken = q.tick(100, &mut rq);
                assert_eq_result!(woken, 1);
                Ok(())
            }),
        },
        Test {
            name: "sleep_fifo_order_preserved",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = SleepQueue::new();
                q.add(1, 3, Some(50), false); // due
                q.add(2, 3, Some(200), false); // not due
                q.add(3, 3, Some(100), false); // due
                q.add(4, 3, Some(300), false); // not due
                let mut rq = ReadyQueues::new();
                let woken = q.tick(100, &mut rq);
                assert_eq_result!(woken, 2);
                assert_eq_result!(rq.all(), vec![1, 3], "FIFO wake order");
                // The sleepers that stay keep their relative order.
                assert_eq_result!(q.pending_deadlines(), vec![Some(200), Some(300)]);
                let remaining: Vec<u64> = q.sleepers.iter().map(|t| t.pid).collect();
                assert_eq_result!(remaining, vec![2, 4]);
                Ok(())
            }),
        },
        Test {
            name: "sleep_signal_wakes_before_deadline",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = SleepQueue::new();
                q.add(1, 3, Some(1000), true); // pending unmasked signal
                q.add(2, 3, Some(1000), false);
                let mut rq = ReadyQueues::new();
                let woken = q.tick(10, &mut rq);
                assert_eq_result!(woken, 1, "signal interrupts sleep early");
                assert_eq_result!(rq.all(), vec![1]);
                assert_eq_result!(q.pending_deadlines(), vec![Some(1000)]);
                Ok(())
            }),
        },
        Test {
            name: "sleep_until_cleared_on_wake",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = SleepQueue::new();
                q.add(1, 3, Some(50), false);
                let mut rq = ReadyQueues::new();
                assert_eq_result!(q.tick(50, &mut rq), 1);
                assert_eq_result!(q.tick(60, &mut rq), 0, "woken thread is not re-woken");
                assert_eq_result!(q.len(), 0);
                Ok(())
            }),
        },
        Test {
            name: "sleep_priority_ready_routing",
            category: "kernel::wait",
            run: Box::new(|| {
                let mut q = SleepQueue::new();
                q.add(1, 2, Some(1), false);
                q.add(2, 7, Some(1), false);
                q.add(3, 9, Some(1), false); // capped at top lane
                let mut rq = ReadyQueues::new();
                let woken = q.tick(1, &mut rq);
                assert_eq_result!(woken, 3);
                assert_eq_result!(rq.lane(2), vec![1]);
                assert_eq_result!(rq.lane(7), vec![2, 3]);
                Ok(())
            }),
        },
        Test {
            name: "sleep_rotate_in_place_bounded",
            category: "kernel::wait",
            run: Box::new(|| {
                // The tick handler runs in IRQ context with IF=0 and must not
                // allocate: rotation is bounded by the queue length and the
                // deque capacity never grows across a tick.
                let mut q = SleepQueue::new();
                for pid in 1..=4u64 {
                    q.add(pid, 3, Some(1000), false);
                }
                let cap_before = q.sleepers.capacity();
                let mut rq = ReadyQueues::new();
                assert_eq_result!(q.tick(100, &mut rq), 0);
                assert_eq_result!(q.len(), 4);
                assert_eq_result!(q.sleepers.capacity(), cap_before, "no growth");
                let order: Vec<u64> = q.sleepers.iter().map(|t| t.pid).collect();
                assert_eq_result!(order, vec![1, 2, 3, 4], "full rotation preserves order");
                Ok(())
            }),
        },
    ]
}
