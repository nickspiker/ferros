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

// errno values as Linux numbers them, for the crates that spell their errors through libc (rav1d). There is no errno on ferros; these are names for the codes they return.
pub const ENOENT: ::c_int = 2;
pub const EIO: ::c_int = 5;
pub const EAGAIN: ::c_int = 11;
pub const ENOMEM: ::c_int = 12;
pub const EINVAL: ::c_int = 22;
pub const ERANGE: ::c_int = 34;
pub const ENOPROTOOPT: ::c_int = 92;

// The C allocator: ferros_rt (the std runtime, linked into every std image) defines these over the process heap, for the C in the graph (PQClean) and the crates that call them by name (rav1d).
extern "C" {
    pub fn malloc(size: size_t) -> *mut ::c_void;
    pub fn calloc(count: size_t, size: size_t) -> *mut ::c_void;
    pub fn realloc(p: *mut ::c_void, size: size_t) -> *mut ::c_void;
    pub fn free(p: *mut ::c_void);
}
