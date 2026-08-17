//! SkyOS audio library.
//!
//! Host `cargo test` builds the crate with the std test harness, so the
//! `no_std` attribute is applied only for the real (bare-metal) build — the
//! same `cfg_attr(not(test), ..)` treatment as libsarga and ade. Under
//! `cfg(test)` the crate compiles as a std lib and the pure-logic
//! `#[cfg(test)]` modules (`tone` beep math, `wav` parsing, `mixer` volume /
//! balance helpers) run on the host.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod mixer;
pub mod tone;
pub mod wav;

/// Beep the PC speaker for a duration. Syscall shim; the tone-generation
/// *math* lives in [`tone`].
pub fn beep(freq_hz: u32, duration_ms: u32) {
    unsafe {
        libsarga::syscall::beep(freq_hz, duration_ms);
    }
}

/// Write raw PCM data to the speaker device. Syscall shim; WAV *parsing*
/// lives in [`wav`].
pub fn play_wav(data: &[u8]) {
    // Basic implementation: write to /dev/speaker
    let fd = unsafe { libsarga::syscall::open(c"/dev/speaker".as_ptr().cast::<u8>(), 1) }; // O_WRONLY
    if fd >= 0 {
        unsafe {
            libsarga::syscall::write(fd, data.as_ptr(), data.len());
        }
        unsafe {
            libsarga::syscall::close(fd);
        }
    }
}
