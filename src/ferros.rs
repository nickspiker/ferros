//! ferros (the ferros kernel's EL0 processes, STD-TARGET.md in the ferros repo): there is no C library, only the C scalar types, so crates that spell their pointer arithmetic in `libc::ptrdiff_t` and friends (rav1d) build. No functions: a crate that calls one fails at link, loud, and gets a ferros path instead.

pub type intmax_t = i64;
pub type uintmax_t = u64;
pub type intptr_t = isize;
pub type uintptr_t = usize;
pub type size_t = usize;
pub type ssize_t = isize;
pub type ptrdiff_t = isize;
pub type off_t = i64;
pub type time_t = i64;
pub type clockid_t = i32;
pub type pid_t = i32;
pub type mode_t = u32;
