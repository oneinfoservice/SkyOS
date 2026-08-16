#![no_std]
#![no_main]
extern crate alloc;
use libsarga::args;
use libsarga::io;
use libsarga::sarga_main;

fn user_main() -> i32 {
    let mut count = false;
    let mut i = 1;
    while i < args::argc() {
        let arg = args::get(i as usize).unwrap_or("");
        if arg == "-c" {
            count = true;
        }
        i += 1;
    }
    let mut prev = alloc::string::String::new();
    let mut dup_count: u64 = 0;
    let buf = libsarga::io::read_stdin_all_bytes();
    let text = alloc::string::String::from_utf8_lossy(&buf);
    for line in text.lines() {
        if line == prev {
            dup_count += 1;
        } else {
            if !prev.is_empty() {
                if count {
                    io::print_str(&alloc::format!("{} {}\n", dup_count, prev));
                } else {
                    io::print_str(&alloc::format!("{}\n", prev));
                }
            }
            prev.clear();
            prev.push_str(line);
            dup_count = 1;
        }
    }
    if !prev.is_empty() {
        if count {
            io::print_str(&alloc::format!("{} {}\n", dup_count, prev));
        } else {
            io::print_str(&alloc::format!("{}\n", prev));
        }
    }
    0
}

sarga_main!(user_main);
