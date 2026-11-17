//! membench: memory bandwidth on the A715s, for the clock lift's memory side (KERNEL.md Order item 8; 2026-11-17, a 30 MB memmove ran at ~8 GB/s and four cores were no faster than one).
//! Spawned as any std payload: `ferros-bridge spawn membench.bin --name membench`; everything it prints is the console's.
//! Two buffers of 2^24 bytes each (past the DSU's L3 and the system cache together), copied (`copy_from_slice`, compiler-builtins' memcpy as every std app has it), read (a sum of words) and written (a fill), by one thread and by four, each the best of 2^2 passes; then the copy by 128-bit pairs (`ldp`/`stp` of q registers) to tell the memcpy's own speed from the memory's.

use std::time::Instant;

/// Bytes a buffer: 2^24 (16 MiB), two of them past every cache on the way to DRAM.
const LEN: usize = 1 << 24;
/// Passes a measure, the best taken.
const PASSES: usize = 1 << 2;

/// The copy by q-register pairs: 64 bytes a turn, `len` a multiple of 64.
fn copy_q(dst: &mut [u8], src: &[u8]) {
    let n = src.len() & !63;
    unsafe {
        let (mut s, mut d, end) = (src.as_ptr(), dst.as_mut_ptr(), src.as_ptr().add(n));
        while s < end {
            core::arch::asm!(
                "ldp q0, q1, [{s}]", "ldp q2, q3, [{s}, #32]", "stp q0, q1, [{d}]", "stp q2, q3, [{d}, #32]",
                s = in(reg) s, d = in(reg) d, out("v0") _, out("v1") _, out("v2") _, out("v3") _,
            );
            s = s.add(64);
            d = d.add(64);
        }
    }
}

/// The best of `PASSES` runs of `job` over `threads` threads, each given its own `1/threads` of the buffers; GB/s by `bytes` moved a pass.
fn best(threads: usize, bytes: usize, src: &[u8], dst: &mut [u8], job: fn(&mut [u8], &[u8]) -> u64) -> (f64, u64) {
    let mut fastest = f64::MAX;
    let mut sink = 0;
    for _ in 0..PASSES {
        let part = src.len() / threads;
        let t = Instant::now();
        std::thread::scope(|sc| {
            let hs: Vec<_> = dst.chunks_mut(part).zip(src.chunks(part)).map(|(d, s)| sc.spawn(move || job(d, s))).collect();
            for h in hs {
                sink ^= h.join().unwrap();
            }
        });
        fastest = fastest.min(t.elapsed().as_secs_f64());
    }
    (bytes as f64 / fastest / 1e9, sink)
}

fn copy_std(d: &mut [u8], s: &[u8]) -> u64 {
    d.copy_from_slice(s);
    d[0] as u64
}
fn copy_pairs(d: &mut [u8], s: &[u8]) -> u64 {
    copy_q(d, s);
    d[0] as u64
}
fn read(_: &mut [u8], s: &[u8]) -> u64 {
    s.chunks_exact(8).map(|w| u64::from_le_bytes(w.try_into().unwrap())).fold(0, |a, w| a.wrapping_add(w))
}
fn write(d: &mut [u8], _: &[u8]) -> u64 {
    d.fill(0x5A);
    d[0] as u64
}

fn main() {
    let src: Vec<u8> = (0..LEN).map(|i| i as u8).collect();
    let mut dst = vec![0u8; LEN];
    let mut sink = 0;
    for threads in [1usize, 4] {
        let rows: [(&str, usize, fn(&mut [u8], &[u8]) -> u64); 4] = [("copy (memcpy)", 2 * LEN, copy_std), ("copy (q pairs)", 2 * LEN, copy_pairs), ("read", LEN, read), ("write", LEN, write)];
        for (name, bytes, job) in rows {
            let (gbs, s) = best(threads, bytes, &src, &mut dst, job);
            sink ^= s;
            println!("membench: {threads} thread(s) {name:<15} {gbs:>6.2} GB/s (read + written)");
        }
    }
    println!("membench: done ({sink:x})");
}
