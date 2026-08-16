# Kernel fix: `[login] failed to create window` respawn loop

Status: **draft patch for when the kernel settles** — the kernel is mid-major-
change, so NOTHING here has been applied. All line numbers reference the
checkout in `kernel/kernel/` at time of writing (Aug 8 2026); treat function
names + syscall numbers as the stable anchors.

> **Queue:** this is **K1 / K1-alt** in `session-lifecycle.md` §6 Kernel
> change queue — the rewrite's consolidated landing checklist (with the
> exact harness condition each option must satisfy).

## Root cause (evidence)

The loop is a **create/map_buffer asymmetry**, not an allocation crash:

- `sys_gui_create_window` (`syscalls/mod.rs:4656`) allocates contiguous
  physical memory. On `None` (memory pressure) it **silently falls back** to a
  kernel-heap copy: `win.content = Some(vec![0; content_len])` (line 4689)
  and still returns a valid window handle. **A window is always created.**
- `sys_gui_map_buffer` (`syscalls/mod.rs:4709`) then does
  `let phys_addr = match win.phys_addr { Some(p) => p, None => return 0 };`
  (lines 4715-4717). For a fallback window `phys_addr` is `None`, so it
  **returns 0 (NULL)**.
- `libsarga::gui::Window::create` (`libsarga/src/gui.rs:435-437`) treats a NULL
  map result as failure: `if buf_ptr.is_null() { return Err(5); }`.
- `login-manager` (`login-manager/src/main.rs:28`) prints
  `[login] failed to create window`, returns 0, and init respawns it (clean
  exit → crash counter reset → **unbounded**; see session-lifecycle.md).

The infuriating part: the fallback window is **fully functional** — `flush`
already copies user pixels into `content` (`syscalls/mod.rs:4774-4784`) and
the compositor already renders content-backed windows
(`gui/window.rs:86-95`). Only the user-side *mapping* is missing. The two
patches below each close the asymmetry; **Option 1 is recommended**.

---

## Patch Option 1 (recommended) — map the heap-content fallback

Make `sys_gui_map_buffer` promote a fallback window to a real shared buffer
instead of returning 0. This makes `Window::create` succeed under memory
pressure, so the respawn loop never triggers. Only if the promotion
allocation ALSO fails does it return 0 (NULL) — now a genuinely rare
double-failure.

`kernel/kernel/src/syscalls/mod.rs` — replace `sys_gui_map_buffer`:

```diff
 pub(crate) fn sys_gui_map_buffer(handle: u64) -> u64 {
     use crate::gui::COMPOSITOR;
-    let comp = COMPOSITOR.lock();
+    let mut comp = COMPOSITOR.lock();
     if handle as usize >= comp.windows.len() { return 0; }
 
-    let win = &comp.windows[handle as usize];
+    let win = &mut comp.windows[handle as usize];
+    if win.phys_addr.is_none() {
+        // Heap-content fallback window (created under memory pressure):
+        // promote it to a real shared buffer so userspace gets a writable
+        // pointer. The compositor already renders content-backed windows
+        // (gui/window.rs draw), so this only fixes the missing user mapping
+        // — the [login] failed to create window loop. If this allocation
+        // also fails, fall through and return 0 (NULL) as before.
+        let content_w = win.width.saturating_sub(2);
+        let content_h = win.height.saturating_sub(22);
+        let size_bytes = content_w * content_h * 4;
+        use crate::memory::buddy::BUDDY_ALLOCATOR;
+        let mut order = 0;
+        while (4096 << order) < size_bytes && order < crate::memory::buddy::MAX_ORDER {
+            order += 1;
+        }
+        if let Some(pa) = BUDDY_ALLOCATOR.lock().allocate_contiguous(order) {
+            let offset = crate::memory::physical_memory_offset();
+            let k_ptr = (offset + pa.as_u64()) as *mut u8;
+            unsafe { core::ptr::write_bytes(k_ptr, 0, (4096 << order) as usize); }
+            win.phys_addr = Some(pa.as_u64());
+            win.content = None; // drop the heap copy; flush's zero-copy path applies
+        }
+    }
+
     let phys_addr = match win.phys_addr {
         Some(p) => p,
         None => return 0,
     };
 
     let content_w = win.width.saturating_sub(2);
     let content_h = win.height.saturating_sub(22);
     let size_bytes = content_w * content_h * 4;
     let pages_needed = size_bytes.div_ceil(4096);
 
     let process_lock = CURRENT_PROCESS.lock();
     let process = match *process_lock { Some(ref p) => p, None => return 0 };
 
     // Find a virtual address to map to
     static NEXT_GUI_MAP_ADDR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0x5000_0000_0000);
     let v_addr = NEXT_GUI_MAP_ADDR.fetch_add(pages_needed as u64 * 4096, core::sync::atomic::Ordering::SeqCst);
 
     use crate::memory::buddy::BuddyFrameAllocator;
     let mut frame_allocator = BuddyFrameAllocator;
     let mut mapper = if let Some(m) = unsafe { process.address_space.mapper() } { m } else { return 0; };
 
     for i in 0..pages_needed {
         let page = Page::<Size4KiB>::containing_address(x86_64::VirtAddr::new(v_addr + i as u64 * 4096));
         let frame = x86_64::structures::paging::PhysFrame::containing_address(x86_64::PhysAddr::new(phys_addr + i as u64 * 4096));
         let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
 
         unsafe {
             if let Ok(t) = mapper.map_to(page, frame, flags, &mut frame_allocator) {
                 t.flush();
             }
         }
     }
 
     process.add_vma(crate::task::process::Vma {
         start: v_addr,
         end: v_addr + pages_needed as u64 * 4096,
         flags: PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
         _name: "gui_buffer",
         file_handle: None,
         file_offset: 0,
         is_shared: false,
         shm_id: None,
     });
 
     v_addr
 }
```

Notes:
- The borrow changes (`comp` and `win` to `&mut`) are required to set
  `phys_addr`; the mapping loop that follows touches neither.
- `win.content = None` is race-free: `COMPOSITOR.lock()` is held for the
  whole function, and `flush`/`render` lock the same mutex — no concurrent
  render can read the dropped `Box`. It is dropped only on promotion success,
  so a failed promotion leaves the renderable content fallback intact.
- `content_w/content_h` are recomputed after the promotion — harmless
  duplication, matches the existing function. Note `win.width-2` == the
  user's `width` (create_window makes the window width+2), so the mapped
  region exactly matches libsarga's buffer slice — no size mismatch.
- The order computation duplicates `create_window`'s; consider extracting a
  small `fn order_for_size(size_bytes: usize) -> u8` helper if the rewrite
  wants to share it.
- No userspace change: `libsarga Window::create` gets a valid non-NULL
  pointer and proceeds. `flush` uses the zero-copy `phys_addr` path.

---

## Patch Option 2 (alternative) — fail `SYS_GUI_CREATE_WINDOW` with `-ENOMEM`

If the design prefers honest failure over resilience: remove the silent heap
fallback and return `-ENOMEM` so `Window::create` yields `Err(12)` instead of
the misleading `Err(5)`. This requires the userspace follow-up below to
actually stop the respawn loop; the kernel patch alone only makes the failure
real.

`kernel/src/syscalls/mod.rs` — `sys_gui_create_window`: replace the
heap-content fallback with an honest `-ENOMEM` return so `Window::create`
yields `Err(12)` instead of the misleading `Err(5)`. The hunks below are
difflib-generated from the live kernel source and verified with
git apply --check (same treatment as Option 2b, Aug 12, 2026); the `@@`
line numbers drift with the rewrite — the hunk context is the stable anchor.

```diff
    --- a/kernel/src/syscalls/mod.rs
    +++ b/kernel/src/syscalls/mod.rs
    @@ -4686,7 +4686,12 @@
             let k_ptr = (offset + pa.as_u64()) as *mut u8;
             unsafe { core::ptr::write_bytes(k_ptr, 0, (4096 << order) as usize); }
         } else {
    -        win.content = Some(alloc::vec![0; content_len].into_boxed_slice());
    +        // No shared-memory space and no silent fallback: fail loudly with
    +        // -ENOMEM so userspace (libsarga Window::create -> Err(12)) can
    +        // report a real error. (errno::Errno::ENOMEM = -12; the syscall
    +        // return is read back as i64 by libsarga's syscall3, and
    +        // Window::create checks `id < 0` -> Err(-id).)
    +        return errno::Errno::ENOMEM as u64;
         }
         
         comp.add_window(win);
```

Notes:
- `errno::Errno::ENOMEM = -12` exists (`syscalls/errno.rs:17`); the same
  `as u64` idiom is already used by `sys_gui_flush` for `EBADF`/`ENOSYS`.
- `content_len` becomes dead in this branch; keep the `let` (it feeds
  `size_bytes` above) or let the rewrite tidy it.
- **Userspace follow-up MANDATORY for any observable effect** (out of scope
  for this kernel patch): login-manager's current `Err(_) => { ... }` arm
  **swallows the error value**, so `Err(5)` → `Err(12)` alone changes
  nothing on screen or in the log — the loop still respawns identically. The
  exact diff is drafted in the **Patch Option 2b** section below, ready to
  apply the moment the kernel lands Option 2. Without that edit, Option 2 is
  a no-op today.

---

## Patch Option 2b (userspace companion) — login-manager reports + bounds

Status: **draft patch, NOT applied** — applies ONLY together with kernel
Option 2 above (the two kernel options are mutually exclusive; Option 1
needs NO userspace change and Option 2b must NOT be applied against
Option 1). Line numbers reference `login-manager/src/main.rs` at time of
writing (Aug 10 2026); the hunk context is the stable anchor. It mirrors
vahid's exit-code discipline (`EXIT_DEVICE_SCAN_FAILED = 1`, see
session-lifecycle.md): a clean exit 0 resets init's crash counter
(UNBOUNDED respawn); a NON-ZERO exit accumulates toward `MAX_RESPAWNS` so
the window-failure loop becomes bounded (`[init] giving up on
login-manager`) instead of respawning forever while `ade` never runs.

```diff
    --- a/login-manager/src/main.rs
    +++ b/login-manager/src/main.rs
    @@ -10,6 +10,11 @@
     const SHADOW_PATH: &str = "/etc/shadow";
    +/// Non-zero exit code for a fatal window-creation failure (kernel Option 2):
    +/// init treats a clean exit 0 as "ran its course" (crash counter reset ->
    +/// unbounded respawn); a non-zero exit accumulates toward MAX_RESPAWNS so
    +/// the loop is bounded.
    +const EXIT_WINDOW_CREATE_FAILED: i32 = 1;
     
     fn verify_password(username: &str, password: &str) -> bool {
         let data = match libsarga::fs::read_to_string(SHADOW_PATH) {
             Ok(d) => d.into_bytes(),
             Err(_) => return false,
    @@ -79,7 +84,12 @@
                     alloc::format!("[login] failed to create window: errno {}\n", e)
                 };
                 io::print_str(&msg);
    -            return 0;
    +            // Non-zero exit (EXIT_WINDOW_CREATE_FAILED): a clean exit 0
    +            // resets init's crash counter (unbounded respawn); non-zero
    +            // accumulates toward MAX_RESPAWNS, so the window-failure loop
    +            // is bounded and init eventually prints `giving up on
    +            // login-manager` instead of looping forever.
    +            return EXIT_WINDOW_CREATE_FAILED;
             }
         };
     
```
```

Notes:
- `Window::create` returns `Result<Self, i64>` (libsarga/src/gui.rs:420) and
  `libsarga::errno::ENOMEM` is `pub const ENOMEM: i32 = 12`
  (libsarga/src/errno.rs:146), so `e == libsarga::errno::ENOMEM as i64` is
  the exact Err(12) test; `pub mod errno` is already exported (lib.rs:9).
- The else-arm uses `alloc::format!` — legal here: `#![no_std]` + `extern
  crate alloc` (main.rs:1-3) and `String` is already imported.
- `EXIT_WINDOW_CREATE_FAILED = 1` mirrors vahid's `EXIT_DEVICE_SCAN_FAILED`
  so both init services with a bounded fatal path use the same convention.
- ALL create/map failures now exit non-zero — even non-ENOMEM ones — because
  any window we cannot get is fatal to the GUI session; only the
  auth-failure loop below (which never exits, by design) is exempt.
- Verification (when the kernel lands): with Option 2 + 2b, a forced-low-
  memory boot must print `[login] window create failed: Out of memory` then
  `[init] giving up on login-manager` after `MAX_RESPAWNS` — asserted by
  the give-up harness (`qemu_giveup_boot.exp`'s unbounded-absence grep
  becomes a positive `giving up on .*login-manager` requirement; see
  session-lifecycle.md §give-up gate).
- The kernel-side TAP spec for the `-ENOMEM` return is
  `kernel-gui-selftest-spec-option2.md` (`gui::option2_*` tests, mutually
  exclusive with the Option 1 selftest spec's `gui::option1_*` family).

---

## Why Option 1 is recommended

- The content fallback already exists and is rendered correctly
  (`gui/window.rs:86-95`); `map_buffer` returning 0 is the **lone
  inconsistency**. Option 1 completes the existing design instead of removing
  it.
- It turns memory pressure into a non-event for the GUI: the login window
  appears and `[login] window created` prints — the qemu_gui_gate.exp PASS.
- Option 2's `-ENOMEM` requires a userspace change (login-manager exit
  non-zero) to stop the loop; without it, login-manager just reports better
  while still respawning forever.
- Worst case for Option 1 (promotion allocation fails too) degrades to
  today's behavior — no regression.

If the kernel rewrite prefers fail-loud semantics, Option 2 + the login-manager
change is the honest alternative; the two are mutually exclusive at the
`create_window` fallback level. The one legitimate case for choosing Option 2
instead: if memory pressure is **persistent** (not transient), Option 1's
promotion allocation fails too and the GUI reports "success" on a blank
window — misleading. Option 1 assumes transient pressure (see the evidence
probe below).

---

## Evidence probe — boot-time memory-pressure marker (Aug 10, 2026)

To settle **persistent vs transient OOM** with per-boot evidence instead of
assumption, login-manager now prints the kernel buddy allocator's **live
free-page count** on serial right before `Window::create`:

```
[login] mem free=12345 pages
```

The number comes from ctlFS `/ctl/sys/mem/free`
(`kernel/src/vfs/ctlfs.rs:201-204` → `BUDDY_ALLOCATOR.count_free_pages()`,
`kernel/src/memory/buddy.rs:148-158`) — the same node the kernel's own
System Monitor terminal reads (`kernel/src/gui/terminal.rs:169-176`). It is
pure kernel memory state, read through the standard VFS read syscall, so no
kernel change was needed to expose it. Because login-manager is init's
`respawn: true` service, a failing boot prints the marker **on every
respawn**, so the serial capture also shows whether free memory recovers
between attempts.

**How to read the evidence:**

| Marker at create time | Verdict | Kernel fix to land |
|---|---|---|
| free near zero (e.g. `< 2k pages` ≈ `< 8 MB`) | **persistent OOM** — the contiguous allocation genuinely cannot be satisfied; Option 1's promotion will also fail | **Option 2 + 2b** (honest `-ENOMEM`, bounded respawn) |
| plenty free (e.g. `> 100k pages`) | **transient / fragmentation** — pressure cleared or the block is only fragmented; promotion can succeed | **Option 1** (map the heap-content fallback) |
| marker missing (`mem free=unavailable`) | ctlFS read failed (kernel VFS regression) | neither — fix the read path first |

**Where it lands:** the GUI gate (`qemu_gui_gate.exp`) captures the marker in
its state-tracked verdict loop and reports `buddy free=N pages` in the FAIL
arm (respawn loop) and in the final PASS line; the ci.yml Verify step greps
`[login] mem free=` as an explicit positive assertion, so the evidence is
collected on **every** kernel build, not just ad-hoc boots. The source/exp/CI
contract is pinned host-side by `tests/test_gui_gate_mem_marker.py`.

The give-up harness (`qemu_giveup_boot.exp`) turns this table into a
CI-asserted contract on the forced-failure boot: login-manager's
window-failure exit-0 respawn loop re-prints the marker on EVERY respawn,
so the harness captures the full per-respawn free-page series live into a
list (`lappend mem_readings`) and classifies first-vs-last - free
recovering = transient -> Option 1; not recovering = persistent -> Option
2. The ci.yml Verify steps assert the capture-and-classification ran
(healthy boot: NOTE or PASS; fail-vahid boot: `PASS: mem series
captured`). The specific transient/persistent verdict is NOT hard-asserted
- a genuine OOM boot must be allowed to print persistent without failing
CI - so the verdict line stays human-readable evidence on every kernel
build instead of an ad-hoc log read. Pinned host-side by `test_giveup_gate.py`.

The kernel-side forcing mechanism that makes that pressure **deterministic**
— a test-only `SKYOS_DRAIN_BUDDY=<order>` boot flag that drains the buddy at
boot and holds the blocks, so login-manager's real 800x600 create runs under
near-zero free pages and prints `[login] failed to create window` — is
specced in `kernel-gui-selftest-spec.md` (**Second user-visible path —
forced-failure boot leg**): the same `drain_order` helper the selftests use,
one level up the stack, bridging the synthetic drain and the QEMU-gate
serial evidence.

The console getty (`/bin/login`, login/src/main.rs) prints the same
`[login] mem free=N pages` ctlFS read at startup (once per boot - the getty
never exits), so the shell-interaction harness (`qemu_shell_test.exp`) and
its local ps1 mirror collect the same per-boot OOM snapshot; the integration
job's Verify step greps it too. Pinned by `test_login_flow.py` (source) and
`test_vahid_contract.py` (harness).

This replaces the doc's earlier "assumes transient pressure" open question
with a measurement: after two or three CI gate boots, the marker's magnitude
in the FAIL path decides the recommendation. (Until the in-flux kernel boots
an ISO with this login-manager, the marker cannot print — the same
kernel-gated status as the rest of this doc.)

---

## Verification plan (when the kernel settles)

1. **Normal boot**: `qemu_gui_gate.exp` must print
   `GUI + device-manager reachability gate: PASS` (greps `[vahid] ready` +
   `[login] window created`). No `[login] failed to create window` in the
   serial log.
2. **Option 1 code path**: a kernel selftest (or a forced-low-memory boot) can
   create a window, confirm `phys_addr` is `None` (content fallback), call
   `sys_gui_map_buffer`, and assert the return is non-zero and the window
   renders. Unit-level: the promotion branch is pure logic over the Window
   struct — directly testable. Full implementable plan: see
   `kernel-gui-selftest-spec.md` (TAP framework wiring, drain-to-force-
   fallback helper, process-seeding prerequisite for the map assertion, and
   the render-to-backbuffer pixel check).
3. **Option 2**: assert login-manager prints the real error and — with the
   userspace follow-up — `[init] giving up on login-manager` appears (bounded).
4. **No userspace change for Option 1**: `libsarga gui.rs` and login-manager
   stay as-is; existing pins (`test_login_flow.py` source contracts) unchanged.

## Assumptions / open questions

- `allocate_contiguous` in `map_buffer` may fail for the same memory-pressure
  reason as in `create_window`; the doc assumes that is rare/transient (a
  window-sized contiguous block freed between create and map). The
  boot-time memory-pressure marker (see Evidence probe above) measures
  whether pressure is in fact transient: free pages at create time on the
  FAIL path, plus the per-respawn series.
- Whether the rewrite keeps the content fallback at all is a design choice;
  Option 1 assumes it stays (it is already the render path).
- The exact syscall-return sign convention (u64 in the kernel → i64 in
  libsarga) is assumed to survive the kernel rewrite — it is what makes both
  the `id < 0` check and `ENOMEM as u64` work today.
