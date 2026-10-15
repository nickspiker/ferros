//! KOHI ("gather") in software: PIPE's entropy gathering and health stack (PIPE TRNG.md, rtl/ring_health.v), run over the entropy sources husky gives the AP.
//!
//! Each source gathers W = 65,536 one-bit samples and must pass, with ring_health.v's exact parameters:
//! - RCT (NIST SP 800-90B repetition count): no run of identical samples longer than 32;
//! - APT (adaptive proportion): in every 1024-sample window, the window's first value occurs at most 614 times (H = 1, alpha = 2^-30);
//! - iterated GF(2) differencing, levels 0..=6 (level k = level k-1 XOR its previous sample): every level's popcount, after the K-sample warmup, within [32000, 33536] (about W/2 +/- 6 sigma) — a periodic pattern of period <= 2^K collapses at some level.
//!
//! Byzantine threshold (TRNG.md: 8 of 16 rings): at least half of the sources present must be healthy, else KOHI fails and there is no value — NGARO: refuse to operate, no retry, no fallback. The healthy sources' buffers are XOR-accumulated (uniform if any one input is) and extracted with BLAKE3 into 256 bits.
//!
//! Sources on husky, in order: the firmware TRNG (SMCCC TRNG_RND64 at EL3), the ARMv9 RNDR register (only where FEAT_RNG exists), and CPU timing jitter (the parity of cycle-counter deltas over loads into a 1 MiB working set — physical, and it has to earn its place through the tests like the others).

use alloc::vec::Vec;

pub const W: usize = 65_536;
const WORDS: usize = W / 64;
const K: usize = 6;
const RCT_C: u32 = 32;
const APT_W: usize = 1024;
const APT_T: u32 = 614;
const LVL_LO: u32 = 32_000;
const LVL_HI: u32 = 33_536;

#[derive(Clone, Copy, Default)]
pub struct Health {
    pub present: bool,
    pub rct_fail: bool,
    pub apt_fail: bool,
    /// Bit k set: level k popcount out of range.
    pub level_fail: u8,
    /// Level-0 popcount (ones among the W samples), for the record.
    pub ones: u32,
    /// Gather time in microseconds.
    pub us: u32,
}

impl Health {
    pub fn ok(&self) -> bool {
        self.present && !self.rct_fail && !self.apt_fail && self.level_fail == 0
    }
    /// Packed for DIAG: present b0, rct_fail b1, apt_fail b2, level_fail b8..15, ones b16..31 (ones / 2, it fits).
    pub fn packed(&self) -> u32 {
        self.present as u32 | (self.rct_fail as u32) << 1 | (self.apt_fail as u32) << 2 | (self.level_fail as u32) << 8 | (self.ones / 2) << 16
    }
}

fn bit(buf: &[u64], i: usize) -> bool {
    buf[i / 64] >> (i % 64) & 1 != 0
}

/// PIPE's three-layer health check over one W-sample buffer (sample i = bit i % 64 of word i / 64).
pub fn health(buf: &[u64]) -> Health {
    let mut h = Health { present: true, ..Default::default() };
    // RCT
    let mut run = 1u32;
    for i in 1..W {
        if bit(buf, i) == bit(buf, i - 1) {
            run += 1;
            if run > RCT_C {
                h.rct_fail = true;
                break;
            }
        } else {
            run = 1;
        }
    }
    // APT
    for w in 0..W / APT_W {
        let base = w * APT_W;
        let r = bit(buf, base);
        let n = (base..base + APT_W).filter(|&i| bit(buf, i) == r).count() as u32;
        if n > APT_T {
            h.apt_fail = true;
            break;
        }
    }
    // Iterated GF(2) differencing: level 0 is the samples; each next level is the previous XOR itself delayed by one sample.
    let mut level: Vec<u64> = buf[..WORDS].to_vec();
    for k in 0..=K {
        if k > 0 {
            let mut carry = 0u64;
            for w in level.iter_mut() {
                let x = *w;
                *w = x ^ (x << 1 | carry);
                carry = x >> 63;
            }
        }
        // Count after the K-sample warmup (the delay chain filling), as ring_health.v does.
        let ones: u32 = level.iter().map(|w| w.count_ones()).sum::<u32>() - (level[0] & ((1u64 << K) - 1)).count_ones();
        if k == 0 {
            h.ones = level.iter().map(|w| w.count_ones()).sum();
        }
        if !(LVL_LO..=LVL_HI).contains(&ones) {
            h.level_fail |= 1 << k;
        }
    }
    h
}

const SMCCC_TRNG_RND64: u64 = 0xC400_0053;

fn trng_words(out: &mut [u64]) -> bool {
    let mut i = 0;
    while i < out.len() {
        let (status, x1, x2, x3): (u64, u64, u64, u64);
        unsafe {
            core::arch::asm!(
                "smc #0",
                inout("x0") SMCCC_TRNG_RND64 => status,
                inout("x1") 192u64 => x1,
                inout("x2") 0u64 => x2,
                inout("x3") 0u64 => x3,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _,
                out("x8") _, out("x9") _, out("x10") _, out("x11") _,
                out("x12") _, out("x13") _, out("x14") _, out("x15") _,
                out("x16") _, out("x17") _,
                options(nostack),
            );
        }
        if status != 0 {
            return false;
        }
        for w in [x3, x2, x1] {
            if i < out.len() {
                out[i] = w;
                i += 1;
            }
        }
    }
    true
}

fn has_feat_rng() -> bool {
    let isar0: u64;
    unsafe { core::arch::asm!("mrs {}, ID_AA64ISAR0_EL1", out(reg) isar0, options(nomem, nostack)) };
    (isar0 >> 60) & 0xF != 0
}

fn rndr_words(out: &mut [u64]) -> bool {
    if !has_feat_rng() {
        return false;
    }
    for o in out.iter_mut() {
        let v: u64;
        unsafe { core::arch::asm!("mrs {}, S3_3_C2_C4_0", out(reg) v, options(nomem, nostack)) };
        *o = v;
    }
    true
}

/// The CPU cycle counter (PMCCNTR_EL0), switched on to count at EL2 (PMCR_EL0.E, PMCNTENSET_EL0.C, PMCCFILTR_EL0.NSH). Linux perf uses it on Pixels, so EL3 does not trap it.
fn cycles_on() {
    unsafe {
        core::arch::asm!(
            "mov {t}, #(1 << 27)",
            "msr pmccfiltr_el0, {t}",
            "mrs {t}, pmcr_el0",
            "orr {t}, {t}, #1",
            "msr pmcr_el0, {t}",
            "mov {t}, #(1 << 31)",
            "msr pmcntenset_el0, {t}",
            "isb",
            t = out(reg) _,
        );
    }
}

fn cycles() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("isb", "mrs {}, pmccntr_el0", out(reg) v) };
    v
}

/// CPU timing jitter: per sample, the cycle count of a few data-dependent loads over a 1 MiB working set (past the L2, so each one lands somewhere in the cache/DRAM hierarchy), folded to its parity. Physical noise from the memory system and the clock domains between them — and it has to pass the same tests as the TRNG to be used.
fn jitter_words(out: &mut [u64]) -> bool {
    const SPAN: usize = 1 << 20;
    cycles_on();
    let a = cycles();
    let b = cycles();
    if a == b {
        return false; // counter not running
    }
    let mut scratch = alloc::vec![0u8; SPAN];
    let mut idx = 0usize;
    for o in out.iter_mut() {
        let mut word = 0u64;
        for b in 0..64 {
            let t0 = cycles();
            for _ in 0..4 {
                idx = (idx.wrapping_mul(2_654_435_761).wrapping_add(scratch[idx] as usize + 64)) & (SPAN - 1);
                scratch[idx] = scratch[idx].wrapping_add(1);
            }
            let dt = cycles().wrapping_sub(t0);
            word |= ((dt.count_ones() & 1) as u64) << b;
        }
        *o = word;
    }
    core::hint::black_box(&scratch);
    true
}

pub const SOURCES: [&str; 3] = ["trng", "rndr", "jitter"];

/// One KOHI: gather every source, health-check each, apply the byzantine threshold, XOR the healthy ones and extract 256 bits under `context`. Returns the value (None = NGARO) and each source's health.
pub fn gather(context: &str) -> (Option<[u8; 32]>, [Health; 3]) {
    let mut health_of = [Health::default(); 3];
    let mut pool = alloc::vec![0u64; WORDS];
    let mut buf = alloc::vec![0u64; WORDS];
    let mut present = 0;
    let mut healthy = 0;
    for (s, h) in health_of.iter_mut().enumerate() {
        let t0 = crate::platform::cntpct();
        let got = match s {
            0 => trng_words(&mut buf),
            1 => rndr_words(&mut buf),
            _ => jitter_words(&mut buf),
        };
        if !got {
            continue;
        }
        present += 1;
        let us = ((crate::platform::cntpct() - t0) * 1_000_000 / crate::platform::cntfrq()) as u32;
        *h = health(&buf);
        h.us = us;
        if h.ok() {
            healthy += 1;
            for (p, b) in pool.iter_mut().zip(buf.iter()) {
                *p ^= *b;
            }
        }
    }
    if healthy == 0 || healthy * 2 < present {
        return (None, health_of);
    }
    let mut bytes = Vec::with_capacity(W / 8);
    for w in &pool {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    (Some(blake3::derive_key(context, &bytes)), health_of)
}
