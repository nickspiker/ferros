//! ferros_rt's heap on the host (x86_64 Linux): the runtime's own sources by path, not copies, so what is tested here is what std links on the device.
//! `futex` is Linux's futex under ferros_rt's signatures, and `sys` is a fake arc: SYS_ARC's grow, shrink and size over one reserved range, the frames given back made unreadable so a touch after a shrink faults the test.
//!
//! cargo test --release
//! taskset -c 0-3 cargo run --release --example bench

pub mod futex;
pub mod sys;

#[path = "../../../ferros_rt/src/tlsf.rs"]
pub mod tlsf;

#[path = "../../../ferros_rt/src/lock.rs"]
pub mod lock;

#[path = "../../../ferros_rt/src/layout.rs"]
pub mod layout;

#[path = "../../../ferros_rt/src/alloc.rs"]
pub mod alloc;

/// SplitMix64: a small deterministic generator for the tests and the benchmark.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next() as usize) % (hi - lo)
    }

    /// Log-uniform in [lo, hi): as many sizes near 16 bytes as near 16 KiB, as a program's allocations are.
    pub fn log_size(&mut self, lo: usize, hi: usize) -> usize {
        let (a, b) = (lo.ilog2(), hi.ilog2());
        loop {
            let e = self.range(a as usize, b as usize + 1) as u32;
            let s = (1usize << e) + self.range(0, 1 << e);
            if s >= lo && s < hi {
                return s;
            }
        }
    }
}
