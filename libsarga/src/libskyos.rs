use crate::io;
use crate::process;
use crate::syscall::{syscall1, SYS_UNAME};
use alloc::string::String;
use alloc::vec::Vec;

/// System information structure
pub struct SysInfo {
    pub total_ram_pages: u64,
    pub free_ram_pages: u64,
    pub uptime_seconds: u64,
    pub process_count: u64,
    pub load_avg_1m: u64,
}

/// Get system information via SYS_SYSINFO syscall
pub fn sysinfo() -> Option<SysInfo> {
    let mut buf = [0u64; 5];
    let ret = unsafe {
        syscall1(
            203, // SYS_SYSINFO
            buf.as_mut_ptr() as u64,
        )
    };
    if ret != 0 {
        return None;
    }
    Some(SysInfo {
        total_ram_pages: buf[0],
        free_ram_pages: buf[1],
        uptime_seconds: buf[2],
        process_count: buf[3],
        load_avg_1m: buf[4],
    })
}

/// Get current working directory
pub fn getcwd() -> Option<String> {
    let mut buf = [0u8; 4096];
    let ret = io::getcwd(&mut buf);
    match ret {
        Ok(n) if n > 0 => {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(n);
            Some(String::from_utf8_lossy(&buf[..len]).into_owned())
        }
        _ => None,
    }
}

/// Change current working directory
pub fn chdir(path: &str) -> bool {
    io::chdir(path).is_ok()
}

/// List directory entries
pub fn list_dir(path: &str) -> Option<Vec<String>> {
    let fd = match io::open(path, 0) {
        Ok(fd) => fd,
        Err(_) => return None,
    };

    let mut entries = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match io::getdents64(fd, &mut buf) {
            Ok(n) if n > 0 => n,
            _ => break,
        };
        let mut off = 0;
        while off < n {
            if off + 18 > n {
                break;
            }
            let d_ino = u64::from_ne_bytes(buf[off..off + 8].try_into().unwrap());
            let d_reclen = u16::from_ne_bytes(buf[off + 16..off + 18].try_into().unwrap()) as usize;
            let name_start = off + 19;
            let name_end = buf[name_start..]
                .iter()
                .position(|&c| c == 0)
                .map(|p| name_start + p)
                .unwrap_or(off + d_reclen);
            if d_ino != 0 {
                let name = String::from_utf8_lossy(&buf[name_start..name_end]).into_owned();
                if name != "." && name != ".." {
                    entries.push(name);
                }
            }
            off += d_reclen;
            if d_reclen == 0 {
                break;
            }
        }
    }
    let _ = io::close(fd);
    Some(entries)
}

/// Get the process ID
pub fn getpid() -> u64 {
    process::getpid()
}

/// Sleep for a given number of milliseconds
pub fn sleep_ms(ms: u64) {
    let _ = io::nanosleep(ms * 1_000_000);
}

pub mod net_ext;

pub fn hostname() -> Option<String> {
    let mut buf = [0u8; 256 * 6]; // utsname is 6 fields of 65 or 256 bytes
    let ret = unsafe { syscall1(SYS_UNAME, buf.as_mut_ptr() as u64) };
    if ret != 0 {
        return None;
    }
    // utsname.nodename is usually second field. offset 65 if field size 65
    // Let's assume 65 for now as per common practice in small kernels
    let offset = 65;
    let len = buf[offset..].iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf8_lossy(&buf[offset..offset + len]).into_owned())
}
