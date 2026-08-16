#![no_std]
#![no_main]
extern crate alloc;
use libsarga::{args, io, println, sarga_main};

fn sysv_sum(data: &[u8]) -> (u16, usize) {
    let mut s: u32 = 0;
    for &b in data {
        s = (s + b as u32) & 0xFFFF;
    }
    let blocks = data.len().div_ceil(512);
    (s as u16, blocks)
}

fn user_main() -> i32 {
    if args::argc() > 1 {
        let path = args::get(1).unwrap();
        match io::read_to_end(path) {
            Ok(s) => {
                let (cksum, blocks) = sysv_sum(&s);
                println!("{} {} {}", cksum, blocks, path);
            }
            Err(_) => {
                println!("sum: {}: No such file", path);
                return 1;
            }
        }
    } else {
        let all = libsarga::io::read_stdin_all_bytes();
        let (cksum, blocks) = sysv_sum(&all);
        println!("{} {}", cksum, blocks);
    }
    0
}
sarga_main!(user_main);
