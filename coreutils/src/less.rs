#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::String;
use alloc::string::ToString;
use libsarga::{args, io, println, sarga_main};

fn user_main() -> i32 {
    let lines: alloc::vec::Vec<String> = if args::argc() > 1 {
        let path = args::get(1).unwrap();
        match io::read_to_string(path) {
            Ok(s) => s.lines().map(|l| l.to_string()).collect(),
            Err(_) => {
                println!("less: {}: No such file", path);
                return 1;
            }
        }
    } else {
        let all = String::from_utf8_lossy(&libsarga::io::read_stdin_all_bytes()).into_owned();
        all.lines().map(|l| l.to_string()).collect()
    };

    let mut pos = 0usize;
    let rows = 24;
    loop {
        let end = core::cmp::min(pos + rows, lines.len());
        for line in lines.iter().take(end).skip(pos) {
            println!("{}", line);
        }
        pos = end;
        if pos >= lines.len() {
            break;
        }
        let mut k = [0u8; 1];
        let n = io::read(0, &mut k).unwrap_or(0);
        if n == 0 {
            break;
        }
        match k[0] {
            b'q' | b'Q' => break,
            b'u' | b'U' => pos = pos.saturating_sub(rows * 2),
            _ => {}
        }
    }
    0
}
sarga_main!(user_main);
