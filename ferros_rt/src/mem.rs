//! `memcpy`, `memmove` and `memset` for every std program on ferros, by 128-bit register pairs (`ldp`/`stp` of q registers).
//! compiler-builtins' `mem` feature gives the target its own, weakly linked, as a word loop: measured 2026-11-17 on the Pro (std/membench, 2^24-byte buffers), one thread copied 13.1 GB/s with it at 1065 MHz against 18.9 by q pairs, and 23.4 against 32.3 at 2367 MHz with the memory clocks lifted. These are strong symbols, so the linker takes them over the builtins'.
//!
//! The shape, the same for all three: up to 64 bytes, the first and the last 2^k bytes for the largest 2^k not over the length (two accesses that overlap in the middle, so every length is covered with no byte loop); past 64, the last 64 bytes are loaded first, then the 64-byte blocks from the start, then the last 64 stored where the blocks end. Every access is unaligned (the A715 and the X3 take an unaligned q pair at the aligned rate within a line, and a page's memory type is Normal throughout an arc).
//! Each length class loads everything it will store before it stores anything, so an overlapping `memmove` is right in either direction up to 64 bytes; past 64 it runs the blocks from the end when the destination is above the source and within the length (`memmove`), with the first 64 bytes held back the same way.
//! The loops and the accesses are inline assembly: a copy written in Rust is what LLVM turns back into a call to `memcpy`.

use core::arch::asm;

/// `memcpy`: `n` bytes from `src` to `dst`, which do not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    unsafe { forward(dst, src, n) };
    dst
}

/// `memmove`: `n` bytes from `src` to `dst`, which may overlap: forward unless `dst` is above `src` and within `n` of it (then the blocks run from the end).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // WHY wrapping: the distance as an unsigned number, as compiler-builtins takes it: below `n` exactly when `dst` lies in (src, src + n).
    if (dst as usize).wrapping_sub(src as usize) < n && n > 64 {
        unsafe { backward(dst, src, n) };
    } else {
        unsafe { forward(dst, src, n) };
    }
    dst
}

/// `memset`: `n` bytes at `dst` set to the low byte of `c`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
    unsafe { fill(dst, c as u8, n) };
    dst
}

/// Up to 64 bytes, any overlap: everything loaded, then everything stored.
#[inline(always)]
unsafe fn small(d: *mut u8, s: *const u8, n: usize) {
    unsafe {
        if n >= 32 {
            asm!("ldp q0, q1, [{s}]", "ldp q2, q3, [{se}]", "stp q0, q1, [{d}]", "stp q2, q3, [{de}]",
                s = in(reg) s, se = in(reg) s.add(n - 32), d = in(reg) d, de = in(reg) d.add(n - 32),
                out("v0") _, out("v1") _, out("v2") _, out("v3") _, options(nostack, preserves_flags));
        } else if n >= 16 {
            asm!("ldr q0, [{s}]", "ldr q1, [{se}]", "str q0, [{d}]", "str q1, [{de}]",
                s = in(reg) s, se = in(reg) s.add(n - 16), d = in(reg) d, de = in(reg) d.add(n - 16),
                out("v0") _, out("v1") _, options(nostack, preserves_flags));
        } else if n >= 8 {
            asm!("ldr {a}, [{s}]", "ldr {b}, [{se}]", "str {a}, [{d}]", "str {b}, [{de}]",
                s = in(reg) s, se = in(reg) s.add(n - 8), d = in(reg) d, de = in(reg) d.add(n - 8),
                a = out(reg) _, b = out(reg) _, options(nostack, preserves_flags));
        } else if n >= 4 {
            asm!("ldr {a:w}, [{s}]", "ldr {b:w}, [{se}]", "str {a:w}, [{d}]", "str {b:w}, [{de}]",
                s = in(reg) s, se = in(reg) s.add(n - 4), d = in(reg) d, de = in(reg) d.add(n - 4),
                a = out(reg) _, b = out(reg) _, options(nostack, preserves_flags));
        } else if n >= 2 {
            asm!("ldrh {a:w}, [{s}]", "ldrh {b:w}, [{se}]", "strh {a:w}, [{d}]", "strh {b:w}, [{de}]",
                s = in(reg) s, se = in(reg) s.add(n - 2), d = in(reg) d, de = in(reg) d.add(n - 2),
                a = out(reg) _, b = out(reg) _, options(nostack, preserves_flags));
        } else if n == 1 {
            asm!("ldrb {a:w}, [{s}]", "strb {a:w}, [{d}]", s = in(reg) s, d = in(reg) d, a = out(reg) _, options(nostack, preserves_flags));
        }
    }
}

/// Forward: the blocks from the start, the last 64 bytes loaded before them and stored after (right for no overlap, and for `dst` at or below `src`).
#[inline(always)]
unsafe fn forward(d: *mut u8, s: *const u8, n: usize) {
    if n <= 64 {
        unsafe { small(d, s, n) };
        return;
    }
    unsafe {
        asm!(
            "ldp q4, q5, [{st}]", "ldp q6, q7, [{st}, #32]",
            "2:",
            "ldp q0, q1, [{s}]", "ldp q2, q3, [{s}, #32]", "add {s}, {s}, #64",
            "stp q0, q1, [{d}]", "stp q2, q3, [{d}, #32]", "add {d}, {d}, #64",
            "cmp {d}, {e}", "b.lo 2b",
            "stp q4, q5, [{e}]", "stp q6, q7, [{e}, #32]",
            s = inout(reg) s => _, d = inout(reg) d => _, st = in(reg) s.add(n - 64), e = in(reg) d.add(n - 64),
            out("v0") _, out("v1") _, out("v2") _, out("v3") _, out("v4") _, out("v5") _, out("v6") _, out("v7") _,
            options(nostack),
        );
    }
}

/// Backward, past 64 bytes with `dst` above `src` and within `n`: the blocks from the end, the first 64 bytes loaded before them and stored after.
#[inline(always)]
unsafe fn backward(d: *mut u8, s: *const u8, n: usize) {
    unsafe {
        asm!(
            "ldp q4, q5, [{s0}]", "ldp q6, q7, [{s0}, #32]",
            "2:",
            "ldp q0, q1, [{sp}]", "ldp q2, q3, [{sp}, #32]", "sub {sp}, {sp}, #64",
            "stp q0, q1, [{dp}]", "stp q2, q3, [{dp}, #32]", "sub {dp}, {dp}, #64",
            "cmp {dp}, {d0}", "b.hi 2b",
            "stp q4, q5, [{d0}]", "stp q6, q7, [{d0}, #32]",
            s0 = in(reg) s, d0 = in(reg) d, sp = inout(reg) s.add(n - 64) => _, dp = inout(reg) d.add(n - 64) => _,
            out("v0") _, out("v1") _, out("v2") _, out("v3") _, out("v4") _, out("v5") _, out("v6") _, out("v7") _,
            options(nostack),
        );
    }
}

/// The fill: the byte in every lane of q0, then the same shape as the copy (the first and last 2^k bytes up to 64, the blocks and the last 64 past it).
#[inline(always)]
unsafe fn fill(d: *mut u8, c: u8, n: usize) {
    unsafe {
        if n > 64 {
            asm!(
                "dup v0.16b, {c:w}",
                "2:",
                "stp q0, q0, [{d}]", "stp q0, q0, [{d}, #32]", "add {d}, {d}, #64",
                "cmp {d}, {e}", "b.lo 2b",
                "stp q0, q0, [{e}]", "stp q0, q0, [{e}, #32]",
                c = in(reg) c as u32, d = inout(reg) d => _, e = in(reg) d.add(n - 64), out("v0") _, options(nostack),
            );
        } else if n >= 32 {
            asm!("dup v0.16b, {c:w}", "stp q0, q0, [{d}]", "stp q0, q0, [{de}]", c = in(reg) c as u32, d = in(reg) d, de = in(reg) d.add(n - 32), out("v0") _, options(nostack, preserves_flags));
        } else if n >= 16 {
            asm!("dup v0.16b, {c:w}", "str q0, [{d}]", "str q0, [{de}]", c = in(reg) c as u32, d = in(reg) d, de = in(reg) d.add(n - 16), out("v0") _, options(nostack, preserves_flags));
        } else if n >= 8 {
            // The byte in every byte of a word: times G#0101_0101_0101_0101.
            let w = c as u64 * 0x0101_0101_0101_0101;
            asm!("str {w}, [{d}]", "str {w}, [{de}]", w = in(reg) w, d = in(reg) d, de = in(reg) d.add(n - 8), options(nostack, preserves_flags));
        } else if n >= 4 {
            let w = c as u32 * 0x0101_0101;
            asm!("str {w:w}, [{d}]", "str {w:w}, [{de}]", w = in(reg) w, d = in(reg) d, de = in(reg) d.add(n - 4), options(nostack, preserves_flags));
        } else if n >= 2 {
            let w = c as u32 * 0x0101;
            asm!("strh {w:w}, [{d}]", "strh {w:w}, [{de}]", w = in(reg) w, d = in(reg) d, de = in(reg) d.add(n - 2), options(nostack, preserves_flags));
        } else if n == 1 {
            asm!("strb {w:w}, [{d}]", w = in(reg) c as u32, d = in(reg) d, options(nostack, preserves_flags));
        }
    }
}
