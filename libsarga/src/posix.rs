//! POSIX-like syscall wrappers for libsarga ABI.
//! These provide a C-compatible interface for porting software.

use crate::errno::Error;
use crate::process;
use crate::syscall;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

// File modes
pub const O_RDONLY: i32 = 0;
pub const O_WRONLY: i32 = 1;
pub const O_RDWR: i32 = 2;
pub const O_CREAT: i32 = 0o100;
pub const O_TRUNC: i32 = 0o1000;
pub const O_APPEND: i32 = 0o2000;

// Seek whence
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

// mmap prot
pub const PROT_NONE: i32 = 0;
pub const PROT_READ: i32 = 1;
pub const PROT_WRITE: i32 = 2;
pub const PROT_EXEC: i32 = 4;

// mmap flags
pub const MAP_SHARED: i32 = 0x01;
pub const MAP_PRIVATE: i32 = 0x02;
pub const MAP_FIXED: i32 = 0x10;
pub const MAP_ANONYMOUS: i32 = 0x20;
pub const MAP_POPULATE: i32 = 0x08000;

// clock ids
pub const CLOCK_MONOTONIC: i64 = 1;
pub const CLOCK_REALTIME: i64 = 0;

// wait options
pub const WNOHANG: i32 = 1;
pub const WUNTRACED: i32 = 2;

// ELF magic
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const SHEBANG: [u8; 2] = *b"#!";

/// Detect if a file starts with a shebang and extract interpreter+arg.
pub fn shebang_interpreter(data: &[u8]) -> Option<(&str, Option<&str>)> {
    if data.len() < 4 {
        return None;
    }
    if data[..2] != SHEBANG {
        return None;
    }
    let line_end = data.iter().position(|&b| b == b'\n').unwrap_or(data.len());
    let line = core::str::from_utf8(&data[2..line_end]).ok()?.trim();
    let mut words = line.split_whitespace();
    let interpreter = words.next()?;
    let arg = words.next();
    Some((interpreter, arg))
}

/// Detect ELF machine type from header bytes.
/// Returns Some(true) if native x86_64, Some(false) if non-native, None if not ELF.
pub fn elf_is_native(data: &[u8]) -> Option<bool> {
    if data.len() < 20 {
        return None;
    }
    if data[..4] != ELF_MAGIC {
        return None;
    }
    // e_ident[4] = class: 1=32-bit, 2=64-bit
    // e_ident[5] = endian: 1=little, 2=big
    // e_machine at offset 18 (little-endian u16)
    let machine = u16::from_le_bytes([data[18], data[19]]);
    // EM_X86_64 = 62
    Some(machine == 62)
}

/// POSIX execvp: search PATH, handle shebang scripts, handle ELF detection.
pub fn execvp(path: &str, args: &[&str], env: &[&str]) -> Result<(), Error> {
    let resolved = if path.contains('/') {
        path.to_string()
    } else {
        match find_in_path(path) {
            Some(p) => p,
            None => {
                // Try with shebang detection on the raw name (search PATH)
                return Err(Error::ENOENT);
            }
        }
    };

    // Try direct execve first (kernel handles ELF natively)
    let r = crate::process::execve(&resolved, args, env);
    if r.is_ok() {
        return r;
    }

    // Read the first few bytes to check for shebang
    let c_path = alloc::ffi::CString::new(resolved.as_bytes()).map_err(|_| Error::EINVAL)?;
    let fd = unsafe { crate::syscall::open(c_path.as_ptr() as *const u8, 0) };
    if fd < 0 {
        return Err(Error::from_i64(fd));
    }
    let mut header = [0u8; 256];
    let n = unsafe { crate::syscall::read(fd, header.as_mut_ptr(), 256) };
    let _ = unsafe { crate::syscall::close(fd) };
    if n <= 0 {
        return Err(Error::ENOEXEC);
    }
    let data = &header[..n as usize];

    // Check for shebang
    if let Some((interp, arg)) = shebang_interpreter(data) {
        let mut interp_args: Vec<&str> = Vec::new();
        interp_args.push(interp);
        if let Some(a) = arg {
            interp_args.push(a);
        }
        interp_args.push(&resolved);
        // Append remaining args after the script name
        for a in args.iter().skip(1) {
            interp_args.push(a);
        }
        return crate::process::execve(interp, &interp_args, env);
    }

    // Check for ELF with non-native machine
    if let Some(true) = elf_is_native(data) {
        // Native ELF but execve failed — maybe permissions? Return original error.
        return r;
    }

    Err(Error::ENOEXEC)
}

fn find_in_path(name: &str) -> Option<String> {
    // Default PATH when environment variables aren't available at lib level
    let path = alloc::string::String::from("/bin:/usr/bin");
    for dir in path.split(':') {
        let full = if dir.ends_with('/') {
            alloc::format!("{}{}", dir, name)
        } else {
            alloc::format!("{}/{}", dir, name)
        };
        let c_str = alloc::ffi::CString::new(full.as_bytes()).ok()?;
        let r = unsafe { crate::syscall::syscall2(4, c_str.as_ptr() as u64, 0) };
        if r == 0 {
            return Some(full);
        }
    }
    None
}

#[repr(C)]
pub struct Timespec {
    pub sec: i64,
    pub nsec: i64,
}

#[repr(C)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_mtime: i64,
    pub st_ctime: i64,
}

/// Open a file. Returns file descriptor or negative errno.
pub fn open(path: &str, flags: i32) -> i64 {
    unsafe { syscall::syscall2(2, path.as_ptr() as u64, flags as u64) }
}

/// Read from a file descriptor.
pub fn read(fd: i64, buf: &mut [u8]) -> i64 {
    unsafe { syscall::syscall3(0, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Write to a file descriptor.
pub fn write(fd: i64, buf: &[u8]) -> i64 {
    unsafe { syscall::syscall3(1, fd as u64, buf.as_ptr() as u64, buf.len() as u64) }
}

/// Close a file descriptor.
pub fn close(fd: i64) -> i64 {
    unsafe { syscall::syscall1(3, fd as u64) }
}

/// Reposition read/write offset.
pub fn lseek(fd: i64, offset: i64, whence: i32) -> i64 {
    unsafe { syscall::syscall3(8, fd as u64, offset as u64, whence as u64) }
}

/// Allocate or map memory.
pub fn mmap(addr: *mut u8, length: usize, prot: i32, flags: i32, fd: i64, offset: i64) -> i64 {
    unsafe {
        syscall::syscall6(
            9,
            addr as u64,
            length as u64,
            prot as u64,
            flags as u64,
            fd as u64,
            offset as u64,
        )
    }
}

/// Unmap memory.
pub fn munmap(addr: *mut u8, length: usize) -> i64 {
    unsafe { syscall::syscall2(11, addr as u64, length as u64) }
}

/// Change data segment size.
pub fn brk(addr: *mut u8) -> i64 {
    unsafe { syscall::syscall1(12, addr as u64) }
}

/// Exit the current process.
pub fn exit(code: i32) -> ! {
    unsafe { syscall::syscall1(60, code as u64) };
    loop {
        core::hint::spin_loop();
    }
}

/// Get current process ID.
pub fn getpid() -> i64 {
    unsafe { syscall::syscall0(39) }
}

/// Get parent process ID.
pub fn getppid() -> i64 {
    unsafe { syscall::syscall0(110) }
}

/// Sleep for specified duration.
pub fn nanosleep(req: &Timespec, _rem: &mut Timespec) -> i64 {
    unsafe { syscall::syscall2(35, req.sec as u64, req.nsec as u64) }
}

/// Yield the CPU.
pub fn sched_yield() -> i64 {
    unsafe { syscall::syscall0(24) }
}

/// Get file metadata.
pub fn fstat(fd: i64, buf: &mut Stat) -> i64 {
    unsafe { syscall::syscall2(5, fd as u64, buf as *mut Stat as u64) }
}

/// Get file metadata by path.
pub fn stat(path: &str, buf: &mut Stat) -> i64 {
    unsafe { syscall::syscall2(4, path.as_ptr() as u64, buf as *mut Stat as u64) }
}

/// Duplicate a file descriptor.
pub fn dup(fd: i64) -> i64 {
    unsafe { syscall::syscall1(32, fd as u64) }
}

/// Duplicate a file descriptor to a specific target.
pub fn dup2(old_fd: i64, new_fd: i64) -> i64 {
    unsafe { syscall::syscall2(33, old_fd as u64, new_fd as u64) }
}

/// Get current working directory.
pub fn getcwd(buf: &mut [u8]) -> i64 {
    unsafe { syscall::syscall2(79, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Change working directory.
pub fn chdir(path: &str) -> i64 {
    unsafe { syscall::syscall1(80, path.as_ptr() as u64) }
}

/// Create a pipe.
pub fn pipe(fds: &mut [i32; 2]) -> i64 {
    unsafe { syscall::syscall1(22, fds.as_mut_ptr() as u64) }
}

/// Wait for a child process.
pub fn wait4(pid: i64, status: &mut i32, options: i32) -> i64 {
    unsafe { syscall::syscall4(61, pid as u64, status as *mut i32 as u64, options as u64, 0) }
}

/// Check file access permissions.
pub fn access(path: &str, mode: i32) -> i64 {
    unsafe { syscall::syscall2(21, path.as_ptr() as u64, mode as u64) }
}

/// Unlink a file.
pub fn unlink(path: &str) -> i64 {
    unsafe { syscall::syscall1(87, path.as_ptr() as u64) }
}

/// Rename a file.
pub fn rename(old: &str, new: &str) -> i64 {
    unsafe { syscall::syscall2(82, old.as_ptr() as u64, new.as_ptr() as u64) }
}

/// Create a directory.
pub fn mkdir(path: &str) -> i64 {
    unsafe { syscall::syscall1(83, path.as_ptr() as u64) }
}

/// Read a symbolic link target.
pub fn readlink(path: &str, buf: &mut [u8]) -> i64 {
    unsafe {
        syscall::syscall3(
            89,
            path.as_ptr() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    }
}

/// Create a symbolic link.
pub fn symlink(target: &str, linkpath: &str) -> i64 {
    unsafe { syscall::syscall2(88, target.as_ptr() as u64, linkpath.as_ptr() as u64) }
}

/// Create a child process via fork.
pub fn fork() -> i64 {
    unsafe { syscall::syscall0(57) }
}

/// Execute a new program.
pub fn execve(path: &str, argv: &[*const u8], envp: &[*const u8]) -> i64 {
    unsafe {
        syscall::syscall3(
            59,
            path.as_ptr() as u64,
            argv.as_ptr() as u64,
            envp.as_ptr() as u64,
        )
    }
}

/// execvpe — search PATH and execute with environment.
pub fn execvpe(file: &str, argv: &[&str], envp: &[&str]) -> Result<(), Error> {
    if file.contains('/') {
        return process::execve(file, argv, envp);
    }
    // Default PATH when envp doesn't have it
    for dir in ["/bin", "/usr/bin"] {
        let full = alloc::format!("{}/{}", dir, file);
        if process::execve(&full, argv, envp).is_ok() {
            return Ok(());
        }
    }
    Err(Error::ENOENT)
}

/// popen — pipe to/from a process (simplified: opens a command for reading).
pub fn popen(command: &str, _mode: &str) -> Result<(i64, u64), Error> {
    let mut fds = [0i32; 2];
    let r = unsafe { syscall::syscall1(22, fds.as_mut_ptr() as u64) };
    if r < 0 {
        return Err(Error::from_i64(r));
    }

    match process::fork() {
        Ok(0) => {
            // Child
            let _ = unsafe { syscall::syscall2(33, fds[1] as u64, 1) }; // dup2 write->stdout
            let _ = unsafe { syscall::syscall1(3, fds[0] as u64) }; // close read
            let _ = unsafe { syscall::syscall1(3, fds[1] as u64) };
            let args: [&str; 3] = ["/bin/sh", "-c", command];
            let _ = process::execve("/bin/sh", &args, &[]);
            process::exit(1);
        }
        Ok(pid) => {
            // Parent
            let _ = unsafe { syscall::syscall1(3, fds[1] as u64) }; // close write
            Ok((fds[0] as i64, pid))
        }
        Err(e) => Err(e),
    }
}

/// pclose — close a popen'd stream and wait for process.
pub fn pclose(fd: i64, pid: u64) -> Result<i32, Error> {
    let _ = unsafe { syscall::syscall1(3, fd as u64) };
    let mut status: i32 = 0;
    let r = unsafe { syscall::syscall4(61, pid, (&mut status as *mut i32) as u64, 0, 0) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(status)
    }
}
