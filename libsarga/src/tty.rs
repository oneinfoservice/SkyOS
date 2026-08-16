//! Console line discipline: termios echo + line reads.
//!
//! Single home for the termios primitives shared by the auth binaries
//! (`login`, `passwd`). Previously each binary carried its own copy of
//! `Termios` + `echo_off`/`echo_on`/`read_line`/`read_password`, pinned in
//! lockstep by `tests/test_login_echo.py`; consolidating here gives the
//! kernel's future TCSETS store exactly one userspace mirror to verify
//! against (kernel-tcsets-echo.md, K4) and one place where the "restore on
//! every read outcome, but only if echo_off succeeded" invariant lives.

use crate::errno::Error;
use crate::io::{self, ioctls, read};
use alloc::vec::Vec;

/// ECHO flag bit in termios `c_lflag` (POSIX).
///
/// NOTE: the kernel's `sys_ioctl` advertises `c_lflag: 0x5` with the comment
/// "ICANON | ECHO", but 0x5 is ISIG|ICANON per POSIX — ECHO (0x8) is not
/// actually set in the advertised value. We clear the POSIX ECHO bit; when
/// the kernel lands a real termios implementation, verify it uses POSIX
/// values (ECHO = 0x8), or this clear silently no-ops.
pub const ECHO: u32 = 0x8;

/// Termios layout mirrored from the kernel's `sys_ioctl` (repr(C), 4 u32
/// fields + c_cc). `c_lflag` is the only field we touch.
#[repr(C)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_cc: [u8; 19],
}

impl Termios {
    /// Zeroed termios — the safe default for the TCGETS-then-write-back flow:
    /// TCGETS fills the fields, so the zeroes are only ever written back for
    /// fields TCGETS did not report.
    pub fn zeroed() -> Self {
        Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_cc: [0; 19],
        }
    }
}

/// Disable input echo on `fd` (TCSETS clear ECHO) so a password typed at the
/// console is not echoed back onto the wire/log. Best-effort: the kernel's
/// TCSETS is currently a no-op returning 0, so this is forward-compatible
/// with a real termios implementation.
///
/// Returns the previous `c_lflag` on success (`Some`) so the caller can
/// restore it after reading; `None` when TCGETS/TCSETS failed (e.g. fd is
/// not a tty) — the caller must then skip the restore so a bogus 0 cannot
/// clobber real termios once the kernel implements TCSETS.
pub fn echo_off(fd: i64) -> Option<u32> {
    let mut t = Termios::zeroed();
    // TCGETS first so the other fields (iflag/oflag/cflag) are preserved
    // when we write back — a TCSETS of a zeroed struct would clobber flow
    // control / canonical flags once the kernel implements it.
    if io::ioctl(fd, ioctls::TCGETS, &mut t as *mut _ as *mut u8).is_err() {
        return None;
    }
    let saved = t.c_lflag;
    t.c_lflag &= !ECHO;
    if io::ioctl(fd, ioctls::TCSETS, &mut t as *mut _ as *mut u8).is_err() {
        return None;
    }
    Some(saved)
}

/// Restore input echo on `fd` to `lflag` (TCSETS). Best-effort; reads the
/// current termios first so untouched fields are preserved.
pub fn echo_on(fd: i64, lflag: u32) {
    let mut t = Termios::zeroed();
    if io::ioctl(fd, ioctls::TCGETS, &mut t as *mut _ as *mut u8).is_err() {
        return;
    }
    t.c_lflag = lflag;
    let _ = io::ioctl(fd, ioctls::TCSETS, &mut t as *mut _ as *mut u8);
}

/// Ensure the console tty echoes typed input (set the ECHO bit) before the
/// username read. The username is read BEFORE `echo_off` runs (only the
/// password is hidden), so it must not depend on the kernel's default
/// c_lflag having ECHO set — a prior TCSETS or a future non-echoing default
/// would otherwise leave the username invisible. Best-effort: silent no-op
/// if TCGETS/TCSETS fails (non-tty fd), mirroring echo_off/echo_on.
pub fn ensure_echo(fd: i64) {
    let mut t = Termios::zeroed();
    if io::ioctl(fd, ioctls::TCGETS, &mut t as *mut _ as *mut u8).is_err() {
        return;
    }
    t.c_lflag |= ECHO;
    let _ = io::ioctl(fd, ioctls::TCSETS, &mut t as *mut _ as *mut u8);
}

/// Read one line from `fd`, terminating on `\n` or `\r`. Returns `Ok(None)`
/// on EOF (zero bytes read), `Ok(Some(line))` for a terminated line (which
/// may be empty), `Err` on a read error.
pub fn read_line(fd: i64) -> Result<Option<Vec<u8>>, Error> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = read(fd, &mut byte)?;
        if n == 0 {
            return Ok(None);
        }
        if byte[0] == b'\n' || byte[0] == b'\r' {
            break;
        }
        buf.push(byte[0]);
    }
    Ok(Some(buf))
}

/// Read a password line from `fd` with input echo disabled for the duration.
/// Mirrors the classic getty/login pattern: disable echo, read, restore.
/// The restore is skipped when echo_off failed (non-tty fd), so a bogus
/// lflag can never clobber real termios.
pub fn read_password(fd: i64) -> Result<Option<Vec<u8>>, Error> {
    let saved = echo_off(fd);
    let r = read_line(fd);
    if let Some(lflag) = saved {
        echo_on(fd, lflag);
    }
    r
}
