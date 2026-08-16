// Host `cargo test` builds the crate with the std test harness, so the
// no_std attributes and lang items are applied only for the real (kernel)
// build (sarga targets are `os = "none"`). Under `cfg(test)` — and whenever
// the crate is compiled as a dependency on a host target, where std already
// provides the panic handler, global allocator, and alloc error handler —
// the crate compiles as a std lib and the tests in errno/net/semver run on
// the host.
#![cfg_attr(not(test), no_std)]
#![cfg_attr(target_os = "none", feature(alloc_error_handler))]

pub extern crate alloc;

pub mod ai;
pub mod args;
pub mod config;
pub mod errno;
pub mod fs;
pub mod gpu;
pub mod gui;
pub mod hash;
pub mod io;
pub mod ipc;
pub mod libskyos;
pub mod mem;
pub mod net;
pub mod posix;
pub mod process;
pub mod pthread;
pub mod semver;
pub mod signal;
pub mod start;
pub mod stdio;
pub mod sync;
pub mod syscall;
pub mod thread;
pub mod time;
pub mod toml;
pub mod vahiai;
pub mod version;

// Widget toolkit
pub mod button;
pub mod checkbox;
pub mod combobox;
pub mod dialog;
pub mod label;
pub mod layout;
pub mod menubar;
pub mod png;
pub mod progress_bar;
pub mod scrollbar;
pub mod slider;
pub mod tab_widget;
pub mod textbox;
pub mod theme;
pub mod widget;

#[macro_export]
macro_rules! sarga_main {
    ($main_fn:path) => {
        #[no_mangle]
        pub extern "Rust" fn main() -> i32 {
            $main_fn()
        }
    };
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    crate::println!("SARGA OS PANIC: {}", info);
    process::exit(1);
}
