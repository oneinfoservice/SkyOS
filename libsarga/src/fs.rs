use crate::syscall::*;

/// Fill `buf` with the NUL-terminated bytes of `s`, returning the byte
/// length on success.
///
/// The kernel takes paths, mount sources/targets, and filesystem names as
/// fixed-size C strings, so every syscall wrapper here funnels through this
/// one validation + copy path. The length rule is deliberately
/// conservative: the string plus its NUL terminator must leave at least one
/// spare byte in the buffer (a 255-byte path into a 256-byte buffer is
/// rejected, exactly as the historical inline checks did), and the string
/// is copied byte-for-byte (UTF-8 multibyte sequences pass through
/// untouched).
fn fill_cstr(buf: &mut [u8], s: &str) -> Result<usize, i64> {
    let bytes = s.as_bytes();
    if bytes.len() + 1 >= buf.len() {
        return Err(crate::errno::EINVAL as i64);
    }
    buf[..bytes.len()].copy_from_slice(bytes);
    buf[bytes.len()] = 0;
    Ok(bytes.len())
}

#[repr(C)]
pub struct Stat {
    pub dev: u64,
    pub ino: u64,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u64,
    pub size: i64,
    pub blksize: i64,
    pub blocks: i64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
}

#[repr(C)]
pub struct StatFs {
    pub f_type: u64,
    pub f_bsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
}

pub fn stat(path: &str) -> Result<Stat, i64> {
    let mut buf = [0u8; 256];
    fill_cstr(&mut buf, path)?;
    let mut s = core::mem::MaybeUninit::<Stat>::uninit();
    let r = unsafe { syscall2(4, buf.as_ptr() as u64, s.as_mut_ptr() as u64) };
    if r < 0 {
        Err(-r)
    } else {
        Ok(unsafe { s.assume_init() })
    }
}

pub fn fstat(fd: i64) -> Result<Stat, i64> {
    let mut s = core::mem::MaybeUninit::<Stat>::uninit();
    let r = unsafe { syscall2(5, fd as u64, s.as_mut_ptr() as u64) };
    if r < 0 {
        Err(-r)
    } else {
        Ok(unsafe { s.assume_init() })
    }
}

pub fn statfs(path: &str) -> Result<StatFs, i64> {
    let mut buf = [0u8; 256];
    fill_cstr(&mut buf, path)?;
    let mut s = core::mem::MaybeUninit::<StatFs>::uninit();
    let r = unsafe { syscall2(137, buf.as_ptr() as u64, s.as_mut_ptr() as u64) };
    if r < 0 {
        Err(-r)
    } else {
        Ok(unsafe { s.assume_init() })
    }
}

pub fn touch(path: &str) -> i64 {
    let mut buf = [0u8; 256];
    if fill_cstr(&mut buf, path).is_err() {
        return -crate::errno::EINVAL as i64;
    }
    let fd = unsafe { syscall2(2, buf.as_ptr() as u64, 0x241u64) };
    if fd >= 0 {
        unsafe { syscall1(3, fd as u64) };
    }
    0
}

pub fn open(path: &str, flags: u64) -> Result<i64, i64> {
    let mut buf = [0u8; 256];
    fill_cstr(&mut buf, path)?;
    let r = unsafe { syscall2(2, buf.as_ptr() as u64, flags) };
    if r < 0 {
        Err(-r)
    } else {
        Ok(r)
    }
}

pub fn read(fd: i64, buf: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe { syscall3(0, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) };
    if r < 0 {
        Err(-r)
    } else {
        Ok(r as usize)
    }
}

pub fn write(fd: i64, buf: &[u8]) -> Result<usize, i64> {
    let r = unsafe { syscall3(1, fd as u64, buf.as_ptr() as u64, buf.len() as u64) };
    if r < 0 {
        Err(-r)
    } else {
        Ok(r as usize)
    }
}

pub fn close(fd: i64) -> i64 {
    unsafe { syscall1(3, fd as u64) }
}

pub fn mkfs(fstype: &str, device: u64) -> Result<(), i64> {
    let mut fs_buf = [0u8; 32];
    fill_cstr(&mut fs_buf, fstype)?;
    let r = unsafe { crate::syscall::syscall2(127, fs_buf.as_ptr() as u64, device) };
    if r < 0 {
        Err(-r)
    } else {
        Ok(())
    }
}

pub fn mount(source: &str, target: &str, fstype: &str, flags: u64) -> Result<(), i64> {
    let mut src_buf = [0u8; 256];
    fill_cstr(&mut src_buf, source)?;

    let mut tgt_buf = [0u8; 256];
    fill_cstr(&mut tgt_buf, target)?;

    let mut fs_buf = [0u8; 32];
    fill_cstr(&mut fs_buf, fstype)?;

    let r = unsafe {
        syscall5(
            165,
            src_buf.as_ptr() as u64,
            tgt_buf.as_ptr() as u64,
            fs_buf.as_ptr() as u64,
            flags,
            0,
        )
    };
    if r < 0 {
        Err(-r)
    } else {
        Ok(())
    }
}

pub fn umount(target: &str) -> Result<(), i64> {
    let mut buf = [0u8; 256];
    fill_cstr(&mut buf, target)?;
    let r = unsafe { syscall2(crate::syscall::SYS_UMOUNT2, buf.as_ptr() as u64, 0) };
    if r < 0 {
        Err(-r)
    } else {
        Ok(())
    }
}

pub fn read_to_string(path: &str) -> Result<alloc::string::String, i64> {
    let fd = open(path, 0)?;
    let mut data = alloc::vec::Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match read(fd, &mut tmp) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&tmp[..n]),
            Err(e) => {
                close(fd);
                return Err(e);
            }
        }
    }
    close(fd);
    Ok(alloc::string::String::from_utf8_lossy(&data).into_owned())
}

pub fn write_file(path: &str, content: &str) -> Result<(), i64> {
    let fd = open(path, 0x241 | 0x200)?;
    let mut written = 0;
    let bytes = content.as_bytes();
    while written < bytes.len() {
        match write(fd, &bytes[written..]) {
            Ok(n) => written += n,
            Err(e) => {
                close(fd);
                return Err(e);
            }
        }
    }
    close(fd);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf256() -> [u8; 256] {
        [0u8; 256]
    }

    #[test]
    fn fill_cstr_copies_and_nul_terminates() {
        let mut b = buf256();
        assert_eq!(fill_cstr(&mut b, "hello").unwrap(), 5);
        assert_eq!(&b[..6], b"hello\0");
        // Everything past the terminator stays untouched (zeros here).
        assert_eq!(&b[6..], &[0u8; 250][..]);
    }

    #[test]
    fn fill_cstr_empty_string() {
        let mut b = buf256();
        assert_eq!(fill_cstr(&mut b, "").unwrap(), 0);
        assert_eq!(b[0], 0);
    }

    #[test]
    fn fill_cstr_multibyte_utf8_is_byte_copied() {
        let mut b = buf256();
        // "é" is 2 bytes in UTF-8; the fill is byte-exact (no transcoding).
        assert_eq!(fill_cstr(&mut b, "héllo").unwrap(), 6);
        assert_eq!(&b[..7], "héllo\0".as_bytes());
    }

    #[test]
    fn fill_cstr_254_byte_path_ok_255_rejected() {
        let mut b = buf256();
        let p254 = "x".repeat(254);
        assert_eq!(fill_cstr(&mut b, &p254).unwrap(), 254);
        assert_eq!(b[254], 0);
        let p255 = "x".repeat(255);
        assert_eq!(fill_cstr(&mut b, &p255), Err(crate::errno::EINVAL as i64));
    }

    #[test]
    fn fill_cstr_small_buf_boundary() {
        // The 32-byte fstype buffer used by mkfs/mount: 30 ok, 31 rejected.
        let mut b = [0u8; 32];
        assert_eq!(fill_cstr(&mut b, "ext4").unwrap(), 4);
        assert_eq!(b[4], 0);
        assert_eq!(fill_cstr(&mut b, &"x".repeat(30)).unwrap(), 30);
        assert_eq!(
            fill_cstr(&mut b, &"x".repeat(31)),
            Err(crate::errno::EINVAL as i64)
        );
    }

    #[test]
    fn stat_rejects_overlong_path_without_syscall() {
        // The length gate runs BEFORE the raw syscall, so the EINVAL path
        // is observable on the host without touching the syscall asm.
        fn assert_einval<T>(r: Result<T, i64>) {
            match r {
                Err(e) => assert_eq!(e, crate::errno::EINVAL as i64),
                Ok(_) => panic!("expected EINVAL, got Ok"),
            }
        }
        let p255 = "x".repeat(255);
        assert_einval(stat(&p255));
        assert_einval(statfs(&p255));
        assert_einval(open(&p255, 0));
        assert_eq!(touch(&p255), -crate::errno::EINVAL as i64);
        assert_einval(umount(&p255));
        assert_einval(mount(&p255, "/", "ext4", 0));
        assert_einval(mount("/", &p255, "ext4", 0));
        assert_einval(mkfs(&"x".repeat(31), 0));
    }

    #[test]
    fn stat_repr_layout_is_stable() {
        // The kernel fills these structs via a raw syscall into a C layout;
        // field order and padding are part of the kernel/userspace ABI.
        assert_eq!(core::mem::size_of::<Stat>(), 88);
        assert_eq!(core::mem::size_of::<StatFs>(), 56);
        assert_eq!(core::mem::offset_of!(Stat, mtime), 72);
    }
}
