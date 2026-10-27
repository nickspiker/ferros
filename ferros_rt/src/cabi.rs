//! The few C symbols the C in a std image's graph calls (PQClean through `tools/std-sysroot`, rav1d by name through libc): the allocator over the process heap, with the size kept in a 16-byte header before each block, and `strlen`. The mem routines (memcpy, memset, memmove, memcmp) are compiler_builtins'.

use core::alloc::Layout;

const HEADER: usize = 16;

fn layout(size: usize) -> Layout {
    unsafe { Layout::from_size_align_unchecked(size + HEADER, HEADER) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: usize) -> *mut u8 {
    let p = unsafe { crate::alloc::alloc(layout(size)) };
    if p.is_null() {
        return p;
    }
    unsafe {
        (p as *mut usize).write(size);
        p.add(HEADER)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn calloc(count: usize, size: usize) -> *mut u8 {
    let n = count * size;
    let p = unsafe { malloc(n) };
    if !p.is_null() {
        unsafe { core::ptr::write_bytes(p, 0, n) };
    }
    p
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    unsafe {
        let base = p.sub(HEADER);
        let size = (base as *const usize).read();
        crate::alloc::dealloc(base, layout(size));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(p: *mut u8, size: usize) -> *mut u8 {
    if p.is_null() {
        return unsafe { malloc(size) };
    }
    let old = unsafe { (p.sub(HEADER) as *const usize).read() };
    let q = unsafe { malloc(size) };
    if !q.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(p, q, if old < size { old } else { size });
            free(p);
        }
    }
    q
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strlen(s: *const u8) -> usize {
    let mut n = 0;
    while unsafe { s.add(n).read() } != 0 {
        n += 1;
    }
    n
}
