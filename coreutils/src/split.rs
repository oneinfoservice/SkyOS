#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::String;
use alloc::string::ToString;
use libsarga::{args, fs, io, println, sarga_main};

fn user_main() -> i32 {
    let mut lines_per_file = 1000usize;
    let mut n = 1;
    if args::argc() >= 3 && args::get(1) == Some("-l") {
        lines_per_file = args::get(2).unwrap_or("1000").parse().unwrap_or(1000);
        n = 3;
    }
    let file = if n >= args::argc() {
        String::new()
    } else {
        args::get(n as usize).unwrap_or("").to_string()
    };

    let content = if file.is_empty() {
        let all = String::from_utf8_lossy(&libsarga::io::read_stdin_all_bytes()).into_owned();
        all
    } else {
        match io::read_to_string(&file) {
            Ok(s) => s,
            Err(_) => {
                println!("split: error");
                return 1;
            }
        }
    };

    let lines: alloc::vec::Vec<&str> = content.lines().collect();
    let mut part = 0;
    let mut pos = 0;
    while pos < lines.len() {
        let end = core::cmp::min(pos + lines_per_file, lines.len());
        let name = alloc::format!("x{:02}", part);
        let mut out = String::new();
        for line in lines.iter().take(end).skip(pos) {
            out.push_str(line);
            out.push('\n');
        }
        let _ = fs::write_file(&name, &out);
        part += 1;
        pos = end;
    }
    0
}
sarga_main!(user_main);
