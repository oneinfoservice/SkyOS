#![no_std]
#![no_main]
extern crate alloc;
use libsarga::{args, io, println, sarga_main};

fn user_main() -> i32 {
    let mut width = 80usize;
    let mut files: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    let mut i = 1;
    while i < args::argc() {
        let a = args::get(i as usize).unwrap_or("");
        if a == "-w" {
            i += 1;
            width = args::get(i as usize).unwrap_or("80").parse().unwrap_or(80);
        } else {
            files.push(a);
        }
        i += 1;
    }

    if files.is_empty() {
        files.push("");
    }

    for f in &files {
        let content = if f.is_empty() {
            let all = alloc::string::String::from_utf8_lossy(&libsarga::io::read_stdin_all_bytes())
                .into_owned();
            all
        } else {
            match io::read_to_string(f) {
                Ok(s) => s,
                Err(_) => {
                    println!("fold: {}: No such file", f);
                    return 1;
                }
            }
        };
        for line in content.lines() {
            let mut pos = 0;
            let bytes = line.as_bytes();
            while pos < bytes.len() {
                let end = core::cmp::min(pos + width, bytes.len());
                let chunk = core::str::from_utf8(&bytes[pos..end]).unwrap_or("");
                println!("{}", chunk);
                pos = end;
            }
        }
    }
    0
}
sarga_main!(user_main);
