//! What C++ compiled without its runtime library calls (feature `cxx`, STD-TARGET.md "C++ through ferros_rt"): `new` and `delete` in every form, the pure-virtual trap, the static-initialisation guards, `__cxa_atexit` and `__dso_handle`.
//! Built for C++ with `-fno-exceptions -fno-rtti -nostdinc++` (openh264 is the first): nothing throws, so a failed `new` ends the process with the allocator's report, and the `nothrow` forms answer null.
//! The memory is the process's one heap through the C allocator std's copy of this crate gives the link (cabi.rs `malloc` and `free`); an alignment past the allocator's 16 bytes over-allocates and keeps the block's start in the word below the object.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::futex::{futex_wait, futex_wake_all};

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(p: *mut c_void);
}

/// What the C allocator aligns every block to (cabi.rs `HEADER`).
const MALLOC_ALIGN: usize = 16;

/// A block of `size` bytes or null.
unsafe fn take(size: usize) -> *mut c_void {
    unsafe { take_aligned(size, MALLOC_ALIGN) }
}

/// A block of `size` bytes aligned to `align` (a power of two, as `std::align_val_t` must be), or null.
/// Past 16: `align + size` bytes from malloc, the object at the first `align` boundary past its first word, the block's start in the word below the object.
unsafe fn take_aligned(size: usize, align: usize) -> *mut c_void {
    // WHY: g++ hands `new[]` SIZE_MAX when an array's count times its element size overflows (so that the allocation fails), and both paths below add to the size (cabi.rs its 16-byte header, this one `align`).
    // PROOF: SIZE_MAX + 16 wraps to 15, and malloc would hand back a 31-byte block for an array the caller then fills; no object can be over isize::MAX bytes, so above it is a failure, and at or below it neither sum can wrap (align is under 2^63).
    if size > isize::MAX as usize {
        return core::ptr::null_mut();
    }
    if align <= MALLOC_ALIGN {
        return unsafe { malloc(size) };
    }
    let base = unsafe { malloc(size + align) } as usize;
    if base == 0 {
        return core::ptr::null_mut();
    }
    let at = (base + 8 + align - 1) & !(align - 1);
    unsafe { ((at - 8) as *mut usize).write(base) };
    at as *mut c_void
}

/// Free what `take_aligned` gave for the same `align`.
unsafe fn give_aligned(p: *mut c_void, align: usize) {
    if p.is_null() {
        return;
    }
    if align <= MALLOC_ALIGN {
        unsafe { free(p) };
        return;
    }
    let base = unsafe { ((p as usize - 8) as *const usize).read() };
    unsafe { free(base as *mut c_void) };
}

/// A throwing `new` that found no memory: C++ would throw `std::bad_alloc`, which nothing here can catch, so the process ends, saying so.
#[cold]
fn out_of_memory(size: usize) -> ! {
    let mut line = [0u8; 160];
    let n = crate::alloc::out_of_memory(size, &mut line);
    crate::panic_report(unsafe { core::str::from_utf8_unchecked(&line[..n]) }, "C++ operator new", 0, 0);
    crate::exit(0xDEAD)
}

unsafe fn new_or_die(size: usize, align: usize) -> *mut c_void {
    let p = unsafe { take_aligned(size, align) };
    if p.is_null() {
        out_of_memory(size);
    }
    p
}

// operator new(size_t), new[](size_t).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Znwm(size: usize) -> *mut c_void {
    unsafe { new_or_die(size, MALLOC_ALIGN) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Znam(size: usize) -> *mut c_void {
    unsafe { new_or_die(size, MALLOC_ALIGN) }
}

// operator new(size_t, const nothrow_t&), new[](size_t, const nothrow_t&).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZnwmRKSt9nothrow_t(size: usize, _: *const c_void) -> *mut c_void {
    unsafe { take(size) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZnamRKSt9nothrow_t(size: usize, _: *const c_void) -> *mut c_void {
    unsafe { take(size) }
}

// operator new(size_t, align_val_t), new[](size_t, align_val_t).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZnwmSt11align_val_t(size: usize, align: usize) -> *mut c_void {
    unsafe { new_or_die(size, align) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZnamSt11align_val_t(size: usize, align: usize) -> *mut c_void {
    unsafe { new_or_die(size, align) }
}

// operator new(size_t, align_val_t, const nothrow_t&), new[](size_t, align_val_t, const nothrow_t&).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZnwmSt11align_val_tRKSt9nothrow_t(size: usize, align: usize, _: *const c_void) -> *mut c_void {
    unsafe { take_aligned(size, align) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZnamSt11align_val_tRKSt9nothrow_t(size: usize, align: usize, _: *const c_void) -> *mut c_void {
    unsafe { take_aligned(size, align) }
}

// operator delete(void*), delete[](void*), and the sized and nothrow forms (the size is the allocator's to know).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdlPv(p: *mut c_void) {
    unsafe { free(p) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdaPv(p: *mut c_void) {
    unsafe { free(p) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdlPvm(p: *mut c_void, _: usize) {
    unsafe { free(p) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdaPvm(p: *mut c_void, _: usize) {
    unsafe { free(p) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdlPvRKSt9nothrow_t(p: *mut c_void, _: *const c_void) {
    unsafe { free(p) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdaPvRKSt9nothrow_t(p: *mut c_void, _: *const c_void) {
    unsafe { free(p) }
}

// The aligned deletes: delete(void*, align_val_t), sized, nothrow, and their array forms.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdlPvSt11align_val_t(p: *mut c_void, align: usize) {
    unsafe { give_aligned(p, align) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdaPvSt11align_val_t(p: *mut c_void, align: usize) {
    unsafe { give_aligned(p, align) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdlPvmSt11align_val_t(p: *mut c_void, _: usize, align: usize) {
    unsafe { give_aligned(p, align) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdaPvmSt11align_val_t(p: *mut c_void, _: usize, align: usize) {
    unsafe { give_aligned(p, align) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdlPvSt11align_val_tRKSt9nothrow_t(p: *mut c_void, align: usize, _: *const c_void) {
    unsafe { give_aligned(p, align) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _ZdaPvSt11align_val_tRKSt9nothrow_t(p: *mut c_void, align: usize, _: *const c_void) {
    unsafe { give_aligned(p, align) }
}

/// `std::nothrow`, the tag the nothrow forms are called with: an empty object, so one byte.
#[unsafe(no_mangle)]
pub static _ZSt7nothrow: u8 = 0;

/// A call to a pure virtual function: an object used before its constructor ran or after its destructor, a bug to stop at.
#[unsafe(no_mangle)]
pub extern "C" fn __cxa_pure_virtual() -> ! {
    crate::panic_report("C++ pure virtual function called", "__cxa_pure_virtual", 0, 0);
    crate::exit(0xDEAD)
}

/// A call to a deleted virtual function, the same kind of bug.
#[unsafe(no_mangle)]
pub extern "C" fn __cxa_deleted_virtual() -> ! {
    crate::panic_report("C++ deleted virtual function called", "__cxa_deleted_virtual", 0, 0);
    crate::exit(0xDEAD)
}

// A function-local static's guard (the Itanium C++ ABI, which AArch64 follows): a 64-bit word whose first byte the compiled code reads (acquire) and skips the call when it is not zero.
// The rest of the word is the guard's state here: free, being initialised, being initialised with a thread asleep on it.
const GUARD_DONE: u64 = 1;
const GUARD_BUSY: u64 = 1 << 8;
const GUARD_WAITED: u64 = 2 << 8;

/// 1 when the caller is to run the initialiser (and then release), 0 when another thread finished it.
/// A thread that finds it being initialised sleeps on the word until the initialiser releases (or aborts) it; the guard is 8-aligned, as the kernel's WAIT needs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __cxa_guard_acquire(guard: *mut u64) -> i32 {
    let g = unsafe { &*(guard as *const AtomicU64) };
    loop {
        let v = g.load(Ordering::Acquire);
        if v & 0xFF != 0 {
            return 0;
        }
        if v == 0 {
            if g.compare_exchange(0, GUARD_BUSY, Ordering::Acquire, Ordering::Acquire).is_ok() {
                return 1;
            }
            continue;
        }
        if v == GUARD_BUSY && g.compare_exchange(GUARD_BUSY, GUARD_WAITED, Ordering::Acquire, Ordering::Acquire).is_err() {
            continue;
        }
        futex_wait(g, GUARD_WAITED, None);
    }
}

/// The initialiser finished: the first byte set (what the compiled code reads), and any thread asleep on the guard woken.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __cxa_guard_release(guard: *mut u64) {
    let g = unsafe { &*(guard as *const AtomicU64) };
    if g.swap(GUARD_DONE, Ordering::Release) == GUARD_WAITED {
        futex_wake_all(g);
    }
}

/// The initialiser did not finish (C++ would be unwinding; nothing here unwinds, so nothing calls this, but the ABI names it): the guard free again for the next thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __cxa_guard_abort(guard: *mut u64) {
    let g = unsafe { &*(guard as *const AtomicU64) };
    if g.swap(0, Ordering::Release) == GUARD_WAITED {
        futex_wake_all(g);
    }
}

/// A static object's destructor, to run at exit: a ferros process runs none (it ends, and the kernel takes its arc whole), so nothing is kept.
#[unsafe(no_mangle)]
pub extern "C" fn __cxa_atexit(_f: Option<unsafe extern "C" fn(*mut c_void)>, _arg: *mut c_void, _dso: *mut c_void) -> i32 {
    0
}

/// The image's handle for `__cxa_atexit` (one image, so its own address is the handle).
#[unsafe(no_mangle)]
pub static __dso_handle: u8 = 0;
