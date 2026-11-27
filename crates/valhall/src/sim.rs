//! A host emulator of Valhall words (feature `sim`, host only): the forms the SSA compiler emits and the device-verified kernels use, decoded from the words themselves, one invocation at a time.
//! It is how a kernel is checked before it reaches the device (owner, 2026-11-11: "Prove on the host what you can"): the compiler's register allocation, its loop copies, its constants in LUT, uniform and register places, its branches, run as the GPU would run them, not the recording they came from.
//! It is checked itself against the words the device ran (tests: the Mandelbrot kernel, bit for bit against the CPU as on the device).
//!
//! What it is not: the hardware's rounding of the special function unit. FRCP and FRSQ come out correctly rounded here (within an ulp on the device, gputest 10); FLOGD, FADD_LSCALE and FREXPE's `log` form are one self-consistent split x = m · 2^e with m in [0.75, 1.5) whose sequence (`F::log2_hw`) gives log₂ x (within an ulp on the device, gputest 12); FEXP is 2 to the fixed-point power. A picture through these looks as the device's does; its bits may differ from the device's by an ulp here and there, as they may from the CPU's.
//! The v2f16 forms (FADD, FMA, FMIN, FMAX), an f32 operand widened from an f16 half, FADD.f32 narrowing into a half register, and the destination clamps are emulated from ISA.xml and Mesa's test vectors, each lane rounded to f16 once (an FMA from its exact f64 product and sum); not yet checked against the device, whose f16 denormals and NaNs it does not claim.
//!
//! An invocation runs alone, so a divergent branch is a plain jump: the per-thread result is the warp's, since nothing here reads another lane.
//! A workgroup (`run_group`, 2026-11-21) runs its invocations one after another up to each BARRIER in turn, with its own workgroup-local storage at `WLS_VA` (what the WLS pointer reads); a word one invocation wrote and another read or wrote between two barriers panics naming both, since on the device that order is the warps', not the kernel's.
//! A workgroup (`run_group`, 2026-11-21) runs its invocations one after another to each BARRIER in turn, with its own workgroup-local storage at `WLS_VA` (what the WLS pointer reads); a word one invocation wrote and another read or wrote between two barriers is a panic naming both, since on the device that order is the warps' and not the kernel's.

extern crate std;

use crate::{LUT, f16_to_f32, f32_to_f16};

/// The memory a kernel sees: `bytes` at GPU virtual address `base`.
pub struct Memory {
    pub base: u64,
    pub bytes: std::vec::Vec<u8>,
}

impl Memory {
    /// The byte offset of `va`, `n` bytes long. An address outside is a fault on the device (the GPU's MMU), so here a panic naming it.
    fn at(&self, va: u64, n: usize) -> usize {
        // WHY: the address is the kernel's own arithmetic; one outside the buffer is the bug this emulator exists to show, as the device's MMU would fault it.
        assert!(va >= self.base && va + n as u64 <= self.base + self.bytes.len() as u64, "valhall::sim: an access at G#{va:X} outside G#{:X}..G#{:X}", self.base, self.base + self.bytes.len() as u64);
        (va - self.base) as usize
    }
    pub fn read32(&self, va: u64) -> u32 {
        let o = self.at(va, 4);
        u32::from_le_bytes(self.bytes[o..o + 4].try_into().unwrap())
    }
    pub fn read8(&self, va: u64) -> u8 {
        self.bytes[self.at(va, 1)]
    }
    pub fn read16(&self, va: u64) -> u16 {
        let o = self.at(va, 2);
        u16::from_le_bytes([self.bytes[o], self.bytes[o + 1]])
    }
    pub fn write32(&mut self, va: u64, v: u32) {
        let o = self.at(va, 4);
        self.bytes[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
}

/// Instructions one invocation may run before the emulator calls it endless (the device's progress timer would stop it).
const STEPS: u64 = 1 << 32;

/// Runs `words` for one invocation: r60 the global id X and r61 Y (the service's preload), `uniforms` its FAU slots, over `mem`. Returns how many instructions it ran.
pub fn run(words: &[u64], uniforms: &[u64], gid: [u32; 2], mem: &mut Memory) -> u64 {
    run_marked(words, uniforms, gid, mem, &mut [])
}

/// As [`run`], marking in `ran` (one flag an instruction, or empty for none) every instruction the invocation ran: the union over a warp's sixteen invocations is what the warp issues, a branch over a block skipped only where all sixteen skip it.
pub fn run_marked(words: &[u64], uniforms: &[u64], gid: [u32; 2], mem: &mut Memory, ran: &mut [bool]) -> u64 {
    let mut inv = Inv::new(gid, 0);
    match exec(words, uniforms, &mut inv, mem, None, ran) {
        Stop::End => inv.steps,
        // WHY: a barrier waits for the rest of a workgroup, which a lone invocation does not have; `run_group` runs one.
        Stop::Barrier => panic!("valhall::sim: a BARRIER in a kernel run one invocation at a time (run_group runs a workgroup)"),
    }
}

/// Where a workgroup's local storage (shared memory) lies in the emulator's addresses: what the WLS pointer special reads (Mesa adds an offset to its low word and keeps its high word, so its low word is 0 here and nothing carries).
pub const WLS_VA: u64 = 0x7F_0000_0000;

/// A workgroup's local storage, with the order of its accesses checked: a word one invocation wrote and another read or wrote between the same two barriers is a race on the device (the warps run in no order the kernel may count on), so here a panic naming it, though the emulator runs the invocations one after another and would read a value.
pub struct Wls {
    bytes: std::vec::Vec<u8>,
    /// Per 32-bit word: the last writer (invocation + 1, 0 none) and the barrier epoch of that write; the last reader (invocation + 1, `MANY` for several) and its epoch.
    write: std::vec::Vec<(u32, u32)>,
    read: std::vec::Vec<(u32, u32)>,
    epoch: u32,
}

const MANY: u32 = u32::MAX;

impl Wls {
    pub fn new(bytes: usize) -> Wls {
        Wls { bytes: std::vec![0; bytes], write: std::vec![(0, 0); bytes.div_ceil(4)], read: std::vec![(0, 0); bytes.div_ceil(4)], epoch: 1 }
    }
    fn holds(&self, va: u64) -> bool {
        va >= WLS_VA && va < WLS_VA + self.bytes.len() as u64
    }
    fn at(&self, va: u64, n: usize) -> usize {
        // WHY: as `Memory::at`: an access past the workgroup's storage is the kernel's bug (on the device another workgroup's storage, or a fault).
        assert!(va + n as u64 <= WLS_VA + self.bytes.len() as u64, "valhall::sim: a workgroup storage access at G#{va:X} past its G#{:X} bytes", self.bytes.len());
        (va - WLS_VA) as usize
    }
    fn read(&mut self, va: u64, n: usize, who: u32) -> u32 {
        let o = self.at(va, n);
        let wd = o / 4;
        let (w, e) = self.write[wd];
        assert!(!(e == self.epoch && w != 0 && w != who + 1), "valhall::sim: invocation {who} read workgroup storage +G#{o:X} that invocation {} wrote with no barrier between", w - 1);
        let (r, re) = self.read[wd];
        self.read[wd] = (if re == self.epoch && r != 0 && r != who + 1 { MANY } else { who + 1 }, self.epoch);
        let mut b = [0u8; 4];
        b[..n].copy_from_slice(&self.bytes[o..o + n]);
        u32::from_le_bytes(b)
    }
    fn write(&mut self, va: u64, v: u32, who: u32) {
        let o = self.at(va, 4);
        let wd = o / 4;
        let (w, e) = self.write[wd];
        assert!(!(e == self.epoch && w != 0 && w != who + 1), "valhall::sim: invocations {} and {who} both wrote workgroup storage +G#{o:X} with no barrier between", w - 1);
        let (r, re) = self.read[wd];
        assert!(!(re == self.epoch && r != 0 && r != who + 1), "valhall::sim: invocation {who} wrote workgroup storage +G#{o:X} that {} read with no barrier between", if r == MANY { std::string::String::from("several") } else { std::format!("invocation {}", r - 1) });
        self.write[wd] = (who + 1, self.epoch);
        self.bytes[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
}

/// One invocation's state between its runs (a barrier stops it).
struct Inv {
    reg: [u32; 64],
    pc: usize,
    steps: u64,
    /// Its index in its workgroup.
    index: u32,
}

impl Inv {
    fn new(gid: [u32; 2], index: u32) -> Inv {
        let mut reg = [0u32; 64];
        reg[60] = gid[0];
        reg[61] = gid[1];
        Inv { reg, pc: 0, steps: 0, index }
    }
}

/// Why an invocation's run stopped.
enum Stop {
    End,
    Barrier,
}

/// Runs one workgroup of `size` invocations, global ids x `first` to `first + size - 1` (y `y`), with `wls_bytes` of workgroup-local storage, zeroed (the device leaves it as it was; a kernel reads only what its workgroup wrote): each invocation runs to its next BARRIER or its end, and every one must reach each barrier before any goes past it. Returns the instructions run in all.
/// The barrier's memory rule is checked as it is the device's: a word of the workgroup's storage written by one invocation and read or written by another between two barriers panics (`Wls`).
pub fn run_group(words: &[u64], uniforms: &[u64], first: u32, y: u32, size: u32, wls_bytes: usize, mem: &mut Memory) -> u64 {
    let mut wls = Wls::new(wls_bytes);
    let mut invs: std::vec::Vec<Inv> = (0..size).map(|i| Inv::new([first + i, y], i)).collect();
    loop {
        let (mut ended, mut waiting) = (0u32, 0u32);
        for inv in invs.iter_mut() {
            match exec(words, uniforms, inv, mem, Some(&mut wls), &mut []) {
                Stop::End => ended += 1,
                Stop::Barrier => waiting += 1,
            }
        }
        // WHY: a barrier some invocations of the workgroup never reach holds the rest for ever on the device (or worse); the kernel must reach every barrier in uniform control flow.
        assert!(waiting == 0 || ended == 0, "valhall::sim: {ended} of {size} invocations ended while {waiting} wait at a barrier");
        if waiting == 0 {
            return invs.iter().map(|v| v.steps).sum();
        }
        wls.epoch += 1;
    }
}

/// Runs `inv` from where it stopped to its next BARRIER (past it, so the next run goes on after it) or its end.
fn exec(words: &[u64], uniforms: &[u64], inv: &mut Inv, mem: &mut Memory, mut wls: Option<&mut Wls>, ran: &mut [bool]) -> Stop {
    let mut reg = inv.reg;
    let mut pc = inv.pc;
    let mut steps = inv.steps;
    let lane = inv.index & 15;
    let who = inv.index;
    let has_wls = wls.is_some();
    loop {
        let w = words[pc];
        if !ran.is_empty() {
            ran[pc] = true;
        }
        steps += 1;
        assert!(steps < STEPS, "valhall::sim: no end after 2^32 instructions");
        let page = (w >> 57) & 3;
        // A source byte: a register, a uniform word (its slot's low 5 bits, the page above), or a LUT entry.
        let src = |reg: &[u32; 64], byte: u64| -> u32 {
            let b = byte & 0xFF;
            match b >> 6 {
                0 => reg[(b & 63) as usize],
                2 => {
                    let slot = (page << 5 | (b >> 1) & 31) as usize;
                    (uniforms[slot] >> (32 * (b & 1))) as u32
                }
                3 if b & 32 == 0 => LUT[(b & 31) as usize],
                // A special FAU value (G#E0 | index << 1 | word, its page in 58:57): the workgroup's storage pointer (page 1, 3: `WLS_VA`), the lane (page 3, 1), the core (page 3, 3: 0 here).
                3 => {
                    let v: u64 = match (page, (b >> 1) & 15) {
                        (1, 3) => {
                            // WHY: a workgroup storage address read where none was given (`run` runs a lone invocation) would send its accesses to the global memory's addresses.
                            assert!(has_wls, "valhall::sim: the WLS pointer read in a run with no workgroup storage (run_group gives one) at {pc}");
                            WLS_VA
                        }
                        (3, 1) => lane as u64,
                        (3, 3) => 0,
                        (p, i) => panic!("valhall::sim: special FAU value page {p} index {i} at {pc}"),
                    };
                    (v >> (32 * (b & 1))) as u32
                }
                _ => panic!("valhall::sim: source byte G#{b:X} at {pc}"),
            }
        };
        // A 32-bit source in ALU position i, and as an f32 with its modifiers (the widen from an f16 half at 28 − 2i, abs, then negate).
        let s = |reg: &[u32; 64], i: u32| src(reg, w >> (8 * i));
        let f = |reg: &[u32; 64], i: u32| {
            let raw = s(reg, i);
            let mut v = match (w >> (28 - 2 * i)) & 3 {
                0 => f32::from_bits(raw),
                1 => f16_to_f32(raw as u16),
                2 => f16_to_f32((raw >> 16) as u16),
                _ => panic!("valhall::sim: a reserved widen at {pc}"),
            };
            if (w >> (39 - 2 * i)) & 1 == 1 {
                v = v.abs();
            }
            if (w >> (38 - 2 * i)) & 1 == 1 {
                v = -v;
            }
            v
        };
        // A v2f16 source in ALU position i: its two lanes by the swizzle at 28 − 2i (H00, H10, H01, H11), then abs and negate on both.
        let h = |reg: &[u32; 64], i: u32| -> [f32; 2] {
            let raw = s(reg, i);
            let (lo, hi) = (f16_to_f32(raw as u16), f16_to_f32((raw >> 16) as u16));
            let mut v = match (w >> (28 - 2 * i)) & 3 {
                0 => [lo, lo],
                1 => [hi, lo],
                2 => [lo, hi],
                _ => [hi, hi],
            };
            for x in v.iter_mut() {
                if (w >> (39 - 2 * i)) & 1 == 1 {
                    *x = x.abs();
                }
                if (w >> (38 - 2 * i)) & 1 == 1 {
                    *x = -*x;
                }
            }
            v
        };
        // The destination clamp (33:32) of FADD, FMA, FMIN and FMAX: a NaN clamps to the low end, and −0 to +0 at a bound of 0 (ISA.xml's `Clamp`).
        let clamp = |v: f32| -> f32 {
            match (w >> 32) & 3 {
                0 => v,
                1 => {
                    if v > 0. { v } else { 0. }
                }
                2 => {
                    if v > -1. { v.min(1.) } else { -1. }
                }
                _ => {
                    if v > 0. { v.min(1.) } else { 0. }
                }
            }
        };
        let pack16 = |v: [f32; 2]| f32_to_f16(clamp(v[0])) as u32 | (f32_to_f16(clamp(v[1])) as u32) << 16;
        let dst = ((w >> 40) & 63) as usize;
        // A full 32-bit destination (write mask 3); FADD.f32 alone writes a half (narrowing to f16, below).
        let full = (w >> 46) & 3 == 3;
        let op = (w >> 48) & 0x1FF;
        let op2 = (w >> 16) & 0xF;
        let put = |reg: &mut [u32; 64], v: u32| {
            assert!(full, "valhall::sim: a half-register destination at {pc} (G#{w:016X})");
            reg[dst] = v;
        };
        let mut next = pc + 1;
        match op {
            // IADD_IMM.i32: a + the immediate in 39:8.
            0x110 => {
                let v = s(&reg, 0).wrapping_add((w >> 8) as u32);
                put(&mut reg, v);
            }
            // MOV.i32.
            0x091 => {
                let v = s(&reg, 0);
                put(&mut reg, v);
            }
            // IADD.u32 (opcode2 0), IMUL.i32 (G#A).
            0x0A0 => {
                let (a, b) = (s(&reg, 0), s(&reg, 1));
                let v = match op2 {
                    0 => a.wrapping_add(b),
                    0xA => a.wrapping_mul(b),
                    _ => panic!("valhall::sim: G#0A0 opcode2 {op2} at {pc}"),
                };
                put(&mut reg, v);
            }
            // ISUB.s32 (opcode2 1).
            0x0A8 => {
                assert_eq!(op2, 1, "valhall::sim: G#0A8 opcode2 at {pc}");
                let v = s(&reg, 0).wrapping_sub(s(&reg, 1));
                put(&mut reg, v);
            }
            // SHADDX.u64: a (a pair or a uniform slot) + (b's low word << 22:20).
            0x1A3 => {
                let a = w & 0xFF;
                let base = if a >> 6 == 2 { uniforms[(page << 5 | (a >> 1) & 31) as usize] } else { reg[(a & 63) as usize] as u64 | (reg[(a & 63) as usize + 1] as u64) << 32 };
                let b = reg[((w >> 8) & 63) as usize] as u64;
                let v = base.wrapping_add(b << ((w >> 20) & 7));
                reg[dst] = v as u32;
                reg[dst + 1] = (v >> 32) as u32;
            }
            // The shift group: A (0), the shift (byte 29:26 - 4 of source 1), B (2); bit 32 shifts left, 25:24 is and, or, xor.
            0x0B4 => {
                let (a, sh, b) = (s(&reg, 0), s(&reg, 1), s(&reg, 2));
                let amount = (sh >> (8 * (((w >> 26) & 15) - 4))) & 0xFF;
                let shifted = if (w >> 32) & 1 == 1 { a.wrapping_shl(amount) } else { a.wrapping_shr(amount) };
                // WHY: a shift of 32 or more is not something the compiler emits (it shifts by constants below 32 and by the LUT's byte indices); `wrapping_` would hide one as a smaller shift.
                assert!(amount < 32, "valhall::sim: a shift by {amount} at {pc}");
                let v = match (w >> 24) & 3 {
                    0 => shifted & b,
                    1 => shifted | b,
                    2 => shifted ^ b,
                    _ => panic!("valhall::sim: shift op at {pc}"),
                };
                put(&mut reg, v);
            }
            // MUX.i32.bit: a's bits where the mask's are set.
            0x0B8 => {
                assert_eq!((w >> 32) & 3, 3, "valhall::sim: MUX other than .bit at {pc}");
                let (a, b, m) = (s(&reg, 0), s(&reg, 1), s(&reg, 2));
                put(&mut reg, (a & m) | (b & !m));
            }
            // ICMP_OR.u32 and FCMP_OR.f32: the compare as the result form (I1 0 or 1, F1 0.0 or 1.0, M1 0 or all ones), OR source 2.
            0x0F0 | 0x0F4 => {
                let cond = (w >> 32) & 7;
                let t = if op == 0x0F0 {
                    let (a, b) = (s(&reg, 0), s(&reg, 1));
                    match cond {
                        0 => a == b,
                        1 => a > b,
                        2 => a >= b,
                        3 => a != b,
                        4 => a < b,
                        5 => a <= b,
                        _ => panic!("valhall::sim: ICMP cond {cond} at {pc}"),
                    }
                } else {
                    let (a, b) = (f(&reg, 0), f(&reg, 1));
                    match cond {
                        0 => a == b,
                        1 => a > b,
                        2 => a >= b,
                        3 => a != b,
                        4 => a < b,
                        5 => a <= b,
                        6 => a < b || a > b,
                        _ => panic!("valhall::sim: FCMP cond {cond} at {pc}"),
                    }
                };
                let v = match (w >> 30) & 3 {
                    0 => t as u32,
                    1 => (t as u32 as f32).to_bits(),
                    2 => if t { u32::MAX } else { 0 },
                    _ => panic!("valhall::sim: compare result form at {pc}"),
                };
                let v = v | s(&reg, 2);
                put(&mut reg, v);
            }
            // Conversions (opcode2 20:16): U32_TO_F32 G#19, S32_TO_F32 9, F32_TO_S32 G#C, F32_TO_U32 G#1C, FROUND G#D, U16_TO_U32 G#14; the round mode 31:30.
            0x090 => {
                let a = s(&reg, 0);
                let round = |x: f32| match (w >> 30) & 3 {
                    0 => x.round_ties_even(),
                    1 => x.ceil(),
                    2 => x.floor(),
                    _ => x.trunc(),
                };
                let v = match (w >> 16) & 0x1F {
                    0x19 => (a as f32).to_bits(),
                    0x09 => (a as i32 as f32).to_bits(),
                    0x0C => round(f32::from_bits(a)) as i32 as u32,
                    0x1C => round(f32::from_bits(a)) as u32,
                    0x0D => round(f32::from_bits(a)).to_bits(),
                    0x14 => (a >> (16 * ((w >> 28) & 1))) & 0xFFFF,
                    o => panic!("valhall::sim: conversion G#{o:X} at {pc}"),
                };
                put(&mut reg, v);
            }
            // FREXPE (opcode2 2), the `log` form (bit 25): the e of x = m · 2^e with m in [0.75, 1.5).
            0x099 => {
                assert!(op2 == 2 && (w >> 24) & 3 == 2, "valhall::sim: FREXPE other than the log form at {pc}");
                let e = split(f32::from_bits(s(&reg, 0))).1;
                put(&mut reg, e as u32);
            }
            // The special function unit: FRCP (0), FRSQ (2), FLOGD (8).
            0x09C => {
                let a = f(&reg, 0) as f64;
                let v = match op2 {
                    0 => (1. / a) as f32,
                    2 => (1. / a.sqrt()) as f32,
                    8 => {
                        let m = split(a as f32).0 as f64;
                        // log₂ m / (m − 1), whose limit at m = 1 is 1 / ln 2.
                        if m == 1. { core::f64::consts::LOG2_E as f32 } else { (m.log2() / (m - 1.)) as f32 }
                    }
                    _ => panic!("valhall::sim: SFU opcode2 {op2} at {pc}"),
                };
                put(&mut reg, v.to_bits());
            }
            // The G#A4 group: FADD (0), FMIN (2), FMAX (3), FEXP (8: a fixed point 8:24 source and the float), FADD_LSCALE (9: a + the mantissa of b).
            0x0A4 => {
                let v = match op2 {
                    0 => f(&reg, 0) + f(&reg, 1),
                    2 => fmin(f(&reg, 0), f(&reg, 1)),
                    3 => fmax(f(&reg, 0), f(&reg, 1)),
                    8 => ((s(&reg, 0) as i32 as f64) / (1u32 << 24) as f64).exp2() as f32,
                    9 => f(&reg, 0) + split(f(&reg, 1)).0,
                    _ => panic!("valhall::sim: G#0A4 opcode2 {op2} at {pc}"),
                };
                let v = if matches!(op2, 0 | 2 | 3) { clamp(v) } else { v };
                // FADD.f32 into a half (write mask 1 the low, 2 the high): narrowed to f16, to nearest even, the other half kept (Mesa's f32 to f16 on Valhall).
                match (w >> 46) & 3 {
                    3 => put(&mut reg, v.to_bits()),
                    m => {
                        assert!(op2 == 0, "valhall::sim: a half destination on G#0A4 opcode2 {op2} at {pc}");
                        let half = f32_to_f16(v) as u32;
                        reg[dst] = if m == 1 { (reg[dst] & 0xFFFF_0000) | half } else { (reg[dst] & 0xFFFF) | half << 16 };
                    }
                }
            }
            // FMA.f32, one rounding.
            0x0B2 => {
                let v = clamp(f(&reg, 0).mul_add(f(&reg, 1), f(&reg, 2)));
                put(&mut reg, v.to_bits());
            }
            // The v2f16 forms, per lane, each rounded to f16 once: FADD (opcode2 0), FMIN (2), FMAX (3); FMA.v2f16 (G#B3), its product and sum exact in f64 before that one rounding.
            0x0A5 => {
                let (a, b) = (h(&reg, 0), h(&reg, 1));
                let v = match op2 {
                    0 => [a[0] + b[0], a[1] + b[1]],
                    2 => [fmin(a[0], b[0]), fmin(a[1], b[1])],
                    3 => [fmax(a[0], b[0]), fmax(a[1], b[1])],
                    _ => panic!("valhall::sim: G#0A5 opcode2 {op2} at {pc}"),
                };
                put(&mut reg, pack16(v));
            }
            0x0B3 => {
                let (a, b, c) = (h(&reg, 0), h(&reg, 1), h(&reg, 2));
                let lane = |k: usize| (a[k] as f64 * b[k] as f64 + c[k] as f64) as f32;
                put(&mut reg, pack16([lane(0), lane(1)]));
            }
            // NOP.
            0x000 => {}
            // BRANCHZ: taken when the source is nonzero, or zero with .eq (bit 36); the offset 34:8 in instructions from the next.
            0x01F => {
                let c = s(&reg, 0);
                let taken = if (w >> 36) & 1 == 1 { c == 0 } else { c != 0 };
                if taken {
                    let off = (((w >> 8) & ((1 << 27) - 1)) as i64) << 37 >> 37;
                    next = (pc as i64 + 1 + off) as usize;
                }
            }
            // LOAD (size at 29:27, lane at 38:36): i32 (3, lane w0 0), and i8 and i16 zero-extended to 32 bits (0 and 1, lane w0: 6 and 2, ISA.xml:300-320); the staging register 45:40, the signed offset 23:8, the address pair 7:0.
            0x060 => {
                let va = addr(&reg, w);
                let n = match ((w >> 27) & 7, (w >> 36) & 7) {
                    (3, 0) => 4,
                    (0, 6) => 1,
                    (1, 2) => 2,
                    (s, l) => panic!("valhall::sim: a load of size {s} lane {l} at {pc}"),
                };
                reg[dst] = match wls.as_deref_mut() {
                    // The workgroup's storage: the same LOAD at the WLS pointer's address (Valhall has no segment of its own for it, bifrost_compile.c `bi_handle_segment`).
                    Some(l) if l.holds(va) => l.read(va, n, who),
                    _ => match n {
                        4 => mem.read32(va),
                        1 => mem.read8(va) as u32,
                        _ => mem.read16(va) as u32,
                    },
                };
            }
            // BARRIER.slot7.wait: this invocation stops past it until its whole workgroup has reached it (`run_group`).
            0x045 => {
                assert!((w >> 30) & 7 == 7 && (w >> 59) & 15 == 9, "valhall::sim: a BARRIER without slot 7 and the wait flow at {pc}");
                inv.reg = reg;
                inv.pc = next;
                inv.steps = steps;
                return Stop::Barrier;
            }
            // ATOM.i32 (opcode2 3 at 29:27): `aadd` (2 at 25:22) of the staging register to the word at the pair plus the unsigned offset 15:8. One invocation at a time here, so the add is whole, as the device's atomic makes it.
            0x068 => {
                assert_eq!(((w >> 27) & 7, (w >> 22) & 15), (3, 2), "valhall::sim: an atomic other than ATOM.i32.aadd at {pc}");
                let p = (w & 63) as usize;
                let va = (reg[p] as u64 | (reg[p + 1] as u64) << 32).wrapping_add((w >> 8) & 0xFF);
                let v = mem.read32(va).wrapping_add(reg[dst]);
                mem.write32(va, v);
            }
            // STORE.i32.
            0x061 => {
                assert_eq!((w >> 27) & 7, 3, "valhall::sim: a store other than i32 at {pc}");
                let va = addr(&reg, w);
                match wls.as_deref_mut() {
                    Some(l) if l.holds(va) => l.write(va, reg[dst], who),
                    _ => mem.write32(va, reg[dst]),
                }
            }
            _ => panic!("valhall::sim: opcode G#{op:X} at {pc} (G#{w:016X})"),
        }
        if (w >> 59) & 15 == 15 {
            inv.reg = reg;
            inv.pc = pc;
            inv.steps = steps;
            return Stop::End;
        }
        pc = next;
    }
}

/// A load's or store's address: the pair in 7:0 plus the signed offset in 23:8.
fn addr(reg: &[u32; 64], w: u64) -> u64 {
    let p = (w & 63) as usize;
    let base = reg[p] as u64 | (reg[p + 1] as u64) << 32;
    base.wrapping_add((w >> 8) as u16 as i16 as i64 as u64)
}

/// x as m · 2^e with m in [0.75, 1.5) (FREXPE's `log` form as this emulator takes it); 0 and the specials as they are, with e 0.
fn split(x: f32) -> (f32, i32) {
    if x == 0. || !x.is_finite() {
        return (x, 0);
    }
    let mut e = x.abs().log2().floor() as i32;
    let mut m = x / (e as f32).exp2();
    if m.abs() >= 1.5 {
        m *= 0.5;
        e += 1;
    }
    (m, e)
}

/// FMAX's and FMIN's order (ISA.xml's `Clamp`): a NaN operand gives the other, and −0 < +0, so max(−0, +0) is +0 and min(+0, −0) is −0 whichever comes first.
/// Rust's f32::max leaves the sign of a zero to the platform, and |−0| as max(−0, +0) came out −0 here (2026-11-13).
fn fmax(a: f32, b: f32) -> f32 {
    if a.is_nan() || b > a || (b == a && a.is_sign_negative()) { b } else { a }
}
fn fmin(a: f32, b: f32) -> f32 {
    if a.is_nan() || b < a || (b == a && a.is_sign_positive()) { b } else { a }
}
