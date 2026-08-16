#![no_std]
#![no_main]
extern crate alloc;
use alloc::vec::Vec;
use libsarga::{args, io, sarga_main};

fn user_main() -> i32 {
    let mut count: usize = 10;
    let mut file: Option<&str> = None;
    let argc = args::argc() as usize;
    let mut i = 1usize;
    while i < argc {
        let s = args::get(i).unwrap_or("");
        if s == "-n" {
            i += 1;
            count = args::get(i).unwrap_or("10").parse().unwrap_or(10);
        } else if !s.starts_with('-') {
            file = Some(s);
        }
        i += 1;
    }

    let mut buf = [0u8; 4096];
    let mut all = Vec::new();
    if let Some(path) = file {
        let fd = match io::open(path, 0) {
            Ok(fd) => fd,
            Err(_) => {
                io::print_str("tail: ");
                io::print_str(path);
                io::print_str(": No such file\n");
                return 1;
            }
        };
        loop {
            let n = match io::read(fd, &mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            all.extend_from_slice(&buf[..n]);
        }
        io::close(fd).ok();
    } else {
        all.extend_from_slice(&libsarga::io::read_stdin_all_bytes());
    }

    let s = core::str::from_utf8(&all).unwrap_or("");
    let lines: Vec<&str> = s.split('\n').collect();
    let start = if lines.len() > count {
        lines.len() - count
    } else {
        0
    };
    for line in &lines[start..] {
        io::write_all(1, line.as_bytes()).ok();
        io::write_all(1, b"\n").ok();
    }
    0
}

sarga_main!(user_main);
