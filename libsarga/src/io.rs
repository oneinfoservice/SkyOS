//! File and stream I/O operations.

use crate::errno::Error;
use crate::syscall::*;
use alloc::string::String;
use alloc::vec::Vec;

/// Resource limit structure.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Rlimit {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

/// System info structure.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SysInfo {
    pub uptime: u64,
    pub loads: [u64; 3],
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub procs: u16,
    pub _pad: [u8; 22],
}

/// tms structure for times().
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Tms {
    pub tms_utime: u64,
    pub tms_stime: u64,
    pub tms_cutime: u64,
    pub tms_cstime: u64,
}

/// File metadata structure.
#[derive(Debug, Clone, Copy)]
pub struct Stat {
    /// Device ID
    pub dev: u64,
    /// Inode number
    pub ino: u64,
    /// Number of hard links
    pub nlink: u64,
    /// File mode and permissions
    pub mode: u32,
    /// User ID of owner
    pub uid: u32,
    /// Group ID of owner
    pub gid: u32,
    /// Total size in bytes
    pub size: u64,
    /// Block size for filesystem I/O
    pub blksize: u64,
}

impl Stat {
    fn from_bytes(buf: &[u8]) -> Self {
        use core::convert::TryInto;
        let g =
            |o: usize| -> u64 { u64::from_ne_bytes(buf[o..o + 8].try_into().unwrap_or([0; 8])) };
        let w =
            |o: usize| -> u32 { u32::from_ne_bytes(buf[o..o + 4].try_into().unwrap_or([0; 4])) };
        Stat {
            dev: g(0),
            ino: g(8),
            mode: w(16),
            nlink: g(24),
            uid: w(32),
            gid: w(36),
            size: g(48),
            blksize: g(64),
        }
    }
}

/// Opens a file at the given path with specified flags.
pub fn open(path: &str, flags: i32) -> Result<i64, Error> {
    let mut buf = [0u8; 256];
    let bytes = path.as_bytes();
    if bytes.len() > 254 {
        return Err(Error::EINVAL);
    }
    buf[..bytes.len()].copy_from_slice(bytes);
    buf[bytes.len()] = 0;
    // SAFETY: open syscall is safe here
    let r = unsafe { crate::syscall::open(buf.as_ptr(), flags) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r)
    }
}

/// Reads from a file descriptor into a buffer.
pub fn read(fd: i64, buf: &mut [u8]) -> Result<usize, Error> {
    // SAFETY: read syscall is safe here
    let r = unsafe { crate::syscall::read(fd, buf.as_mut_ptr(), buf.len()) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as usize)
    }
}

/// Writes from a buffer to a file descriptor.
pub fn write(fd: i64, buf: &[u8]) -> Result<usize, Error> {
    // SAFETY: write syscall is safe here
    let r = unsafe { crate::syscall::write(fd, buf.as_ptr(), buf.len()) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as usize)
    }
}

/// Writes all bytes in a buffer to a file descriptor, retrying if necessary.
pub fn write_all(fd: i64, mut buf: &[u8]) -> Result<(), Error> {
    while !buf.is_empty() {
        let n = write(fd, buf)?;
        if n == 0 {
            return Err(Error::EIO);
        }
        buf = &buf[n..];
    }
    Ok(())
}

/// Closes a file descriptor.
pub fn close(fd: i64) -> Result<(), Error> {
    // SAFETY: close syscall is safe here
    let r = unsafe { crate::syscall::close(fd) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Retrieves file metadata for a given path.
pub fn stat(path: &str) -> Result<Stat, Error> {
    let mut buf = [0u8; 144];
    let mut path_buf = [0u8; 256];
    let bytes = path.as_bytes();
    if bytes.len() > 254 {
        return Err(Error::EINVAL);
    }
    path_buf[..bytes.len()].copy_from_slice(bytes);
    path_buf[bytes.len()] = 0;
    // SAFETY: stat syscall is safe here
    let r = unsafe {
        crate::syscall::syscall3(
            SYS_STAT,
            path_buf.as_ptr() as u64,
            buf.as_mut_ptr() as u64,
            0,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(Stat::from_bytes(&buf))
    }
}

/// Retrieves file metadata for an open file descriptor.
pub fn fstat(fd: i64, buf: &mut Stat) -> Result<(), Error> {
    let mut raw_buf = [0u8; 144];
    // SAFETY: fstat syscall is safe here
    let r = unsafe { crate::syscall::fstat(fd, raw_buf.as_mut_ptr()) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        *buf = Stat::from_bytes(&raw_buf);
        Ok(())
    }
}

/// Creates a new directory.
pub fn mkdir(path: &str, mode: u32) -> Result<(), Error> {
    let mut buf = [0u8; 256];
    let bytes = path.as_bytes();
    if bytes.len() > 254 {
        return Err(Error::EINVAL);
    }
    buf[..bytes.len()].copy_from_slice(bytes);
    buf[bytes.len()] = 0;
    // SAFETY: mkdir syscall is safe here
    let r = unsafe { crate::syscall::mkdir(buf.as_ptr(), mode) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Deletes a file.
pub fn unlink(path: &str) -> Result<(), Error> {
    let mut buf = [0u8; 256];
    let bytes = path.as_bytes();
    if bytes.len() > 254 {
        return Err(Error::EINVAL);
    }
    buf[..bytes.len()].copy_from_slice(bytes);
    buf[bytes.len()] = 0;
    // SAFETY: unlink syscall is safe here
    let r = unsafe { crate::syscall::unlink(buf.as_ptr()) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Reads an entire file into a String.
pub fn read_to_string(path: &str) -> Result<String, Error> {
    let fd = open(path, 0)?;
    let mut buf = [0u8; 4096];
    let mut result = String::new();
    loop {
        match read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                    result.push_str(s);
                }
            }
            Err(e) => {
                let _ = close(fd);
                return Err(e);
            }
        }
    }
    let _ = close(fd);
    Ok(result)
}

/// Reads an entire file into a byte buffer. Single home for the
/// read-whole-file loop previously duplicated in `login` and `passwd`
/// (each had a private `read_whole_file`); both now call this.
pub fn read_to_end(path: &str) -> Result<Vec<u8>, Error> {
    let fd = open(path, 0)?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        match read(fd, &mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            // Close on the error path too: this is the shared whole-file
            // reader for login/passwd/su, so a mid-file read error must not
            // leak the descriptor (the originals' `?` did).
            Err(e) => {
                let _ = close(fd);
                return Err(e);
            }
        }
    }
    let _ = close(fd);
    Ok(buf)
}

/// Read ALL of stdin (fd 0) until EOF into a raw byte Vec.
///
/// Stops on the first read error and returns the bytes read so far -- the
/// exact semantics of the per-tool stdin loops this consolidates (every
/// caller previously did `Err(_) => break` and used the partial buffer).
/// Returns a plain Vec (not Result) deliberately: the consolidated tools
/// all treated a read error as EOF and never surfaced it.
pub fn read_stdin_all_bytes() -> Vec<u8> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        match read(0, &mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
    buf
}

/// Prints a string to standard output.
pub fn print_str(s: &str) {
    let _ = write_all(1, s.as_bytes());
}

/// Retrieves the current working directory.
pub fn getcwd(buf: &mut [u8]) -> Result<usize, Error> {
    // SAFETY: getcwd syscall is safe here
    let r =
        unsafe { crate::syscall::syscall2(SYS_GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as usize)
    }
}

/// Changes the current working directory.
pub fn chdir(path: &str) -> Result<(), Error> {
    let mut buf = [0u8; 256];
    let bytes = path.as_bytes();
    if bytes.len() > 254 {
        return Err(Error::EINVAL);
    }
    buf[..bytes.len()].copy_from_slice(bytes);
    buf[bytes.len()] = 0;
    // SAFETY: chdir syscall is safe here
    let r = unsafe { crate::syscall::syscall1(SYS_CHDIR, buf.as_ptr() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Open a file relative to a directory fd.
pub fn openat(dirfd: i64, path: &str, flags: i32, mode: u32) -> Result<i64, Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe {
        syscall4(
            SYS_OPENAT,
            dirfd as u64,
            p.as_ptr() as u64,
            flags as u64,
            mode as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r)
    }
}

/// Create a directory relative to a directory fd.
pub fn mkdirat(dirfd: i64, path: &str, mode: u32) -> Result<(), Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe { syscall3(SYS_MKDIRAT, dirfd as u64, p.as_ptr() as u64, mode as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Unlink a file relative to a directory fd.
pub fn unlinkat(dirfd: i64, path: &str, flags: i32) -> Result<(), Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe { syscall3(SYS_UNLINKAT, dirfd as u64, p.as_ptr() as u64, flags as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Create a symlink relative to a directory fd.
pub fn symlinkat(target: &str, dirfd: i64, path: &str) -> Result<(), Error> {
    let mut t = Vec::from(target.as_bytes());
    t.push(0);
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe {
        syscall3(
            SYS_SYMLINKAT,
            t.as_ptr() as u64,
            dirfd as u64,
            p.as_ptr() as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Read symlink target relative to a directory fd.
pub fn readlinkat(dirfd: i64, path: &str, buf: &mut [u8]) -> Result<usize, Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe {
        syscall4(
            SYS_READLINKAT,
            dirfd as u64,
            p.as_ptr() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as usize)
    }
}

/// Rename a file, potentially across directories.
pub fn renameat(olddirfd: i64, oldpath: &str, newdirfd: i64, newpath: &str) -> Result<(), Error> {
    let mut op = Vec::from(oldpath.as_bytes());
    op.push(0);
    let mut np = Vec::from(newpath.as_bytes());
    np.push(0);
    let r = unsafe {
        syscall4(
            SYS_RENAMEAT,
            olddirfd as u64,
            op.as_ptr() as u64,
            newdirfd as u64,
            np.as_ptr() as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Get file stats relative to a directory fd.
pub fn fstatat(dirfd: i64, path: &str, flags: i32) -> Result<Stat, Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let mut raw = [0u8; 144];
    let r = unsafe {
        syscall4(
            SYS_FSTATAT,
            dirfd as u64,
            p.as_ptr() as u64,
            raw.as_mut_ptr() as u64,
            flags as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(Stat::from_bytes(&raw))
    }
}

/// Check file access relative to a directory fd.
pub fn faccessat(dirfd: i64, path: &str, mode: i32, flags: i32) -> Result<(), Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe {
        syscall4(
            SYS_FACCESSAT,
            dirfd as u64,
            p.as_ptr() as u64,
            mode as u64,
            flags as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Link a file (hard link).
pub fn link(old: &str, new: &str) -> Result<(), Error> {
    let mut o = Vec::from(old.as_bytes());
    o.push(0);
    let mut n = Vec::from(new.as_bytes());
    n.push(0);
    let r = unsafe { syscall2(SYS_LINK, o.as_ptr() as u64, n.as_ptr() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Get file stats without following symlinks.
pub fn lstat(path: &str) -> Result<Stat, Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let mut raw = [0u8; 144];
    let r = unsafe { syscall2(SYS_LSTAT, p.as_ptr() as u64, raw.as_mut_ptr() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(Stat::from_bytes(&raw))
    }
}

/// Set file timestamps with nanosecond precision.
pub fn utimensat(dirfd: i64, path: &str, times: &[u8; 32], flags: i32) -> Result<(), Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe {
        syscall4(
            SYS_UTIMENSAT,
            dirfd as u64,
            p.as_ptr() as u64,
            times.as_ptr() as u64,
            flags as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Pre-allocate space for a file.
pub fn fallocate(fd: i64, mode: i32, offset: i64, len: i64) -> Result<(), Error> {
    let r = unsafe {
        syscall4(
            SYS_FALLOCATE,
            fd as u64,
            mode as u64,
            offset as u64,
            len as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Copy data between file descriptors efficiently.
pub fn sendfile(
    out_fd: i64,
    in_fd: i64,
    offset: Option<&mut i64>,
    count: usize,
) -> Result<usize, Error> {
    let off_ptr = offset.map_or(0, |o| o as *mut i64 as u64);
    let r = unsafe {
        syscall4(
            SYS_SENDFILE,
            out_fd as u64,
            in_fd as u64,
            off_ptr,
            count as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as usize)
    }
}

/// Truncate a file to a specified length.
pub fn truncate(path: &str, len: i64) -> Result<(), Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe { syscall2(SYS_TRUNCATE, p.as_ptr() as u64, len as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Truncate an open file to a specified length.
pub fn ftruncate(fd: i64, len: i64) -> Result<(), Error> {
    let r = unsafe { syscall2(SYS_FTRUNCATE, fd as u64, len as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Change permissions of a file.
pub fn chmod(path: &str, mode: u32) -> Result<(), Error> {
    let mut p = Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe { syscall2(SYS_CHMOD, p.as_ptr() as u64, mode as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Change filesystem permissions mask.
pub fn umask(mask: u32) -> u32 {
    unsafe { syscall1(SYS_UMASK, mask as u64) as u32 }
}

/// Get filesystem statistics.
pub fn statfs(path: &str) -> Result<crate::fs::StatFs, i64> {
    crate::fs::statfs(path)
}

/// Read a symbolic link target.
pub fn readlink(path: &str, buf: &mut [u8]) -> Result<usize, Error> {
    let mut p = alloc::vec::Vec::from(path.as_bytes());
    p.push(0);
    let r = unsafe {
        syscall3(
            SYS_READLINK,
            p.as_ptr() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as usize)
    }
}

/// Create a symbolic link.
pub fn symlink(target: &str, linkpath: &str) -> Result<(), Error> {
    let mut t = alloc::vec::Vec::from(target.as_bytes());
    t.push(0);
    let mut l = alloc::vec::Vec::from(linkpath.as_bytes());
    l.push(0);
    let r = unsafe { syscall2(SYS_SYMLINK, t.as_ptr() as u64, l.as_ptr() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Rename a file.
pub fn rename(old: &str, new: &str) -> Result<(), Error> {
    let mut o = alloc::vec::Vec::from(old.as_bytes());
    o.push(0);
    let mut n = alloc::vec::Vec::from(new.as_bytes());
    n.push(0);
    let r = unsafe { syscall2(SYS_RENAME, o.as_ptr() as u64, n.as_ptr() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Create an eventfd file descriptor.
pub fn eventfd(initval: u32, flags: i32) -> Result<i64, Error> {
    let r = unsafe { syscall2(SYS_EVENTFD, initval as u64, flags as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r)
    }
}

/// Create an eventfd2 file descriptor (with flags support).
pub fn eventfd2(initval: u32, flags: i32) -> Result<i64, Error> {
    let r = unsafe { syscall2(SYS_EVENTFD2, initval as u64, flags as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r)
    }
}

/// Reads directory entries from an open file descriptor.
pub fn getdents64(fd: i64, buf: &mut [u8]) -> Result<usize, Error> {
    // SAFETY: getdents64 syscall is safe here
    let r = unsafe { crate::syscall::getdents64(fd, buf.as_mut_ptr(), buf.len()) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as usize)
    }
}

/// Sleeps the current thread for a given number of nanoseconds.
pub fn nanosleep(ns: u64) -> Result<(), Error> {
    // SAFETY: nanosleep syscall is safe here — kernel expects (seconds, nanoseconds)
    let secs = ns / 1_000_000_000;
    let rem_ns = ns % 1_000_000_000;
    let r = unsafe { crate::syscall::syscall2(SYS_NANOSLEEP, secs, rem_ns) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Get system information.
pub fn sysinfo() -> Result<SysInfo, Error> {
    let mut info = SysInfo {
        uptime: 0,
        loads: [0; 3],
        totalram: 0,
        freeram: 0,
        sharedram: 0,
        bufferram: 0,
        totalswap: 0,
        freeswap: 0,
        procs: 0,
        _pad: [0; 22],
    };
    let r = unsafe { syscall1(SYS_SYSINFO, (&mut info as *mut SysInfo) as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(info)
    }
}

/// Get process times.
pub fn times(buf: &mut Tms) -> Result<u64, Error> {
    let r = unsafe { syscall1(SYS_TIMES, buf as *mut Tms as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as u64)
    }
}

/// Get the current time for the given clock ID (CLOCK_REALTIME=0, CLOCK_MONOTONIC=1).
/// Returns (seconds, nanoseconds).
pub fn clock_gettime(clock_id: i64) -> Result<(i64, i64), Error> {
    let mut ts = crate::posix::Timespec { sec: 0, nsec: 0 };
    let r = unsafe {
        syscall2(
            SYS_CLOCK_GETTIME,
            clock_id as u64,
            (&mut ts as *mut crate::posix::Timespec) as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok((ts.sec, ts.nsec))
    }
}

/// Get parent process ID.
pub fn getppid() -> u64 {
    unsafe { syscall0(SYS_GETPPID) as u64 }
}

/// Get resource limits.
pub fn getrlimit(resource: i32) -> Result<Rlimit, Error> {
    let mut rlim = Rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let r = unsafe {
        syscall2(
            SYS_GETRLIMIT,
            resource as u64,
            (&mut rlim as *mut Rlimit) as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(rlim)
    }
}

/// Set resource limits.
pub fn setrlimit(resource: i32, rlim: &Rlimit) -> Result<(), Error> {
    let r = unsafe {
        syscall2(
            SYS_SETRLIMIT,
            resource as u64,
            (rlim as *const Rlimit) as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Flushes filesystem buffers to disk.
pub fn sync() -> i64 {
    // SAFETY: sync syscall is safe here
    unsafe { crate::syscall::syscall0(36) }
}

/// Reboots the system.
pub fn reboot() -> i64 {
    // SAFETY: reboot syscall is safe here
    unsafe { crate::syscall::syscall3(169, 0xDEAD_BEEF, 0x28121969, 1) }
}

/// Powers off the system.
pub fn poweroff() -> i64 {
    // SAFETY: reboot syscall is safe here
    unsafe { crate::syscall::syscall3(169, 0xDEAD_BEEF, 0x28121969, 0) }
}

/// Changes permissions of an open file.
pub fn fchmod(fd: u64, mode: u32) -> Result<(), Error> {
    // SAFETY: fchmod syscall is safe here
    let r = unsafe { crate::syscall::syscall2(SYS_FCHMOD, fd, mode as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Changes ownership of an open file.
pub fn fchown(fd: u64, uid: u32, gid: u32) -> Result<(), Error> {
    // SAFETY: fchown syscall is safe here
    let r = unsafe { crate::syscall::syscall3(SYS_FCHOWN, fd, uid as u64, gid as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Mouse state information.
#[derive(Debug, Clone, Copy)]
pub struct MouseState {
    /// X coordinate
    pub x: u64,
    /// Y coordinate
    pub y: u64,
    /// Button bitmask
    pub buttons: u8,
    /// Scroll delta
    pub scroll: i8,
}

/// Retrieves the current mouse state for a window.
pub fn get_mouse(handle: u64) -> MouseState {
    // SAFETY: gui get_mouse syscall is safe here
    let packed = unsafe { crate::syscall::syscall1(120, handle) };
    if packed < 0 {
        MouseState {
            x: 0,
            y: 0,
            buttons: 0,
            scroll: 0,
        }
    } else {
        MouseState {
            x: packed as u64 & 0xFFFF,
            y: (packed as u64 >> 16) & 0xFFFF,
            buttons: ((packed as u64 >> 32) & 0xFF) as u8,
            scroll: ((packed as u64 >> 40) & 0xFF) as i8,
        }
    }
}

/// Sets the title of a window.
pub fn set_title(handle: u64, title: &str) {
    let mut buf = [0u8; 65];
    let bytes = title.as_bytes();
    let len = bytes.len().min(64);
    buf[..len].copy_from_slice(&bytes[..len]);
    buf[len] = 0;
    // SAFETY: gui set_title syscall is safe here
    unsafe {
        crate::syscall::syscall2(121, handle, buf.as_ptr() as u64);
    }
}

/// Destroys a window.
pub fn destroy_window(handle: u64) {
    // SAFETY: gui destroy_window syscall is safe here
    unsafe {
        crate::syscall::syscall1(122, handle);
    }
}

/// Resizes a window.
pub fn resize_window(handle: u64, width: u64, height: u64) {
    // SAFETY: gui resize_window syscall is safe here
    unsafe {
        crate::syscall::syscall3(123, handle, width, height);
    }
}

/// Moves a window to a new location.
pub fn move_window(handle: u64, x: u64, y: u64) {
    // SAFETY: gui move_window syscall is safe here
    unsafe {
        crate::syscall::syscall3(124, handle, x, y);
    }
}

/// Reads data from the system clipboard.
pub fn clipboard_read(buf: &mut [u8]) -> usize {
    // SAFETY: clipboard_read syscall is safe here
    // The kernel ABI returns the byte count as u64 (never a negative errno for
    // syscall 125), but syscall3 yields i64, so keep the guard as
    // defense-in-depth: a hypothetical errno must not wrap into a huge usize.
    let r = unsafe { crate::syscall::syscall3(125, 0, buf.as_mut_ptr() as u64, buf.len() as u64) };
    if r < 0 {
        0
    } else {
        r as usize
    }
}

/// Writes data to the system clipboard.
pub fn clipboard_write(data: &[u8]) {
    // SAFETY: clipboard_write syscall is safe here
    unsafe {
        crate::syscall::syscall3(125, 1, data.as_ptr() as u64, data.len() as u64);
    }
}

/// Retrieves the length of data currently in the clipboard.
pub fn clipboard_len() -> usize {
    // SAFETY: clipboard_len syscall is safe here
    // Same ABI note as clipboard_read: guard is defense-in-depth.
    let r = unsafe { crate::syscall::syscall1(125, 2) };
    if r < 0 {
        0
    } else {
        r as usize
    }
}

/// Sends a system notification.
pub fn notify(text: &str, duration_ms: u64) {
    let mut buf = [0u8; 257];
    let bytes = text.as_bytes();
    let len = bytes.len().min(256);
    buf[..len].copy_from_slice(&bytes[..len]);
    buf[len] = 0;
    // SAFETY: notify syscall is safe here
    unsafe {
        crate::syscall::syscall3(126, buf.as_ptr() as u64, duration_ms, 0);
    }
}

/// Creates a pseudo-terminal pair.
pub fn openpty() -> Result<(i64, i64), Error> {
    // SAFETY: openpty syscall is safe here
    let r = unsafe { crate::syscall::syscall0(210) };
    if r < 0 {
        return Err(Error::from_i64(r));
    }
    // Kernel packs: master | (slave << 16)
    Ok(((r & 0xFFFF), ((r >> 16) & 0xFFFF)))
}

/// Duplicates a file descriptor.
pub fn dup2(oldfd: i64, newfd: i64) -> Result<(), Error> {
    // SAFETY: dup2 syscall is safe here
    let r = unsafe { crate::syscall::syscall2(SYS_DUP2, oldfd as u64, newfd as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// ioctl request numbers (Linux termios values; the kernel's sys_ioctl
/// implements TIOCGWINSZ/TCGETS/TCSETS/FIONBIO).
pub mod ioctls {
    /// Get terminal attributes.
    pub const TCGETS: u64 = 0x5401;
    /// Set terminal attributes.
    pub const TCSETS: u64 = 0x5402;
}

/// Perform an ioctl on a file descriptor. `argp` is a caller-owned buffer
/// whose contents are interpreted per-request (e.g. the termios struct the
/// caller owns for TCGETS/TCSETS — no `Termios` type is defined here; the
/// layout lives with the consumer, `libsarga::tty`, mirrored from the
/// kernel's `sys_ioctl`). Returns the ioctl return value (usually 0 on
/// success).
pub fn ioctl(fd: i64, request: u64, argp: *mut u8) -> Result<i64, Error> {
    // SAFETY: ioctl syscall is safe here; the caller must pass a valid argp
    // for the request.
    let r = unsafe { crate::syscall::syscall3(SYS_IOCTL, fd as u64, request, argp as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r)
    }
}

/// Mount a filesystem.
pub fn mount(source: &str, target: &str, fstype: &str, flags: u64) -> Result<(), Error> {
    let mut src = Vec::from(source.as_bytes());
    src.push(0);
    let mut tgt = Vec::from(target.as_bytes());
    tgt.push(0);
    let mut fs = Vec::from(fstype.as_bytes());
    fs.push(0);
    // SAFETY: mount syscall is safe here
    let r = unsafe {
        crate::syscall::syscall6(
            SYS_MOUNT,
            src.as_ptr() as u64,
            tgt.as_ptr() as u64,
            fs.as_ptr() as u64,
            flags,
            0,
            0,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Unmount a filesystem.
pub fn umount(target: &str) -> Result<(), Error> {
    let mut tgt = Vec::from(target.as_bytes());
    tgt.push(0);
    // SAFETY: umount syscall is safe here
    let r = unsafe { crate::syscall::syscall2(SYS_UMOUNT2, tgt.as_ptr() as u64, 0) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// File descriptor set for select().
#[repr(C)]
pub struct FdSet {
    pub bits: [u64; 16], // Up to 1024 FDs
}

impl FdSet {
    pub fn new() -> Self {
        Self { bits: [0; 16] }
    }

    pub fn set(&mut self, fd: i64) {
        if (0..1024).contains(&fd) {
            self.bits[(fd / 64) as usize] |= 1 << (fd % 64);
        }
    }

    pub fn clear(&mut self, fd: i64) {
        if (0..1024).contains(&fd) {
            self.bits[(fd / 64) as usize] &= !(1 << (fd % 64));
        }
    }

    pub fn is_set(&self, fd: i64) -> bool {
        if (0..1024).contains(&fd) {
            (self.bits[(fd / 64) as usize] & (1 << (fd % 64))) != 0
        } else {
            false
        }
    }
}

impl Default for FdSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Monitor multiple file descriptors.
pub fn select(
    nfds: i32,
    readfds: Option<&mut FdSet>,
    writefds: Option<&mut FdSet>,
    exceptfds: Option<&mut FdSet>,
    timeout_ms: Option<i32>,
) -> Result<i32, Error> {
    let r = unsafe {
        crate::syscall::syscall5(
            23, // SYS_SELECT
            nfds as u64,
            readfds.map_or(0, |f| f as *mut _ as u64),
            writefds.map_or(0, |f| f as *mut _ as u64),
            exceptfds.map_or(0, |f| f as *mut _ as u64),
            timeout_ms.map_or(0, |t| t as u64),
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as i32)
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        let s = $crate::alloc::format!($($arg)*);
        $crate::io::print_str(&s);
    }}
}
#[macro_export]
macro_rules! println {
    () => { $crate::io::print_str("\n") };
    ($($arg:tt)*) => {{
        let s = $crate::alloc::format!($($arg)*);
        $crate::io::print_str(&s);
        $crate::io::print_str("\n");
    }}
}
