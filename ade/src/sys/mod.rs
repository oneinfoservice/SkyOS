pub(crate) mod vfs;

// Pure-logic subsystems, host-testable under `cargo test` (the same
// cfg(not(test)) treatment as libsarga): no syscalls, only integer/string
// math, so their #[cfg(test)] modules run on the host.
pub mod audio;
pub mod display;
pub mod input;
pub mod network;
