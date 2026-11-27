//! ferros (STD-TARGET.md in the ferros repo): an EL0 process of the ferros kernel. Everything platform-specific is in the `ferros_rt` crate — syscalls, futex, threads and TLS, time, allocator, console, entropy, the entry that calls `main` — and this layer is the thin binding. No fs, net sockets, processes, env or args: a ferros process speaks the vault and the `ip` server through capabilities, not through std.

#![forbid(unsafe_op_in_unsafe_fn)]

pub use ferros_rt::futex;

use crate::io as std_io;

// SAFETY: must be called only once during runtime initialization.
// NOTE: ferros_rt's entry has already relocated the image, built the heap and installed the main thread's TLS before `main` runs.
pub unsafe fn init(_argc: isize, _argv: *const *const u8, _sigpipe: u8) {}

// SAFETY: must be called only once during runtime cleanup.
// NOTE: this is not guaranteed to run, for example when the program aborts.
pub unsafe fn cleanup() {}

pub fn unsupported<T>() -> std_io::Result<T> {
    Err(unsupported_err())
}

pub fn unsupported_err() -> std_io::Error {
    std_io::Error::UNSUPPORTED_PLATFORM
}

/// The kernel prints `name: exit G#DEAD` for an abort, as it does for a no_std payload's panic.
pub fn abort_internal() -> ! {
    ferros_rt::exit(0xDEAD)
}
