use crate::Test;

// ---------------------------------------------------------------------------
// Port 1: 4-level page index math (mirrors x86_64 Page::p4_index() etc. as
// used by `kernel/src/memory/paging.rs`'s manual walks and handle_cow).
// ---------------------------------------------------------------------------

fn p4_index(addr: u64) -> usize {
    ((addr >> 39) & 0x1FF) as usize
}
fn p3_index(addr: u64) -> usize {
    ((addr >> 30) & 0x1FF) as usize
}
fn p2_index(addr: u64) -> usize {
    ((addr >> 21) & 0x1FF) as usize
}
fn p1_index(addr: u64) -> usize {
    ((addr >> 12) & 0x1FF) as usize
}
fn page_offset(addr: u64) -> usize {
    (addr & 0xFFF) as usize
}

// ---------------------------------------------------------------------------
// Port 2: frame refcount table (`kernel/src/memory/frame_info.rs`).
//
// Managed frames start at 0 and saturate on increment; decrement of a count
// <= 1 zeroes it and queues the frame for deferred free (returning 0).
// Frames never touched report count 1 (the kernel's unmanaged default), which
// is what lets exclusively-owned frames skip refcounting entirely.
// ---------------------------------------------------------------------------

struct FrameRefs {
    counts: std::collections::HashMap<u64, u16>,
    deferred: Vec<u64>,
}

impl FrameRefs {
    fn new() -> Self {
        FrameRefs {
            counts: std::collections::HashMap::new(),
            deferred: Vec::new(),
        }
    }

    fn count(&self, phys: u64) -> u16 {
        self.counts.get(&phys).copied().unwrap_or(1)
    }

    fn increment(&mut self, phys: u64) {
        let c = self.counts.entry(phys).or_insert(0);
        *c = c.saturating_add(1);
    }

    /// Mirrors frame_info::decrement: >1 -> subtract; <=1 -> 0 + deferred.
    fn decrement(&mut self, phys: u64) -> u16 {
        let c = self.counts.entry(phys).or_insert(0);
        if *c > 1 {
            *c -= 1;
            return *c;
        }
        *c = 0;
        self.deferred.push(phys);
        0
    }

    fn drain_deferred(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.deferred)
    }
}

// ---------------------------------------------------------------------------
// Port 3: COW page-table entry semantics (`kernel/src/memory/paging.rs`).
//
// clone_recursive's leaf handling: writable pages become read-only with the
// software COW bit (bit 9) set and the frame refcount incremented. handle_cow:
// non-COW -> None; refcount > 1 -> allocate a fresh frame, copy, decrement the
// old one; refcount == 1 -> flip back in place. Either way the entry ends up
// writable with the COW bit cleared.
// ---------------------------------------------------------------------------

const WRITABLE: u64 = 1 << 1;
const COW_BIT: u64 = 1 << 9;

#[derive(Clone, Copy)]
struct Pte {
    frame: u64,
    bits: u64,
}

impl Pte {
    fn writable(&self) -> bool {
        self.bits & WRITABLE != 0
    }
    fn cow(&self) -> bool {
        self.bits & COW_BIT != 0
    }
}

/// Mock frame allocator: hands out ascending frame numbers.
struct MockAlloc {
    next: u64,
}

impl MockAlloc {
    fn new(start: u64) -> Self {
        MockAlloc { next: start }
    }
    fn allocate(&mut self) -> Option<u64> {
        let f = self.next;
        self.next += 1;
        Some(f)
    }
}

/// Mirrors clone_recursive's leaf COW marking.
fn mark_cow(entry: &mut Pte, refs: &mut FrameRefs) {
    if entry.writable() {
        entry.bits &= !WRITABLE;
        entry.bits |= COW_BIT;
        refs.increment(entry.frame);
    }
}

/// Mirrors AddressSpace::handle_cow. Returns None when the page is not COW.
fn handle_cow(entry: &mut Pte, refs: &mut FrameRefs, alloc: &mut MockAlloc) -> Option<bool> {
    if !entry.cow() {
        return None;
    }
    let old = entry.frame;
    if refs.count(old) > 1 {
        let new = alloc.allocate()?;
        // (copy of the 4096-byte page omitted: only frame identity matters here)
        entry.frame = new;
        refs.decrement(old);
    }
    entry.bits |= WRITABLE;
    entry.bits &= !COW_BIT;
    Some(true)
}

pub fn tests() -> Vec<Test> {
    vec![
        // ---- Page index math ----
        Test {
            name: "page_index_zero_address",
            category: "kernel::paging",
            run: Box::new(|| {
                assert_eq_result!((p4_index(0), p3_index(0), p2_index(0), p1_index(0), page_offset(0)), (0, 0, 0, 0, 0));
                Ok(())
            }),
        },
        Test {
            name: "page_index_kernel_boundary",
            category: "kernel::paging",
            run: Box::new(|| {
                // 0xFFFF_8000_0000_0000 is the kernel half boundary: p4 = 256.
                assert_eq_result!(p4_index(0xFFFF_8000_0000_0000), 256);
                assert_eq_result!((p3_index(0xFFFF_8000_0000_0000), p2_index(0xFFFF_8000_0000_0000), p1_index(0xFFFF_8000_0000_0000)), (0, 0, 0));
                Ok(())
            }),
        },
        Test {
            name: "page_index_top_of_address_space",
            category: "kernel::paging",
            run: Box::new(|| {
                let top = 0xFFFF_FFFF_FFFF_F000u64;
                assert_eq_result!(p4_index(top), 511);
                assert_eq_result!(p3_index(top), 511);
                assert_eq_result!(p2_index(top), 511);
                assert_eq_result!(p1_index(top), 511);
                assert_eq_result!(page_offset(top), 0);
                Ok(())
            }),
        },
        Test {
            name: "page_index_roundtrip",
            category: "kernel::paging",
            run: Box::new(|| {
                // A 48-bit canonical address must split and reassemble losslessly.
                let addr = 0x0000_1234_5678_9000u64;
                let rebuilt = ((p4_index(addr) as u64) << 39)
                    | ((p3_index(addr) as u64) << 30)
                    | ((p2_index(addr) as u64) << 21)
                    | ((p1_index(addr) as u64) << 12)
                    | (page_offset(addr) as u64);
                assert_eq_result!(rebuilt, addr);
                Ok(())
            }),
        },
        // ---- Frame refcounts ----
        Test {
            name: "frame_refs_unmanaged_default_one",
            category: "kernel::paging",
            run: Box::new(|| {
                let refs = FrameRefs::new();
                assert_eq_result!(refs.count(0x1000), 1, "never-touched frames read as 1");
                Ok(())
            }),
        },
        Test {
            name: "frame_refs_increment_tracks_shares",
            category: "kernel::paging",
            run: Box::new(|| {
                let mut refs = FrameRefs::new();
                refs.increment(0x1000);
                refs.increment(0x1000);
                assert_eq_result!(refs.count(0x1000), 2, "two COW marks -> refcount 2");
                Ok(())
            }),
        },
        Test {
            name: "frame_refs_decrement_above_one",
            category: "kernel::paging",
            run: Box::new(|| {
                let mut refs = FrameRefs::new();
                refs.increment(0x1000);
                refs.increment(0x1000);
                refs.increment(0x1000);
                let remaining = refs.decrement(0x1000);
                assert_eq_result!(remaining, 2);
                assert_result!(refs.deferred.is_empty(), "count > 1 does not defer");
                Ok(())
            }),
        },
        Test {
            name: "frame_refs_decrement_to_zero_defers",
            category: "kernel::paging",
            run: Box::new(|| {
                let mut refs = FrameRefs::new();
                refs.increment(0x1000); // count 1
                let remaining = refs.decrement(0x1000);
                assert_eq_result!(remaining, 0);
                assert_eq_result!(refs.deferred.as_slice(), &[0x1000][..], "final drop queues deferred free");
                assert_eq_result!(refs.count(0x1000), 0);
                Ok(())
            }),
        },
        Test {
            name: "frame_refs_drain_empties_deferred",
            category: "kernel::paging",
            run: Box::new(|| {
                let mut refs = FrameRefs::new();
                refs.increment(0x1000);
                refs.decrement(0x1000);
                refs.increment(0x2000);
                refs.decrement(0x2000);
                let drained = refs.drain_deferred();
                assert_eq_result!(drained, vec![0x1000, 0x2000]);
                assert_result!(refs.deferred.is_empty(), "drain empties the queue");
                Ok(())
            }),
        },
        // ---- COW marking (clone_recursive leaf) ----
        Test {
            name: "cow_mark_writable_clears_write_sets_bit9",
            category: "kernel::paging",
            run: Box::new(|| {
                let mut refs = FrameRefs::new();
                let mut pte = Pte { frame: 0x1000, bits: WRITABLE | 0x20 };
                mark_cow(&mut pte, &mut refs);
                assert_result!(!pte.writable(), "COW-marked page is read-only");
                assert_result!(pte.cow(), "COW bit 9 set");
                // Managed frames start at 0 in the table, so ONE increment
                // reads back as 1 -- the kernel's `count` returns the raw
                // table value, not the unmanaged default.
                assert_eq_result!(refs.count(0x1000), 1, "marking increments the frame refcount");
                assert_eq_result!(pte.frame, 0x1000, "frame unchanged");
                Ok(())
            }),
        },
        Test {
            name: "cow_mark_readonly_untouched",
            category: "kernel::paging",
            run: Box::new(|| {
                let mut refs = FrameRefs::new();
                let mut pte = Pte { frame: 0x1000, bits: 0x20 }; // read-only
                mark_cow(&mut pte, &mut refs);
                assert_result!(!pte.cow(), "read-only pages are shared, not COW-marked");
                assert_result!(!pte.writable(), "still read-only");
                assert_eq_result!(refs.count(0x1000), 1, "no refcount bump for shared pages");
                Ok(())
            }),
        },
        // ---- handle_cow ----
        Test {
            name: "cow_handle_non_cow_returns_none",
            category: "kernel::paging",
            run: Box::new(|| {
                let mut refs = FrameRefs::new();
                let mut alloc = MockAlloc::new(0x5000);
                let mut pte = Pte { frame: 0x1000, bits: WRITABLE };
                let r = handle_cow(&mut pte, &mut refs, &mut alloc);
                assert_result!(r.is_none(), "fault on a non-COW page is not ours");
                assert_eq_result!(pte.frame, 0x1000, "untouched");
                Ok(())
            }),
        },
        Test {
            name: "cow_handle_single_ref_in_place",
            category: "kernel::paging",
            run: Box::new(|| {
                let mut refs = FrameRefs::new();
                let mut alloc = MockAlloc::new(0x5000);
                let mut pte = Pte { frame: 0x1000, bits: COW_BIT };
                let r = handle_cow(&mut pte, &mut refs, &mut alloc);
                assert_eq_result!(r, Some(true));
                assert_eq_result!(pte.frame, 0x1000, "sole owner keeps the frame");
                assert_result!(pte.writable(), "writable again");
                assert_result!(!pte.cow(), "COW bit cleared");
                assert_eq_result!(refs.count(0x1000), 1, "exclusive frame keeps default count");
                Ok(())
            }),
        },
        Test {
            name: "cow_handle_shared_ref_copies",
            category: "kernel::paging",
            run: Box::new(|| {
                let mut refs = FrameRefs::new();
                let mut alloc = MockAlloc::new(0x5000);
                let mut pte = Pte { frame: 0x1000, bits: COW_BIT };
                refs.increment(0x1000); // second share
                refs.increment(0x1000); // third share -> count 3
                let r = handle_cow(&mut pte, &mut refs, &mut alloc);
                assert_eq_result!(r, Some(true));
                assert_eq_result!(pte.frame, 0x5000, "shared frame gets a fresh copy");
                assert_result!(pte.writable(), "copy is writable");
                assert_result!(!pte.cow(), "COW bit cleared");
                // Two increments (count 2) minus the COW-resolution decrement.
                assert_eq_result!(refs.count(0x1000), 1, "old frame dropped one share");
                assert_eq_result!(refs.count(0x5000), 1, "new frame is exclusively owned");
                Ok(())
            }),
        },
    ]
}
