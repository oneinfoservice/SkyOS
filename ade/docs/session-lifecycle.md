# ADE Session Lifecycle — Logout Flow Trace

Status: trace of the **current** code (Aug 7, 2026). Line refs are "as of" this
date and will drift; function names are the stable anchors.

> **Phase A — console getty: COMPLETE (Aug 7, 2026).** init's service table
> now spawns a console getty (`/bin/login` on the inherited console fds,
> `respawn: true`), so the serial `login:` prompt is deterministic and the
> console path to a root/skyos session works. The shell-prompt assumption in
> the CI harnesses is corrected (sash's default prompt is `sash[/]> `, not
> `$ `/`# `), and `login` now execs the shell with argv[0] (empty argv made
> argv scans misbehave — same fix init's spawn documents). The GUI path
> (login-manager) remains UNVERIFIED (Gap 1), which is why the console getty
> was chosen first. See the status block in §4.

This document traces the full logout loop end to end —

```
init ──spawn──▶ login-manager ──execve──▶ ade ──exit(0)──▶ init ──respawn──▶ login-manager (fresh login)
                                                    ▲
                              Ctrl+Alt+Backspace with no windows
```

— and lists exactly what is missing before that loop can be exercised
automatically in CI. It was written while the kernel is in major change; the
kernel-side claims are flagged as **UNVERIFIED** and are not load-bearing for
the plan.

> **Kernel rewrite?** The consolidated landing checklist for every
> kernel-gated item in this doc lives in **§6 Kernel change queue** —
> one place, one list, each entry gated by an existing harness.

---

## 1. The flow, hop by hop

### Hop 1 — boot: kernel → `/bin/init`

The kernel's boot state machine locates and execs userspace init
(`kernel/src/boot/state.rs`): search paths `/bin/init`, `/init`, `/sbin/init`,
argv `["/bin/init"]`. Userspace init is `init/` in this repo.

### Hop 2 — init spawns the session services

`init/src/main.rs` hardcodes four services (lines 78–109), all `respawn: true`:

| name | exec | role | respawn |
|---|---|---|---|
| vahid | `/bin/vahid` | device manager (PCI scan + `/dev` nodes) | true |
| login-manager | `/bin/login-manager` | GUI session | true |
| svc | `/bin/svc` | service daemon | true |
| getty | `/bin/login` | console getty (Phase A) | true |

Note: `init` does **not** read `/etc/init.toml` — the initrd's
`INIT_TOML_CONTENT` (`build_initrd.py`) duplicates the same table but is
currently inert. vahid is a **device manager**, not a display server —
window creation is kernel-served (see §1 and Gap 1).

### Hop 3 — login-manager authenticates and becomes ade

`login-manager/src/main.rs`:
- Creates a GUI window (`Window::create`, `:60`). On success it prints
  `[login] window created` (added Aug 8, 2026 — the GUI reachability gate's
  PASS marker; there was previously no success marker at all). On failure it
  prints `[login] failed to create window` and exits **0** (`:66-67`) —
  which init treats as a clean exit and respawns (see Hop 6). The mechanism
  behind this failure is traced in §1, “The `[login] failed to create
  window` respawn loop — kernel-side trace”: it is a kernel-syscall
  mismatch (create succeeds silently, map returns NULL), not a missing
  display server — there is no display server in the chain.
- On Enter: `verify_password(user, pass)` (line 54) reads `/etc/shadow`
  (`SHADOW_PATH`, line 9) and PBKDF2-verifies via
  `libsarga::hash::verify_password` (`libsarga/src/hash.rs:58`).
- On success: `process::execve("/bin/ade", ["/bin/ade"], [])` (line 55).
  **execve replaces the process image: login-manager's pid becomes ade.**

### Hop 4 — ade session loop

`ade/src/main.rs`:
- `while !desktop.session.is_ending()` (line ~77) is the session loop.
- On entry: `[ade] session established`, `[ade] desktop running`.
- Terminal windows route keys to the pty (`desktop.focused_has_pty()`), so
  Backspace inside a terminal reaches the shell, not the session gate.

### Hop 5 — session-end triggers (single keymap path, exit code 0)

**RESOLVED (Aug 7, 2026): the any-Backspace gate is gone.** The old
main.rs keyboard gate (any Backspace/DEL outside a pty → `request_end()`)
was the documented footgun: Backspace could never edit text in a plain
(non-terminal) window, and a true Ctrl+Alt+Backspace chord was undetectable
(no modifiers in the one-byte input stream). The gate was removed entirely;
`main.rs` no longer intercepts keys — everything goes to
`desktop.handle_event`, and the loop breaks when `session.is_ending()`.

**The two session-end paths — `desktop.rs`:**

- **Ctrl+Alt+Backspace** — the keymap chord (Phase C, Aug 7, 2026). It
  resolves to `KeyAction::Quit` (a desktop grab, so it works even while a
  terminal is focused); with **no windows open** it calls
  `session.request_end()` and returns. With any window open it is a
  deliberate no-op. The Alt bit cannot arrive in the one-byte stream yet,
  so this path is kernel-gated (synthetic-testable only).
- **Esc on an empty desktop** — the byte-deliverable path (Aug 9, 2026).
  Esc arrives as Unicode 0x1B, the one distinct control byte the stream
  carries; the a11y Esc arm (`handle_a11y_key`) ends the session when
  nothing is open — no a11y ring, windows, switcher, or overlay. This is
  what the QEMU GUI-login harness uses (`sendkey esc`).
- **Ctrl+Q and plain 'q' are deliberately unbound** — the old gates are
  gone; neither can end a session.

The chord calls `request_end()` — **not** `process::exit` — so the main
loop unwinds, prints `[ade] session ended`, and `user_main` returns
`session.exit_code()` = `EXIT_LOGOUT = 0`
(`ade/src/service/session.rs:132,136,143`). Because login-manager exec'd
ade, **ade's exit code is login-manager's exit code.** Backspace still edits
text in plain windows and reaches the shell inside a terminal — pinned by
`testing/input.rs::test_session_end_gate` (Backspace never ends a session;
Ctrl+Q / plain 'q' never end a session; the chord with a window open never
ends; the chord on an empty desktop does; Esc on an empty desktop does;
Esc with a window/menu/panel/ring open never does — a ring is dismissed
first and the NEXT Esc ends; near-miss chords — Ctrl+Backspace,
Alt+Backspace, Ctrl+Alt+Shift+Backspace, Ctrl+Alt+Q — never do).

### Hop 6 — init observes the exit and respawns

`init/src/main.rs` blocks in `waitpid(-1, 0)` (line 103). On the
login-manager pid's exit:

```rust
svc.pid = None;
if status == 0 { svc.crashes = 0; }          // clean logout resets the counter
if svc.respawn {
    svc.crashes += 1;
    if svc.crashes > MAX_RESPAWNS { give up } // 5
    else { nanosleep(500ms); spawn(); }       // fresh login-manager
}
```

So a clean logout (exit 0) → 500 ms pause → a brand-new GUI login prompt.
That is the complete logout loop, and it already works *as designed* — the
serial markers exist to assert on it:
`[ade] session ending via keyboard`, `[ade] session ended`,
`[init] service login-manager exited`, `[init] starting service: login-manager`.

**Respawn accounting contract (the clean / non-zero split).** The block
above is the entire contract init has for every service, and its two
halves must stay in this exact order — the reset **before** the
increment:

1. **Clean exit (`status == 0`)** — `svc.crashes = 0` resets first, so a
   cleanly exiting service always lands at `crashes == 1` (reset, then
   `+= 1`) and give-up can never fire for it. `EXIT_LOGOUT = 0` is what
   makes logout an unbounded respawn loop: ade exits 0 → init sees a
   clean exit → respawns login-manager forever.
2. **Non-zero exit** — accumulates `crashes 1..6`, respawning on 1..5 and
   giving up on the 6th (strictly greater than `MAX_RESPAWNS = 5`):
   exactly 5 respawns, then `svc.respawn = false` and no further counting
   (the increment is guarded by `if svc.respawn`).

init's view of an exit is **binary** (`status == 0` vs everything else);
it never inspects session.rs's finer `exit_class` (128 + signal), so a
signal-killed service counts toward `MAX_RESPAWNS` exactly like any other
non-zero exit.

The authoritative, host-verifiable spec for this state machine is
**`RespawnAccounting` in `tests/test_vahid_contract.py`** — a faithful
port of the waitpid-loop accounting (same order, same conditions) whose
behavioral tests pin the split end to end: clean-exit reset and the
unbounded login-manager loop (`test_init_resets_crashes_on_clean_exit`,
`test_login_manager_clean_exit_loop_is_unbounded`), non-zero accumulation
to give-up (`test_init_accumulates_crashes_and_gives_up`,
`test_vahid_nonzero_exit_is_bounded`), reset-then-increment ordering
(`test_init_resets_then_increments_order`), and a mixed streak
(`test_mixed_streak_clean_exit_resets`). The port's `MAX_RESPAWNS = 5` is
cross-pinned to the source constant by
`test_port_matches_source_max_respawns`. The session side of the contract
(ade's `EXIT_LOGOUT = 0` and the exit-code pass-through to init) is
separately pinned by `TestInitRespawnContract` in
`tests/test_login_flow.py` — the two files hold the two halves, so either
side changing without the other fails the host-tests CI job.

**The kernel gate: init's waitpid reaps no exits (Aug 10, 2026).** All of
the accounting above presupposes that init's `process::waitpid(-1, 0)`
(init/src/main.rs) actually returns a child exit. On the current kernel it
does not. Live-boot evidence (release/skyos-selftest-run.iso, QEMU
`-serial file:` capture): services die — `svc` exits 1 (Usage path) and
`login-manager` SEGVs at `addr=0x0` (`[SIGSEGV] pid=105`, `[KILL3] mark
exited`) — yet the serial log shows **zero** `[init] service X exited`
markers, so init never reaps, never respawns, and never prints
`[init] giving up on X`. Kernel-side, `sys_wait4`
(kernel/src/syscalls/mod.rs:2879) and `sys_exit` (mod.rs:1855, which sets
`exit_code`) both exist, so the likely break is in the exit→reap chain —
`sys_wait4` scans the parent's `children` list for an `exit_code`-bearing
child, but no reap ever fires, so the exact failure point (child
reaped/lost before the scan, or the exit never landing in a visible
`exit_code`) is unconfirmed while the kernel is mid-major-change; it is
not a missing syscall number. Until a working reap lands, the boundedness
markers are
**unprintable**, which is why the golden-trace test
(tests/test_init_golden_trace.py) covers the accounting host-side.

**`tests/qemu_giveup_boot.exp` — the serial-log proof, kernel-gated.** This
harness (wired into the CI gate job) boots the ISO and asserts the two
boundedness claims against real init markers, but **probes exit delivery
first**: only when `[init] service X exited` is observed (waitpid live) do
the assertions become hard requirements; otherwise it reports
`KERNEL-GATED` (exit 0) and the ci.yml Verify step defers. Once live, it
requires `[init] giving up on svc` — svc is the one service that exits
non-zero on **every** boot (init spawns it without argv, so the Usage path
at svc/src/main.rs:121-122 always returns 1), proving the bounded non-zero
half — and requires `[init] giving up on login-manager` to stay ABSENT
(the exit-0 window-failure loop), proving the unbounded clean-exit half.
Note vahid's own give-up is not the bounded marker: vahid's fatal path
needs a `/dev` node creation failure that no stock boot produces — svc is
the real instance, and vahid's boundedness stays pinned host-side in
`RespawnAccounting`. Host pins: tests/test_giveup_gate.py.

### The serial `login:` prompt — provenance and exact format

Who prints it: **`/bin/login`, spawned by init's `getty` service with no
argv** (`init/src/main.rs`: `Service { name: "getty", exec: "/bin/login",
respawn: true }`). init forks and `execve`s it on the inherited console
fds, so fd 0 = serial input, fd 1 = serial output. The kernel never prints
it: `grep -rn 'login' --include='*.rs'` in the kernel repo matches **zero**
source lines (only binary artifacts). `login-manager` is GUI-only and its
serial output is limited to `[login] failed to create window` and
`[login] execve failed, continuing` — neither contains the prompt.

Exact byte strings `/bin/login` writes (`login/src/main.rs`, line refs as
of Aug 7, 2026) — all via `io::print_str`; no trailing newline unless
shown:

| Output | Line | Notes |
|---|---|---|
| `login: ` | 102 | prompt; trailing space, no newline; only when `argc <= 1` (the getty path) |
| `login: unknown user\n` | 122 | unknown username → re-prompt (loop, no respawn) → fresh `login: ` |
| `Password: ` | 132 | trailing space, no newline |
| `\nInvalid password encoding\n` | 143 | non-UTF8 password bytes |
| `\nLogin incorrect\n` | 156 | wrong password → re-prompt (loop, no respawn) → fresh `login: ` |
| `\n` | 165 | only after a successful verify, before `execve(shell)` |

`read_line(0)` (lines 17–31) terminates on `\n` **or** `\r`, so the
harnesses' `send "root\r"` works. Success `execve`s the shell from
`/etc/passwd` (`root:...:/bin/sash`), so the next serial marker is sash's
prompt.

**Echo trace (Aug 8, 2026) — where typed input does/doesn't echo:** the
kernel serial driver is **TX-only** (`serial::getc`/`is_received` have
zero callers kernel-wide; the IDT has no COM1 IRQ handler), `/dev/tty0`
read pops `tty::TTY_INPUT` (PS/2-keyboard-fed) and never echoes, and
`login::read_line` reads bytes without writing them back — so **the guest
never echoes the password on the wire**. The "echo" visible in CI logs is
the **expect harness logging its own `send`** (`log_user` defaults on;
the QEMU monitor also echoes literal `sendkey s k y o s` commands). Kernel
termios is a stub: `TCGETS` advertises `c_lflag = ICANON|ECHO` but
`TCSETS` is a no-op returning 0, so there is nothing to suppress kernel-
side today. Two mitigations landed Aug 8, 2026: (1) `login` now wraps the
password read in `read_password` — `TCSETS` clear-ECHO / restore (the
classic getty pattern; forward-compatible with a real termios
implementation when the kernel lands one); (2) all four harnesses
(`qemu_shell_test.exp`, `qemu_gui_login.exp`, `qemu_ade_selftest.exp`,
`test_login.ps1`) send the password under `log_user 0` so the CI log
stops capturing the credential.

Why the expect patterns are evidence-backed (each maps to a producing line
above):

- `"login:"` (both exp scripts' boot gate) → `login.rs:233`; expect's
default glob matches it as a substring of `login: `.
- `"Password:"` → `login.rs:267`.
- `{sash\[}` → the exec'd `/bin/sash` prompt (`sash[/]> ` form); the
backslash escapes expect's glob `[...]` class.
- `"Login incorrect"` → `login.rs:292`. With the re-prompt loop (Aug 7,
2026) this no longer implies the getty exited — a mistyped password
re-prompts in place. The verdict split (Aug 8, 2026):
`qemu_ade_selftest.exp` now **asserts** the re-prompt — it mistypes the
password once, treats `Login incorrect` as a PASS, then requires a fresh
`login: ` with **no** intervening `[init] starting service: getty` marker
(a `Login incorrect` at the *real* login is still a FAIL arm);
`qemu_shell_test.exp` still times out at `{sash\[}`.
- `"SARGA OS PANIC"` (both scripts' panic arms) → expected to be the
**kernel** panic banner (as assumed by the exp scripts' failure arms —
kernel-side, **UNVERIFIED** while the kernel is in major change). What is
verified here is only the negative half: no `login.rs` line produces it.

Caveat: a failed login no longer exits — `login` re-prompts in place
(`login/src/main.rs` loops on bad credentials and on bare-Enter at the
username prompt), so a serial log can legally contain multiple `login: ` /
`Login incorrect` sequences without an intervening respawn. `read_line`
returns `Ok(None)` on real serial EOF (zero bytes), which is the only
input-side reason the getty exits (plus read errors and session end after
the shell exits). `MAX_RESPAWNS = 5` therefore cannot be exhausted by
mistypes — not even five bare Enters — and still bounds real crash loops.
The harnesses treat the *first* `login:` as the boot gate and assert the
*first* `Password:` / shell prompt after their own input, which stays
correct.

**Attempt cap (Aug 8, 2026):** login now counts failed attempts
(`MAX_FAILED_ATTEMPTS = 10`) across all three failure paths (unknown user,
invalid password encoding, wrong password) and, on the cap, prints a
"Too many failed attempts" notice and sleeps `BACKOFF_NS = 30 s` before
re-prompting — it still **never exits**, so the MAX_RESPAWNS semantics
above are unchanged. The cap only throttles the PBKDF2 verify (10k
iterations per attempt) so a brute-forcer or a stuck terminal cannot
hammer it at full speed. The harnesses' fixed `root/skyos` sequence never
reaches the cap, so no harness verdict changes.

**GUI attempt-cap evidence (Aug 12, 2026):** login-manager's password
field uses the identical throttle, with the full contract mirroring the
console getty above and pinned by
`tests/test_login_flow.py::TestGuiAttemptCapContract` (constants, call
topology, disarm clear, no-exit inventory) plus the behavioral state
machine (`TestNoteFailedAttemptStateMachine`):

- **Cap 10** — `MAX_FAILED_ATTEMPTS = 10` (`login-manager/src/main.rs:10`).
  Every bad login calls `note_failed_attempt` (`:34-39`): it increments
the counter and, on reaching the cap, sets the window message
`Too many failed attempts - pausing 30s` (`:37`) and announces it on
serial (`:38`). Below the cap only the bad-creds announce
(`[login] invalid credentials - re-prompting`, `:123`) fires.
- **30 s post-flush backoff** — `BACKOFF_NS = 30 s` (`:12`). The sleep
runs in the main loop AFTER `win.flush()` (`:287`) so the pause message
stays visible for the whole pause; only a successful `nanosleep` disarms
the counter (`:296-299`) — EINTR keeps both the counter and the message
armed — and the successful disarm also clears the pause message
(`:298`), so the next attempt shows the plain `Invalid username or
password` (`:116`) instead of the stale cap message.
- **Empty-username parity guard** — a stray Enter with an empty username
re-prompts WITHOUT counting (`user.is_empty()` guard at `:102-104`),
matching the console getty's bare-Enter guard, so a stuck Enter cannot
burn the brute-force budget.
- **Never-exits contract** — exactly three `return` sites: `verify_password`
(shadow read failure `:17`; the final expression is the PBKDF2 result),
the startup window-create failure (`:67`), and the successful `execve`
(`:108`, which replaces the process image and never returns). No
`return 1`, no `process::exit`, and no `panic!` anywhere — an auth
failure re-prompts in the window, so the GUI path can never burn
`MAX_RESPAWNS = 5` the way the console getty used to.

The two paths' constants are pinned **in lockstep** by
`tests/test_login_flow.py::test_gui_and_console_throttle_constants_agree`
— a brute-forcer cannot switch to the weaker path, and a drift in either
side fails CI before any boot. The only remaining asymmetry is **call
topology, not throttling**: the console getty counts all three failure
paths (`login/src/main.rs`, 3 `note_failed_attempt` sites), the GUI counts
only the bad-creds branch (1 site, `:118`; an `execve` failure after a
correct login is not a failure and must not count).

**Post-cap serial trace (Aug 12, 2026):** with the console ECHO spec
applied, the 10th failure's flow is unchanged in the observable log. The
pause message `Too many failed attempts - pausing 30s` is a **direct
serial `print_str`** (`login/src/main.rs:33`, `login-manager/src/main.rs:38`)
— it is *output*, not echoed *input*, so the kernel ECHO bit (which only
suppresses/permits the tty read-path echo) cannot hide or delay it. The
wire sequence below is the **console getty's** (it reads the tty): 10×
(`login:` → username echoes → `Password:` → password suppressed by
`echo_off` → failure announce) → pause message → 30 s silence → fresh
`login:`. The **GUI path** produces a shorter trace — no tty prompts; it
draws into a window, so its serial log carries only the bad-creds announce
(`[login] invalid credentials - re-prompting`) and the pause announce. The
window shows the pause message for the whole 30 s backoff (the flush happens
before the sleep); the successful disarm then clears it, so the next attempt
displays the plain `Invalid username or password` — framebuffer-only, the
serial trace is unchanged.
Harness split (both legs drive the same constants on real hardware): the
console cap probe (`qemu_ade_selftest.exp`) drives 10 wrong passwords on
the getty and asserts the marker fires **exactly once** with a fresh
`login:` after the backoff; the GUI harness (`qemu_gui_login.exp`,
audit #17) drives the same 10 wrong passwords into login-manager's window
and asserts the marker fires **exactly once** (on the 10th), with NO
`[init] starting service: login-manager` respawn line anywhere between
the first rejection and the correct login, and an 11th wrong password
after the 30 s backoff proving the counter was reset — both legs are
pinned in `tests/test_login_flow.py`.

**GUI path verified (Aug 8, 2026):** login-manager has the same property —
verified by source trace and pinned in `tests/test_login_flow.py`
(`test_gui_login_bad_password_reprompts_not_exits`). On a bad password it
sets `error_msg = "Invalid username or password"`, clears `password_buf`,
and falls through to re-render — no exit. Its only two `return 0` sites are
the startup window-create failure (`:67`) and the successful `execve`
(`:108`, which replaces the process image and never returns). There is no
`return 1` and no `process::exit` anywhere, so auth failures can never burn
init's `MAX_RESPAWNS = 5` budget on the GUI path either — matching the
console getty's in-place re-prompt loop. (`execve` failure similarly
re-prompts: `[login] execve failed, continuing`.)

### The `[login] failed to create window` respawn loop — kernel-side trace

Traced Aug 8, 2026 against the kernel repo (which is mid-major-change; the
stable anchors are function names and syscall numbers, not line numbers).

**Headline: `add_window` cannot fail.** `Compositor::add_window`
(`kernel/src/gui/mod.rs:153`) is an unconditional push — no `Result`, no
window-count limit, no memory check, no init-state gate — and
`sys_gui_create_window` always returns a valid handle
(`(windows.len() - 1) as u64`). The loop is a **two-syscall mismatch**:
`SYS_GUI_CREATE_WINDOW` (#100) degrades silently, and `SYS_GUI_MAP_BUFFER`
(#103) is the only step that can actually fail login-manager's
`Window::create`.

The chain, with the exact failure sites:

1. `login-manager/src/main.rs:60` calls `Window::create("SARGA OS", 800, 600)`.
2. `libsarga/src/gui.rs:420` `Window::create` issues `SYS_GUI_CREATE_WINDOW`
   (#100), then `SYS_GUI_MAP_BUFFER` (#103). The **only** error return that
   can fire is `buf_ptr.is_null() → Err(5)` (`gui.rs:434`). (`id < 0` never
   happens: the kernel returns `(len-1) as u64`, never negative.)
3. `sys_gui_create_window` (`kernel/src/syscalls/mod.rs:4656`):
   - title: `read_user_string` failure defaults to `"User App"` — never an error;
   - `Window::new(0, 0, w+2, h+22, …)` — always succeeds (2 px border + 22 px chrome);
   - **G3 framebuffer**: `size_bytes = 800·600·4 = 1,920,000`; smallest order
     with `4096<<order ≥ size` is **9 — a 2 MB contiguous block**;
     `BUDDY_ALLOCATOR.allocate_contiguous(9)`:
     - `Some(pa)` → `phys_addr = Some(pa)`, block zeroed; or
     - **`None` → silent fallback `content = Some(heap vec)`, `phys_addr`
       stays `None`, and the syscall still returns success**
       (`mod.rs:4682-4690`);
   - `add_window(win)` — infallible; returns the new handle.
4. `sys_gui_map_buffer` (`kernel/src/syscalls/mod.rs:4709`) returns 0 in
   exactly these cases:
   - **`win.phys_addr` is `None` → `return 0` (line 4717) — THE trigger**,
     reached whenever step 3's buddy allocation failed;
   - `CURRENT_PROCESS` unset → 0 (line 4726);
   - `process.address_space.mapper()` unavailable → 0 (line 4734);
   - `handle ≥ windows.len()` → 0 (line 4712; cannot fire for a fresh handle).
   Note the mapping loop swallows `map_to` errors (`if let Ok(t)`, line 4742),
   so a partially-failed map returns a non-NULL address — a *silent fake
   success* that would fault later, distinct from the NULL path driving this
   loop.
5. Back in libsarga: NULL → `Err(5)` → login-manager prints
   `[login] failed to create window` and exits **0**.

**Why it loops forever (init side):** `init/src/main.rs:120-145` resets
`crashes = 0` on a clean (status 0) exit, then increments to 1 and respawns
since `1 ≤ MAX_RESPAWNS`. Because login-manager *always* exits 0 on this
failure, **the crash counter never accumulates — `MAX_RESPAWNS = 5` never
trips and the loop is genuinely infinite** (a non-zero exit would give up
after 5). Each iteration also leaks one window into the compositor's
`windows` Vec (never destroyed), so the loop grows that list without bound.

**Failure conditions, ranked:**

| Condition | Site | Effect |
|---|---|---|
| Buddy allocator cannot supply a contiguous 2 MB block (order 9) for the framebuffer — exhaustion or fragmentation below 2 MB contiguous | `syscalls/mod.rs:4682` | create *succeeds* with `content` fallback; map returns NULL → loop |
| No process context (`CURRENT_PROCESS` unset) at map time | `syscalls/mod.rs:4726` | map returns NULL → loop |
| Address-space mapper unavailable | `syscalls/mod.rs:4734` | map returns NULL → loop |
| Window-count limit | none exists | `add_window` never checks; not a failure mode |

There is **no window limit** (`windows: Vec` grows unboundedly) and **no
compositor-ready gate** (COMPOSITOR is a `lazy_static`, always available).
"Limits" and "init state" are therefore not the trigger — the trigger is
the create/map_buffer fallback asymmetry plus the two context checks.

**Fix directions** (kernel-side, when the kernel settles):
- Fail the syscall: make `sys_gui_create_window` return `-ENOMEM` (negative,
  so libsarga's `id < 0 → Err(-id)` fires) when the framebuffer can't be
  allocated, instead of silently degrading to `content`.
- Or teach `sys_gui_map_buffer` to map the `content` heap fallback (the
  copy path already exists in `sys_gui_flush`), making the fallback usable
  by the libsarga API.
- Add a `MAX_WINDOWS` bound with an explicit error return.
- Init-side: give login-manager a non-zero exit (or a recognizable marker)
  for window-create failure so `MAX_RESPAWNS` can eventually give up; today
  the clean-exit reset makes the loop unbounded.

Current-boot note: the kernel repo's `serial.log` (stale) shows an earlier
PANIC in `drivers/block/cache.rs` (`copy_from_slice` 512 vs 1024) — the
kernel is mid-major-change and its boot path is unstable, so the loop above
is not empirically reachable in the current tree; this trace documents the
mechanism for when the kernel boots again.

---

## 2. Verified vs. unverified

**Verified in this checkout (userspace sources):**
- init table, respawn logic, exit-code semantics (Hop 2, 6).
- login-manager verify + execve + window-failure exit(0) (Hop 3).
- ade session loop, both exit paths, `EXIT_LOGOUT = 0` (Hop 4, 5).
- The shadow file already contains a PBKDF2 entry
  (`build_initrd.py:177`): `root`, salt `SKYOSDESKTOPSALT` (hex
  `534b594f534445534b544f5053414c54`, **16 bytes**), password `skyos`,
  10 000 iterations — the old "shadow has no PBKDF2 entry" blocker from
  the rebuild plan is **already resolved**. **Salt-length fix (Aug 7,
  2026):** the previous salt `SKYOSDESTOPSALT` was **15 bytes** — libsarga's
  `verify_password` enforces `s.len() == 16` (`libsarga/src/hash.rs:78`),
  so the console/GUI login would have rejected root/skyos even with the
  right password. The host test `tests/test_login_flow.py` caught it (its
  byte-exact port of `verify_password` returned false for the positive
  case); salt corrected to the 16-byte `SKYOSDESKTOPSALT` and the stored dk
  regenerated (`PBKDF2-HMAC-SHA256("skyos", salt, 10000)`).
- **Credential + parse semantics are now host-verified on every push:**
  `tests/test_login_flow.py` (runs in the new `host-tests` CI job, no QEMU,
  stdlib only) asserts root/skyos verifies against the real initrd
  constants, wrong passwords/unknown users/malformed entries fail, the
  stored dk is literally `PBKDF2("skyos", …)`, passwd → `/bin/sash`, and
  source pins for login's execve argv[0].
- `login` (console) and `login-manager` (GUI) share the same shadow + verify
  path.
- The `[login] failed to create window` mechanism (§1 trace): `add_window`
  is infallible; the loop is the create/map_buffer fallback asymmetry plus
  init's clean-exit crash-counter reset.
- **vahid's exit-code discipline (Aug 8, 2026):** `EXIT_DEVICE_SCAN_FAILED
  = 1` on fatal device-node failure (non-zero, so init gives up after
  `MAX_RESPAWNS`), status lines (`[vahid] ready`, `[vahid] FATAL: …`) on
  both paths, and a healthy sleep loop that never exits — pinned by
  `tests/test_vahid_contract.py` (10 tests, host-runnable, in the
  `host-tests` CI job alongside `test_login_flow.py`).
- **Bogus mknod syscall removed from `create_devices` (Aug 8, 2026):** the
  old code called `syscall3(0x7d, path, major, minor)` before the O_CREAT
  fallback — but `0x7d` is **SYS_CLIPBOARD (125), not mknod**, so it could
  never create a node, and its result was discarded (`let _ =`) anyway.
  It is deleted; node creation is the O_CREAT fallback alone
  (`open(path, O_CREAT|O_WRONLY)`), whose result drives `all_ok` → the
  `FATAL`/exit(1) discipline unchanged — the exit code was already honest
  because the bogus call's return was never consulted. **mknod target
  contract for the kernel rewrite (Aug 10, 2026):** audited
  `kernel/src/syscalls/numbers.rs` — the kernel reserves **no mknod
  number today** (no `SYS_MKNOD` constant, no placeholder comment, and
  neither candidate number is dispatched; unknown numbers fall to the
  `_ =>` default at `mod.rs:820`, printing `[SYSCALL] Unknown syscall`
  and returning `-ENOSYS`). The table otherwise mirrors Linux x86_64
  numbering for the filesystem family (MKDIR=83, UNLINK=87, RENAME=82,
  plus the `*at` family 257–269), so the natural landing numbers are:
  - **`SYS_MKNOD = 133`** (Linux x86_64 classic): `mknod(pathname,
    mode, dev)` — arg pattern identical to the existing
    `sys_mkdir(path, mode)` at `mod.rs:4980`.
  - **`SYS_MKNODAT = 259`** (Linux x86_64 `*at`): `mknodat(dirfd,
    pathname, mode, dev)` — 259 sits in the table's existing gap
    between `MKDIRAT (258)` and `FSTATAT (262)`, and `sys_mkdirat`
    (`mod.rs:6491`) is the template.
  **Recommended: 259** (`mknodat` with `AT_FDCWD = -100`), because the
  `*at` family is already the kernel's fs convention; **133** is the
  fallback if the rewrite prefers classic numbers. Semantics: create a
  character device node; `mode = S_IFCHR (0x2000) | 0o600`; `dev`
  packed as Linux `dev_t` = `(major << 8) | minor` — which round-trips
  vahid's existing node table `(null,1,3) (zero,1,5) (random,1,8)
  (urandom,1,9) (tty,5,0) (console,5,1)` verbatim. Errors: `-EEXIST`
  if the node exists, `-EFAULT` on bad path, `-ENOSYS` until
  dispatched. **Gated landing in vahid** (`vahid/src/main.rs:68`
  `create_devices`): call mknod first; on `-ENOSYS` fall back to
  today's O_CREAT (keeps mixed userspace/kernel boots working); on any
  other error drive `all_ok = false` as today; on success verify the
  node exists rather than trusting the return. The `(major, minor)`
  fields are already carried in the nodes table (`_major, _minor`) —
  un-underscore them and pass through.
- **What `open(path, O_CREAT)` actually does on devfs paths — full trace
  (Aug 10, 2026):** the O_CREAT fallback is vahid's ONLY node-creation
  path, so its real behavior matters. Traced end-to-end from current
  kernel source:
  1. `vahid/src/main.rs:79,86` calls `open("/dev/random", 0x41)`
     (`0x41` = `O_CREAT|O_WRONLY`). `libsarga::io::open` (io.rs:84) →
     `crate::syscall::open` → `syscall2(SYS_OPEN=2, path, flags)`
     (syscall.rs:216).
  2. Kernel `sys_open` (mod.rs:1342): `resolve_path` (mod.rs:205)
     selects the longest-prefix mount (mod.rs:184-201). `/dev` is the
     devfs mount (mod.rs:494-495, mounted unconditionally, shadowing
     the initrd tarfs's empty `dev/` dir — initrd.tar contains only
     `dev/`, no children). Traversal then hits devfs's fixed children
     (devfs.rs:316-378): on the committed CI default branch, **null,
     zero, tty0, tty, fb0, speaker, input/{event0,event1}** — there is
     NO `random`/`urandom`/`console` anywhere in that tree. The
     in-flight kernel (the uncommitted rewrite) mints **random,
     urandom, console** natively in DevFs::new (devfs.rs:359-369,
     between `tty` and `fb0`); the devfs contract test keys its node
     pin to whichever tree is checked out.
  3. `resolve_path("/dev/random")` → None on the CI default branch →
     the `O_CREAT` branch (mod.rs:1368-1399) splits parent/name,
     resolves `/dev` → devfs root → calls `parent_node.create("random")`.
     (On the in-flight kernel the path instead resolves to the native
     `random` node and the open succeeds as a no-op, step 4 not reached.)
  4. **`DevNode` implements `VfsNode` without overriding `create`**
     (devfs.rs:28-299 implements name/is_dir/read/write/statfs/stat/
     ioctl/children/find_child only) — so it uses the trait default
     `fn create → Err(())` (mod.rs:104-106). `if let Ok(new_node)`
     fails, and `sys_open` falls through to **`ENOENT`** (mod.rs:1400).
  **Conclusion (state-keyed to the kernel tree): while the nodes are
  absent (CI default branch), the O_CREAT fallback creates NEITHER a
  devfs node NOR a plain file for non-native names — it fails with
  `ENOENT`.** The native names that exist (`null`, `zero`, `tty`) still
  succeed because they resolve (O_CREAT is a no-op on an existing path);
  on that tree `random`/`urandom`/`console` cannot be created at all —
  vahid's `create_devices` would return false → `FATAL` → exit 1 →
  bounded give-up (3 of its 6 nodes fail). **Once the in-flight kernel
  lands** (random/urandom/console native in DevFs::new), all six names
  resolve and the same O_CREAT opens succeed as no-ops —
  `create_devices` returns true. (Contrast: on ramfs/tmpfs `create` IS
  implemented (ramfs.rs:192) and O_CREAT makes a plain file; devfs still
  has no `create` override in either state — the in-flight nodes are
  native, not mknod-created.)
  **Why the Aug 7 selftest-ISO boot showed all six opens succeeding
  (explained by the in-flight kernel):** the ISO's kernel came from the
  same uncommitted-rewrite lineage that today mints `random`/`urandom`/
  `console` natively — so all six of vahid's opens resolved and
  `[vahid] ready` printed with ZERO `FAILED to create` lines. The
  committed CI default branch (last commit Aug 5, a18848f) still lacks
  the nodes and still has no `create` override (git log -S 'urandom' /
  -S 'fn create' on devfs.rs: empty), so on THAT tree the ENOENT
  conclusion above holds. **Settlement mechanism:** the devfs contract
  test (`tests/test_vahid_contract.py`
  ::test_kernel_devfs_still_no_create_no_non_native_nodes) keys its pin
  to the checked-out tree — absence pin on the CI default branch,
  positive pin on the in-flight kernel — and the `/dev` probe in
  `qemu_gui_gate.exp` (stage 5: login → `ls /dev` asserting all six
  names → `dd if=/dev/zero of=/dev/null bs=16 count=1`) settles it on
  the next fresh CI boot.**

**UNVERIFIED (kernel in major change, or would need a QEMU run):**
- ~~Who (if anyone) prints the console `login:` prompt in an ISO boot.~~
  **RESOLVED with evidence (Aug 7, 2026): the prompt is exclusively
  userspace `/bin/login` — the kernel source contains zero `login`
  literals.** `grep -rn 'login' --include='*.rs'` across the kernel repo
  matches nothing (only binary artifacts like `bootimage-*.bin`  and `initrd.tar` do); `login/src/main.rs:233` prints the prompt
  (`io::print_str("login: ")`). `login-manager` (GUI) never writes it —
  its only serial output is `[login] failed to create window` and
  `[login] execve failed, continuing`. See §1 “The serial `login:` prompt”.
- Whether login-manager's `Window::create` succeeds in a stock boot — see
  Gap 1.

---

## 3. Gaps blocking end-to-end testability

**Gap 1 (blocking): the GUI session path is unverified in a stock boot —
and userspace cannot fix it. GATE ADDED Aug 8, 2026** (the open question is
now asserted by CI on every kernel build, even though it remains kernel-
gated): the new `gui-gate` CI job boots the ISO and watches for
login-manager's serial markers — PASS on `[login] window created`, FAIL on
`[login] failed to create window` — via `tests/qemu_gui_gate.exp` (see
“GUI reachability gate” below). login-manager now prints the success
marker, so a healthy GUI and the respawn loop are distinguishable in one
boot. Two facts established Aug 7, 2026:

1. **`Window::create` is a kernel syscall.** libsarga's `Window::create`
   issues `SYS_GUI_CREATE_WINDOW` (#100) directly
   (`libsarga/src/gui.rs:423`); the kernel serves it in-process against its
   `COMPOSITOR` static (`kernel/src/syscalls/mod.rs:4651`,
   `sys_gui_create_window` → `COMPOSITOR.lock().add_window(win)`). There is
   **no userspace display server** in the chain.
2. **vahid is a device manager, not a display server.** `vahid/src/main.rs`
   scans PCI (`/sys/bus/pci/devices/`) and mknods `/dev/{null,zero,random,
   urandom,tty,console}`, then sleeps. It has zero window-serving capability
   and cannot affect `Window::create`. **Since Aug 7, 2026 init starts vahid
   first in its table** (device nodes before any GUI app) — this was the
   "start vahid ahead of login-manager" probe, and it does **not** change
   GUI reachability. **Exit-code discipline (Aug 8, 2026):** vahid now
   reports its health to init — `scan_pci()` returns `Option<usize>` (not
   `()`, so a missing sysfs is an observable degraded state),
   `create_devices()` returns `bool`, and a failed device-node creation
   prints `[vahid] FATAL: failed to create device nodes` and exits with
   `EXIT_DEVICE_SCAN_FAILED = 1` (NON-ZERO), so init's crash accounting
   accumulates toward `MAX_RESPAWNS` and gives up instead of respawning a
   broken device manager forever. A healthy vahid never exits (the sleep
   loop is the last statement), so init sees no exit event. The contract is
   pinned by `tests/test_vahid_contract.py` (host-runnable, in the
   `host-tests` CI job).   the failure mode (`[login] failed to create window`,
   exit 0 → infinite respawn loop → ade never runs) is a **kernel**
   question — now traced to its exact mechanism in §1, “The `[login] failed
   to create window` respawn loop — kernel-side trace” (`add_window` is
   infallible; the trigger is the create/map_buffer fallback asymmetry).
   The kernel is under external major change; **not decidable here.** Until a QEMU boot proves login-manager reaches its password
   field, treat the GUI session as unverified; the console getty (Phase A)
   is the working session path.

**Gap 2 (automation): GUI key injection — traced with evidence and
CORRECTED (Aug 8, 2026).** The input chain is confirmed from kernel source:
QEMU `sendkey` synthesizes events on the emulated i8042 PS/2 controller
(ports 0x60/0x64) → `kernel/src/drivers/ps2.rs` →
`task/keyboard.rs::add_scancode` (`GUI_SCANCODE_QUEUE`) →
`gui_refresh_task` (`kernel/src/main.rs`; `pc_keyboard` constructed with
`HandleControl::Ignore`) → `COMPOSITOR.handle_keyboard` → focused window's
`key_events` (`kernel/src/gui/window.rs`) → `SYS_GUI_GET_KEY` (#105) →
`Window::get_key()` (`libsarga/src/gui.rs`). What that means for Phase B:

- **Letters/digits/printables arrive as Unicode bytes** — `sendkey r` reaches
  login-manager's username buffer. ✓
- **Tab / Enter / Backspace arrive as Unicode bytes TOO.** The Aug 7 finding
  that they "decode as RawKey and are dropped" is **DISPROVEN** by reading
  the vendored pc-keyboard 0.5.1 crate: `src/layouts/us104.rs` maps
  `KeyCode::Backspace → Unicode(0x08)`, `KeyCode::Tab → Unicode(0x09)`,
  `KeyCode::Enter → Unicode(10)`, and `HandleControl::Ignore` only affects
  Ctrl+letter (it leaves the letter alone), not these three keys. So
  login-manager's Tab (0x09) / Enter (0x0A|0x0D) / Backspace (0x7F|0x08)
  handling IS reachable — **the login half of Phase B is expected to pass on
  a kernel built from the current source.** (Gap 2's original
  `RawKey(Tab|Enter|Backspace)` unblock note is therefore obsolete.)
- The harness ends the session with **Esc on the empty ade desktop**
  (`sendkey esc`, Aug 9, 2026) — the byte-deliverable session-end path:
  Esc arrives as Unicode 0x1B (the one distinct control byte the stream
  carries) and ade ends the session when nothing is open, pinned by
  `testing/input.rs::test_session_end_gate`. The Ctrl+Alt+Backspace chord
  remains kernel-gated on **modifier delivery, not RawKey mapping**:
  pc-keyboard decodes Backspace to Unicode(0x08) even with Ctrl+Alt held
  (the US104 entry is unconditional), and `gui/window.rs` only forwards
  `c as u8`, so ade receives plain 0x08 (edit Backspace) and never the
  chord. The kernel must deliver modifier state (Alt) through the byte
  stream for the chord to arrive (design:
  `docs/kernel-gui-modifier-delivery.md`). (Userspace-side the chord is
  fully tested — synthetically, via `KeyEvent::new` — in
  `testing/input.rs::test_session_end_gate`.)
- login-manager prints **`[login] window created`** on successful
  `Window::create` (added Aug 8, 2026 for the GUI reachability gate) and
  `[login] failed to create window` / `[login] execve failed, continuing`
  on failure, so a successful window creation is now directly observable —
  but it still prints no marker for a *successful login*, so `[ade] session
  established` remains the only end-to-end success signal.
- **Full hop-by-hop trace of this chain + the exact kernel/userspace change
  set for modifier delivery is in “The raw-key → byte input path” below**
  (Aug 8, 2026 trace, line refs against the current kernel tree).
- The GUI Login button has **no mouse click handler** (only the eye toggle
  and power menu do), so a mouse-path workaround isn't available today
  either — and would depend on the kernel's PS/2 mouse IRQ path (UNVERIFIED).
- **Runtime evidence gap (Aug 8, 2026):** the only locally available ISO
  (release/skyos-selftest-run.iso, built Aug 7) panics loading `/bin/init`
  (`[PANIC] CR2 … 0xffffffff8035c020` right after `[init] SARGA init
  starting`) — that build predates the kernel's major change, so it cannot
  validate sendkey end-to-end here. CI builds a fresh kernel + ISO per run
  (`tests/probe_sendkey.py` is the no-expect runtime probe for when a fresh
  ISO exists; `tests/qemu_gui_login.exp` is the expect-based CI harness).

### The raw-key → byte input path — end-to-end trace and the modifier-delivery change (Aug 8, 2026)

Full trace of how a physical key becomes the byte `ade` reads, with the exact
change set required for Ctrl/Alt/Shift bits to reach userspace. Line refs are
against the **current** kernel tree (`kernel/kernel/src/`) at audit time; the
kernel is mid-major-change, so function names are the stable anchors.

**The chain, hop by hop (all verified by source read):**

1. **IRQ1 → scancode.** `keyboard_interrupt_handler` (`interrupts.rs:647`,
   registered at `:85`) drains the i8042 status/data ports (0x64/0x60); its
   one-shot `[KBD] IRQ1 fired!` probe is at `:655` (the CI-greppable
   evidence that QEMU `sendkey` routes into this path). Non-mouse bytes feed
   `crate::keyboard::handle_scancode(byte)` (`:670`) and, for the console
   TTY, `crate::tty::feed_scancode(byte)` (`:671`); LAPIC EOI at `:676`.
   Note: the IRQ12 mouse handler (`mouse_interrupt_handler`, `:617`) drains
   the **same** 0x64/0x60 ports and routes non-mouse bytes to
   `handle_scancode` (`:636`) too — so a keyboard scancode can be consumed
   by whichever IRQ fires first; `[KBD] IRQ1 fired!` is the probe, not the
   sole entry.
2. **Scancode queue.** `keyboard.rs:7` `handle_scancode` →
   `task/keyboard.rs:17` `add_scancode` pushes to `GUI_SCANCODE_QUEUE` **and**
   `SCANCODE_QUEUE` (both `ArrayQueue<u8>`, cap 100, `:10-11`); GUI side
   pops via `try_pop_scancode` (`:31`).
3. **Decode (async task).** `gui_refresh_task` (`main.rs:421`, spawned
   `:417`, 100 Hz) drains the queue (`:432`), then **two independent
   decoders run on the same scancode**:
   - **Naive raw-scancode modifier tracking** into `COMPOSITOR` (`:437-441`):
     `0x38`/`0xB8` → `alt_held` set/clear, `0xE0` is a **no-op stub**
     (`:439`), `0x5B`/`0xDB` → `super_held` set/clear, and Alt+Tab
     confirm-on-Alt-release (`:443`). Right-hand Ctrl/Alt and Shift are not
     tracked at all.
   - **pc-keyboard decode:** `kbd.add_byte(scancode)` →
     `kbd.process_keyevent(key_event)` (`:455-456`) →
     `comp.handle_keyboard(key)` (`:457`). `kbd` is constructed with
     **`HandleControl::Ignore`** (`main.rs:428`).
4. **Compositor.** `Compositor::handle_keyboard` (`gui/mod.rs:580`):
   `RawKey` arm (`:582`) — Alt+F4 close (`:583-590`), then forwards to the
   focused window (`:594-596`); `Unicode` arm (`:599`) — Alt+Ctrl+0x04
   close (`:601-605`), Super+arrow snap (`:608-635`), Alt+Tab cycle
   (`:636-648`), Alt+Tab confirm/Escape (`:650-668`), then **falls through to
   forward the Unicode char to the focused window** (`:670`:
   `self.windows[idx].handle_keyboard(key)`).
5. **Window → byte queue.** `Window::handle_keyboard` (`gui/window.rs:190`);
   `key_events: VecDeque<u8>` (`:26`). Kernel-internal terminal widgets
   (first branch, `:191-200`) feed `term.handle_char`; **RawKey is dropped**
   (`:202`). Non-terminal windows (ade's path, `:203-211`) do
   `self.key_events.push_back(c as u8)` (`:207`); RawKey dropped (`:209`).
   This is where the char becomes a **single byte with zero modifier bits**.
6. **Syscall.** `sys_gui_get_key` (`syscalls/mod.rs:4792`, dispatched
   `:678`) pops `win.key_events.pop_front().map(|k| k as u64).unwrap_or(0)` —
   0 means empty. Return type is already `u64`.
7. **libsarga.** `Window::get_key` (`libsarga/src/gui.rs:464`):
   `k == 0 → None`, else `Some(k as u8)` — the **`as u8` truncation at
   `:470` is exactly where modifier bits would be thrown away** even if the
   kernel sent them.
8. **ade.** `main.rs:65` `while let Some(key) = desktop_win.get_key()` →
   `desktop.handle_event(Event::Key(key))` → `handle_key` →
   `handle_key_event_raw(KeyEvent::from_byte(key), key)`
   (`desktop.rs:807`). `KeyEvent::from_byte` (`input/mod.rs:77-93`) folds
   `0x01..=0x1A` → Ctrl+letter, special-cases 0x08/0x0A/0x0D, and **forces
   `alt`/`shift` to false** — a documented pre-existing limitation
   (`input/mod.rs:50-53`). `KeyEvent { code, ctrl, alt, shift }` (`:55-62`)
   and `KeyEvent::new(code, ctrl, alt, shift)` (`:63-73`) are the canonical
   chord form the routing table (`resolve`) already matches on all three bits.

**Where modifiers are lost today (four independent drop points):**

| Drop point | Site | Effect |
|---|---|---|
| `HandleControl::Ignore` | `main.rs:428` | Ctrl+letter decodes to the **plain letter** (pc-keyboard 0.5.1 `lib.rs:200-207`; Ctrl silently dropped) — every Ctrl+letter shortcut is currently dead as text |
| `key_events: VecDeque<u8>` + `push_back(c as u8)` | `gui/window.rs:26,207` | queue holds only the byte; the compositor's `alt_held`/`super_held` state (`main.rs:437-441`) never enters the stream |
| `Some(k as u8)` | `libsarga/src/gui.rs:470` | truncates any high bits on the userspace side of the syscall |
| `from_byte` forces alt/shift false | `ade/src/input/mod.rs:77-93` | last-mile rejection even if the byte arrived |

Backspace itself is not the problem — pc-keyboard maps it to `Unicode(0x08)`
unconditionally (`us104.rs:108`; Tab `:109` → 0x09, Enter `:317` → 0x0A), so a
hardware Ctrl+Alt+Backspace reaches `handle_keyboard` as Unicode(0x08) with the
Ctrl/Alt held *in the compositor's state only* — never in the byte.

> **The concrete unblock design — `docs/kernel-gui-modifier-delivery.md`
> (Aug 9, 2026).** Two reviewable change sets against the live kernel tree:
> **Design A** (recommended) widens `Window::key_events` to `VecDeque<u16>`
> with modifier bits 8..11 (alt/ctrl/shift/super), tracks `ctrl_held`/
> `shift_held` in `gui_refresh_task`'s raw-scancode matcher, and forwards the
> state through `Compositor::handle_keyboard` — the syscall needs **no**
> change (`sys_gui_get_key` already returns `u64`), and ade's `KeyEvent`/
> `resolve` keymap (built in Phase 3) already matches all three bits, so the
> chord fires with no routing change. **Design B** (minimal, ~8 kernel lines)
> encodes the chord as sentinel byte `0x1E` at the compositor, decoded by a
> `from_byte` arm into `KeyEvent::new(BACKSPACE, true, true, false)`. The
> userspace half of A (libsarga `get_key → Option<u16>` + ade `from_raw`) is
> additive and can land before the kernel settles.
>
> **Unblocked before the kernel settles (Aug 9, 2026):** Esc on an empty
> desktop is now a second session-end path — 0x1B is the one control byte
> the stream does carry — so `tests/qemu_gui_login.exp` sends `esc` and the
> logout half of Phase B passes on today's kernel; the chord spec above
> remains the upgrade path when the kernel delivers Alt.

**The exact change set to deliver modifier bits (in delivery order):**

*Kernel (`kernel/kernel/src`):*

1. `main.rs:428` — `HandleControl::Ignore` → **`MapLettersToUnicode`**
   (pc-keyboard `lib.rs:201-205`): Ctrl+letter now decodes to U+0001–U+001A
   instead of the plain letter. Un-gates Ctrl+letter in GUI apps (terminal
   Ctrl+C → 0x03) and makes the chord's Ctrl bit representable.
2. `main.rs:437-441` — complete the kernel's own raw-scancode modifier
   tracking: implement the `0xE0` extended-prefix **stub** (`:439`,
   currently a no-op) so R-Ctrl (E0 1D/9D) and R-Alt (E0 38/B8) are tracked,
   and add Shift (0x2A/0x36). This is the **primary** recommendation —
   pc-keyboard 0.5.1's `Modifiers` is a private field (`lib.rs:52`; the only
   public accessor is `get_ctrl_handling`, `:326`), so sourcing mods from it
   would require forking/vendoring the crate — consistent with the
   conclusion already recorded in `kernel-keyboard-gate.md`. Keep
   `alt_held`/`super_held` only for the compositor's own chords (Alt+Tab,
   Alt+F4, Super+snap), which consume scancodes directly — the packed mods
   for delivery are computed from the same completed state, one source.
3. `gui/mod.rs` Unicode arm + `gui/window.rs:26,207` — pack the char with
   its mods: `packed = (c as u16) | (alt<<8) | (ctrl<<9) | (shift<<10)`;
   (bit8 = alt, bit9 = ctrl, bit10 = shift — the layout pinned by the landed
   userspace half; see `docs/kernel-gui-modifier-delivery.md`, Design A. An
   earlier draft of this doc had ctrl/alt swapped; the chord value is
   identical either way since both bits are set, but partial-modifier decodes
   pin the order: `0x0108` = alt-only, `0x0208` = ctrl-only.)
   widen `key_events` to `VecDeque<u16>`; the compositor passes the mods
   into `Window::handle_keyboard` (or pushes the packed value itself after
   its own chord checks). The kernel-terminal branch (`:191-200`) is
   unaffected. Backspace+Ctrl+Alt = `0x08 | 0x100 | 0x200 = 0x308`.
4. `syscalls/mod.rs:4792-4797` — `sys_gui_get_key` returns the u16 packed
   value; return type is already u64, so **no ABI change**. Keep `0` =
   empty. Caveat: **Ctrl+Space decodes to U+0000 under `MapLettersToUnicode`**
   and would alias "empty" — either map Ctrl+Space away kernel-side or
   accept-and-document the single lost key; no other real key produces NUL.
   Other `key_events` consumers verified before widening:
   `objects/window_object.rs:55` only tests `is_empty()` (unaffected), and
   `sys_gui_get_key` is the sole pop site — no kernel-internal widget reads
   the queue (window widgets render only, `gui/window.rs:112-113,160`).

*Userspace (follows the kernel change):*

5. `libsarga/src/gui.rs:464-470` — `get_key() -> Option<u8>` →
   `Option<u16>`; drop the `as u8` truncation (`:470`). `k == 0 → None`
   unchanged. **Landed (Aug 10, 2026).** All twelve non-ade GUI consumers
   (login-manager, calculator, sarga-term, sargaedit, sargafiles,
   sargasettings, sargastore, sargaview, search, skystore, installer, and
   notes' is_some loop, which needs no edit) compile against the wider type;
   the byte-only ones truncate with `let key = key as u8;` at their call
   site — they never need the chord.
6. `ade/src/input/mod.rs` — **landed as `KeyEvent::from_raw(packed: u16)`**:
   `byte = packed & 0xFF`, alt/ctrl/shift from bits 8/9/10, then reuse the
   existing `from_byte` decode and OR in the mods (bit8 = alt, bit9 = ctrl,
   bit10 = shift per Design A). **The routing table needs no change** —
   `resolve()` already matches all three bits, and `test_keymap` /
   `test_session_end_gate` / `test_logout_protocol_from_chord` already prove
   the chord end-to-end synthetically
   (`0x308 → KeyEvent{0x7F, ctrl, alt} → KeyAction::Quit → request_end()`),
   now joined by `test_from_raw`, which pins the decode and its inertness
   (zero high bits ⇒ byte-identical to `from_byte`).
7. `desktop.rs` — **landed:** `handle_key` unpacks the u16 value via
   `from_raw` and passes the plain low byte (`(key & 0xFF) as u8`) to the
   pty write path, so Ctrl+C → 0x03 still reaches the shell; `handle_a11y_key`
   got a modifier guard (`key & 0xFF00 != 0 ⇒ false`) so the chord cannot be
   swallowed by the a11y pre-handler. `Event::Key` carries u16. The stale
   “alt/shift never arrive” comments and the kernel-gated notes become
   obsolete when the kernel lands — remove them then.

**Verification plan (when the kernel settles):**

- Boot the ISO, then QEMU monitor **`sendkey ctrl-alt-backspace`** — the
  hyphenated chord form is required; the harness's per-key `sendkey_seq`
  helper presses+releases each key, so Ctrl/Alt would be released before
  Backspace and no chord would ever form. Assert `[ade] session ended` +
  `[init] service login-manager exited` + respawn — exactly what Phase B's
  `qemu_gui_login.exp` already checks.
- `[KBD] IRQ1 fired!` (`interrupts.rs:655`) is the one-shot CI probe that
  sendkey actually routes into this path.
- Userspace is already proven: `test_keymap` pins the chord in the table,
  `test_session_end_gate` pins the Desktop no-op rules, and   `test_logout_protocol_from_chord` drives the full protocol from an
   injected `KeyEvent`. `test_from_raw` (landed) pins the decode: 0x308 →
   ctrl+alt+Backspace → Quit, 0x0D plain, 0x0103 → Ctrl+C with the alt bit
   (bit8 = alt), plus the zero-high-bits ≡ from_byte inertness identity.

**Side effects to note for the kernel rewrite:** the compositor's own
“Alt+Ctrl+0x04 close” arm (`gui/mod.rs:601-605`) becomes reachable under
`MapLettersToUnicode` (0x04 = Ctrl+D; it is dead today because Ctrl+D decodes
to plain `'d'` under `Ignore`). Alt+Tab (`:636`) and Super+snap (`:608`) are
unaffected — Tab/arrows decode to Unicode regardless of the control setting.

**Gap 3 (design debt) — RESOLVED (Aug 7, 2026).** The any-Backspace session
gate was removed from main.rs. **Phase C (Aug 7, 2026): the session-end key
is now the Ctrl+Alt+Backspace chord** — routed through `KeyAction::Quit` in
`desktop.rs`, pinned by `testing/input.rs::test_session_end_gate`. The
userspace pipeline is fully modifier-aware: `KeyEvent` carries ctrl/alt/shift,
`Binding` matches all three bits exactly, and `Desktop::handle_key_event`
injects full events, so the chord is expressible and tested synthetically.
What remains kernel-gated: the one-byte input stream cannot deliver Alt, so
a *hardware* Ctrl+Alt+Backspace still needs the kernel to deliver modifier
state (Backspace itself already arrives as Unicode 0x08 — no RawKey mapping
is needed) — see Gap 2. Backspace edits text everywhere (plain windows +
terminals), and Ctrl+Q / plain 'q' are unbound, so typing can never trip
the logout loop.

**Gap 4 (evidence): login has never been forced to succeed by CI.**
The kernel integration job runs `qemu_shell_test.exp … || true` — shell-test
failures are invisible — and that script's `root`/`root` credentials are
stale (the shadow's password is `skyos`). The new ade-selftest job uses
`root`/`skyos` but waits for a console `login:` that Gap 2's UNVERIFIED
finding calls into question.

---

## 4. What it takes to make the logout loop end-to-end testable

Suggested order (each step independently shippable, gates: clippy/build/fmt
+ the ade-selftest CI job green):

1. **Phase A — make a session reachable (userspace only). DONE via the
   console-getty fallback (Aug 7, 2026).** init's service table now spawns
   `getty` → `/bin/login` on the console (fd 0/1 inherited from init), so
   the console path the CI harness already drives works: serial `login:` →
   root/skyos (PBKDF2 shadow) → `/bin/sash` shell. The three CI harnesses
   (`qemu_ade_selftest.exp`, `qemu_shell_test.exp`, `test_login.ps1`)
   previously waited for a `$ `/`# ` shell prompt that sash never prints —
   they now match sash's real `sash[/]> `. `login` passes argv[0] to the
   shell (empty argv breaks argv scans). The GUI-first alternative (start
   vahid before login-manager) remains on the table but was not chosen:
   login-manager's display path is UNVERIFIED (Gap 1) and the console path
   is already CI-drivable.

   **First-run caveat:** the qemu_shell_test.exp per-check assumptions
   (`uname -a` → `SkyOS|sarga`, `ps` → `init|PID`, `futex_test`/`perm_test`
   present in the CI kernel feature set) have never actually run green —
   the job was `|| true`-masked with stale creds and broken patterns. A red
   first run on those checks is the un-masking working as intended, not a
   Phase A regression. `MAX_RESPAWNS=5` bounds real crash loops, but bad
   passwords no longer consume it — login re-prompts in place on a mistype
   (see §1), so five bad passwords can no longer kill the console getty.

2. **Phase B — GUI key injection. HARNESS + CI WIRED (Aug 7, 2026),
   corrected Aug 8, 2026.** `tests/qemu_gui_login.exp` boots the
   ISO with `-serial mon:stdio`, drives login-manager's fields via monitor
   `sendkey` (`root`, `tab`, `skyos`, `ret`), waits for `[ade] session
   established`, then sends the Ctrl+Alt+Backspace chord (ade's ONLY
   session-end key since Phase C; Ctrl+Q / plain 'q' are unbound) and
   asserts `[ade] session ended` + `[init] service login-manager exited` +
   `[init] starting service: login-manager`. Wired as the `gui-login` CI
   job. The login half is expected to PASS on a current kernel (Tab/Enter
   arrive as Unicode bytes — pc-keyboard 0.5.1 US104 layout, Gap 2). The
   logout half stays kernel-gated on MODIFIER delivery (Alt never survives
   the one-byte stream), so the chord arrives as plain Backspace until the
   kernel delivers modifier state; the job fails fast with that diagnostic.
   The session-level fallback — a selftest asserting `request_end()` →
   `is_ending()` + `exit_code() == 0` — is already covered by
   `testing/session.rs::test_session_end_protocol`.

### GUI reachability gate (Aug 8, 2026)

Closes Gap 1's open question (“does `Window::create` succeed in a stock
boot?”) by asserting it on every kernel build:

- **`tests/qemu_gui_gate.exp`** — boots the ISO (`-serial mon:stdio`),
  waits for init's service-spawn loop (stable prefix `[init] starting
  service:` — NOT the full name, since the kernel's serial path has been
  seen garbling service names), then matches the FIRST window marker:
  `[login] window created` → PASS; `[login] failed to create window` →
  FAIL (the respawn loop). Panic, boot-gate timeout (kernel never reached
  userspace), and eof are distinct failure arms. Phase 2 has its own
  120 s timeout so the total stays under the job cap. Verdict line:
  `GUI reachability gate: PASS`.
- **`gui-gate` CI job** in `.github/workflows/ci.yml` — mirrors the
  `ade-selftest` build pipeline (kernel, userspace, initrd, bootimage, ISO)
  and fails the pipeline if the gate verdict is missing.
- **login-manager now prints `[login] window created`** on successful
  `Window::create` — the PASS marker (before Aug 8, 2026 there was no
  success marker, so a healthy GUI was indistinguishable from a hang).

Status: the gate is wired and will go green automatically when the kernel's
GUI subsystem serves a window; until then it fails fast with the exact
marker/phase named (`failed to create window` vs. boot timeout).

**Local runtime probe (Aug 9, 2026) — first real boot evidence.** Rebuilt
the full pipeline from current sources (kernel release via the precompiled
`x86_64-unknown-none` core/alloc — build-std hit a hashbrown-0.14
`rustc-std-workspace-alloc` E0464 collision on the floating nightly; see
note below — userspace release, `build_initrd.py`, bootimage builder, and
`make_iso.py --pycdlib` now producing a TRUE hybrid ISO after the El Torito
`bootcatfile` fix), booted in QEMU, and drove it with
`tests/probe_sendkey.py`:

- init reaches the service-spawn loop ✓ (`[init] starting service:`).
- vahid (pid 103) execs, then **`[vahid] FATAL: failed to create device
  nodes`** — `/dev/random`, `/dev/urandom`, `/dev/console` creation all
  fail on this kernel build. Bounded per its exit-code contract (1 exec, no
  respawn loop).
- login-manager (pid 106) execs, stays runnable
  (`ready[pidSome(106)]` throughout), and prints **neither**
  `[login] window created` nor `[login] failed to create window` —
  `Window::create` **hangs** in-kernel. Both syscalls
  (`sys_gui_create_window`, `sys_gui_map_buffer`) are non-blocking and
  complete in source; the return never reaches userspace. No panic, no
  fault: the GUI-reachability blocker confirmed at runtime, not a marker
  mismatch (the markers are verified live, `login-manager/src/main.rs:62,66`).
- The console getty (`/bin/login`, pid 108) **does** reach its `login: `
  prompt — the Phase A console session path is alive on this boot.
- `[MOUSE-DIAG]` lines are kernel-side boot noise
  (`kernel/src/main.rs:474`), unrelated to login-manager.

Verdict: `[ade] session established` was **not** reached via the GUI path on
this kernel build; the GUI half remains kernel-gated exactly as documented
above, while the Phase A console path works end-to-end up to the prompt.

Kernel-build note: the floating nightly produced E0464 (duplicate
`liballoc` — build-std sysroot hashbrown → `rustc-std-workspace-alloc` shim
colliding with the kernel's own `hashbrown = { version = "0.14", features =
["alloc"] }`). Local workaround, no repo change: build
`x86_64-unknown-none` WITHOUT `-Z build-std`. CI's kernel job should be
re-checked against the same toolchain drift once the kernel settles.

3. **Phase C — fix the chord. USERSIDE COMPLETE (Aug 7, 2026).** The gate
   is narrowed to a real Ctrl+Alt+Backspace chord: `KeyEvent` carries
   ctrl/alt/shift, the routing table matches all three bits, the chord is a
   desktop grab (works from a terminal), Ctrl+Q / plain 'q' are unbound,
   and `testing/input.rs::test_session_end_gate` pins the chord + near-miss
   rejection. Backspace edits text everywhere. The remaining kernel-side
   half (delivering the Alt modifier through the one-byte window stream —
   Backspace itself already arrives as Unicode 0x08, so no RawKey mapping is
   needed) is tracked in Gap 2; the `qemu_gui_login.exp` harness now sends
   `ctrl-alt-backspace` and goes green automatically once the kernel
   delivers modifier state.

4. **Phase D — stop `|| true` masking.** Make the kernel integration job's
   shell-interaction checks (and the corrected `qemu_shell_test.exp`
   credentials, root/skyos) actually gate the pipeline.

---

## 5. Open questions

- ~~Who prints the console `login:` in the CI boot?~~ **RESOLVED (Aug 7,
  2026): `/bin/login` (init's getty service), not the kernel — see §1,
  “The serial `login:` prompt”, for the exact output format and the
  evidence (kernel source has zero `login` literals).**
- Should the session end be a *logout* (back to login-manager) or eventually
  a *reboot/poweroff*? `EXIT_LOGOUT = 0` is the only code today; init has no
  interpretation for others, so reboot/poweroff needs a protocol extension on
  both sides.
- Is the GUI-first path (vahid in init's table) the right call, or should the
  console getty be primary? (Recommendation: console getty first — the CI
  harness already drives consoles, and it unblocks console-login → ade →
  logout → respawn without kernel input work.)

---

## 6. Kernel change queue — one landing checklist for the rewrite

Every kernel-gated item this doc and its companions defer to the rewrite,
consolidated into a single landing list. **Landing condition** = the harness
or pin that proves the change is done; a change is NOT landed until its
condition passes. All docs are drafts (kernel mid-major-change); function
names + syscall numbers are the stable anchors, line numbers drift.

**Landed column (CHECKLIST-OK):** the `landed` column is DERIVED, not
hand-edited — a row flips from `pending` to `CHECKLIST-OK` only when its
gate doc carries the evidence-quoting marker `**CHECKLIST-OK**` (added by
the rewrite on landing, next to the doc's Status line, quoting the
landing-condition PASS/ok evidence, e.g. `**CHECKLIST-OK:** ok N -
gui::option1_*`). `tests/checklist_gate.py` greps the docs for the
markers + evidence and asserts the column agrees on every CI run
(`--report` prints the current ticks), so the rewrite ticks items off by
landing evidence, never by editing the table by hand.

| # | Change (doc) | What lands | Landing condition | landed |
|---|---|---|---|---|
| K1 | GUI window fix — [`kernel-gui-window-fix.md`](kernel-gui-window-fix.md): promote the heap-content fallback in `sys_gui_map_buffer` (**Option 1**, recommended) so a window created under memory pressure maps to a real shared buffer and `Window::create` succeeds | No userspace change; login-manager's `[login] window created` path works under pressure; respawn loop dies | `qemu_gui_gate.exp` prints `GUI + device-manager reachability gate: PASS`; kernel selftest `gui::option1_*` (see K3) passes | pending |
| K1-alt | GUI window fix, honest-failure variant — same doc **Option 2 + 2b**: `sys_gui_create_window` returns `-ENOMEM` (no silent fallback) and login-manager reports `Out of memory` + exits non-zero (bounded) | `[login] window create failed: Out of memory` then `[init] giving up on login-manager` after MAX_RESPAWNS | give-up harness's unbounded-absence grep flips to a POSITIVE `giving up on .*login-manager` requirement (see the fix doc's Option 2b notes); K1 and K1-alt are mutually exclusive — pick by the memory-pressure evidence (fix doc's Evidence probe: persistent OOM → K1-alt, transient → K1) | pending |
| K2 | Keyboard modifiers + control bytes — [`kernel-keyboard-gate.md`](kernel-keyboard-gate.md): deliver the Alt bit (mods byte `byte | (mods << 8)`) and control-letter semantics through the window key stream, so Tab/Enter (Phase B GUI login) and Ctrl+Alt+Backspace (session-end chord) reach ade | `key_events: VecDeque<u32>` carries mods; `gui/mod.rs:571` `handle_keyboard(key, mods)`; libsarga `get_key` returns `u16` (packed byte+mods); ade `input::from_byte` gains the mods-aware variant | `[KBD] IRQ1 fired!` routing gate; `qemu_gui_login.exp` `sendkey tab` / `sendkey ret` reach the password field; chord → `[ade] session ended` + init respawn; host pins `test_keymap` + `test_session_end_gate` extend `from_byte` to decoded mods | pending |
| K3 | Option 1 selftest — [`kernel-gui-selftest-spec.md`](kernel-gui-selftest-spec.md): three TAP tests (`gui::option1_fallback_forced`, `gui::option1_promotion_maps`, `gui::option1_renderable`) that drain the buddy to force the fallback, promote, and render-check | `kernel/src/tests/gui_tests.rs` + `pub(crate)` on `sys_gui_create_window` | `ok N - gui::option1_*` lines in the self_test serial (kernel CI greps `not ok`) | pending |
| K3-alt | Option 2 selftest — [`kernel-gui-selftest-spec-option2.md`](kernel-gui-selftest-spec-option2.md): two TAP tests (`gui::option2_enomem_forced`, `gui::option2_create_succeeds_when_room`) asserting `create_window` returns `-ENOMEM` under a drained buddy and a valid handle when room exists, plus the host-pinned Option 2b `Err(12)` contract (`Out of memory` + `EXIT_WINDOW_CREATE_FAILED`) | same `gui_tests.rs` REPLACING the `option1_*` registrations (mutually exclusive with K3); login-manager 2b hunk | `ok N - gui::option2_*` in the self_test serial; host pin `test_selftest_spec_contract.py`; give-up harness positive `giving up on .*login-manager` when the kernel lands | pending |
| K4 | termios ECHO — [`kernel-tcsets-echo.md`](kernel-tcsets-echo.md): honor the ECHO bit (store termios on tty0, clear/set via TCSETS, echo only when set) | login's `echo_off` no longer leaks passwords on serial | `tests/test_login_echo.py` already pins the userspace half (TCGETS failure → None, ECHO cleared, fields preserved); kernel side verified by a no-echo console login in `qemu_shell_test.exp` | pending |
| K5 | DRMCTL shape fix — [`kernel-drmctl-fix.md`](kernel-drmctl-fix.md): SET_MODE reads a `ModeInfo` struct from `arg` via `copy_from_user` (CREATE_DUMB pattern; today reads `_fd`/`request` → permanent `EINVAL`); MAP_DUMB gets a real id→vaddr registry (`IrqSafeMutex` + `AtomicU64` ids, `-ENOENT` on miss) instead of returning the framebuffer; `destroy_dumb(id)` carries the id | sargasettings resolution selection actually changes the mode; `gpu.rs` UNUSABLE comments removed | kernel selftest `ok N - gui::drmctl_set_mode_ok` + `gui::drmctl_map_dumb_roundtrip`; QEMU harness (`tests/qemu_drm_probe.exp`) greps `DRM: set_mode` | pending |
| K6 | Exit→reap chain — [`kernel-exit-reap-chain.md`](kernel-exit-reap-chain.md): make a child's `exit_code` observable to the parent's `waitpid` scan. Working tree already has `sys_exit`/SIGSEGV marking + the `sys_wait4` reap; the blocking gap is `kill_process`'s `table.remove(&pid)` (child vanishes before reap, stale `children` entry); hygiene: raw-status convention comment + drop the `status != 42` print guard | `init` logs `[init] service svc exited` + `giving up on svc` on real exits; OOM-killed children become reapable | `tests/qemu_giveup_boot.exp` flips from `KERNEL-GATED:` to hard PASS (`[init] service svc exited` + `giving up on svc`); `tests/test_init_golden_trace.py::SVC_EXIT_BOOT` replays to `respawn × MAX_RESPAWNS` then `gave_up` | pending |
| K7 | Free-page low-water node — [`kernel-mem-lowwater.md`](kernel-mem-lowwater.md): `min_free_pages` on `FragmentationStats` updated in allocate/deallocate (+ `add_region` boot seed), exposed as `/ctl/sys/mem/lowwater` | `/ctl/sys/mem/lowwater` returns the minimum free-page count since boot (monotonic, never rises); the Option 1 vs 2 evidence series gets a one-read-per-boot baseline | kernel selftest `ok N - buddy::low_water_monotonic` (alloc→free→alloc never raises the reading); host pin `TestOption2bDocDiff::test_lowwater_diff_applies_cleanly_to_kernel` keeps the draft patch applying; userspace follow-up (not in the patch): `[login] mem lowwater=N` marker | pending |
| K8 | Clipboard ownership — [`kernel-owns-facility-audit.md`](kernel-owns-facility-audit.md) (F1 rewire completion + F8 hardening): `sys_clipboard` (125) is the single shared clipboard store — sash yank and ade portal copy both write `COMPOSITOR.clipboard`, portal paste reads it, ClipboardManager survives only as the 16-entry history overlay | cross-system paste works end-to-end (a sash yank pastes into ade apps and vice versa); `sys_clipboard` copies through the `user_access` boundary (`copy_from_user`/`copy_to_user`) instead of the raw pointer derefs at `syscalls/mod.rs:4876,4885`; libsarga clipboard wrappers surface errno via `Error::from_i64` instead of clamping to 0; the F1 rewire's `test_clipboard_contract.py` (already green) stays green | `ok N - syscalls::clipboard_copy_roundtrip` in the self_test serial (write-then-read through the syscall, with an `-EFAULT` arm); F8 hardening visible — `sys_clipboard` copies through the `user_access` boundary (`copy_from_user`/`copy_to_user`) instead of raw pointer derefs; host pin `tests/test_clipboard_contract.py` stays green (portal reads the kernel store, sash yanks write it, manager is history overlay only, wrapper `if r < 0` guards live); `tests/qemu_clipboard_probe.exp` PASSes on a real boot — console sash yank (Esc `Y`) then `sendkey ctrl-b`, the GUI kernel-store read prints the yanked bytes | pending |
| K9 | /dev node mknod contract — [`kernel-owns-facility-audit.md`](kernel-owns-facility-audit.md) (dev-node item), full contract in this doc's 0x7d note: dispatch `SYS_MKNODAT=259` (recommended; `SYS_MKNOD=133` fallback) creating a character device node (`S_IFCHR` 0x2000 with 0o600 perms, `dev` packed per the note); vahid's `create_devices` calls mknod first, falls back to O_CREAT on `-ENOSYS`, drives `all_ok` on any other error, verifies the node on success | the six `(name, major, minor)` nodes — null 1,3; zero 1,5; random 1,8; urandom 1,9; tty 5,0; console 5,1 — are real character devices, not plain O_CREAT files; the `test_no_bogus_mknod_syscall` kernel-numbers absence leg flips in the same change | `ok N - vfs::mknodat_creates_dev_node` in the self_test serial; `tests/test_vahid_contract.py` mknod pins updated in the same change (the numbers.rs leg trips on merge — update doc and gated landing together); `qemu_gui_gate.exp` still greps `[vahid] created /dev/<name>` for all six and `[vahid] scanned N PCI device(s)` | pending |
| K10 | NIC/DHCP RX-TX — [`kernel-net-dhcp.md`](kernel-net-dhcp.md): smoltcp never obtains a lease on real hardware (E1000 RX/TX does not deliver; one-shot DHCP attempt with no renew). Fix the E1000 receive path and add a DHCP retry so the exchange completes, and correct the misleading `Network: Stack initialized ... fallback IP 10.0.2.15` print (VGA-only; the address is never assigned — only the default route to 10.0.2.2 is added) | `[DHCP] configured IP:` appears in the serial log of a real boot with the e1000 device; the misleading fallback-IP line is corrected | `tests/qemu_net_probe.exp` flips from `KERNEL-GATED:` to hard `PASS: DHCP lease obtained on real hardware`; the TCP-echo leg (blocked on K10 + a guest socket tool, see the doc) becomes a real stage | pending |

Not gated (context only, no landing harness):

- [`kernel-owns-facility-audit.md`](kernel-owns-facility-audit.md) — userspace↔kernel facility ownership audit; the clipboard items (F1 rewire completion + F8 hardening) and the dev-node/mknod item are gated above (K8, K9); the remaining findings (F2–F5) and the ownership map stay read-only context.
- [`kernel-gui-modifier-delivery.md`](kernel-gui-modifier-delivery.md) — earlier unblock design for the chord; superseded by K2's concrete `MapLettersToUnicode` + mods-byte spec (kept for history).