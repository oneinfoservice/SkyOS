#![no_std]
#![no_main]

extern crate alloc;
extern crate libsarga;

use alloc::string::ToString;
use alloc::vec::Vec;
use libsarga::auth::{BACKOFF_NS, MAX_FAILED_ATTEMPTS};
use libsarga::io::{self, read_to_end};
use libsarga::process::{execve, setgid, setuid};
use libsarga::sarga_main;
use libsarga::tty::{ensure_echo, read_line, read_password};

const PASSWD_PATH: &str = "/etc/passwd";
const SHADOW_PATH: &str = "/etc/shadow";

fn note_failed_attempt(failures: &mut u32) {
    *failures += 1;
    if *failures >= MAX_FAILED_ATTEMPTS {
        io::print_str("\nToo many failed attempts - pausing 30s\n");
        // Only disarm the counter when the pause actually happened: a failed
        // nanosleep (e.g. EINTR) must not skip the backoff and arm the next
        // burst at full speed.
        if io::nanosleep(BACKOFF_NS).is_ok() {
            *failures = 0;
        }
    }
}

fn lookup_user(username: &str) -> Option<(u32, u32, Vec<u8>, Vec<u8>)> {
    let data = read_to_end(PASSWD_PATH).ok()?;
    for line in data.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(7, |&b| b == b':');
        let name = parts.next()?;
        if name == username.as_bytes() {
            let _pw_passwd = parts.next()?;
            let uid_str = parts.next()?;
            let gid_str = parts.next()?;
            let _gecos = parts.next()?;
            let home = parts.next()?;
            let shell = parts.next()?;
            let uid = core::str::from_utf8(uid_str).ok()?.parse::<u32>().ok()?;
            let gid = core::str::from_utf8(gid_str).ok()?.parse::<u32>().ok()?;
            return Some((uid, gid, home.to_vec(), shell.to_vec()));
        }
    }
    None
}

fn verify_password(username: &str, password: &str) -> bool {
    let data = match read_to_end(SHADOW_PATH) {
        Ok(d) => d,
        Err(_) => return false,
    };
    libsarga::hash::verify_password(&data, username, password)
}

fn user_main() -> i32 {
    let argc = libsarga::args::argc();
    // `login <user>` passes a fixed username on argv; the console getty (no
    // argv) prompts for both. Non-interactive invocations keep the old
    // exit-on-failure semantics: no caller exists today, and looping a
    // scripted call would hang it.
    let fixed_user = if argc > 1 {
        Some(libsarga::args::get(1).unwrap_or("root").to_string())
    } else {
        None
    };

    // Boot-time memory-pressure marker (evidence for
    // kernel-gui-window-fix.md Option 1 vs Option 2): read the kernel
    // buddy allocator's live free-page count from ctlFS at getty startup,
    // so the shell-interaction harness (qemu_shell_test.exp) collects the
    // same OOM evidence the GUI gate's login-manager does. The getty never
    // exits (mistype re-prompt loop below), so this prints once per boot,
    // not per respawn. Same ctlFS node and print shape as login-manager
    // (login-manager/src/main.rs:53-59) - the harnesses grep the same
    // '[login] mem free=' prefix.
    match libsarga::fs::read_to_string("/ctl/sys/mem/free") {
        Ok(free) => {
            let pages = free.trim().split(' ').next().unwrap_or("?");
            io::print_str(&alloc::format!("[login] mem free={} pages\n", pages));
        }
        Err(_) => io::print_str("[login] mem free=unavailable\n"),
    }

    // Interactive getty path: loop on bad credentials so a mistype
    // re-prompts instead of exiting. Exiting would make init respawn the
    // getty, and five consecutive exits would exhaust MAX_RESPAWNS and kill
    // the console login until reboot. `failures` tracks the attempt cap:
    // after MAX_FAILED_ATTEMPTS the loop pauses (BACKOFF_NS) but still never
    // exits, so the getty outlives any brute-force or stuck-terminal burst.
    let mut failures: u32 = 0;
    loop {
        let username = match fixed_user.as_deref() {
            Some(u) => u.to_string(),
            None => {
                io::print_str("login: ");
                // The username echoes (unlike the password below): set ECHO
                // explicitly instead of relying on the kernel's 0xB default.
                ensure_echo(0);
                let name_bytes = match read_line(0) {
                    Ok(Some(b)) => b,
                    Ok(None) | Err(_) => libsarga::process::exit(1),
                };
                if name_bytes.is_empty() {
                    // Bare Enter: re-prompt instead of exiting, so a mistype
                    // can't burn an init respawn (real EOF returns Ok(None)).
                    continue;
                }
                core::str::from_utf8(&name_bytes)
                    .unwrap_or("root")
                    .to_string()
            }
        };

        let (uid, gid, _home, _shell) = match lookup_user(&username) {
            Some(v) => v,
            None => {
                io::print_str(
                    "login: unknown user
",
                );
                if fixed_user.is_some() {
                    return 1;
                }
                note_failed_attempt(&mut failures);
                continue;
            }
        };

        io::print_str("Password: ");
        let pw_bytes = match read_password(0) {
            Ok(Some(b)) => b,
            Ok(None) | Err(_) => libsarga::process::exit(1),
        };

        let password = match core::str::from_utf8(&pw_bytes) {
            Ok(s) => s.to_string(),
            Err(_) => {
                io::print_str(
                    "
Invalid password encoding
",
                );
                if fixed_user.is_some() {
                    return 1;
                }
                note_failed_attempt(&mut failures);
                continue;
            }
        };

        if !verify_password(&username, &password) {
            io::print_str(
                "
Login incorrect
",
            );
            if fixed_user.is_some() {
                return 1;
            }
            note_failed_attempt(&mut failures);
            continue;
        }

        io::print_str(
            "
",
        );
        let _ = setuid(uid as u64);
        let _ = setgid(gid as u64);

        let shell_name = core::str::from_utf8(&_shell).unwrap_or("/bin/sash");
        let home_dir = core::str::from_utf8(&_home).unwrap_or("/");

        let env = [
            alloc::format!("HOME={}", home_dir),
            alloc::format!("USER={}", username),
            alloc::format!("LOGNAME={}", username),
            alloc::format!("SHELL={}", shell_name),
            "TERM=xterm-256color".to_string(),
        ];
        let env_refs: Vec<&str> = env
            .iter()
            .map(|s: &alloc::string::String| s.as_str())
            .collect();

        // Pass argv[0] = the shell name: an empty argv makes argv/opt scans
        // misbehave (init's spawn comment documents the same fix). execve
        // only returns on failure — behave like the old exit path.
        let _ = execve(shell_name, &[shell_name], &env_refs);
        return 1;
    }
}
sarga_main!(user_main);
