#![no_std]
#![no_main]
extern crate alloc;
use libsarga::args;
use libsarga::io;
use libsarga::sarga_main;

fn user_main() -> i32 {
    let mut delete = false;
    let mut from = "";
    let mut to = "";
    let mut i = 1;
    while i < args::argc() {
        let arg = args::get(i as usize).unwrap_or("");
        if arg == "-d" {
            delete = true;
        } else if from.is_empty() {
            from = arg;
        } else if to.is_empty() {
            to = arg;
        }
        i += 1;
    }
    if from.is_empty() {
        io::print_str("Usage: tr [-d] <from> <to>\n");
        return 0;
    }
    let data = libsarga::io::read_stdin_all_bytes();
    let mut map = alloc::collections::BTreeMap::new();
    let from_chars: alloc::vec::Vec<char> = from.chars().collect();
    let to_chars: alloc::vec::Vec<char> = to.chars().collect();
    let fill = *to_chars.last().unwrap_or(&'\0');
    for (idx, &fc) in from_chars.iter().enumerate() {
        let tc = if idx < to_chars.len() {
            to_chars[idx]
        } else {
            fill
        };
        map.insert(fc, tc);
    }
    let text = alloc::string::String::from_utf8_lossy(&data);
    for ch in text.chars() {
        if delete {
            if !map.contains_key(&ch) {
                let mut b = [0u8; 4];
                let s = ch.encode_utf8(&mut b);
                let _ = io::write(1, s.as_bytes());
            }
        } else {
            let out = map.get(&ch).unwrap_or(&ch);
            let mut b = [0u8; 4];
            let s = out.encode_utf8(&mut b);
            let _ = io::write(1, s.as_bytes());
        }
    }
    0
}

sarga_main!(user_main);
