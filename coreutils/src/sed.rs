#![no_std]
#![no_main]
extern crate alloc;
use libsarga::args;
use libsarga::io;
use libsarga::sarga_main;

fn user_main() -> i32 {
    let mut expr = "";
    let mut i = 1;
    while i < args::argc() {
        let arg = args::get(i as usize).unwrap_or("");
        if arg == "-e" || arg == "--expression" {
            i += 1;
            expr = args::get(i as usize).unwrap_or("");
        } else if arg.starts_with('-') && arg.len() > 1 && arg.as_bytes()[1] == b'e' {
            expr = &arg[2..];
        } else {
            expr = arg;
        }
        i += 1;
    }
    if expr.is_empty() {
        io::print_str("Usage: sed 's/pattern/replacement/'\n");
        return 0;
    }
    let data = libsarga::io::read_stdin_all_bytes();
    let text = alloc::string::String::from_utf8_lossy(&data);
    if let Some(rest) = expr.strip_prefix("s/") {
        let parts: alloc::vec::Vec<&str> = rest.split('/').collect();
        let pattern = parts.first().unwrap_or(&"");
        let replacement = parts.get(1).unwrap_or(&"");
        for line in text.lines() {
            if pattern.is_empty() {
                io::print_str(line);
                io::print_str("\n");
            } else {
                let result = if let Some(pos) = line.find(pattern) {
                    let mut out = alloc::string::String::new();
                    out.push_str(&line[..pos]);
                    out.push_str(replacement);
                    out.push_str(&line[pos + pattern.len()..]);
                    out
                } else {
                    alloc::string::String::from(line)
                };
                io::print_str(&result);
                io::print_str("\n");
            }
        }
    } else if expr == "q" {
        if let Some(first) = text.lines().next() {
            io::print_str(first);
            io::print_str("\n");
        }
    } else if expr == "p" {
        for line in text.lines() {
            io::print_str(line);
            io::print_str("\n");
        }
    } else if expr.starts_with("d") {
        return 0;
    } else {
        for line in text.lines() {
            io::print_str(line);
            io::print_str("\n");
        }
    }
    0
}

sarga_main!(user_main);
