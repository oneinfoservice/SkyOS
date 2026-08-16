#![no_std]
#![no_main]

extern crate alloc;
extern crate libsarga;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsarga::errno::Error;
use libsarga::io::{self, close, open, read_to_end};
use libsarga::process::geteuid;
use libsarga::sarga_main;
use libsarga::tty::read_password;

fn hex_encode(bytes: &[u8]) -> Vec<u8> {
    hex::encode(bytes).into_bytes()
}

fn generate_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];

    // Try to read from /dev/urandom if available (best entropy source)
    if let Ok(fd) = libsarga::io::open("/dev/urandom", 0) {
        let mut buf = [0u8; 16];
        if libsarga::io::read(fd, &mut buf).is_ok() {
            let _ = libsarga::io::close(fd);
            return buf;
        }
        let _ = libsarga::io::close(fd);
    }

    // Fallback: use clock-based entropy (better than fixed constant)
    // NOTE: This is not cryptographically secure. A proper getrandom syscall should be added.
    let pid = libsarga::process::getpid();
    let time = libsarga::io::clock_gettime(0).unwrap_or((0, 0));

    let mut seed = pid
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add(time.0 as u64)
        .wrapping_add(time.1 as u64);

    for s in &mut salt {
        seed = seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB);
        *s = (seed >> 8) as u8;
    }

    salt
}

fn set_password(username: &str, new_password: &str) -> Result<(), Error> {
    let data = read_to_end("/etc/shadow")?;

    let salt = generate_salt();
    let pw = new_password.as_bytes();
    let mut dk = [0u8; 32];
    libsarga::hash::pbkdf2_sha256(pw, &salt, &mut dk, 10000)?;

    let salt_enc = hex_encode(&salt);
    let dk_enc = hex_encode(&dk);
    let salt_hex = core::str::from_utf8(&salt_enc).unwrap_or("");
    let dk_hex = core::str::from_utf8(&dk_enc).unwrap_or("");
    let new_line = alloc::format!(
        "{}:PBKDF2-{}:{}:10000:0:99999:7:::\n",
        username,
        salt_hex,
        dk_hex
    );

    let mut out = Vec::new();
    for line in data.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, |&b| b == b':');
        let name = parts.next().unwrap_or(b"");
        if name == username.as_bytes() {
            out.extend_from_slice(new_line.as_bytes());
        } else {
            out.extend_from_slice(line);
            out.push(b'\n');
        }
    }

    let fd = open("/etc/shadow", 0x41)?;
    libsarga::io::write_all(fd, &out)?;
    let _ = close(fd);
    Ok(())
}

fn user_main() -> i32 {
    let argc = libsarga::args::argc();
    let euid = geteuid();

    let target_user = if argc > 1 {
        libsarga::args::get(1).unwrap_or("").to_string()
    } else {
        "root".to_string()
    };

    if target_user.is_empty() || target_user == "-h" || target_user == "--help" {
        io::print_str("Usage: passwd [username]\n");
        return 0;
    }

    if euid != 0 {
        io::print_str("passwd: only root can change passwords\n");
        return 1;
    }

    io::print_str("New password: ");
    let pw1 = match read_password(0) {
        Ok(Some(b)) => core::str::from_utf8(&b).unwrap_or("").to_string(),
        Ok(None) => String::new(),
        Err(_) => libsarga::process::exit(1),
    };
    io::print_str("Retype new password: ");
    let pw2 = match read_password(0) {
        Ok(Some(b)) => core::str::from_utf8(&b).unwrap_or("").to_string(),
        Ok(None) => String::new(),
        Err(_) => libsarga::process::exit(1),
    };
    if pw1 != pw2 {
        io::print_str("\npasswd: passwords do not match\n");
        return 1;
    }
    if pw1.is_empty() {
        io::print_str("\npasswd: password cannot be empty\n");
        return 1;
    }

    match set_password(&target_user, &pw1) {
        Ok(_) => io::print_str("passwd: password updated successfully\n"),
        Err(e) => {
            io::print_str(&alloc::format!("passwd: update failed: {}\n", e));
            return 1;
        }
    }
    0
}

sarga_main!(user_main);
