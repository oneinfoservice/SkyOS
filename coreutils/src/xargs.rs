#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use libsarga::{args, println, sarga_main, syscall};

fn user_main() -> i32 {
    if args::argc() < 2 {
        println!("Usage: xargs <command> [args...]");
        return 0;
    }
    let mut cmd_args = Vec::new();
    for i in 1..args::argc() {
        if let Some(s) = args::get(i as usize) {
            cmd_args.push(String::from(s));
        }
    }
    let input = String::from_utf8_lossy(&libsarga::io::read_stdin_all_bytes()).into_owned();
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut child_args = cmd_args.clone();
        child_args.push(String::from(trimmed));
        let pid = unsafe { syscall::syscall0(57) };
        if pid == 0 {
            let mut arg_ptrs = alloc::vec::Vec::new();
            for a in &child_args {
                arg_ptrs.push(a.as_ptr() as u64);
            }
            arg_ptrs.push(0);
            unsafe {
                syscall::syscall1(59, arg_ptrs.as_ptr() as u64);
            }
            unsafe {
                syscall::syscall1(60, 0);
            }
        } else {
            unsafe {
                syscall::syscall2(61, pid as u64, 0u64);
            }
        }
    }
    0
}

sarga_main!(user_main);
