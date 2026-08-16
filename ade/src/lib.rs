//! ADE desktop environment library.
//!
//! Host `cargo test` builds the crate with the std test harness, so the
//! `no_std` attribute is applied only for the real (bare-metal) build — the
//! same `cfg_attr(not(test), ..)` treatment as libsarga. Under `cfg(test)`
//! the crate compiles as a std lib and the pure-logic `#[cfg(test)]` modules
//! in `sys/{audio,display,input,network}` and `util/*` run on the host.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod apps;
pub mod core;
pub mod input;
pub mod ipc;
pub mod layout;
pub mod render;
pub mod sec;
pub mod service;
pub mod sys;
pub mod util;
