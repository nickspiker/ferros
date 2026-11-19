//! The ferros runtime under Rust's `std` (STD-TARGET.md): what `library/std/src/sys/pal/ferros` in the ferros-rust fork calls (the syscalls, the futex, threads with their TLS blocks, time, the allocator, the console, entropy, exit, and the image's entry), and the clients every std app shares (`tcp`).
//!
//! `memcpy`, `memmove` and `memset` are its own (mem.rs, by q-register pairs), over compiler-builtins' weak word loops.
//!
//! `no_std` and nothing but `core`: built as a dependency of std (`rustc-dep-of-std`), usable on its own by a no_std payload. One rule throughout: every address handed to the kernel is an arc offset (pointer − arc base), and every buffer lives in the arc — the image, its zeroed tail, the heap carved from that tail, the stacks carved from the heap.
//!
//! Feature `runtime` (on by default, and in std's copy): the image's entry, the C allocator symbols, the mem routines and getrandom's backends, every symbol the link takes by name.
//! An app reaches the clients thru a copy of its own beside std's (std keeps this crate to itself): `ferros-rt = { path = …, default-features = false }`, without `runtime`, so the two copies' symbols never meet at the link.

#![no_std]
#![allow(clippy::missing_safety_doc)]

pub mod alloc;
#[cfg(feature = "runtime")]
pub mod cabi;
pub mod console;
pub mod futex;
#[cfg(feature = "runtime")]
pub mod getrandom;
pub mod layout;
pub mod lock;
#[cfg(feature = "runtime")]
pub mod mem;
pub mod random;
pub mod start;
pub mod sys;
pub mod tcp;
pub mod thread;
pub mod time;
pub mod tlsf;

pub use start::{cap, exit, grants, input, panic_report};
