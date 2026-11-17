//! memcheck: ferros_rt's `memcpy`, `memmove` and `memset` (ferros_rt/src/mem.rs) checked on the device against a reference that moves one byte at a time thru volatile accesses (which LLVM can never turn back into a call to the functions under test).
//! Every length from 0 to 2^9, then 2^12 + 1 and 2^16 + 3; every source offset from 0 to 2^4 (alignment) and five destination offsets; for `memmove` every overlap with the destination from 2^7 below the source to 2^7 above it. The guard bytes around each destination must come back untouched: each case compares the whole window it could have touched and 2^6 bytes past either end.
//! `ferros-bridge spawn memcheck.bin --name memcheck`: one line for each function, and a line for the first mismatch of each if any.

/// Bytes checked past either end of the window a case may touch.
const GUARD: usize = 1 << 6;
/// The largest overlap tried either way, and the largest offset.
const DELTA: isize = 1 << 7;
const OFFS: usize = 1 << 4;

unsafe extern "C" {
    fn memcpy(d: *mut u8, s: *const u8, n: usize) -> *mut u8;
    fn memmove(d: *mut u8, s: *const u8, n: usize) -> *mut u8;
    fn memset(d: *mut u8, c: i32, n: usize) -> *mut u8;
}

/// The window's pattern: a byte from the position, with a seed so every case starts different.
fn pattern(buf: &mut [u8], seed: u32) {
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(2654435761).wrapping_add(seed).rotate_right(13) as u8;
    }
}

/// The window copied as the reference starts, byte by byte and volatile: not by the `memcpy` under test.
fn vcopy(to: &mut [u8], from: &[u8]) {
    for i in 0..to.len() {
        unsafe { core::ptr::write_volatile(&mut to[i], core::ptr::read_volatile(&from[i])) };
    }
}

/// The first byte where two windows differ.
fn diff(x: &[u8], y: &[u8]) -> Option<usize> {
    x.iter().zip(y).position(|(p, q)| p != q)
}

fn main() {
    let mut lens: Vec<usize> = (0..=(1 << 9)).collect();
    lens.extend([(1 << 12) + 1, (1 << 16) + 3]);
    let mut cases = [0u64; 3];
    let mut bad = [0u64; 3];
    let mut seed = 0u32;
    // Where the source sits in the window: room for the guards, the largest overlap below it and the offsets.
    let base = GUARD + DELTA as usize + OFFS;
    for &n in &lens {
        let win = base + n + DELTA as usize + OFFS + GUARD;
        let mut a = vec![0u8; win];
        let mut b = vec![0u8; win];
        let mut want = vec![0u8; win];
        let mut tmp = vec![0u8; n];
        // Every overlap for the lengths to 2^9; the edges around a block for the two long ones.
        let deltas: Vec<isize> = if n <= 1 << 9 { (-DELTA..=DELTA).collect() } else { vec![-DELTA, -65, -64, -63, -1, 0, 1, 63, 64, 65, DELTA] };
        for so in 0..OFFS {
            seed = seed.wrapping_add(1);
            let s = base + so;
            // memcpy: the source in `b`, the destination in `a`.
            for dof in [0usize, 1, 7, 8, 15] {
                let d = base + dof;
                pattern(&mut a, seed);
                pattern(&mut b, seed ^ 0x5A5A);
                vcopy(&mut want, &a);
                for i in 0..n {
                    unsafe { core::ptr::write_volatile(&mut want[d + i], core::ptr::read_volatile(&b[s + i])) };
                }
                unsafe { memcpy(a.as_mut_ptr().add(d), b.as_ptr().add(s), n) };
                cases[0] += 1;
                if let Some(at) = diff(&a, &want) {
                    if bad[0] == 0 {
                        println!("memcheck: memcpy WRONG: n {n} dst +{dof} src +{so}: first difference at {at} (dst at {d})");
                    }
                    bad[0] += 1;
                }
            }
            // memmove within `a`, every overlap: the reference reads every source byte before it writes any.
            for &dl in &deltas {
                let d = (s as isize + dl) as usize;
                pattern(&mut a, seed);
                vcopy(&mut want, &a);
                for i in 0..n {
                    tmp[i] = unsafe { core::ptr::read_volatile(&want[s + i]) };
                }
                for i in 0..n {
                    unsafe { core::ptr::write_volatile(&mut want[d + i], tmp[i]) };
                }
                unsafe { memmove(a.as_mut_ptr().add(d), a.as_ptr().add(s), n) };
                cases[1] += 1;
                if let Some(at) = diff(&a, &want) {
                    if bad[1] == 0 {
                        println!("memcheck: memmove WRONG: n {n} src +{so} delta {dl}: first difference at {at} (src at {s})");
                    }
                    bad[1] += 1;
                }
            }
            // memset: only the low byte of the value counts.
            pattern(&mut a, seed);
            vcopy(&mut want, &a);
            let c = seed as u8 ^ 0xA5;
            for i in 0..n {
                unsafe { core::ptr::write_volatile(&mut want[s + i], c) };
            }
            unsafe { memset(a.as_mut_ptr().add(s), c as i32 | 0x7F00, n) };
            cases[2] += 1;
            if let Some(at) = diff(&a, &want) {
                if bad[2] == 0 {
                    println!("memcheck: memset WRONG: n {n} dst +{so}: first difference at {at}");
                }
                bad[2] += 1;
            }
        }
    }
    for (k, name) in ["memcpy", "memmove", "memset"].iter().enumerate() {
        println!("memcheck: {name}: {} cases, {} wrong", cases[k], bad[k]);
    }
    println!("memcheck: done");
}
