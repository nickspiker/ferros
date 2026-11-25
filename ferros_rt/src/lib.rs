//! The ferros runtime under Rust's `std` (STD-TARGET.md): what `library/std/src/sys/pal/ferros` in the ferros-rust fork calls (the syscalls, the futex, threads with their TLS blocks, time, the allocator, the console, entropy, exit, and the image's entry), and the clients every std app shares (`tcp`).
//!
//! `memcpy`, `memmove` and `memset` are its own (mem.rs, by q-register pairs), over compiler-builtins' weak word loops.
//!
//! `no_std` and nothing but `core`: built as a dependency of std (`rustc-dep-of-std`), usable on its own by a no_std payload. One rule throughout: every address handed to the kernel is an arc offset (pointer − arc base), and every buffer lives in the arc — the image, its zeroed tail, the heap carved from that tail, the stacks carved from the heap.
//!
//! Feature `runtime` (on by default, and in std's copy): the image's entry, the C allocator symbols, the mem routines and getrandom's backends, every symbol the link takes by name.
//! An app reaches the clients thru a copy of its own beside std's (std keeps this crate to itself): `ferros-rt = { path = …, default-features = false }`, without `runtime`, so the two copies' symbols never meet at the link.
//!
//! Feature `cxx` (an app's copy only, never std's): what C++ built without its runtime library calls, and the POSIX and stdio calls of the C and C++ libraries it links (cxx.rs, posix.rs, stdio.rs; STD-TARGET.md "C++ through ferros_rt"). openh264 is the first; the crate that builds it turns this on, so only a process that links C++ carries these symbols. Its threads and allocations go to std's copy's heap (alloc.rs).

#![no_std]
#![allow(clippy::missing_safety_doc)]
#![cfg_attr(feature = "cxx", feature(c_variadic, thread_local))]

pub mod alloc;
#[cfg(feature = "runtime")]
pub mod cabi;
pub mod console;
#[cfg(feature = "cxx")]
pub mod cxx;
pub mod futex;
#[cfg(feature = "runtime")]
pub mod getrandom;
pub mod layout;
pub mod lock;
#[cfg(feature = "runtime")]
pub mod mem;
#[cfg(feature = "cxx")]
pub mod posix;
pub mod random;
pub mod start;
#[cfg(feature = "cxx")]
pub mod stdio;
pub mod sys;
pub mod tcp;
pub mod thread;
pub mod time;
pub mod tlsf;

pub use start::{cap, exit, grants, input, panic_report};
