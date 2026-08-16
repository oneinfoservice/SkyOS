#![no_std]
#![no_main]
extern crate alloc;
use libsarga::io;
use libsarga::sarga_main;

fn user_main() -> i32 {
    let data = libsarga::io::read_stdin_all_bytes();
    let text = alloc::string::String::from_utf8_lossy(&data);
    let mut line_num: u64 = 1;
    for line in text.lines() {
        io::print_str(&alloc::format!("{:>6}\t{}\n", line_num, line));
        line_num += 1;
    }
    0
}

sarga_main!(user_main);
