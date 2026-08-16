#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use libsarga::{args, io, print, println, sarga_main};

fn grep_lines(
    lines: &[&str],
    pattern: &str,
    show_filename: bool,
    filename: &str,
    case_insensitive: bool,
    invert: bool,
    line_num: bool,
) {
    for (idx, line) in lines.iter().enumerate() {
        let matched = if case_insensitive {
            line.to_lowercase().contains(&pattern.to_lowercase())
        } else {
            line.contains(pattern)
        };

        let show = matched ^ invert;
        if show {
            if show_filename {
                print!("{}:", filename);
            }
            if line_num {
                print!("{}:", idx + 1);
            }
            println!("{}", line);
        }
    }
}

fn grep_recursive(dir: &str, pattern: &str, case_insensitive: bool, invert: bool, line_num: bool) {
    let fd = match io::open(dir, 0) {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut buf = [0u8; 4096];
    loop {
        match libsarga::io::getdents64(fd, &mut buf) {
            Ok(n) if n > 0 => {
                let mut offset = 0;
                while offset < n {
                    let reclen = u16::from_ne_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
                    let name_len = buf[offset + 18] as usize;
                    if name_len > 0 {
                        let name_bytes = &buf[offset + 19..offset + 19 + name_len];
                        let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_len);
                        if let Ok(name) = core::str::from_utf8(&name_bytes[..name_end]) {
                            if name != "." && name != ".." {
                                let full_path = if dir == "/" {
                                    alloc::format!("/{}", name)
                                } else {
                                    alloc::format!("{}/{}", dir, name)
                                };
                                let mut st = [0u64; 32];
                                if unsafe {
                                    libsarga::syscall::syscall2(
                                        4,
                                        full_path.as_ptr() as u64,
                                        st.as_mut_ptr() as u64,
                                    )
                                } == 0
                                {
                                    if (st[1] & 0o170000) == 0o040000 {
                                        grep_recursive(
                                            &full_path,
                                            pattern,
                                            case_insensitive,
                                            invert,
                                            line_num,
                                        );
                                    } else {
                                        if let Ok(text) = io::read_to_string(&full_path) {
                                            let lines: Vec<&str> = text.lines().collect();
                                            grep_lines(
                                                &lines,
                                                pattern,
                                                true,
                                                &full_path,
                                                case_insensitive,
                                                invert,
                                                line_num,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    offset += reclen;
                }
            }
            _ => break,
        }
    }
    let _ = io::close(fd);
}

fn user_main() -> i32 {
    let mut i = 1;
    let mut recursive = false;
    let mut case_insensitive = false;
    let mut invert = false;
    let mut line_num = false;
    let mut files = Vec::new();
    let mut pattern = String::new();

    while i < args::argc() {
        if let Some(s) = args::get(i as usize) {
            if s == "-r" || s == "-R" {
                recursive = true;
            } else if s == "-i" {
                case_insensitive = true;
            } else if s == "-v" {
                invert = true;
            } else if s == "-n" {
                line_num = true;
            } else if s.starts_with('-') {
            } else if pattern.is_empty() {
                pattern = String::from(s);
            } else {
                files.push(String::from(s));
            }
        }
        i += 1;
    }

    if pattern.is_empty() {
        println!("Usage: grep [-r] [-i] [-v] [-n] <pattern> [file...]");
        return 1;
    }

    if files.is_empty() {
        if recursive {
            grep_recursive(".", &pattern, case_insensitive, invert, line_num);
        } else {
            let text = String::from_utf8_lossy(&libsarga::io::read_stdin_all_bytes()).into_owned();
            let lines: Vec<&str> = text.lines().collect();
            grep_lines(
                &lines,
                &pattern,
                false,
                "stdin",
                case_insensitive,
                invert,
                line_num,
            );
        }
    } else {
        let show_filename = files.len() > 1 || recursive;
        for file in &files {
            let mut st = [0u64; 32];
            let is_dir = unsafe {
                libsarga::syscall::syscall2(4, file.as_ptr() as u64, st.as_mut_ptr() as u64)
            } == 0
                && (st[1] & 0o170000) == 0o040000;
            if is_dir {
                if recursive {
                    grep_recursive(file, &pattern, case_insensitive, invert, line_num);
                } else {
                    println!("grep: {}: Is a directory", file);
                }
            } else {
                if let Ok(text) = io::read_to_string(file) {
                    let lines: Vec<&str> = text.lines().collect();
                    grep_lines(
                        &lines,
                        &pattern,
                        show_filename,
                        file,
                        case_insensitive,
                        invert,
                        line_num,
                    );
                } else {
                    println!("grep: {}: not found", file);
                }
            }
        }
    }
    0
}
sarga_main!(user_main);
