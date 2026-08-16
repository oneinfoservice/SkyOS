#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::num::Wrapping;
use libsarga::{args, io, println, sarga_main};

struct XorShift {
    state: Wrapping<u64>,
}
impl XorShift {
    fn new(seed: u64) -> Self {
        XorShift {
            state: Wrapping(seed),
        }
    }
    fn next(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.0.wrapping_mul(0x2545F4914F6CDD1Du64)
    }
    fn range(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        (self.next() as usize) % max
    }
}

fn entropy_seed() -> u64 {
    if let Ok(fd) = io::open("/dev/urandom", 0) {
        let mut buf = [0u8; 8];
        if io::read(fd, &mut buf).is_ok() {
            let _ = io::close(fd);
            return u64::from_le_bytes(buf);
        }
        let _ = io::close(fd);
    }
    let clock = unsafe { libsarga::syscall::syscall1(96, 0) } as u64;
    0x9E37_79B9_7F4A_7C15 ^ clock
}

fn user_main() -> i32 {
    let lines: Vec<alloc::string::String> = if args::argc() > 1 {
        match io::read_to_string(args::get(1).unwrap()) {
            Ok(s) => s.lines().map(|l| l.to_string()).collect(),
            Err(_) => {
                println!("shuf: error");
                return 1;
            }
        }
    } else {
        let all = alloc::string::String::from_utf8_lossy(&libsarga::io::read_stdin_all_bytes())
            .into_owned();
        all.lines().map(|l| l.to_string()).collect()
    };

    let mut rng = XorShift::new(entropy_seed() | 1);
    let mut shuffled = lines.clone();
    let n = shuffled.len();
    for i in (1..n).rev() {
        let j = rng.range(i + 1);
        shuffled.swap(i, j);
    }
    for line in &shuffled {
        println!("{}", line);
    }
    0
}
sarga_main!(user_main);
