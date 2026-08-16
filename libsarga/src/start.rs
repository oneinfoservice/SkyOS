#[cfg_attr(not(test), no_mangle)]
#[cfg_attr(not(test), link_section = ".text._start")]
/// # Safety
/// Caller must pass a valid initial stack pointer with argc at offset 0 and the
/// argv pointer array laid out immediately after it.
pub unsafe extern "C" fn _start(stack: *const u64) -> ! {
    crate::args::init(stack);
    extern "Rust" {
        fn main() -> i32;
    }
    let code = main();
    crate::process::exit(code);
}
