#![no_std]
#![no_main]
extern crate alloc;
use libsarga::io;
use libsarga::sarga_main;

fn user_main() -> i32 {
    let data = libsarga::io::read_stdin_all_bytes();
    let text = alloc::string::String::from_utf8_lossy(&data);
    let mut lines: alloc::vec::Vec<&str> = text.lines().collect();
    while let Some(line) = lines.pop() {
        let _ = io::write(1, line.as_bytes());
        let _ = io::write(1, b"\n");
    }
    0
}

sarga_main!(user_main);
