use crate::errno::Error;
use crate::sync::RawMutex;
use crate::syscall::*;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// # Safety
/// Caller must ensure `addr`/`len` describe a valid range and `fd` is valid
/// unless MAP_ANONYMOUS is set; the kernel may fault on invalid inputs.
pub unsafe fn mmap(
    addr: u64,
    len: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: u64,
) -> Result<u64, i64> {
    let r = syscall6(
        9,
        addr,
        len as u64,
        prot as u64,
        flags as u64,
        fd as u64,
        offset,
    );
    if r < 0 {
        Err(-r)
    } else {
        Ok(r as u64)
    }
}

/// # Safety
/// Caller must ensure `addr`/`len` describe a range previously returned by
/// `mmap`; unmapping invalid ranges may fault.
pub unsafe fn munmap(addr: u64, len: usize) -> Result<(), i64> {
    let r = syscall2(11, addr, len as u64);
    if r < 0 {
        Err(-r)
    } else {
        Ok(())
    }
}

pub fn brk(addr: u64) -> u64 {
    unsafe { crate::syscall::syscall1(12, addr) as u64 }
}

/// Slab allocator for small objects to reduce mmap overhead
const SLAB_SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 2048];

struct SlabAllocator {
    free_lists: [AtomicUsize; SLAB_SIZES.len()], // Each stores a pointer to free list head
    lock: RawMutex,
}

impl SlabAllocator {
    const fn new() -> Self {
        // ponytail: const-local to repeat into the array; not a global
        #[allow(clippy::declare_interior_mutable_const)] // used only as a fresh init value
        const ZERO: AtomicUsize = AtomicUsize::new(0);
        SlabAllocator {
            free_lists: [ZERO; SLAB_SIZES.len()],
            lock: RawMutex::new(),
        }
    }

    fn slab_index(&self, size: usize) -> Option<usize> {
        SLAB_SIZES.iter().position(|&s| s >= size)
    }

    unsafe fn alloc_from_slab(&self, layout: Layout) -> Option<*mut u8> {
        if let Some(idx) = self.slab_index(layout.size()) {
            self.lock.lock();
            // SAFETY: free-list head is either 0 or a block pointer written by dealloc_to_slab
            let head = self.free_lists[idx].load(Ordering::Acquire);
            if head != 0 {
                // Pop from free list
                let ptr = head as *mut u8;
                // Read next pointer from first word
                let next = *(ptr as *const usize);
                self.free_lists[idx].store(next, Ordering::Release);
                self.lock.unlock();
                return Some(ptr);
            }
            self.lock.unlock();
        }
        None
    }

    unsafe fn dealloc_to_slab(&self, ptr: *mut u8, layout: Layout) {
        if let Some(idx) = self.slab_index(layout.size()) {
            self.lock.lock();
            // Push to free list
            let head = self.free_lists[idx].load(Ordering::Acquire);
            *(ptr as *mut usize) = head;
            self.free_lists[idx].store(ptr as usize, Ordering::Release);
            self.lock.unlock();
        }
    }
}

pub struct SargaMapper {
    slab: SlabAllocator,
}

impl SargaMapper {
    const fn new() -> Self {
        SargaMapper {
            slab: SlabAllocator::new(),
        }
    }
}

unsafe impl GlobalAlloc for SargaMapper {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Try slab allocator for small objects
        if let Some(ptr) = self.slab.alloc_from_slab(layout) {
            return ptr;
        }

        // Fall back to mmap for large allocations
        let size = (layout.size() + 4095) & !4095;
        match mmap(0, size, 3, 0x22, -1, 0) {
            // PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS
            Ok(ptr) => ptr as *mut u8,
            Err(_) => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Try to return to slab allocator
        if self.slab.slab_index(layout.size()).is_some() {
            self.slab.dealloc_to_slab(ptr, layout);
            return;
        }

        // munmap for large allocations
        let size = (layout.size() + 4095) & !4095;
        let _ = munmap(ptr as u64, size);
    }
}

#[cfg(not(test))]
#[global_allocator]
pub static ALLOCATOR: SargaMapper = SargaMapper::new();

// The allocator lang items must not be defined when the crate is compiled for
// the host test harness: std already provides both, and a second definition is
// E0152 duplicate lang item.
#[cfg(not(test))]
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}

#[cfg_attr(not(test), no_mangle)]
/// # Safety
/// Caller must ensure `dest`/`src` point to valid, non-overlapping regions of
/// at least `n` bytes each.
pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    for i in 0..n {
        *dest.add(i) = *src.add(i);
    }
    dest
}

#[cfg_attr(not(test), no_mangle)]
/// # Safety
/// Caller must ensure `s` points to a valid writable region of at least `n`
/// bytes.
pub unsafe extern "C" fn memset(s: *mut u8, c: i32, n: usize) -> *mut u8 {
    for i in 0..n {
        *s.add(i) = c as u8;
    }
    s
}

#[cfg_attr(not(test), no_mangle)]
/// # Safety
/// Caller must ensure `s1`/`s2` point to valid readable regions of at least
/// `n` bytes each.
pub unsafe extern "C" fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    for i in 0..n {
        let a = *s1.add(i);
        let b = *s2.add(i);
        if a != b {
            return (a as i32) - (b as i32);
        }
    }
    0
}

// ── Shared Memory ─────────────────────────────────────────────────

pub fn shmget(key: i32, size: usize, flags: i32) -> Result<i32, Error> {
    let r = unsafe { syscall3(SYS_SHMGET, key as u64, size as u64, flags as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as i32)
    }
}

pub fn shmat(shmid: i32, addr: Option<*const u8>, flags: i32) -> Result<*mut u8, Error> {
    let addr_ptr = addr.unwrap_or_default() as u64;
    let r = unsafe { syscall3(SYS_SHMAT, shmid as u64, addr_ptr, flags as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as *mut u8)
    }
}

pub fn shmdt(addr: *const u8) -> Result<(), Error> {
    let r = unsafe { syscall1(SYS_SHMDT, addr as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

#[repr(C)]
pub struct ShmIdDs {
    pub shm_perm: IpcPerm,
    pub shm_segsz: usize,
    pub shm_atime: u64,
    pub shm_dtime: u64,
    pub shm_ctime: u64,
    pub shm_cpid: i32,
    pub shm_lpid: i32,
    pub shm_nattch: u64,
}

#[repr(C)]
pub struct IpcPerm {
    pub key: i32,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u16,
    pub _pad: [u8; 6],
}

pub const IPC_RMID: i32 = 0;
pub const IPC_SET: i32 = 1;
pub const IPC_STAT: i32 = 2;
pub const IPC_CREAT: i32 = 0o1000;
pub const IPC_EXCL: i32 = 0o2000;

pub fn shmctl(shmid: i32, cmd: i32, buf: Option<&mut ShmIdDs>) -> Result<(), Error> {
    let buf_ptr = buf.map_or(0, |b| b as *mut ShmIdDs as u64);
    let r = unsafe { syscall3(SYS_SHMCTL, shmid as u64, cmd as u64, buf_ptr) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

// ── memfd_create ──────────────────────────────────────────────────

pub fn memfd_create(name: &str, flags: u32) -> Result<i64, Error> {
    let mut buf = alloc::vec::Vec::from(name.as_bytes());
    buf.push(0);
    let r = unsafe { syscall2(SYS_MEMFD_CREATE, buf.as_ptr() as u64, flags as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r)
    }
}

// ─── POSIX timers ──────────────────────────────────────────────────

#[repr(C)]
pub struct Sigevent {
    pub sigev_value: i64,
    pub sigev_signo: i32,
    pub sigev_notify: i32,
}

#[repr(C)]
pub struct Itimerspec {
    pub it_interval: crate::posix::Timespec,
    pub it_value: crate::posix::Timespec,
}

pub fn timer_create(clockid: i32, evp: Option<&Sigevent>) -> Result<i32, Error> {
    let evp_ptr = evp.map_or(0, |e| e as *const Sigevent as u64);
    let mut timerid: i32 = 0;
    let r = unsafe {
        syscall3(
            SYS_TIMER_CREATE,
            clockid as u64,
            evp_ptr,
            (&mut timerid as *mut i32) as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(timerid)
    }
}

pub fn timer_settime(
    timerid: i32,
    flags: i32,
    new: &Itimerspec,
    old: Option<&mut Itimerspec>,
) -> Result<(), Error> {
    let old_ptr = old.map_or(0, |o| o as *mut Itimerspec as u64);
    let r = unsafe {
        syscall4(
            SYS_TIMER_SETTIME,
            timerid as u64,
            flags as u64,
            new as *const Itimerspec as u64,
            old_ptr,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

pub fn timer_gettime(timerid: i32) -> Result<Itimerspec, Error> {
    let mut val = Itimerspec {
        it_interval: crate::posix::Timespec { sec: 0, nsec: 0 },
        it_value: crate::posix::Timespec { sec: 0, nsec: 0 },
    };
    let r = unsafe {
        syscall2(
            SYS_TIMER_GETTIME,
            timerid as u64,
            (&mut val as *mut Itimerspec) as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(val)
    }
}

pub fn timer_getoverrun(timerid: i32) -> Result<i32, Error> {
    let r = unsafe { syscall1(SYS_TIMER_GETOVERRUN, timerid as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as i32)
    }
}

pub fn timer_delete(timerid: i32) -> Result<(), Error> {
    let r = unsafe { syscall1(SYS_TIMER_DELETE, timerid as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

// ── mprotect ──────────────────────────────────────────────────────

pub fn mprotect(addr: u64, len: usize, prot: i32) -> Result<(), Error> {
    let r = unsafe { syscall3(SYS_MPROTECT, addr, len as u64, prot as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

// ── swapon/swapoff ──────────────────────────────────────────────────

pub fn swapon(path: &str, flags: i32) -> Result<(), Error> {
    let mut buf = alloc::vec::Vec::from(path.as_bytes());
    buf.push(0);
    let r = unsafe { syscall2(SYS_SWAPON, buf.as_ptr() as u64, flags as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

pub fn swapoff(path: &str) -> Result<(), Error> {
    let mut buf = alloc::vec::Vec::from(path.as_bytes());
    buf.push(0);
    let r = unsafe { syscall1(SYS_SWAPOFF, buf.as_ptr() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

#[cfg_attr(not(test), no_mangle)]
/// # Safety
/// Caller must ensure `dest`/`src` point to valid regions of at least `n`
/// bytes; regions may overlap.
pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dest < src as *mut u8 {
        for i in 0..n {
            *dest.add(i) = *src.add(i);
        }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            *dest.add(i) = *src.add(i);
        }
    }
    dest
}
