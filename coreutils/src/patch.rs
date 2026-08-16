#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::String;
use libsarga::args;
use libsarga::io;
use libsarga::sarga_main;

fn read_file(path: &str) -> alloc::string::String {
    let fd = unsafe { libsarga::syscall::syscall2(2, path.as_ptr() as u64, 0) };
    if fd < 0 {
        return String::new();
    }
    let mut data = alloc::vec::Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { libsarga::syscall::syscall3(0, fd as u64, buf.as_mut_ptr() as u64, 4096) };
        if n <= 0 {
            break;
        }
        data.extend_from_slice(&buf[..n as usize]);
    }
    let _ = unsafe { libsarga::syscall::syscall1(3, fd as u64) };
    alloc::string::String::from_utf8_lossy(&data).into_owned()
}

fn write_file(path: &str, content: &str) -> bool {
    let flags = 0x241u64 | 0x200u64;
    let fd = unsafe {
        libsarga::syscall::syscall3(2, path.as_ptr() as u64, content.as_ptr() as u64, flags)
    };
    if fd < 0 {
        return false;
    }
    let n = unsafe {
        libsarga::syscall::syscall3(1, fd as u64, content.as_ptr() as u64, content.len() as u64)
    };
    let _ = unsafe { libsarga::syscall::syscall1(3, fd as u64) };
    n == content.len() as i64
}

fn user_main() -> i32 {
    let mut file = "";
    let mut i = 1;
    while i < args::argc() {
        let arg = args::get(i as usize).unwrap_or("");
        if arg == "-p1" || arg == "--strip=1" { /* skip */
        } else {
            file = arg;
        }
        i += 1;
    }
    let diff = if file.is_empty() {
        alloc::string::String::from_utf8_lossy(&libsarga::io::read_stdin_all_bytes()).into_owned()
    } else {
        read_file(file)
    };
    let mut current_file = alloc::string::String::new();
    let mut removes: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
    for line in diff.lines() {
        if let Some(name) = line.strip_prefix("--- ") {
            if let Some((n, _)) = name.split_once('\t') {
                current_file = alloc::string::String::from(n);
            }
        } else if line.starts_with("--- /dev/null") {
            if let Some(next) = diff.lines().find(|l| l.starts_with("+++ ")) {
                current_file = alloc::string::String::from(&next[4..]);
            }
        } else if let Some(name) = line.strip_prefix("+++ ") {
            if name.starts_with("/dev/null") {
                continue;
            }
            if current_file.is_empty() {
                current_file = alloc::string::String::from(name);
            }
        } else if line.starts_with('-') && !line.starts_with("---") {
            removes.push(alloc::string::String::from(&line[1..]));
        }
    }
    if !current_file.is_empty() {
        let content = read_file(&current_file);
        let mut new_content = alloc::string::String::new();
        for line in content.lines() {
            let mut skip = false;
            for r in &removes {
                if line == r.as_str() {
                    skip = true;
                    io::print_str(&alloc::format!("Removing: {}\n", line));
                    break;
                }
            }
            if !skip {
                new_content.push_str(line);
                new_content.push('\n');
            }
        }
        if write_file(&current_file, &new_content) {
            io::print_str(&alloc::format!("Patched {}\n", current_file));
        } else {
            io::print_str(&alloc::format!("Failed to write {}\n", current_file));
        }
    }
    0
}

sarga_main!(user_main);
