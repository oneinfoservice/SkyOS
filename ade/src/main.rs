//! ADE desktop entrypoint — init, event loop, rendering.
//!
//! The desktop modules live in the `ade` library (see `lib.rs`) so the pure
//! logic is host-testable; this binary is the bare-metal entrypoint only.
//! The `no_std`/`no_main` attributes and the manual `main` are applied only
//! for the real build — under `cfg(test)` the std test harness supplies its
//! own `main`.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
extern crate alloc;
use ade::core::desktop::Desktop;
use ade::core::event::Event;
use ade::core::geometry::Point;
use ade::input;
use ade::render;
use ade::render::compositor::Compositor;
use ade::util;
use libsarga::gui::Window;
use libsarga::io;
#[cfg(not(test))]
use libsarga::sarga_main;

fn user_main() -> i32 {
    io::print_str("[ade] starting desktop environment\n");

    // Test mode: run the in-process selftest suite and exit with its result.
    // Deliberately runs before any GUI window is created, so `ade --selftest`
    // works from a plain console session (CI boots the ISO and drives it over
    // the serial line — see .github/workflows/ci.yml, the `ade-selftest` job).
    if (0..libsarga::args::argc()).any(|i| libsarga::args::get(i as usize) == Some("--selftest")) {
        // Keymap contract marker: the routing-table dump, printed before the
        // suite verdict so the CI grep gate (ci.yml 'Verify ade selftest
        // verdict') can assert the keymap contract on every QEMU run even if
        // the test_keymap pin itself were ever unwired from run_all. The
        // exact literal is cross-pinned by tests/test_ade_selftest_gate.py
        // against this source — update BOTH when the table legitimately
        // changes. `ctrlq=no` means Ctrl+Q is unbound (no desktop action).
        let dump = input::dump_bindings();
        io::print_str(&alloc::format!(
            "[input] bindings={} quit={} chord={} ctrlq={} grabs={}\n",
            dump.count,
            dump.quit_count,
            if dump.has_quit_chord { "yes" } else { "no" },
            if dump.ctrl_q_unbound { "no" } else { "yes" },
            dump.desktop_grabs,
        ));
        let mut test_desktop = Desktop::new(800, 600);
        let ok = util::testing::run_all(&mut test_desktop);
        io::print_str(if ok {
            "[ade] selftest PASS\n"
        } else {
            "[ade] selftest FAIL\n"
        });
        libsarga::process::exit(if ok { 0 } else { 1 });
    }

    let mut desktop_win = match Window::create("SARGA OS Desktop", 800, 600) {
        Ok(w) => w,
        Err(e) => {
            io::print_str(&alloc::format!("[ade] failed to create window: {}\n", e));
            return 0;
        }
    };

    let mut desktop = Desktop::new(desktop_win.width, desktop_win.height);
    let mut compositor = match Compositor::new(desktop_win.width, desktop_win.height) {
        Some(c) => c,
        None => {
            io::print_str("[ade] failed to allocate compositor buffers\n");
            return 0;
        }
    };
    // Session lifecycle: desktop environment session established
    let _ = io::write_all(1, b"[ade] session established\n");
    // ponytail: terminal auto-launch removed — opens on icon click instead
    io::print_str("[ade] desktop running\n");

    let mut last_frame_ticks = 0u64;
    while !desktop.session.is_ending() {
        desktop.tick();

        while let Some(key) = desktop_win.get_key() {
            // Session lifecycle: the session-end key is a keymap action, not
            // a raw-key gate here. Ctrl+Alt+Backspace resolves to
            // `KeyAction::Quit` inside `Desktop::handle_key`, which calls
            // `request_end()` only when the window list is empty. Esc on an
            // empty desktop is the byte-deliverable second path — 0x1B is the
            // one distinct control byte the stream carries, ended in the a11y
            // Esc arm. Ctrl+Q and plain 'q' are deliberately unbound;
            // Backspace is NOT a session key — it edits text in plain windows
            // and goes to the shell inside a terminal, so text entry can
            // never trip the logout loop. `key` is the u16 packed value
            // (low byte = char, bits 8..10 = mods); the kernel does not send
            // modifier bits yet, so `from_raw` behaves exactly like
            // `from_byte` until the Phase C kernel change lands — see
            // docs/kernel-gui-modifier-delivery.md, Design A.
            desktop.handle_event(Event::Key(key));
            if desktop.session.is_ending() {
                break;
            }
        }
        if desktop.session.is_ending() {
            break;
        }

        let ms = desktop_win.get_mouse();
        let (pressed, released, dragging) =
            desktop.update_mouse(ms.x as i32, ms.y as i32, ms.buttons & 1 != 0);
        if ms.scroll != 0 {
            desktop.handle_event(Event::Scroll(ms.scroll));
        }
        let mouse_pt = Point::new(ms.x as i32, ms.y as i32);
        if pressed {
            desktop.handle_event(Event::MouseClick(mouse_pt));
        } else if ms.buttons & 4 != 0 {
            desktop.handle_event(Event::MouseMiddle(mouse_pt));
        } else if ms.buttons & 2 != 0 {
            desktop.handle_event(Event::MouseRight(mouse_pt));
        } else if dragging {
            desktop.handle_event(Event::MouseDrag(mouse_pt));
        }
        if released {
            desktop.handle_event(Event::MouseRelease);
        }

        if desktop.damage.is_dirty() {
            let clock_str = desktop.prepare_clock();
            let snap = desktop.snapshot();
            render::render(&mut desktop_win, &snap, &clock_str, &mut compositor);
            if let Err(e) = desktop_win.flush() {
                io::print_str(&alloc::format!("[ade] flush error: {}\n", e));
            }
            desktop.damage.clear();
        }
        // ponytail: adaptive frame pacing — shorten sleep if frame took longer
        let frame_ticks = desktop.clock_ticks - last_frame_ticks;
        last_frame_ticks = desktop.clock_ticks;
        let target_ns = 16_666_667u64;
        let elapsed_ns = frame_ticks * target_ns;
        let sleep_ns = if elapsed_ns < target_ns {
            target_ns - elapsed_ns
        } else {
            1_000_000
        };
        unsafe {
            libsarga::syscall::syscall2(35, 0, sleep_ns);
        }
    }
    // Session lifecycle: session ended — print the exit code and ending
    // state so the CI grep gates can assert the idempotent-unwind contract
    // on real input (EXIT_LOGOUT = 0; init treats 0 as a clean exit and respawns
    // login-manager), not just in the synthetic host tests. The marker lands
    // exactly once per session, at unwind.
    io::print_str(&alloc::format!(
        "[ade] session ended code={} ending={}\n",
        desktop.session.exit_code(),
        desktop.session.is_ending(),
    ));
    desktop.session.exit_code()
}

#[cfg(not(test))]
sarga_main!(user_main);
