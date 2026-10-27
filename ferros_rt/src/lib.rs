//! The ferros runtime under Rust's `std` (STD-TARGET.md): what `library/std/src/sys/pal/ferros` in the ferros-rust fork calls — the syscalls, the futex, threads with their TLS blocks, time, the allocator, the console, entropy, exit, and the image's entry.
//!
//! `no_std` and nothing but `core`: built as a dependency of std (`rustc-dep-of-std`), usable on its own by a no_std payload. One rule throughout: every address handed to the kernel is an arc offset (pointer − arc base), and every buffer lives in the arc — the image, its zeroed tail, the heap carved from that tail, the stacks carved from the heap.

#![no_std]
#![allow(clippy::missing_safety_doc)]

pub mod alloc;
pub mod console;
pub mod futex;
pub mod getrandom;
pub mod heap;
pub mod random;
pub mod start;
pub mod sys;
pub mod thread;
pub mod time;

pub use start::{cap, exit, grants, input};
