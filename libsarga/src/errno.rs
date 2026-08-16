//! Error handling and standard error codes.

use core::sync::atomic::AtomicI32;

// Fallback global errno when TLS is not available
static __ERRNO: AtomicI32 = AtomicI32::new(0);

/// Returns a pointer to the thread-local errno location.
/// Uses FS-segment TLS if available, otherwise falls back to a global static.
#[cfg_attr(not(test), no_mangle)]
pub extern "C" fn __errno_location() -> *mut i32 {
    // Try TLS first (FS segment based, offset 0 = errno). Skipped on the host
    // test harness: the raw syscall traps in ring 3, and std provides its own
    // errno slot, so the static fallback is the correct host behavior.
    #[cfg(not(test))]
    {
        let mut base = 0u64;
        let r = unsafe { crate::syscall::syscall2(158, 0x1003, &mut base as *mut u64 as u64) };
        if r == 0 && base != 0 {
            return unsafe { &mut *(base as *mut i32) };
        }
    }
    __ERRNO.as_ptr()
}

/// Sets the current thread's error number.
pub fn set_errno(err: i32) {
    unsafe {
        *__errno_location() = err;
    }
}

/// Gets the current thread's error number.
pub fn get_errno() -> i32 {
    unsafe { *__errno_location() }
}

/// Standard Sarga OS Error enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Error {
    EPERM = 1,
    ENOENT = 2,
    ESRCH = 3,
    EINTR = 4,
    EIO = 5,
    ENXIO = 6,
    E2BIG = 7,
    ENOEXEC = 8,
    EBADF = 9,
    ECHILD = 10,
    EAGAIN = 11,
    ENOMEM = 12,
    EACCES = 13,
    EFAULT = 14,
    ENOTBLK = 15,
    EBUSY = 16,
    EEXIST = 17,
    EXDEV = 18,
    ENODEV = 19,
    ENOTDIR = 20,
    EISDIR = 21,
    EINVAL = 22,
    ENFILE = 23,
    EMFILE = 24,
    ENOTTY = 25,
    ETXTBSY = 26,
    EFBIG = 27,
    ENOSPC = 28,
    ESPIPE = 29,
    EROFS = 30,
    EMLINK = 31,
    EPIPE = 32,
    EDOM = 33,
    ERANGE = 34,
    ENOSYS = 38,
    EAFNOSUPPORT = 97,
    EADDRINUSE = 98,
    EUNKNOWN = 1000,
}

impl Error {
    /// Converts a raw i64 return value from a syscall into an Error.
    pub fn from_i64(err: i64) -> Self {
        match -(err as i32) {
            1 => Error::EPERM,
            2 => Error::ENOENT,
            3 => Error::ESRCH,
            4 => Error::EINTR,
            5 => Error::EIO,
            6 => Error::ENXIO,
            7 => Error::E2BIG,
            8 => Error::ENOEXEC,
            9 => Error::EBADF,
            10 => Error::ECHILD,
            11 => Error::EAGAIN,
            12 => Error::ENOMEM,
            13 => Error::EACCES,
            14 => Error::EFAULT,
            15 => Error::ENOTBLK,
            16 => Error::EBUSY,
            17 => Error::EEXIST,
            18 => Error::EXDEV,
            19 => Error::ENODEV,
            20 => Error::ENOTDIR,
            21 => Error::EISDIR,
            22 => Error::EINVAL,
            23 => Error::ENFILE,
            24 => Error::EMFILE,
            25 => Error::ENOTTY,
            26 => Error::ETXTBSY,
            27 => Error::EFBIG,
            28 => Error::ENOSPC,
            29 => Error::ESPIPE,
            30 => Error::EROFS,
            31 => Error::EMLINK,
            32 => Error::EPIPE,
            33 => Error::EDOM,
            34 => Error::ERANGE,
            38 => Error::ENOSYS,
            97 => Error::EAFNOSUPPORT,
            98 => Error::EADDRINUSE,
            _ => Error::EUNKNOWN,
        }
    }
}

impl From<Error> for i64 {
    fn from(e: Error) -> i64 {
        -(e as i32) as i64
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const ENXIO: i32 = 6;
pub const E2BIG: i32 = 7;
pub const ENOEXEC: i32 = 8;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const ENOTBLK: i32 = 15;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;
pub const EXDEV: i32 = 18;
pub const ENODEV: i32 = 19;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ENFILE: i32 = 23;
pub const EMFILE: i32 = 24;
pub const ENOTTY: i32 = 25;
pub const ETXTBSY: i32 = 26;
pub const EFBIG: i32 = 27;
pub const ENOSPC: i32 = 28;
pub const ESPIPE: i32 = 29;
pub const EROFS: i32 = 30;
pub const EMLINK: i32 = 31;
pub const EPIPE: i32 = 32;
pub const EDOM: i32 = 33;
pub const ERANGE: i32 = 34;
pub const ENOSYS: i32 = 38;
pub const EAFNOSUPPORT: i32 = 97;
pub const EADDRINUSE: i32 = 98;
pub const ENOTSUP: i32 = 95;
pub const EOVERFLOW: i32 = 75;
pub const ECANCELED: i32 = 125;
pub const EOWNERDEAD: i32 = 130;
pub const ENOTRECOVERABLE: i32 = 131;
pub const ETIME: i32 = 62;
pub const ENODATA: i32 = 61;
pub const EPROTO: i32 = 71;
pub const EBADMSG: i32 = 74;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_i64_standard_errors() {
        assert_eq!(Error::from_i64(-1), Error::EPERM);
        assert_eq!(Error::from_i64(-2), Error::ENOENT);
        assert_eq!(Error::from_i64(-4), Error::EINTR);
        assert_eq!(Error::from_i64(-9), Error::EBADF);
        assert_eq!(Error::from_i64(-13), Error::EACCES);
        assert_eq!(Error::from_i64(-22), Error::EINVAL);
        assert_eq!(Error::from_i64(-38), Error::ENOSYS);
        assert_eq!(Error::from_i64(-97), Error::EAFNOSUPPORT);
        assert_eq!(Error::from_i64(-98), Error::EADDRINUSE);
    }

    #[test]
    fn test_from_i64_unknown_error() {
        assert_eq!(Error::from_i64(-999), Error::EUNKNOWN);
        assert_eq!(Error::from_i64(0), Error::EUNKNOWN);
        assert_eq!(Error::from_i64(-100), Error::EUNKNOWN);
    }

    #[test]
    fn test_error_to_i64() {
        assert_eq!(i64::from(Error::EPERM), -1);
        assert_eq!(i64::from(Error::EINVAL), -22);
        assert_eq!(i64::from(Error::ENOSYS), -38);
    }

    #[test]
    fn test_errno_get_set() {
        set_errno(0);
        assert_eq!(get_errno(), 0);
        set_errno(Error::EACCES as i32);
        assert_eq!(get_errno(), 13);
        set_errno(0);
    }
}
