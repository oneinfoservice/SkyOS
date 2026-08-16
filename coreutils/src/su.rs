#![no_std]
#![no_main]

extern crate alloc;
extern crate libsarga;

use alloc::string::ToString;
use libsarga::io::{self, read_to_end};
use libsarga::process::{execve, geteuid, setgid, setuid};
use libsarga::sarga_main;
use libsarga::tty::read_password;

fn lookup_user(username: &str) -> Option<(u32, u32, alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> {
    let data = read_to_end("/etc/passwd").ok()?;
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
    let data = match read_to_end("/etc/shadow") {
        Ok(d) => d,
        Err(_) => return false,
    };
    libsarga::hash::verify_password(&data, username, password)
}

fn user_main() -> i32 {
    let argc = libsarga::args::argc();

    if argc < 2 {
        io::print_str("Usage: su [username]\n");
        return 0;
    }

    let target_user = libsarga::args::get(1).unwrap_or("root");
    let (uid, gid, home, shell) = match lookup_user(target_user) {
        Some(v) => v,
        None => {
            io::print_str(&alloc::format!("su: unknown user: {}\n", target_user));
            return 1;
        }
    };

    let euid = geteuid();
    if euid != 0 {
        io::print_str("Password: ");
        // Hidden input: the console tty must not echo the password back
        // onto the wire/log. Shared libsarga::tty::read_password
        // (echo_off -> read -> echo_on, restoring on every outcome).
        let pw_bytes = match read_password(0) {
            Ok(Some(b)) => b,
            Ok(None) | Err(_) => libsarga::process::exit(1),
        };
        let password = core::str::from_utf8(&pw_bytes).unwrap_or("");
        if !verify_password(target_user, password) {
            io::print_str("\nsu: incorrect password\n");
            return 1;
        }
        io::print_str("\n");
    }

    let _ = setgid(gid as u64);
    let _ = setuid(uid as u64);

    let shell_name = core::str::from_utf8(&shell).unwrap_or("/bin/sash");
    let home_dir = core::str::from_utf8(&home).unwrap_or("/");

    let env = [
        alloc::format!("HOME={}", home_dir),
        alloc::format!("USER={}", target_user),
        alloc::format!("LOGNAME={}", target_user),
        alloc::format!("SHELL={}", shell_name),
        "TERM=xterm-256color".to_string(),
    ];
    let env_refs: alloc::vec::Vec<&str> = env
        .iter()
        .map(|s: &alloc::string::String| s.as_str())
        .collect();

    let _ = execve(shell_name, &[], &env_refs);
    1
}

sarga_main!(user_main);
