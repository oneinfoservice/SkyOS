#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::String;
use core::fmt::Write;
use libsarga::{args, io, println, sarga_main};
use md5::{Digest, Md5};

fn md5(data: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest);
    out
}

fn user_main() -> i32 {
    if args::argc() < 2 {
        let all = libsarga::io::read_stdin_all_bytes();
        let hash = md5(&all);
        let mut s = String::new();
        for &b in &hash {
            let _ = write!(s, "{:02x}", b);
        }
        println!("{}  -", s);
        return 0;
    }
    for i in 1..args::argc() as usize {
        let path = args::get(i).unwrap_or("");
        match io::read_to_end(path) {
            Ok(content) => {
                let hash = md5(&content);
                let mut s = String::new();
                for &b in &hash {
                    let _ = write!(s, "{:02x}", b);
                }
                println!("{}  {}", s, path);
            }
            Err(_) => {
                println!("md5sum: {}: No such file", path);
                return 1;
            }
        }
    }
    0
}
sarga_main!(user_main);
