#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::String;
use libsarga::auth::{BACKOFF_NS, MAX_FAILED_ATTEMPTS};
use libsarga::theme::Theme;
use libsarga::{gui::Window, sarga_main};
use libsarga::{io, process};

const SHADOW_PATH: &str = "/etc/shadow";

fn verify_password(username: &str, password: &str) -> bool {
    let data = match libsarga::fs::read_to_string(SHADOW_PATH) {
        Ok(d) => d.into_bytes(),
        Err(_) => return false,
    };
    libsarga::hash::verify_password(&data, username, password)
}

/// Count one failed GUI login attempt. When the cap is reached, show the
/// pause message in the window and announce it on serial. The BACKOFF_NS
/// sleep itself runs in the main loop AFTER win.flush() (see below), so the
/// window shows the message for the whole pause instead of freezing on a
/// stale frame; only a successful sleep disarms the counter (a failed
/// nanosleep, e.g. EINTR, must not skip the backoff and arm the next burst
/// at full speed) and clears the pause message, so the next attempt shows
/// the plain "Invalid username or password". Never exits, by design: the
/// GUI session is init service
/// "login-manager" with respawn: true, so an auth-failure exit would burn
/// MAX_RESPAWNS the same way the console getty's used to — the loop just
/// re-prompts in the window.
fn note_failed_attempt(failures: &mut u32, error_msg: &mut String) {
    *failures += 1;
    if *failures >= MAX_FAILED_ATTEMPTS {
        *error_msg = String::from("Too many failed attempts - pausing 30s");
        io::print_str("\nToo many failed attempts - pausing 30s\n");
    }
}

fn user_main() -> i32 {
    let theme = Theme::dark();
    // Boot-time memory-pressure marker (evidence for
    // kernel-gui-window-fix.md Option 1 vs Option 2): read the kernel
    // buddy allocator's live free-page count from ctlFS right before
    // the GUI buffer allocation. If Window::create then fails, this
    // number tells the gate whether the failure was persistent OOM
    // (free near zero -> Option 2's honest -ENOMEM is the right fix)
    // or transient/fragmentation (plenty free -> Option 1's
    // map-the-heap-fallback). Printed on every respawn, so the serial
    // capture also shows whether free memory recovers between attempts.
    match libsarga::fs::read_to_string("/ctl/sys/mem/free") {
        Ok(free) => {
            let pages = free.trim().split(' ').next().unwrap_or("?");
            io::print_str(&alloc::format!("[login] mem free={} pages\n", pages));
        }
        Err(_) => io::print_str("[login] mem free=unavailable\n"),
    }
    let mut win = match Window::create("SARGA OS", 800, 600) {
        Ok(w) => {
            io::print_str("[login] window created\n");
            w
        }
        Err(e) => {
            // WHY marker for the GUI-hang gate: paired with the
            // "[login] mem free=N pages" line above, this settles whether
            // the window-failure loop is the known ENOMEM path (errno 12 -
            // kernel-gui-window-fix.md Option 2: create_window returns
            // -ENOMEM when the contiguous allocation fails) or something
            // new (any other errno, e.g. 5 = map_buffer returned NULL).
            // free~0 + Out of memory = persistent OOM; plenty free + any
            // errno = a fresh cause, not the documented ENOMEM loop. (The
            // exit stays 0 here by design - the non-zero Option 2b exit is
            // kernel-gated and only lands with the kernel change.)
            let msg = if e == libsarga::errno::ENOMEM as i64 {
                alloc::string::String::from(
                    "[login] failed to create window: Out of memory (errno 12)\n",
                )
            } else {
                alloc::format!("[login] failed to create window: errno {}\n", e)
            };
            io::print_str(&msg);
            return 0;
        }
    };

    let mut username_buf = alloc::vec::Vec::new();
    let mut password_buf = alloc::vec::Vec::new();
    let mut active_field = 0usize;
    let mut error_msg = String::new();
    let mut failures: u32 = 0;

    let mut show_password = false;
    let mut power_menu = false;
    let mut m_was_pressed = false;

    loop {
        let mouse = win.get_mouse();
        let mx = mouse.x;
        let my = mouse.y;
        let m_pressed = mouse.buttons != 0;
        let m_down = m_pressed && !m_was_pressed;
        m_was_pressed = m_pressed;

        while let Some(key) = win.get_key() {
            let key = key as u8;
            match key {
                0x09 => {
                    active_field = (active_field + 1) % 2;
                    // Serial announce for the QEMU gate's Tab/Enter routing
                    // probe (kernel-keyboard-gate.md section 3, Phase B):
                    // Tab arrives as Unicode 0x09 through the GUI pipeline,
                    // and this marker proves the focus advance happened on
                    // real hardware - the gate asserts the '-> password'
                    // leg after one Tab.
                    let which = if active_field == 1 {
                        "password"
                    } else {
                        "username"
                    };
                    io::print_str(&alloc::format!("\n[login] tab: focus -> {}\n", which));
                } // Tab
                0x0A | 0x0D => {
                    let user = core::str::from_utf8(&username_buf).unwrap_or("");
                    let pass = core::str::from_utf8(&password_buf).unwrap_or("");
                    // Parity with the console getty's bare-Enter guard: a
                    // stray Enter with no username re-prompts WITHOUT
                    // consuming a failed attempt, so it can't silently burn
                    // the brute-force budget.
                    if user.is_empty() {
                        error_msg.clear();
                        continue;
                    }
                    if verify_password(user, pass) {
                        match process::execve("/bin/ade", &["/bin/ade"], &[]) {
                            Ok(_) => return 0,
                            Err(_) => {
                                let _ = io::write_all(1, b"[login] execve failed, continuing\n");
                                error_msg.clear();
                                password_buf.clear();
                            }
                        }
                    } else {
                        error_msg = String::from("Invalid username or password");
                        password_buf.clear();
                        note_failed_attempt(&mut failures, &mut error_msg);
                        // Serial announce (parity with the console getty's
                        // "Login incorrect"): the QEMU harness asserts this
                        // marker to prove a bad password re-prompts in place
                        // (window up, no exit/respawn) on real hardware.
                        io::print_str("\n[login] invalid credentials - re-prompting\n");
                    }
                }
                0x7F | 0x08 => {
                    if active_field == 0 {
                        username_buf.pop();
                    } else {
                        password_buf.pop();
                    }
                    error_msg.clear();
                }
                c if (0x20..0x7F).contains(&c) => {
                    if active_field == 0 {
                        if username_buf.len() < 32 {
                            username_buf.push(c);
                        }
                    } else {
                        if password_buf.len() < 64 {
                            password_buf.push(c);
                        }
                    }
                    error_msg.clear();
                }
                _ => {}
            }
        }

        win.clear(theme.bg_primary);
        win.draw_gradient_rect(0, 0, 800, 600, theme.bg_primary, 0xFF000000, true);

        // Login panel
        let panel_w = 400u32;
        let panel_h = 320u32;
        let px = (800 - panel_w) / 2;
        let py = (600 - panel_h) / 2;

        win.draw_rounded_rect(
            px,
            py,
            panel_w,
            panel_h,
            theme.border_radius,
            theme.bg_surface,
        );
        win.draw_rounded_rect_outline(px, py, panel_w, panel_h, theme.border_radius, theme.border);

        // Logo / Title
        win.draw_gradient_rect(
            px + 10,
            py + 10,
            panel_w - 20,
            40,
            theme.accent,
            theme.accent_dark,
            false,
        );
        win.draw_string_centered(py + 22, "SARGA OS", 0xFFFFFFFF, 0);

        // Username
        let field_x = px + 40;
        let field_w = panel_w - 80;
        win.draw_string(field_x, py + 70, "Username", theme.text_secondary, 0);
        let uy = py + 95;
        let u_bg = if active_field == 0 {
            theme.bg_elevated
        } else {
            theme.bg_primary
        };
        win.draw_rounded_rect(field_x, uy, field_w, 35, 6, u_bg);
        if active_field == 0 {
            win.draw_rounded_rect_outline(field_x, uy, field_w, 35, 6, theme.accent);
        }
        let u_text = core::str::from_utf8(&username_buf).unwrap_or("");
        win.draw_string(field_x + 10, uy + 10, u_text, theme.text, 0);

        // Password
        win.draw_string(field_x, py + 145, "Password", theme.text_secondary, 0);
        let pwy = py + 170;
        let p_bg = if active_field == 1 {
            theme.bg_elevated
        } else {
            theme.bg_primary
        };
        win.draw_rounded_rect(field_x, pwy, field_w, 35, 6, p_bg);
        if active_field == 1 {
            win.draw_rounded_rect_outline(field_x, pwy, field_w, 35, 6, theme.accent);
        }

        let pw_text: String = if show_password {
            core::str::from_utf8(&password_buf).unwrap_or("").into()
        } else {
            "*".repeat(password_buf.len())
        };
        win.draw_string(field_x + 10, pwy + 10, &pw_text, theme.text, 0);

        // Show Password toggle
        let eye_x = field_x + field_w - 30;
        let eye_y = pwy + 10;
        win.draw_string(
            eye_x,
            eye_y,
            if show_password { "O" } else { "X" },
            theme.text_disabled,
            0,
        );
        if m_down
            && mx >= eye_x as u64
            && mx < (eye_x + 20) as u64
            && my >= eye_y as u64
            && my < (eye_y + 20) as u64
        {
            show_password = !show_password;
            let _ = io::nanosleep(100_000_000);
        }

        // Error message
        if !error_msg.is_empty() {
            win.draw_string_centered(py + 215, &error_msg, theme.error, 0);
        }

        // Login button
        let btn_w = 120;
        let btn_x = px + (panel_w - btn_w) / 2;
        let btn_y = py + 250;
        win.draw_gradient_rect(
            btn_x,
            btn_y,
            btn_w,
            35,
            theme.accent,
            theme.accent_dark,
            true,
        );
        win.draw_string_centered(btn_y + 10, "Login", 0xFFFFFFFF, 0);

        // Power button
        let pwr_x = 760u32;
        let pwr_y = 560u32;
        win.draw_rounded_rect(pwr_x, pwr_y, 30, 30, 15, theme.bg_elevated);
        win.draw_string(pwr_x + 10, pwr_y + 7, "P", 0xFFFFFFFF, 0);
        if m_down && mx >= pwr_x as u64 && my >= pwr_y as u64 {
            power_menu = !power_menu;
            let _ = io::nanosleep(100_000_000);
        }

        if power_menu {
            let menu_w = 120;
            let menu_h = 80;
            let mx_pos = pwr_x - menu_w + 30;
            let my_pos = pwr_y - menu_h - 5;
            win.draw_rounded_rect(mx_pos, my_pos, menu_w, menu_h, 8, theme.bg_surface);
            win.draw_rounded_rect_outline(mx_pos, my_pos, menu_w, menu_h, 8, theme.border);
            win.draw_string(mx_pos + 10, my_pos + 15, "Reboot", theme.text, 0);
            win.draw_string(mx_pos + 10, my_pos + 45, "Shutdown", theme.text, 0);

            if m_down && mx >= mx_pos as u64 && mx < (mx_pos + menu_w) as u64 {
                if my >= (my_pos + 10) as u64 && my < (my_pos + 40) as u64 {
                    io::reboot();
                } else if my >= (my_pos + 40) as u64 && my < (my_pos + 70) as u64 {
                    io::poweroff();
                }
            }
        }

        let _ = win.flush();
        // The 30 s backoff runs AFTER the frame is flushed so the "Too many
        // failed attempts" message stays on screen for the whole pause; only
        // a successful sleep disarms the counter (EINTR must not skip the
        // backoff and re-arm the next burst), and the successful disarm also
        // clears the pause message so the next attempt shows the plain
        // "Invalid username or password" instead of the stale cap message.
        // `&&` short-circuits exactly like the nested form: below the cap,
        // nanosleep never runs.
        if failures >= MAX_FAILED_ATTEMPTS && io::nanosleep(BACKOFF_NS).is_ok() {
            failures = 0;
            error_msg.clear();
        }
        let _ = io::nanosleep(16_000_000);
    }
}

sarga_main!(user_main);
