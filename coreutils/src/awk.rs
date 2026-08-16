#![no_std]
#![no_main]
extern crate alloc;
use libsarga::args;
use libsarga::io;
use libsarga::sarga_main;

fn user_main() -> i32 {
    let mut action = "";
    let mut pattern = "";
    let mut fs = ' ';
    let mut i = 1;
    while i < args::argc() {
        let arg = args::get(i as usize).unwrap_or("");
        if let Some(stripped) = arg.strip_prefix("-F") {
            let f = if arg.len() > 2 {
                stripped
            } else {
                i += 1;
                args::get(i as usize).unwrap_or("")
            };
            fs = f.as_bytes()[0] as char;
        } else if action.is_empty() {
            if let Some((p, a)) = arg.split_once('{') {
                pattern = p.trim();
                action = a.trim_end_matches('}');
            } else {
                action = arg;
            }
        }
        i += 1;
    }
    if action.is_empty() {
        io::print_str("Usage: awk 'pattern { action }'\n");
        return 0;
    }
    let data = libsarga::io::read_stdin_all_bytes();
    let text = alloc::string::String::from_utf8_lossy(&data);
    let mut line_num: u64 = 0;
    for line in text.lines() {
        line_num += 1;
        let fields: alloc::vec::Vec<&str> = line.split(fs).collect();
        let matched = pattern.is_empty() || line.contains(pattern);
        if matched {
            if action.contains("print") || action.contains("print $0") {
                io::print_str(line);
                io::print_str("\n");
            } else if action.starts_with("print $") {
                for part in action.split(';') {
                    let part = part.trim();
                    if part.starts_with("print ") {
                        let field_spec = part.trim_start_matches("print ");
                        if let Ok(n) = field_spec.trim().parse::<usize>() {
                            if n > 0 && n <= fields.len() {
                                io::print_str(fields[n - 1]);
                                io::print_str("\n");
                            }
                        } else {
                            io::print_str(line);
                            io::print_str("\n");
                        }
                    }
                }
            } else if action.contains("NR") {
                io::print_str(&alloc::format!("{}\n", line_num));
            } else {
                io::print_str(line);
                io::print_str("\n");
            }
        }
    }
    0
}

sarga_main!(user_main);
