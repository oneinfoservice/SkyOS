#![no_std]
#![no_main]
extern crate alloc;
use libsarga::args;
use libsarga::io;
use libsarga::sarga_main;

fn user_main() -> i32 {
    let mut delim = b'\t';
    let mut fields: alloc::vec::Vec<(usize, usize)> = alloc::vec::Vec::new();
    let mut i = 1;
    while i < args::argc() {
        let arg = args::get(i as usize).unwrap_or("");
        if let Some(stripped) = arg.strip_prefix("-d") {
            let d = if arg.len() > 2 {
                stripped
            } else {
                i += 1;
                args::get(i as usize).unwrap_or("")
            };
            delim = d.as_bytes()[0];
        } else if let Some(stripped) = arg.strip_prefix("-f") {
            let f = if arg.len() > 2 {
                stripped
            } else {
                i += 1;
                args::get(i as usize).unwrap_or("")
            };
            for part in f.split(',') {
                if let Some((a, b)) = part.split_once('-') {
                    if let (Ok(s), Ok(e)) = (a.parse::<usize>(), b.parse::<usize>()) {
                        fields.push((s, e));
                    }
                } else if let Ok(n) = part.parse::<usize>() {
                    fields.push((n, n));
                }
            }
        }
        i += 1;
    }
    if fields.is_empty() {
        io::print_str("Usage: cut -d<delim> -f<fields>\n");
        return 0;
    }
    let data = libsarga::io::read_stdin_all_bytes();
    let text = alloc::string::String::from_utf8_lossy(&data);
    for line in text.lines() {
        let parts: alloc::vec::Vec<&str> = line.split(delim as char).collect();
        let mut first = true;
        for &(start, end) in &fields {
            if start >= 1 && start <= parts.len() {
                let s = start - 1;
                let e = core::cmp::min(end, parts.len());
                if !first {
                    let mut db = [0u8; 1];
                    db[0] = delim;
                    let _ = io::write(1, &db);
                }
                first = false;
                for p in &parts[s..e] {
                    let _ = io::write(1, p.as_bytes());
                }
            }
        }
        let _ = io::write(1, b"\n");
    }
    0
}

sarga_main!(user_main);
