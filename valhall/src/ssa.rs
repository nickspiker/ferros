//! A recorder and compiler for whole kernels (GPU.md, "The resident service, and the attest mark's path"): code written once against a lane type runs on the CPU's lanes as it always did, and run once with this module's [`F`], [`M`] and [`U`] it records every operation into an SSA graph, which [`compile`] turns into Valhall words through the builder.
//!
//! The values are handles: a `u32` naming a node. The arithmetic on them records a node and returns its handle, so the code being recorded neither knows nor cares that it is not computing.
//! A recorded program has one shape: straight-line code with blocks a mask skips ([`when`], nested as the code nests them), at most one loop at a time (no nesting) whose carried state is a `Copy` value made only of handles, and stores at the end.
//!
//! # The loop
//! [`iterate`] records the loop body once. On the CPU the same hook runs `head`, leaves when no lane is still running, then runs `tail`; here both are recorded and the tail runs on the pass that stops too, leaving the state as it was for an invocation that stopped (the several-lane code's own selects), which gives the values a break would have left with no branch but the backward one (the form the device ran for the Mandelbrot).
//!
//! # Blocks, params, f16
//! [`when`] records a block that runs only where its mask is set: a branch over it for the rest, so a warp none of whose invocations needs it does not run it (Mahere's map: off the globe, under the panel, the terrain's shading where no cell stands in).
//! [`param`] records a value the run passes rather than the words: placed as a constant is, never folded, so [`Recording::key`] and the words do not depend on it and one compiled kernel serves every run of the same structure ([`Compiled::bind`] writes the values into their uniform words and immediates; a frame's camera, light and inks).
//! [`H`] is a pair of f16 in one register, Valhall's v2f16: one instruction for two lanes; its halves are read by other v2f16 operations as swizzles and by f32 ones as widens, at no cost.
//!
//! # What the compiler folds
//! Constants by value (one uniform word or LUT entry a value), negations into the operands that read them, products into fused adds, max(x, 0) and min(max(x, 0), 1) into the destination clamp of the add or multiply that made x, a constant added to a load's or store's index into its byte offset (so a row of a table reads from one address), and the wait of a load followed by another of the same address into the second.
//!
//! # Registers
//! Linear scan over the program order, a value's register freed after its last use; a value defined before the loop and used in it lives to the loop's end; the loop's carried values (its phis) keep one register each for the whole loop.
//! r58:r59 are the store address, r60:r61 the global id (preloaded), and r57 a scratch the loop head writes. The kernel asks for 64 registers a thread when it uses any above r15 (in 32-register mode Valhall has r0-r15 and r48-r63 only).

use crate::*;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ops::{Add, BitAnd, BitOr, BitXor, Div, Mul, Neg, Not, Shl, Shr, Sub};

/// An f32 per invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct F(u32);
/// A mask per invocation: all ones where true, zero where false (a compare's `M1` result).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct M(u32);
/// A u32 per invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct U(u32);

/// Two f16 per invocation in one register (v2f16): lane 0 the low half, lane 1 the high. Valhall's paired half-precision forms do both lanes in one instruction, at the rate of one f32 (ferros GPU-KISTS.md, the owner on f16, 2026-11-11: "we get 2x and most of the mahere composite work can either be done in integer or f16").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct H(u32);

#[derive(Clone, Copy, Debug, Hash)]
enum Op {
    Const(u32),
    /// A value a run of the kernel passes ([`param`]): placed as a constant is (a uniform word, or an immediate), but never folded and never a LUT entry, so the words do not depend on it and one compiled kernel serves every value ([`Compiled::bind`]). Its index in [`Rec::params`].
    Param(u32),
    /// The code to the matching [`Op::BlockEnd`] runs only where the mask is set ([`when`]): a branch over it for the rest.
    BlockBegin(u32),
    BlockEnd,
    /// v2f16 arithmetic, per lane.
    HAdd(u32, u32),
    HMul(u32, u32),
    HFma(u32, u32, u32),
    HMin(u32, u32),
    HMax(u32, u32),
    HNeg(u32),
    /// Two f32 narrowed to f16 (round to nearest even) into one register's low and high halves.
    HPack(u32, u32),
    /// An f32 narrowed to f16 into a register's low half alone (its high half undefined): read only thru [`Op::HBroadcast`] of its low half.
    HNarrow(u32),
    /// A v2f16 with both lanes its high half (true) or its low.
    HBroadcast(u32, bool),
    /// The high half (true) or the low of a v2f16, as an f32.
    HWiden(u32, bool),
    Gid,
    FAdd(u32, u32),
    FMul(u32, u32),
    FFma(u32, u32, u32),
    FNeg(u32),
    FMin(u32, u32),
    FMax(u32, u32),
    FRcp(u32),
    FRsq(u32),
    FFloor(u32),
    FCmp(Cond, u32, u32),
    Sel(u32, u32, u32),
    IMul(u32, u32),
    /// Wrapping integer add.
    IAdd(u32, u32),
    /// Wrapping integer subtract.
    ISub(u32, u32),
    /// An unsigned integer compare, all ones where true (ICMP_OR.u32's `M1` result).
    ICmp(Cond, u32, u32),
    /// A 32-bit word of one of the caller's uniform slots (`reserve`), as a value.
    Uniform(u8, u8),
    And(u32, u32),
    Or(u32, u32),
    Xor(u32, u32),
    Shl(u32, u32),
    Shr(u32, u32),
    F2U(u32),
    F2I(u32),
    U2F(u32),
    /// Signed integer to float.
    S2F(u32),
    /// FREXPE: the exponent as an integer, with the `sqrt` and `log` forms.
    FrexpE(u32, bool, bool),
    /// FLOGD, FADD_LSCALE, FEXP (fixed, float): the special function unit's logarithm and exponent pieces.
    FLogd(u32),
    FAddLscale(u32, u32),
    FExp(u32, u32),
    /// A loop-carried value: its value before the loop.
    Phi(u32),
    /// The loop's top (after its phis).
    LoopHead,
    /// The loop's bottom: each phi takes its update, and the loop goes round again while `cont` is set. Its updates are in [`Rec::loops`].
    LoopEnd(u32),
    /// After the loop's head: an invocation no longer running leaves here, its phis taking the head's outputs (in [`Loop::exits`]), as the CPU's loop returns them; the tail runs only for the ones still going.
    LoopBreak(u32),
    /// value to *(uniform slot's address + (index << shift) + offset).
    Store { slot: u8, shift: u8, offset: i16, index: u32, value: u32 },
    /// *(uniform slot's address + (index << shift) + offset), 32 bits.
    Load { slot: u8, shift: u8, offset: i16, index: u32 },
}

impl Op {
    fn sources(&self, mut f: impl FnMut(u32)) {
        use Op::*;
        match *self {
            Const(_) | Param(_) | BlockEnd | Gid | LoopHead | Uniform(..) => {}
            BlockBegin(a) | HNeg(a) | HNarrow(a) | HBroadcast(a, _) | HWiden(a, _) | FNeg(a) | FRcp(a) | FRsq(a) | FFloor(a) | F2U(a) | F2I(a) | U2F(a) | S2F(a) | FrexpE(a, _, _) | FLogd(a) | Phi(a) | LoopEnd(a) | LoopBreak(a) => f(a),
            FAddLscale(a, b) | FExp(a, b) | HAdd(a, b) | HMul(a, b) | HMin(a, b) | HMax(a, b) | HPack(a, b) => {
                f(a);
                f(b)
            }
            FAdd(a, b) | FMul(a, b) | FMin(a, b) | FMax(a, b) | FCmp(_, a, b) | IMul(a, b) | IAdd(a, b) | ISub(a, b) | ICmp(_, a, b) | And(a, b) | Or(a, b) | Xor(a, b) | Shl(a, b) | Shr(a, b) => {
                f(a);
                f(b)
            }
            FFma(a, b, c) | HFma(a, b, c) | Sel(a, b, c) => {
                f(a);
                f(b);
                f(c)
            }
            Store { value, index, .. } => {
                f(index);
                f(value)
            }
            Load { index, .. } => f(index),
        }
    }
    /// The same operation with each source mapped through `f`.
    fn map_sources(self, f: impl Fn(u32) -> u32) -> Op {
        use Op::*;
        match self {
            Const(_) | Param(_) | BlockEnd | Gid | LoopHead | Uniform(..) => self,
            BlockBegin(a) => BlockBegin(f(a)),
            HNeg(a) => HNeg(f(a)),
            HNarrow(a) => HNarrow(f(a)),
            HBroadcast(a, hi) => HBroadcast(f(a), hi),
            HWiden(a, hi) => HWiden(f(a), hi),
            FNeg(a) => FNeg(f(a)),
            FRcp(a) => FRcp(f(a)),
            FRsq(a) => FRsq(f(a)),
            FFloor(a) => FFloor(f(a)),
            F2U(a) => F2U(f(a)),
            F2I(a) => F2I(f(a)),
            U2F(a) => U2F(f(a)),
            S2F(a) => S2F(f(a)),
            FrexpE(a, s, l) => FrexpE(f(a), s, l),
            FLogd(a) => FLogd(f(a)),
            Phi(a) => Phi(f(a)),
            LoopEnd(a) => LoopEnd(f(a)),
            LoopBreak(a) => LoopBreak(f(a)),
            FAddLscale(a, b) => FAddLscale(f(a), f(b)),
            FExp(a, b) => FExp(f(a), f(b)),
            HAdd(a, b) => HAdd(f(a), f(b)),
            HMul(a, b) => HMul(f(a), f(b)),
            HMin(a, b) => HMin(f(a), f(b)),
            HMax(a, b) => HMax(f(a), f(b)),
            HPack(a, b) => HPack(f(a), f(b)),
            FAdd(a, b) => FAdd(f(a), f(b)),
            FMul(a, b) => FMul(f(a), f(b)),
            FMin(a, b) => FMin(f(a), f(b)),
            FMax(a, b) => FMax(f(a), f(b)),
            FCmp(c, a, b) => FCmp(c, f(a), f(b)),
            IMul(a, b) => IMul(f(a), f(b)),
            IAdd(a, b) => IAdd(f(a), f(b)),
            ISub(a, b) => ISub(f(a), f(b)),
            ICmp(c, a, b) => ICmp(c, f(a), f(b)),
            And(a, b) => And(f(a), f(b)),
            Or(a, b) => Or(f(a), f(b)),
            Xor(a, b) => Xor(f(a), f(b)),
            Shl(a, b) => Shl(f(a), f(b)),
            Shr(a, b) => Shr(f(a), f(b)),
            FFma(a, b, c) => FFma(f(a), f(b), f(c)),
            HFma(a, b, c) => HFma(f(a), f(b), f(c)),
            Sel(a, b, c) => Sel(f(a), f(b), f(c)),
            Store { slot, shift, offset, index, value } => Store { slot, shift, offset, index: f(index), value: f(value) },
            Load { slot, shift, offset, index } => Load { slot, shift, offset, index: f(index) },
        }
    }
    /// The kind's name, for `Compiled::kinds`.
    fn name(&self) -> &'static str {
        use Op::*;
        match self {
            Const(_) => "const",
            Param(_) => "param",
            BlockBegin(_) => "blockbegin",
            BlockEnd => "blockend",
            HAdd(..) => "hadd",
            HMul(..) => "hmul",
            HFma(..) => "hfma",
            HMin(..) => "hmin",
            HMax(..) => "hmax",
            HNeg(_) => "hneg",
            HPack(..) => "hpack",
            HNarrow(_) => "hnarrow",
            HBroadcast(..) => "hbroadcast",
            HWiden(..) => "hwiden",
            Gid => "gid",
            FAdd(..) => "fadd",
            FMul(..) => "fmul",
            FFma(..) => "ffma",
            FNeg(_) => "fneg",
            FMin(..) => "fmin",
            FMax(..) => "fmax",
            FRcp(_) => "frcp",
            FRsq(_) => "frsq",
            FFloor(_) => "ffloor",
            FCmp(..) => "fcmp",
            Sel(..) => "sel",
            IMul(..) => "imul",
            IAdd(..) => "iadd",
            ISub(..) => "isub",
            ICmp(..) => "icmp",
            Uniform(..) => "uniform",
            And(..) => "and",
            Or(..) => "or",
            Xor(..) => "xor",
            Shl(..) => "shl",
            Shr(..) => "shr",
            F2U(_) => "f2u",
            F2I(_) => "f2i",
            U2F(_) => "u2f",
            S2F(_) => "s2f",
            FrexpE(..) => "frexpe",
            FLogd(_) => "flogd",
            FAddLscale(..) => "faddlscale",
            FExp(..) => "fexp",
            Phi(_) => "phi",
            LoopHead => "loophead",
            LoopEnd(_) => "loopend",
            LoopBreak(_) => "loopbreak",
            Store { .. } => "store",
            Load { .. } => "load",
        }
    }
    fn has_value(&self) -> bool {
        !matches!(self, Op::LoopHead | Op::LoopEnd(_) | Op::LoopBreak(_) | Op::Store { .. } | Op::BlockBegin(_) | Op::BlockEnd)
    }
}

struct Loop {
    /// (phi node, its update node).
    carried: Vec<(u32, u32)>,
    /// (phi node, the head's output for it): what a leaving invocation's phis take at the break.
    exits: Vec<(u32, u32)>,
}

struct Rec {
    nodes: Vec<Op>,
    loops: Vec<Loop>,
    /// Uniform slots 0 to reserved - 1 are the caller's (its addresses); the constants go from there.
    reserved: u8,
    /// Divides, reciprocals and square roots refined to the CPU's correctly rounded results, and no multiply fused into an add (`exact`); otherwise the special function unit's values as they come (within an ulp) and contraction, which is faster and looks the same.
    exact: bool,
    /// The uniform slots the run passes (`uniform_slots`): the caller's reserved ones, then two constants a slot.
    slots: u8,
    /// The values of the [`param`]s, by index.
    params: Vec<u32>,
    /// Blocks open now ([`when`]).
    open: u32,
}

/// The recording in progress. Single-threaded by construction: a kernel is recorded on one core, between [`begin`] and [`compile`].
static mut REC: Option<Rec> = None;

fn rec() -> &'static mut Rec {
    // SAFETY: one recording at a time on one thread (the module's contract, above); `begin` makes it Some before any handle exists.
    unsafe { (*(&raw mut REC)).as_mut().expect("valhall::ssa: record between begin() and compile()") }
}

/// Record `op`, or what it comes to when its inputs are constants (worked out here, in the IEEE arithmetic the GPU does for these: add, multiply, negate, compare, the integer ops and conversions), or the chosen input of a select on a constant mask. The GPU computes the same bits with fewer instructions; folding nothing whose GPU result could differ (min, max, floor, the reciprocals).
/// 1.0's bits.
const ONE: u32 = 0x3F80_0000;

fn push(op: Op) -> u32 {
    let r = rec();
    let c = |i: u32| match r.nodes[i as usize] {
        Op::Const(b) => Some(b),
        _ => None,
    };
    let f = f32::from_bits;
    let both = |a: u32, b: u32| c(a).zip(c(b));
    let mask = |t: bool| if t { u32::MAX } else { 0 };
    let folded: Option<Result<u32, u32>> = match op {
        Op::FAdd(a, b) => both(a, b).map(|(x, y)| Ok((f(x) + f(y)).to_bits())),
        // A product with exactly 1 is the other factor (x · 1 = x for every x, signed zeros too).
        Op::FMul(a, b) => both(a, b).map(|(x, y)| Ok((f(x) * f(y)).to_bits())).or_else(|| if c(b) == Some(ONE) { Some(Err(a)) } else if c(a) == Some(ONE) { Some(Err(b)) } else { None }),
        Op::FNeg(a) => c(a).map(|x| Ok((-f(x)).to_bits())),
        Op::FCmp(cond, a, b) => both(a, b).and_then(|(x, y)| match cond {
            Cond::Gt => Some(Ok(mask(f(x) > f(y)))),
            Cond::Lt => Some(Ok(mask(f(x) < f(y)))),
            Cond::Ge => Some(Ok(mask(f(x) >= f(y)))),
            _ => None,
        }),
        // A select on a constant mask is its chosen input (masks are all ones or zero; anything else is left to the GPU).
        Op::Sel(m, a, b) => c(m).and_then(|v| if v == u32::MAX { Some(Err(a)) } else if v == 0 { Some(Err(b)) } else { None }),
        Op::And(a, b) => both(a, b).map(|(x, y)| Ok(x & y)),
        Op::Or(a, b) => both(a, b).map(|(x, y)| Ok(x | y)),
        Op::Xor(a, b) => both(a, b).map(|(x, y)| Ok(x ^ y)),
        Op::IMul(a, b) => both(a, b).map(|(x, y)| Ok(x.wrapping_mul(y))),
        Op::IAdd(a, b) => both(a, b).map(|(x, y)| Ok(x.wrapping_add(y))),
        Op::ISub(a, b) => both(a, b).map(|(x, y)| Ok(x.wrapping_sub(y))),
        Op::ICmp(cond, a, b) => both(a, b).and_then(|(x, y)| match cond {
            Cond::Eq => Some(Ok(mask(x == y))),
            Cond::Ne => Some(Ok(mask(x != y))),
            Cond::Lt => Some(Ok(mask(x < y))),
            Cond::Ge => Some(Ok(mask(x >= y))),
            Cond::Gt => Some(Ok(mask(x > y))),
            Cond::Le => Some(Ok(mask(x <= y))),
            _ => None,
        }),
        Op::Shl(a, s) => both(a, s).filter(|(_, y)| *y < 32).map(|(x, y)| Ok(x << y)),
        Op::Shr(a, s) => both(a, s).filter(|(_, y)| *y < 32).map(|(x, y)| Ok(x >> y)),
        Op::U2F(a) => c(a).map(|x| Ok((x as f32).to_bits())),
        _ => None,
    };
    match folded {
        Some(Ok(bits)) => {
            r.nodes.push(Op::Const(bits));
            (r.nodes.len() - 1) as u32
        }
        Some(Err(chosen)) => chosen,
        None => {
            r.nodes.push(op);
            (r.nodes.len() - 1) as u32
        }
    }
}

/// The bits of `x` if it is a constant.
fn const_of(x: u32) -> Option<u32> {
    match rec().nodes[x as usize] {
        Op::Const(b) => Some(b),
        _ => None,
    }
}

/// Start recording a kernel.
pub fn begin() {
    unsafe { *(&raw mut REC) = Some(Rec { nodes: Vec::new(), loops: Vec::new(), reserved: 1, exact: false, slots: 16, params: Vec::new(), open: 0 }) };
}

/// A value the kernel takes from its run rather than from its words: recorded as a constant would be, placed as one (a uniform word, or an immediate the run patches), but never folded into the code, so the kernel's words and [`Recording::key`] do not depend on it. A frame's light, camera or inks: one compiled kernel serves every frame ([`Compiled::bind`]), where a constant would make a new kernel each time it changed.
pub fn param(bits: u32) -> U {
    let r = rec();
    r.params.push(bits);
    U(push(Op::Param((r.params.len() - 1) as u32)))
}

/// `f`'s values where `m` is set and `otherwise`'s elsewhere, `f`'s code skipped by every invocation where `m` is clear: on the GPU a branch over it, so a warp none of whose invocations needs it does not run it, and one where some do runs it for those only.
/// `S` must be made of handles only, as [`iterate`]'s state is. `f` may read anything recorded before; what it records is for its own values (read after the block only through what it returns, which this merges by `m`).
pub fn when<S: Copy>(m: M, otherwise: S, f: impl FnOnce() -> S) -> S {
    let words = core::mem::size_of::<S>() / 4;
    assert!(core::mem::size_of::<S>() % 4 == 0 && core::mem::align_of::<S>() <= 4, "valhall::ssa::when: the values must be handles only");
    let ids = |s: &S| -> Vec<u32> { (0..words).map(|i| unsafe { core::ptr::read_unaligned((s as *const S as *const u32).add(i)) }).collect() };
    // A constant mask chooses at record time (as a select on it does).
    if let Some(v) = const_of(m.0) {
        return if v == 0 { otherwise } else { f() };
    }
    rec().open += 1;
    push(Op::BlockBegin(m.0));
    let inner = f();
    push(Op::BlockEnd);
    rec().open -= 1;
    let (a, b) = (ids(&inner), ids(&otherwise));
    let mut out = otherwise;
    for (i, (&x, &y)) in a.iter().zip(&b).enumerate() {
        let v = if x == y { x } else { push(Op::Sel(m.0, x, y)) };
        unsafe { core::ptr::write_unaligned((&mut out as *mut S as *mut u32).add(i), v) };
    }
    out
}

/// A load's or a store's index with a constant added (`x + c`) as its own index and the constant in the byte offset, so loads at neighbouring words of one index share one address ([`compile`] works an address out once for them all).
fn split_index(index: u32, shift: u8, offset: i16) -> (u32, i16) {
    if let Op::IAdd(a, b) = rec().nodes[index as usize] {
        for (x, c) in [(a, b), (b, a)] {
            if let Some(c) = const_of(c) {
                let at = ((c as i64) << shift) + offset as i64;
                // WHY: the byte offset is the LOAD's and STORE's signed 16-bit immediate (23:8); a larger one would wrap to a different address, so it stays in the index.
                if at <= i16::MAX as i64 {
                    return (x, at as i16);
                }
            }
        }
    }
    (index, offset)
}

/// The invocation's global id (X), preloaded in r60.
pub fn gid() -> U {
    U(push(Op::Gid))
}

/// `value` to the address in uniform `slot` (64-bit) plus `gid << shift` plus `offset` bytes.
pub fn store(slot: u8, shift: u8, offset: i16, value: U) {
    let g = gid();
    store_at(slot, g, shift, offset, value);
}

/// `value` to the address in uniform `slot` plus `index << shift` plus `offset` bytes. The address is worked out in 32 bits (the slot's high word kept): the layer arena lies below 4 GiB.
pub fn store_at(slot: u8, index: U, shift: u8, offset: i16, value: U) {
    let (index, offset) = split_index(index.0, shift, offset);
    push(Op::Store { slot, shift, offset, index, value: value.0 });
}

/// 32 bits from the address in uniform `slot` plus `index << shift` plus `offset` bytes (the same address as `store_at`).
pub fn load(slot: u8, index: U, shift: u8, offset: i16) -> U {
    let (index, offset) = split_index(index.0, shift, offset);
    U(push(Op::Load { slot, shift, offset, index }))
}

/// Word `word` (0 low, 1 high) of the caller's uniform slot `slot`, as a value: what a run passes besides addresses (where a band of a grid starts, so one kernel serves every band).
pub fn uniform(slot: u8, word: u8) -> U {
    // WHY: slots from `reserved` up hold the kernel's own constants; reading one as the caller's would take a constant for the caller's value. A word past 1 has no 32 bits to name.
    assert!(slot < rec().reserved && word < 2, "valhall::ssa: uniform({slot}, {word}) outside the caller's slots");
    U(push(Op::Uniform(slot, word)))
}

/// Record the exact forms (see `Rec::exact`) from here, for a test against the CPU's pixels bit for bit. Fast by default.
pub fn exact(on: bool) {
    rec().exact = on;
}

/// Uniform slots 0 to n - 1 for the caller (its addresses, set before `begin`'s recording uses any constant): the kernel's constants go from slot n. One by default.
pub fn reserve(n: u8) {
    rec().reserved = n;
}

/// How many uniform slots the run will pass in all (16 by default, what the GPU service took until 2026-11-11; up to 64 since): every constant the compiler places in one is read by its instruction with no move before it (a kernel of per-frame constants, Mahere's map: 206 of its 836 words were those moves).
pub fn uniform_slots(n: u8) {
    // WHY: a slot index is 7 bits (`u`), and the service's descriptor page holds 64 after its own words; more would run past what a run can pass.
    assert!((rec().reserved..=64).contains(&n), "valhall::ssa: {n} uniform slots (the reserved {} to 64)", rec().reserved);
    rec().slots = n;
}

impl F {
    pub fn splat(v: f32) -> F {
        F(push(Op::Const(v.to_bits())))
    }
    fn op(op: Op) -> F {
        F(push(op))
    }
    /// The special function unit's 1 / self alone, unrefined (FRCP).
    pub fn rcp_estimate(self) -> F {
        F::op(Op::FRcp(self.0))
    }
    /// The special function unit's 1 / sqrt(self) alone, unrefined (FRSQ).
    pub fn rsq_estimate(self) -> F {
        F::op(Op::FRsq(self.0))
    }
    /// self · b + c with one rounding.
    pub fn mul_add(self, b: F, c: F) -> F {
        F::op(Op::FFma(self.0, b.0, c.0))
    }
    /// 1 / self: the special function unit's (within an ulp), refined by one Newton step when exact.
    pub fn rcp(self) -> F {
        if !rec().exact {
            return self.rcp_estimate();
        }
        let r = F::op(Op::FRcp(self.0));
        let e = F::op(Op::FFma(self.neg().0, r.0, F::splat(1.).0));
        F::op(Op::FFma(r.0, e.0, r.0))
    }
    /// The square root from the reciprocal square root, refined once (within an ulp, near or at the correctly rounded value), 0 at 0.
    pub fn sqrt(self) -> F {
        let rs = F::op(Op::FRsq(self.0));
        let s = self * rs;
        if !rec().exact {
            // x · (1 / √x), within an ulp or so; 0 at 0 (where it would be 0 · ∞).
            return F::select(self.gt(F::splat(0.)), s, self);
        }
        let res = F::op(Op::FFma(s.neg().0, s.0, self.0));
        let half_rs = rs * 0.5;
        let s1 = F::op(Op::FFma(res.0, half_rs.0, s.0));
        F::select(self.gt(F::splat(0.)), s1, self)
    }
    pub fn sqrt_rounded(self) -> F {
        self.sqrt()
    }
    pub fn abs(self) -> F {
        self.max(-self)
    }
    pub fn min(self, o: F) -> F {
        F::op(Op::FMin(self.0, o.0))
    }
    pub fn max(self, o: F) -> F {
        F::op(Op::FMax(self.0, o.0))
    }
    pub fn floor(self) -> F {
        F::op(Op::FFloor(self.0))
    }
    /// self to the power n by squaring and multiplying, n known when recorded.
    pub fn powi(self, n: i32) -> F {
        let mut e = n.unsigned_abs();
        let (mut base, mut acc) = (self, None::<F>);
        while e > 0 {
            if e & 1 == 1 {
                acc = Some(match acc {
                    Some(a) => a * base,
                    None => base,
                });
            }
            e >>= 1;
            if e > 0 {
                base = base * base;
            }
        }
        let p = acc.unwrap_or_else(|| F::splat(1.));
        if n < 0 { p.rcp() } else { p }
    }
    pub fn gt(self, o: F) -> M {
        M(push(Op::FCmp(Cond::Gt, self.0, o.0)))
    }
    pub fn lt(self, o: F) -> M {
        M(push(Op::FCmp(Cond::Lt, self.0, o.0)))
    }
    pub fn ge(self, o: F) -> M {
        M(push(Op::FCmp(Cond::Ge, self.0, o.0)))
    }
    pub fn select(m: M, a: F, b: F) -> F {
        F::op(Op::Sel(m.0, a.0, b.0))
    }
    pub fn to_bits(self) -> U {
        U(self.0)
    }
    pub fn from_bits(u: U) -> F {
        F(u.0)
    }
    /// `as u32`: toward zero, negatives to 0.
    pub fn to_u32(self) -> U {
        U(push(Op::F2U(self.0)))
    }
    /// `as i32 as u32`: toward zero, negatives in two's complement.
    pub fn to_i32(self) -> U {
        U(push(Op::F2I(self.0)))
    }
    /// log₂ self on the special function unit, Mesa's Valhall sequence (`bi_flog2_32`): FREXPE's exponent as a float, plus FLOGD times FADD_LSCALE(−1, self). Four instructions where a polynomial takes about twenty; within an ulp of the true value over 2^16 arguments 2^-40 to 2^40 with the `log` form (gputest 12, 2026-11-05; the other forms are wrong). `sqrt` and `log` are FREXPE's two forms.
    pub fn log2_hw_with(self, sqrt: bool, log: bool) -> F {
        let e = F::op(Op::S2F(push(Op::FrexpE(self.0, sqrt, log))));
        let add = F::op(Op::FAddLscale(F::splat(-1.).0, self.0));
        let l = F::op(Op::FLogd(self.0));
        F::op(Op::FFma(l.0, add.0, e.0))
    }
    pub fn log2_hw(self) -> F {
        self.log2_hw_with(false, true)
    }
    /// 2 to the power self on the special function unit, Mesa's sequence (`bi_fexp_32`): self as 8:24 fixed point, then FEXP. Three instructions.
    pub fn exp2_hw(self) -> F {
        let scaled = self * F::splat((1u32 << 24) as f32);
        let fixed = push(Op::F2I(scaled.0));
        F::op(Op::FExp(fixed, scaled.0))
    }
    pub fn from_u32(u: U) -> F {
        F::op(Op::U2F(u.0))
    }
    /// `u as i32 as f32`: the bits taken as a signed integer.
    pub fn from_i32(u: U) -> F {
        F::op(Op::S2F(u.0))
    }
}

impl Add for F {
    type Output = F;
    fn add(self, o: F) -> F {
        F::op(Op::FAdd(self.0, o.0))
    }
}
impl Sub for F {
    type Output = F;
    fn sub(self, o: F) -> F {
        self + -o
    }
}
impl Mul for F {
    type Output = F;
    fn mul(self, o: F) -> F {
        F::op(Op::FMul(self.0, o.0))
    }
}
/// a / b: a times b's refined reciprocal, then one correction of the quotient by its residual.
impl Div for F {
    type Output = F;
    fn div(self, o: F) -> F {
        // By a constant: its reciprocal from here, rounded once (exact for a power of two, which then needs the multiply alone); otherwise the multiply and one correction of the quotient by its residual. Three instructions where the refined reciprocal took six, and a reciprocal one rounding from the true one.
        if let Some(cb) = const_of(o.0) {
            let cv = f32::from_bits(cb);
            let rv = 1. / cv;
            let rf = F::splat(rv);
            if !rec().exact || (cb & 0x007F_FFFF == 0 && rv.is_normal() && cv.is_normal()) {
                return self * rf;
            }
            let q = self * rf;
            let res = F::op(Op::FFma(o.neg().0, q.0, self.0));
            return F::op(Op::FFma(res.0, rf.0, q.0));
        }
        // Otherwise the same on the special function unit's reciprocal as it comes (within an ulp of 1 / o): on the device every one of 2^19 random quotients came out correctly rounded, the CPU's divide, without refining the reciprocal first (gputest 10, 2026-11-05). Four instructions; 1 / o three (its multiply by 1 folds away).
        let r = o.rcp_estimate();
        let q = self * r;
        if !rec().exact {
            return q;
        }
        let res = F::op(Op::FFma(o.neg().0, q.0, self.0));
        F::op(Op::FFma(res.0, r.0, q.0))
    }
}
impl Neg for F {
    type Output = F;
    fn neg(self) -> F {
        F::op(Op::FNeg(self.0))
    }
}
impl Add<f32> for F {
    type Output = F;
    fn add(self, o: f32) -> F {
        self + F::splat(o)
    }
}
impl Sub<f32> for F {
    type Output = F;
    fn sub(self, o: f32) -> F {
        self - F::splat(o)
    }
}
impl Mul<f32> for F {
    type Output = F;
    fn mul(self, o: f32) -> F {
        self * F::splat(o)
    }
}
impl Div<f32> for F {
    type Output = F;
    fn div(self, o: f32) -> F {
        self / F::splat(o)
    }
}

impl M {
    pub fn splat(b: bool) -> M {
        M(push(Op::Const(if b { u32::MAX } else { 0 })))
    }
    /// Recorded code runs every guarded block: the blocks are select-guarded inside, so the values are the same as skipping them where no invocation needs them.
    pub fn any(self) -> bool {
        true
    }
}
impl BitAnd for M {
    type Output = M;
    fn bitand(self, o: M) -> M {
        M(push(Op::And(self.0, o.0)))
    }
}
impl BitOr for M {
    type Output = M;
    fn bitor(self, o: M) -> M {
        M(push(Op::Or(self.0, o.0)))
    }
}
impl Not for M {
    type Output = M;
    fn not(self) -> M {
        let ones = push(Op::Const(u32::MAX));
        M(push(Op::Xor(self.0, ones)))
    }
}

impl U {
    pub fn splat(v: u32) -> U {
        U(push(Op::Const(v)))
    }
    pub fn wrapping_mul(self, o: U) -> U {
        U(push(Op::IMul(self.0, o.0)))
    }
    /// Per invocation, `a` where the mask is set and `b` elsewhere.
    pub fn select(m: M, a: U, b: U) -> U {
        U(push(Op::Sel(m.0, a.0, b.0)))
    }
    /// Per invocation, `a` where the mask is set and `b` elsewhere, for masks.
    pub fn select_mask(m: M, a: M, b: M) -> M {
        M(push(Op::Sel(m.0, a.0, b.0)))
    }
    /// self << s, the amount a value per invocation (below 32: the shift group reads its low byte).
    pub fn shl_by(self, s: U) -> U {
        U(push(Op::Shl(self.0, s.0)))
    }
    /// self >> s (logical), the amount a value per invocation (below 32).
    pub fn shr_by(self, s: U) -> U {
        U(push(Op::Shr(self.0, s.0)))
    }
    /// Unsigned compares, all ones where true.
    pub fn eq(self, o: U) -> M {
        M(push(Op::ICmp(Cond::Eq, self.0, o.0)))
    }
    pub fn ne(self, o: U) -> M {
        M(push(Op::ICmp(Cond::Ne, self.0, o.0)))
    }
    pub fn lt(self, o: U) -> M {
        M(push(Op::ICmp(Cond::Lt, self.0, o.0)))
    }
    pub fn ge(self, o: U) -> M {
        M(push(Op::ICmp(Cond::Ge, self.0, o.0)))
    }
    /// A mask's bits as a value (all ones or zero), and a value as a mask (it must be all ones or zero: `select` takes each bit).
    pub fn from_mask(m: M) -> U {
        U(m.0)
    }
    pub fn as_mask(self) -> M {
        M(self.0)
    }
}
impl Sub for U {
    type Output = U;
    fn sub(self, o: U) -> U {
        U(push(Op::ISub(self.0, o.0)))
    }
}
impl Add for U {
    type Output = U;
    fn add(self, o: U) -> U {
        U(push(Op::IAdd(self.0, o.0)))
    }
}
impl BitAnd for U {
    type Output = U;
    fn bitand(self, o: U) -> U {
        U(push(Op::And(self.0, o.0)))
    }
}
impl BitOr for U {
    type Output = U;
    fn bitor(self, o: U) -> U {
        U(push(Op::Or(self.0, o.0)))
    }
}
impl BitXor for U {
    type Output = U;
    fn bitxor(self, o: U) -> U {
        U(push(Op::Xor(self.0, o.0)))
    }
}
impl Shl<u32> for U {
    type Output = U;
    fn shl(self, s: u32) -> U {
        let k = push(Op::Const(s));
        U(push(Op::Shl(self.0, k)))
    }
}
impl Shr<u32> for U {
    type Output = U;
    fn shr(self, s: u32) -> U {
        let k = push(Op::Const(s));
        U(push(Op::Shr(self.0, k)))
    }
}

impl H {
    /// Two f16 constants, `lo` in lane 0 and `hi` in lane 1, each rounded to nearest even.
    pub fn splat2(lo: f32, hi: f32) -> H {
        H(push(Op::Const((f32_to_f16(hi) as u32) << 16 | f32_to_f16(lo) as u32)))
    }
    pub fn splat(v: f32) -> H {
        H::splat2(v, v)
    }
    /// Two f16 a run passes ([`param`]), `lo` in lane 0 and `hi` in lane 1.
    pub fn param2(lo: f32, hi: f32) -> H {
        H(param((f32_to_f16(hi) as u32) << 16 | f32_to_f16(lo) as u32).0)
    }
    /// Two f32 narrowed to f16 (to nearest even), `lo` in lane 0 and `hi` in lane 1: an instruction each (FADD.f32 into a half register, Mesa's f32 to f16 on Valhall).
    pub fn pack(lo: F, hi: F) -> H {
        H(push(Op::HPack(lo.0, hi.0)))
    }
    /// An f32 narrowed to f16 in both lanes: one instruction (into a low half, read broadcast).
    pub fn splat_f(x: F) -> H {
        H(push(Op::HNarrow(x.0))).lo()
    }
    /// Both lanes this half: a swizzle of the instructions that read it, no instruction of its own.
    pub fn lo(self) -> H {
        H(push(Op::HBroadcast(self.0, false)))
    }
    pub fn hi(self) -> H {
        H(push(Op::HBroadcast(self.0, true)))
    }
    /// A lane as an f32: a widen of the f32 instruction that reads it (FADD, FMA, FMIN, FMAX), else one instruction.
    pub fn lo_f(self) -> F {
        F(push(Op::HWiden(self.0, false)))
    }
    pub fn hi_f(self) -> F {
        F(push(Op::HWiden(self.0, true)))
    }
    /// self · b + c per lane, one rounding.
    pub fn mul_add(self, b: H, c: H) -> H {
        H(push(Op::HFma(self.0, b.0, c.0)))
    }
    pub fn min(self, o: H) -> H {
        H(push(Op::HMin(self.0, o.0)))
    }
    pub fn max(self, o: H) -> H {
        H(push(Op::HMax(self.0, o.0)))
    }
    /// Per invocation, `a` where the mask is set and `b` elsewhere (both lanes).
    pub fn select(m: M, a: H, b: H) -> H {
        H(push(Op::Sel(m.0, a.0, b.0)))
    }
    pub fn to_bits(self) -> U {
        U(self.0)
    }
    pub fn from_bits(u: U) -> H {
        H(u.0)
    }
}
impl Add for H {
    type Output = H;
    fn add(self, o: H) -> H {
        H(push(Op::HAdd(self.0, o.0)))
    }
}
impl Sub for H {
    type Output = H;
    fn sub(self, o: H) -> H {
        self + -o
    }
}
impl Mul for H {
    type Output = H;
    fn mul(self, o: H) -> H {
        H(push(Op::HMul(self.0, o.0)))
    }
}
impl Neg for H {
    type Output = H;
    fn neg(self) -> H {
        H(push(Op::HNeg(self.0)))
    }
}

/// The loop hook, recorded: `head` then `tail` once each over phis for every handle in `init`; each carried value takes what `tail` left; round again while running and fewer than `max` passes. `head` and `tail` get the pass number as an f32. Returns the state the loop leaves with.
/// `tail` must leave a stopped invocation's state unchanged (select on `running`, as code written for several lanes does with `live`): the pass that stops runs the tail too, and leaves with what it returns.
/// `S` must be made of handles only ([`F`], [`M`], [`U`], in tuples, arrays or structs of them): it is read and written as a row of `u32`s.
pub fn iterate<S: Copy>(max: u32, init: S, head: impl FnOnce(F, S) -> (S, M), tail: impl FnOnce(F, S) -> S) -> S {
    let words = core::mem::size_of::<S>() / 4;
    assert!(core::mem::size_of::<S>() % 4 == 0 && core::mem::align_of::<S>() <= 4, "valhall::ssa::iterate: the state must be handles only");
    let as_ids = |s: &S| -> Vec<u32> { (0..words).map(|i| unsafe { core::ptr::read_unaligned((s as *const S as *const u32).add(i)) }).collect() };
    let from_ids = |ids: &[u32]| -> S {
        let mut s = init;
        for (i, &v) in ids.iter().enumerate() {
            unsafe { core::ptr::write_unaligned((&mut s as *mut S as *mut u32).add(i), v) };
        }
        s
    };
    let n_nodes = rec().nodes.len() as u32;
    let init_ids = as_ids(&init);
    // A handle names a node already recorded: anything else in the state is not a handle.
    assert!(init_ids.iter().all(|&v| v < n_nodes), "valhall::ssa::iterate: the state holds something that is not a handle");
    let zero = F::splat(0.);
    let n = F(push(Op::Phi(zero.0)));
    let phis: Vec<u32> = init_ids.iter().map(|&v| push(Op::Phi(v))).collect();
    push(Op::LoopHead);
    let (after_head, running) = head(n, from_ids(&phis));
    // An invocation that stopped leaves here with the head's state, as the CPU's loop does; the tail then runs only where it goes on (so a lane type with one lane's meaning needs no selects freezing the stopped ones).
    let exits: Vec<(u32, u32)> = phis.iter().copied().zip(as_ids(&after_head)).collect();
    push(Op::LoopBreak(running.0));
    let after_tail = tail(n, after_head);
    // Only invocations still running reach the tail, so its output is the update: no select here, and no second copy of the state alive to the loop's end (it ran the register file out on the attest mark, 2026-11-05).
    let updates: Vec<u32> = as_ids(&after_tail);
    let n1 = n + 1.;
    let more = n1.lt(F::splat(max as f32));
    let cont = running & more;
    let mut carried = Vec::with_capacity(words + 1);
    carried.push((n.0, n1.0));
    carried.extend(phis.iter().copied().zip(updates));
    let r = rec();
    r.loops.push(Loop { carried, exits });
    push(Op::LoopEnd(cont.0));
    from_ids(&phis)
}

/// The compiled kernel: its words, whether it asks for 64 registers a thread, and what it took (nodes recorded, nodes live after dead code goes, the most registers live at once).
pub struct Compiled {
    pub words: Vec<u64>,
    pub regs64: bool,
    pub nodes: usize,
    pub kept: usize,
    pub peak: usize,
    /// The loop body's instructions, and how many of them load a constant (the cost a pass pays, per invocation per iteration).
    pub loop_words: usize,
    pub loop_consts: usize,
    /// The uniform slots 1 and up the kernel reads its constants from: the caller passes them after slot 0 (its output address).
    pub consts: Vec<u64>,
    /// The kept nodes by kind (`Op` names, for measuring the compiler's lowering), the most frequent first.
    pub kinds: Vec<(&'static str, usize)>,
    /// Where each [`param`] went, by index: its uniform word, and the immediates that carry it.
    pub params: Vec<ParamPlace>,
    reserved: u8,
}

/// A [`param`]'s places in a compiled kernel: the uniform word it is read from (slot, word), if it has one, and the instructions whose 32-bit immediate is it (where the uniform port was taken, or no uniform word was left).
#[derive(Clone, Debug, Default)]
pub struct ParamPlace {
    pub uniform: Option<(u8, u8)>,
    pub words: Vec<u32>,
}

impl Compiled {
    /// This kernel for another recording with the same key ([`Recording::key`]), from that recording's [`Recording::params`]: each value written into its uniform word and its immediates. No compile, the words otherwise the same.
    pub fn bind(&mut self, values: &[u32]) {
        // WHY: a recording with another number of params has another key; binding it here would leave some of this kernel's params at the last frame's values.
        assert_eq!(values.len(), self.params.len(), "valhall::ssa::Compiled::bind: {} values for {} params", values.len(), self.params.len());
        for (p, &v) in self.params.iter().zip(values) {
            if let Some((slot, w)) = p.uniform {
                let c = &mut self.consts[(slot - self.reserved) as usize];
                *c = (*c & !(0xFFFF_FFFFu64 << (32 * w))) | (v as u64) << (32 * w);
            }
            for &at in &p.words {
                let word = &mut self.words[at as usize];
                *word = (*word & !(0xFFFF_FFFFu64 << 8)) | (v as u64) << 8;
            }
        }
    }
}

/// A recording ended ([`finish`]) and not yet compiled.
pub struct Recording {
    rec: Rec,
}

/// FNV-1a over whole words: the recording's key.
struct Fnv(u64);
impl Fnv {
    fn mix(&mut self, x: u64) {
        self.0 = (self.0 ^ x).wrapping_mul(0x100_0000_01B3);
    }
}
impl core::hash::Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.mix(b as u64);
        }
    }
    fn write_u8(&mut self, x: u8) {
        self.mix(x as u64);
    }
    fn write_u16(&mut self, x: u16) {
        self.mix(x as u64);
    }
    fn write_u32(&mut self, x: u32) {
        self.mix(x as u64);
    }
    fn write_u64(&mut self, x: u64) {
        self.mix(x);
    }
    fn write_i16(&mut self, x: i16) {
        self.mix(x as u16 as u64);
    }
    fn write_isize(&mut self, x: isize) {
        self.mix(x as u64);
    }
    fn write_usize(&mut self, x: usize) {
        self.mix(x as u64);
    }
}

/// End the recording, for its key and params ([`Recording::key`], [`Recording::params`]) before or instead of compiling it.
pub fn finish() -> Recording {
    let rec = unsafe { (*(&raw mut REC)).take().expect("valhall::ssa: finish without begin") };
    // WHY: a block left open has no end for its branch to land on.
    assert!(rec.open == 0, "valhall::ssa: a block still open at the recording's end");
    Recording { rec }
}

impl Recording {
    /// The recording's structure: every operation and constant, each param by its index and not its value. Two recordings with the same key compile to the same words but for their params' values, so a kernel compiled once serves both ([`Compiled::bind`]).
    pub fn key(&self) -> u64 {
        use core::hash::Hash;
        let r = &self.rec;
        let mut h = Fnv(0xCBF2_9CE4_8422_2325);
        r.nodes.hash(&mut h);
        for l in &r.loops {
            l.carried.hash(&mut h);
            l.exits.hash(&mut h);
        }
        (r.reserved, r.exact, r.slots, r.params.len()).hash(&mut h);
        core::hash::Hasher::finish(&h)
    }

    /// The params' values, by index: what [`Compiled::bind`] takes.
    pub fn params(&self) -> &[u32] {
        &self.rec.params
    }

    /// The words, for these params' values.
    /// Dead code goes first (the recorder runs every block `any()` guards; what reaches no store and no loop state is dropped). Constants hold no register: each use loads its constant into r54-r56 just before the instruction, so a constant made once and used everywhere costs no register across the kernel.
    /// Panics if the program still needs more value registers than r0-r53 (no spilling yet), naming how many it needed.
    pub fn compile(self) -> Compiled {
        compile_rec(self.rec)
    }
}

/// The value registers: r0-r53. r54-r56 take constants at their uses, r57 the loop head's write, r58:r59 the store address, r60:r61 the global id (preloaded).
const POOL: u8 = 54;
const CONST_REGS: [u8; 3] = [54, 55, 56];
const SCRATCH: u8 = 57;
const ADDR: u8 = 58;

/// f16 1.0 in both lanes.
const H_ONE: u32 = 0x3C00_3C00;

/// The recording to Valhall words; the recording ends.
pub fn compile() -> Compiled {
    finish().compile()
}

/// What an operation's float sources are, for the source modifiers: f32 ones (negate, and a widen from an f16 half where `widen`), v2f16 ones (negate and a swizzle), or none.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    None,
    F32 { widen: bool },
    F16,
}

fn kind(op: &Op) -> (Kind, [u32; 3], usize) {
    use Op::*;
    match *op {
        FAdd(a, b) | FMul(a, b) | FMin(a, b) | FMax(a, b) => (Kind::F32 { widen: true }, [a, b, 0], 2),
        FCmp(_, a, b) => (Kind::F32 { widen: true }, [a, b, 0], 2),
        FFma(a, b, c) => (Kind::F32 { widen: true }, [a, b, c], 3),
        FNeg(a) => (Kind::F32 { widen: true }, [a, 0, 0], 1),
        FRcp(a) | FRsq(a) | HNarrow(a) => (Kind::F32 { widen: true }, [a, 0, 0], 1),
        HPack(a, b) => (Kind::F32 { widen: true }, [a, b, 0], 2),
        HAdd(a, b) | HMul(a, b) | HMin(a, b) | HMax(a, b) => (Kind::F16, [a, b, 0], 2),
        HFma(a, b, c) => (Kind::F16, [a, b, c], 3),
        HNeg(a) => (Kind::F16, [a, 0, 0], 1),
        _ => (Kind::None, [0; 3], 0),
    }
}

/// `op` with its float sources replaced (the same kind and arity as `kind` gave).
fn with_sources(op: Op, s: [u32; 3]) -> Op {
    use Op::*;
    match op {
        FAdd(..) => FAdd(s[0], s[1]),
        FMul(..) => FMul(s[0], s[1]),
        FMin(..) => FMin(s[0], s[1]),
        FMax(..) => FMax(s[0], s[1]),
        FCmp(c, ..) => FCmp(c, s[0], s[1]),
        FFma(..) => FFma(s[0], s[1], s[2]),
        FNeg(_) => FNeg(s[0]),
        FRcp(_) => FRcp(s[0]),
        FRsq(_) => FRsq(s[0]),
        HPack(..) => HPack(s[0], s[1]),
        HNarrow(_) => HNarrow(s[0]),
        HAdd(..) => HAdd(s[0], s[1]),
        HMul(..) => HMul(s[0], s[1]),
        HMin(..) => HMin(s[0], s[1]),
        HMax(..) => HMax(s[0], s[1]),
        HFma(..) => HFma(s[0], s[1], s[2]),
        HNeg(_) => HNeg(s[0]),
        other => other,
    }
}

fn compile_rec(rec: Rec) -> Compiled {
    let n = rec.nodes.len();
    let mut work = rec.nodes.clone();
    // Constants by value: every source naming a constant names the first node with its bits, so one uniform word or LUT entry serves each value however often it was made (Mahere's map made 0 and 1 dozens of times, and each took a uniform word of its own).
    {
        let mut first: Vec<(u32, u32)> = Vec::new();
        let mut canon: Vec<u32> = (0..n as u32).collect();
        for (i, op) in work.iter().enumerate() {
            if let Op::Const(b) = *op {
                match first.binary_search_by_key(&b, |e| e.0) {
                    Ok(j) => canon[i] = first[j].1,
                    Err(j) => first.insert(j, (b, i as u32)),
                }
            }
        }
        for op in work.iter_mut() {
            *op = op.map_sources(|s| canon[s as usize]);
        }
    }
    // Source modifiers, folded into the operations that read them: a negation (Valhall negates a float source for free; 509 of the attest ball's 4836 nodes were negations, each an instruction), an f16 half read by an f32 operation (its widen), a broadcast half read by a v2f16 one (its swizzle). A modifier nothing else reads then goes with the dead code.
    let mut negmask = alloc::vec![0u8; n];
    let mut swz = alloc::vec![[Swz16::H01; 3]; n];
    let mut wid = alloc::vec![[Widen::None; 3]; n];
    for i in 0..n {
        let (kd, mut srcs, ns) = kind(&work[i]);
        for p in 0..ns {
            loop {
                let s = srcs[p] as usize;
                match (kd, work[s]) {
                    (Kind::F32 { .. }, Op::FNeg(x)) if wid[i][p] == Widen::None => {
                        srcs[p] = x;
                        negmask[i] ^= 1 << p;
                    }
                    (Kind::F32 { widen: true }, Op::HWiden(h, hi)) if wid[i][p] == Widen::None => {
                        srcs[p] = h;
                        wid[i][p] = if hi { Widen::H1 } else { Widen::H0 };
                    }
                    // Inside a widen the source is a v2f16: its negation is the f32's, and a broadcast picks which half the widen reads.
                    (Kind::F32 { .. }, Op::HNeg(x)) if wid[i][p] != Widen::None => {
                        srcs[p] = x;
                        negmask[i] ^= 1 << p;
                    }
                    (Kind::F32 { .. }, Op::HBroadcast(x, hi)) if wid[i][p] != Widen::None => {
                        srcs[p] = x;
                        wid[i][p] = if hi { Widen::H1 } else { Widen::H0 };
                    }
                    (Kind::F16, Op::HNeg(x)) => {
                        srcs[p] = x;
                        negmask[i] ^= 1 << p;
                    }
                    // A broadcast of a broadcast reads the inner one's half.
                    (Kind::F16, Op::HBroadcast(x, hi)) => {
                        srcs[p] = x;
                        swz[i][p] = if hi { Swz16::H11 } else { Swz16::H00 };
                    }
                    _ => break,
                }
            }
        }
        work[i] = with_sources(work[i], srcs);
    }
    // Contraction (not when exact): an add of a product nothing else reads becomes one fused multiply-add, the product's negations carried into its factors and the add's into the sign of the product or of the addend; the multiply then goes with the dead code. The same for v2f16.
    let mut users = alloc::vec![0u32; n];
    if !rec.exact {
        for op in &work {
            op.sources(|s| users[s as usize] += 1);
        }
        for i in 0..n {
            let (a, b, half) = match work[i] {
                Op::FAdd(a, b) => (a, b, false),
                Op::HAdd(a, b) => (a, b, true),
                _ => continue,
            };
            for (p, m, other) in [(0usize, a, b), (1, b, a)] {
                let (x, y) = match (half, work[m as usize]) {
                    (false, Op::FMul(x, y)) | (true, Op::HMul(x, y)) => (x, y),
                    _ => continue,
                };
                // The product read with no modifier of the add's own (a widen or a swizzle on it reads another value than the product).
                if users[m as usize] != 1 || wid[i][p] != Widen::None || swz[i][p] != Swz16::H01 {
                    continue;
                }
                let product = (negmask[m as usize] & 3) ^ (negmask[i] >> p & 1);
                let addend = negmask[i] >> (1 - p) & 1;
                work[i] = if half { Op::HFma(x, y, other) } else { Op::FFma(x, y, other) };
                negmask[i] = product | addend << 2;
                wid[i] = [wid[m as usize][0], wid[m as usize][1], wid[i][1 - p]];
                swz[i] = [swz[m as usize][0], swz[m as usize][1], swz[i][1 - p]];
                users[m as usize] = 0;
                break;
            }
        }
    }
    // Clamps, folded into the instruction that makes the value (Valhall's destination clamp on FADD and FMA, f32 and v2f16): max(x, 0) is x clamped positive and min(that, 1) is x clamped to [0, 1], where x is an add, a multiply or a fused one read by nothing else. Two instructions of the CPU's `clamp(0, 1)` cost none.
    let mut clamp = alloc::vec![Clamp::None; n];
    {
        users.iter_mut().for_each(|u| *u = 0);
        for op in &work {
            op.sources(|s| users[s as usize] += 1);
        }
        // A loop's carried values and exits read their nodes too: a clamp there would change what the loop carries.
        for l in &rec.loops {
            for &(phi, upd) in l.carried.iter().chain(&l.exits) {
                users[phi as usize] += 1;
                users[upd as usize] += 1;
            }
        }
        let mut repl: Vec<u32> = (0..n as u32).collect();
        for i in 0..n {
            work[i] = work[i].map_sources(|s| repl[s as usize]);
            let (a, b, half, max) = match work[i] {
                Op::FMax(a, b) => (a, b, false, true),
                Op::FMin(a, b) => (a, b, false, false),
                Op::HMax(a, b) => (a, b, true, true),
                Op::HMin(a, b) => (a, b, true, false),
                _ => continue,
            };
            for (p, x, z) in [(0usize, a, b), (1, b, a)] {
                let bound = match work[z as usize] {
                    Op::Const(c) => c,
                    _ => continue,
                };
                let plain = negmask[i] >> p & 1 == 0 && wid[i][p] == Widen::None && swz[i][p] == Swz16::H01 && negmask[i] >> (1 - p) & 1 == 0 && swz[i][1 - p] == Swz16::H01 && wid[i][1 - p] == Widen::None;
                let maker = match (half, work[x as usize]) {
                    (false, Op::FAdd(..) | Op::FMul(..) | Op::FFma(..)) | (true, Op::HAdd(..) | Op::HMul(..) | Op::HFma(..)) => true,
                    _ => false,
                };
                if !plain || !maker || users[x as usize] != 1 {
                    continue;
                }
                let one = if half { H_ONE } else { ONE };
                let to = match (max, bound, clamp[x as usize]) {
                    (true, 0, Clamp::None) => Clamp::ZeroInf,
                    (false, c, Clamp::ZeroInf) if c == one => Clamp::ZeroOne,
                    _ => continue,
                };
                clamp[x as usize] = to;
                repl[i] = x;
                users[x as usize] = users[i];
                break;
            }
        }
    }
    let nodes = &work;
    let imm_bits = |i: u32| -> Option<u32> {
        match nodes[i as usize] {
            Op::Const(b) => Some(b),
            Op::Param(p) => Some(rec.params[p as usize]),
            _ => None,
        }
    };
    let is_imm = |i: u32| matches!(nodes[i as usize], Op::Const(_) | Op::Param(_));
    let param_of = |i: u32| match nodes[i as usize] {
        Op::Param(p) => Some(p),
        _ => None,
    };
    // Dead code: live from the stores, the loop ends' conditions and every loop's state, backwards.
    let mut keep = alloc::vec![false; n];
    let seed = |keep: &mut Vec<bool>| {
        for (i, op) in nodes.iter().enumerate() {
            if matches!(op, Op::Store { .. } | Op::LoopHead | Op::LoopEnd(_) | Op::LoopBreak(_)) {
                keep[i] = true;
            }
        }
        for l in &rec.loops {
            for &(phi, upd) in l.carried.iter().chain(&l.exits) {
                keep[phi as usize] = true;
                keep[upd as usize] = true;
            }
        }
    };
    let flow = |keep: &mut Vec<bool>| {
        for i in (0..n).rev() {
            if keep[i] {
                nodes[i].sources(|s| keep[s as usize] = true);
            }
        }
    };
    seed(&mut keep);
    flow(&mut keep);
    // A block stays only where something in it does (an instruction of its own), and then its mask is live too.
    {
        let mut open: Vec<usize> = Vec::new();
        let mut live: Vec<(usize, usize)> = Vec::new();
        for i in 0..n {
            match nodes[i] {
                Op::BlockBegin(_) => open.push(i),
                Op::BlockEnd => {
                    let b = open.pop().expect("valhall::ssa: a block's end without its begin");
                    if (b + 1..i).any(|j| keep[j] && !is_imm(j as u32) && !matches!(nodes[j], Op::Gid | Op::BlockBegin(_) | Op::BlockEnd)) {
                        live.push((b, i));
                    }
                }
                _ => {}
            }
        }
        if !live.is_empty() {
            for &(b, e) in &live {
                keep[b] = true;
                keep[e] = true;
            }
            flow(&mut keep);
        }
    }
    // Each kept node's last use (constants have no register, so none); a loop stretches it.
    let mut last = alloc::vec![0usize; n];
    for (i, op) in nodes.iter().enumerate() {
        if keep[i] {
            op.sources(|s| last[s as usize] = last[s as usize].max(i));
        }
    }
    // An update that may take its phi's register (written after the phi's last read in the loop).
    let mut coalesce: Vec<Option<u32>> = alloc::vec![None; n];
    let (mut head_at, mut loop_no) = (None, 0usize);
    for (i, op) in nodes.iter().enumerate() {
        match op {
            Op::LoopHead => head_at = Some(i),
            Op::LoopEnd(_) => {
                let h = head_at.take().expect("valhall::ssa: a loop end without its head");
                let brk = (h..i).find(|&j| matches!(nodes[j], Op::LoopBreak(_))).expect("valhall::ssa: a loop without its break");
                // The head's outputs live to the break, where a leaving invocation's phis take them. The break writes the phis' registers for every invocation there, so a phi the head changed must not be read after it (the tail sees the head's output instead).
                for &(phi, out) in &rec.loops[loop_no].exits {
                    last[out as usize] = last[out as usize].max(brk);
                    if out != phi {
                        for j in brk + 1..i {
                            if keep[j] {
                                nodes[j].sources(|s| assert!(s != phi, "valhall::ssa: the tail reads a phi the head changed (node {phi} at {j})"));
                            }
                        }
                    }
                }
                // A head output may take its phi's register when the phi is not read again after it is written (x², y², |z|² in tapuwae's orbit: the head works them out from z and nothing reads the old ones): the break's copy is then nothing. Not when the node is any other field's value too.
                let mut head_took = alloc::vec![false; rec.loops[loop_no].exits.len()];
                for (e, &(phi, out)) in rec.loops[loop_no].exits.iter().enumerate() {
                    let o = out as usize;
                    if out == phi || o <= h || o >= brk || is_imm(out) || coalesce[o].is_some() {
                        continue;
                    }
                    let shared = rec.loops[loop_no].exits.iter().filter(|x| x.1 == out).count() > 1 || rec.loops[loop_no].carried.iter().any(|&(p, u)| u == out && p != phi);
                    let mut inner = h;
                    for j in h..i {
                        if keep[j] {
                            nodes[j].sources(|s| {
                                if s == phi {
                                    inner = j;
                                }
                            });
                        }
                    }
                    if !shared && inner <= o {
                        coalesce[o] = Some(phi);
                        head_took[e] = true;
                    }
                }
                for &(phi, upd) in &rec.loops[loop_no].carried {
                    // A phi whose head output holds its register: its update shares it only by being that same node.
                    if rec.loops[loop_no].exits.iter().zip(&head_took).any(|(x, &t)| t && x.0 == phi) {
                        last[phi as usize] = last[phi as usize].max(i);
                        last[upd as usize] = last[upd as usize].max(i);
                        continue;
                    }
                    // The phi's last read inside the loop, before it is stretched to the end: its update may take its register after it (coalescing).
                    let mut inner = h;
                    for j in h..i {
                        if keep[j] {
                            nodes[j].sources(|s| {
                                if s == phi {
                                    inner = j;
                                }
                            });
                        }
                    }
                    // A phi that is another field's update is read again by the loop's end copies, after every update is written: its own update may not take its register then.
                    let read_at_end = rec.loops[loop_no].carried.iter().any(|&(p, u)| u == phi && p != phi);
                    // Only an update written after the break: the break's copies write the phis' registers.
                    if upd != phi && (upd as usize) > brk && (upd as usize) < i && !is_imm(upd) && inner <= upd as usize && !read_at_end {
                        coalesce[upd as usize] = Some(phi);
                    }
                    last[phi as usize] = last[phi as usize].max(i);
                    last[upd as usize] = last[upd as usize].max(i);
                }
                // A value from before the loop used inside it lives to the loop's end.
                for j in h..i {
                    if keep[j] {
                        nodes[j].sources(|s| {
                            if (s as usize) < h {
                                last[s as usize] = last[s as usize].max(i);
                            }
                        });
                    }
                }
                loop_no += 1;
            }
            _ => {}
        }
    }
    // The loops' spans (head, end), and how often each constant is loaded inside each.
    let mut spans: Vec<(usize, usize)> = Vec::new();
    {
        let mut h = None;
        for (i, op) in nodes.iter().enumerate() {
            match op {
                Op::LoopHead => h = Some(i),
                Op::LoopEnd(_) => spans.push((h.take().unwrap_or(i), i)),
                _ => {}
            }
        }
    }
    // Registers: linear scan over the kept nodes that hold a value (constants and the global id excepted), with `hoist[l]` the constants loop l holds in registers from its head to its end. Returns the registers, the highest used, the peak, and each loop's peak.
    // A block needs nothing of its own here: an invocation that branches over it runs none of it, so a register freed or taken inside it is the block's business; what is read after it is the select of its values and the values from before it, which live across it as any value does.
    // Returns the registers, the highest used, the peak, each loop's peak, and each loop's hoisted constants with the register each holds (a constant is one node however many loops use it, so its register is the loop's, not the node's).
    let allocate = |hoist: &[Vec<u32>]| -> Result<(Vec<u8>, u8, usize, Vec<usize>, Vec<Vec<(u32, u8)>>), usize> {
        let mut reg = alloc::vec![u8::MAX; n];
        let mut free: Vec<u8> = (0..POOL).rev().collect();
        let mut live: Vec<(usize, u8)> = Vec::new();
        let (mut high, mut peak) = (0u8, 0usize);
        let mut loop_peak = alloc::vec![0usize; spans.len()];
        let mut held: Vec<Vec<(u32, u8)>> = spans.iter().map(|_| Vec::new()).collect();
        for (i, op) in nodes.iter().enumerate() {
            if !keep[i] {
                continue;
            }
            live.retain(|&(end, r)| {
                if end < i {
                    free.push(r);
                    false
                } else {
                    true
                }
            });
            if let Some(l) = spans.iter().position(|s| s.0 == i) {
                for &c in &hoist[l] {
                    let r = free.pop().ok_or(i)?;
                    high = high.max(r);
                    held[l].push((c, r));
                    live.push((spans[l].1, r));
                }
            }
            if matches!(op, Op::Gid) {
                reg[i] = 60;
            } else if let Some(phi) = coalesce[i] {
                // The phi's register, which its own entry keeps to the loop's end: the update is written over the phi after the phi's last read.
                reg[i] = reg[phi as usize];
            } else if op.has_value() && !is_imm(i as u32) {
                let r = free.pop().ok_or(i)?;
                high = high.max(r);
                reg[i] = r;
                live.push((last[i].max(i), r));
            }
            peak = peak.max(live.len());
            for (l, s) in spans.iter().enumerate() {
                if i >= s.0 && i <= s.1 {
                    loop_peak[l] = loop_peak[l].max(live.len());
                }
            }
        }
        Ok((reg, high, peak, loop_peak, held))
    };
    // First without hoisting, for each loop's pressure; then each loop's most-loaded constants into the registers it leaves free (owner, 2026-11-05: the mark within a 120 Hz frame; 188 of the ball loop's 625 instructions were constant loads).
    let none: Vec<Vec<u32>> = spans.iter().map(|_| Vec::new()).collect();
    let first = allocate(&none).unwrap_or_else(|i| panic!("valhall::ssa: out of registers at node {i} of {n} (no spilling yet)"));
    let loop_peak = first.3.clone();
    let mut hoist: Vec<Vec<u32>> = Vec::new();
    for (l, &(h, e)) in spans.iter().enumerate() {
        let mut uses: Vec<(u32, usize)> = Vec::new();
        for j in h..e {
            if keep[j] {
                nodes[j].sources(|s| {
                    if is_imm(s) {
                        match uses.iter_mut().find(|u| u.0 == s) {
                            Some(u) => u.1 += 1,
                            None => uses.push((s, 1)),
                        }
                    }
                });
            }
        }
        uses.sort_by(|a, b| b.1.cmp(&a.1));
        // The first allocation succeeded, so the loop never held more than the pool: exact.
        let room = POOL as usize - loop_peak[l];
        hoist.push(uses.iter().take(room).map(|u| u.0).collect());
    }
    let (reg, high, peak, _, held) = allocate(&hoist).unwrap_or(first);
    // A hoisted constant's register, for a use at node `at`: inside its loop only (a use before or after the loop loads it as any other constant; reading the register there read whatever value had taken it: 595 black pixels on the attest ball, 2026-11-05).
    let hoisted = |s: u32, at: usize| -> Option<u8> {
        let l = spans.iter().position(|&(h, e)| at > h && at < e)?;
        held[l].iter().find(|x| x.0 == s).map(|x| x.1)
    };
    let hoisted_at = |s: u32, at: usize| hoisted(s, at).is_some();
    // Uniform words for the constants used most (a use inside a loop counting 2^6 times one outside), the slots after the caller's (`reserve`) up to `uniform_slots`: read by the instruction itself, no load. LUT values and hoisted ones need none; a param is never a LUT value (its value is the run's).
    let mut weight: Vec<(u32, usize)> = Vec::new();
    for (j, op) in nodes.iter().enumerate() {
        if !keep[j] {
            continue;
        }
        let in_loop = spans.iter().any(|s| j > s.0 && j < s.1);
        op.sources(|s| {
            if is_imm(s) && !hoisted_at(s, j) {
                let lut = matches!(nodes[s as usize], Op::Const(bits) if LUT.contains(&bits));
                if !lut {
                    let w = if in_loop { 1 << 6 } else { 1 };
                    match weight.iter_mut().find(|x| x.0 == s) {
                        Some(x) => x.1 += w,
                        None => weight.push((s, w)),
                    }
                }
            }
        });
    }
    weight.sort_by(|a, b| b.1.cmp(&a.1));
    // The service passes `slots` uniform slots (16 unless the caller asked for more): the caller's `reserved` first, then two constants a slot.
    let reserved = rec.reserved;
    let ufau: Vec<(u32, (u8, u8))> = weight.iter().take(2 * (rec.slots - reserved) as usize).enumerate().map(|(k, w)| (w.0, (reserved + (k / 2) as u8, (k % 2) as u8))).collect();
    let mut consts = alloc::vec![0u64; ufau.len().div_ceil(2)];
    let mut params: Vec<ParamPlace> = alloc::vec![ParamPlace::default(); rec.params.len()];
    for &(s, (sl, w)) in &ufau {
        let bits = imm_bits(s).unwrap();
        consts[(sl - reserved) as usize] |= (bits as u64) << (32 * w as u32);
        if let Some(p) = param_of(s) {
            params[p as usize].uniform = Some((sl, w));
        }
    }
    let regs64 = high > 15;
    let kept = keep.iter().filter(|&&k| k).count();
    // Emission.
    // SAFETY: an all-zero Kernel is the empty one (no words, no labels bound: None is the zero discriminant, no branches); built in place on the heap, its 128 KiB never on the stack.
    let mut k: Box<Kernel<16384>> = unsafe { Box::<Kernel<16384>>::new_zeroed().assume_init() };
    let (mut loop_consts, mut loop_start, mut loop_words) = (0usize, 0usize, 0usize);
    let mut top = None;
    let mut exit: Option<Label> = None;
    let mut loop_no = 0usize;
    let mut blocks: Vec<Label> = Vec::new();
    // What r58:r59 hold: the address of (slot, index, shift), and the slot whose high word r59 holds; forgotten where control flow joins (a block's end, a loop's top and its exits), since the paths into a join may have left other addresses there.
    let mut at_addr: Option<(u8, u32, u8)> = None;
    let mut hi_slot: Option<u8> = None;
    // (param, word index) for every immediate that carries a param.
    let mut patches: Vec<(u32, u32)> = Vec::new();
    // The next instruction after node i, for a load's wait: a load of the same address next needs none (its own wait covers both, so a row of a table's words lands in one wait).
    let next_emitted = |i: usize| (i + 1..n).find(|&j| keep[j] && !is_imm(j as u32) && !matches!(nodes[j], Op::Gid));
    for (i, op) in nodes.iter().enumerate() {
        if !keep[i] || is_imm(i as u32) {
            continue;
        }
        // This instruction's constant sources, each in the cheapest place the FAU rule allows: an immediate of the LUT, a uniform word (`ufau`), or else a constant register loaded just before. The rule: one 64-bit uniform slot an instruction, and at most two distinct 32-bit uniform/LUT words, the form's own LUT zero counted (va_validate.c).
        let mut words: [u16; 2] = [u16::MAX; 2];
        let mut nw = 0usize;
        let mut slot: Option<u8> = None;
        if fixed_zero(op) {
            words[0] = 0x100;
            nw = 1;
        }
        let mut place: [(u32, Src); 3] = [(u32::MAX, Src::Lut(Lut::ZERO)); 3];
        let mut np = 0;
        let mut loads = 0;
        op.sources(|s| {
            if !is_imm(s) || hoisted_at(s, i) || place[..np].iter().any(|p| p.0 == s) {
                return;
            }
            let bits = imm_bits(s).unwrap();
            let mut take = |key: u16, sl: Option<u8>| -> bool {
                let has = words[..nw].contains(&key);
                let slot_ok = match (sl, slot) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                };
                if slot_ok && (has || nw < 2) {
                    if !has {
                        words[nw] = key;
                        nw += 1;
                    }
                    if sl.is_some() {
                        slot = sl;
                    }
                    true
                } else {
                    false
                }
            };
            let lut = if param_of(s).is_none() { LUT.iter().position(|&v| v == bits) } else { None };
            let src = if let Some(li) = lut {
                if take(0x100 | li as u16, None) { Some(Src::Lut(Lut::index(li as u8))) } else { None }
            } else if let Some(&(sl, w)) = ufau.iter().find(|u| u.0 == s).map(|u| &u.1) {
                if take((sl as u16) << 1 | w as u16, Some(sl)) { Some(Src::Uniform(if w == 0 { u(sl).w0() } else { u(sl).w1() })) } else { None }
            } else {
                None
            };
            let src = src.unwrap_or_else(|| {
                let cr = CONST_REGS[loads];
                loads += 1;
                if let Some(p) = param_of(s) {
                    patches.push((p, k.len() as u32));
                }
                k.iadd_imm_i32(r(cr), Lut::ZERO, bits);
                Src::Reg(r(cr))
            });
            place[np] = (s, src);
            np += 1;
        });
        if top.is_some() {
            loop_consts += loads;
        }
        let rr = |s: u32| -> Src {
            match place[..np].iter().find(|p| p.0 == s) {
                Some(p) => p.1,
                None => Src::Reg(r(hoisted(s, i).unwrap_or(reg[s as usize]))),
            }
        };
        // A float operand as read, with the negation and the widen folded into it above.
        let fl = |s: u32, p: usize| -> Float {
            let f = Float { src: rr(s), neg: false, abs: false, widen: wid[i][p] };
            if negmask[i] >> p & 1 == 1 { f.neg() } else { f }
        };
        // A v2f16 operand as read, with its negation and swizzle.
        let hl = |s: u32, p: usize| -> Half2 { Half2 { src: rr(s), swz: swz[i][p], neg: negmask[i] >> p & 1 == 1, abs: false } };
        let zero16 = Half2::from(Lut::ZERO).neg();
        let d = if op.has_value() { r(reg[i]) } else { r(0) };
        let c = clamp[i];
        let clamped = |pl: Placed<'_>| {
            if c != Clamp::None {
                pl.clamp(c);
            }
        };
        match *op {
            Op::Const(_) | Op::Param(_) | Op::Gid => {}
            Op::FAdd(a, b) => clamped(k.fadd_f32(d, fl(a, 0), fl(b, 1))),
            Op::FMul(a, b) => clamped(k.fma_f32(d, fl(a, 0), fl(b, 1), Lut::ZERO.neg())),
            Op::FFma(a, b, c) => clamped(k.fma_f32(d, fl(a, 0), fl(b, 1), fl(c, 2))),
            Op::FNeg(a) => {
                k.fadd_f32(d, fl(a, 0).neg(), Lut::ZERO.neg());
            }
            Op::FMin(a, b) => {
                k.fmin_f32(d, fl(a, 0), fl(b, 1));
            }
            Op::FMax(a, b) => {
                k.fmax_f32(d, fl(a, 0), fl(b, 1));
            }
            Op::FRcp(a) => {
                k.frcp_f32(d, fl(a, 0));
            }
            Op::FRsq(a) => {
                k.frsq_f32(d, fl(a, 0));
            }
            Op::FFloor(a) => {
                k.fround_f32(d, rr(a), Round::Rtn);
            }
            Op::FCmp(c, a, b) => {
                k.fcmp_or_f32(d, c, CmpResult::M1, fl(a, 0), fl(b, 1), Lut::ZERO);
            }
            Op::HAdd(a, b) => clamped(k.fadd_v2f16(d, hl(a, 0), hl(b, 1))),
            Op::HMul(a, b) => clamped(k.fma_v2f16(d, hl(a, 0), hl(b, 1), zero16)),
            Op::HFma(a, b, c) => clamped(k.fma_v2f16(d, hl(a, 0), hl(b, 1), hl(c, 2))),
            Op::HMin(a, b) => {
                k.fmin_v2f16(d, hl(a, 0), hl(b, 1));
            }
            Op::HMax(a, b) => {
                k.fmax_v2f16(d, hl(a, 0), hl(b, 1));
            }
            Op::HNeg(a) => {
                k.fadd_v2f16(d, hl(a, 0).neg(), zero16);
            }
            Op::HPack(a, b) => {
                k.fadd_f32(Dst::H0(d), fl(a, 0), Lut::ZERO.neg());
                k.fadd_f32(Dst::H1(d), fl(b, 1), Lut::ZERO.neg());
            }
            Op::HNarrow(a) => {
                k.fadd_f32(Dst::H0(d), fl(a, 0), Lut::ZERO.neg());
            }
            // A broadcast or a widen read by an instruction that takes no swizzle or widen (a select, a store, a compare): made, x + −0 being x.
            Op::HBroadcast(a, hi) => {
                k.fadd_v2f16(d, Half2 { src: rr(a), swz: if hi { Swz16::H11 } else { Swz16::H00 }, neg: false, abs: false }, zero16);
            }
            Op::HWiden(a, hi) => {
                let f = Float { src: rr(a), neg: false, abs: false, widen: if hi { Widen::H1 } else { Widen::H0 } };
                k.fadd_f32(d, f, Lut::ZERO.neg());
            }
            Op::Sel(m, a, b) => {
                k.mux_i32(d, Mux::Bit, rr(a), rr(b), rr(m));
            }
            Op::IMul(a, b) => {
                k.imul_i32(d, rr(a), rr(b));
            }
            Op::IAdd(a, b) => {
                k.iadd_u32(d, rr(a), rr(b));
            }
            Op::ISub(a, b) => {
                k.isub_s32(d, rr(a), rr(b));
            }
            Op::ICmp(c, a, b) => {
                k.icmp_or_u32(d, c, CmpResult::M1, rr(a), rr(b), Lut::ZERO);
            }
            Op::Uniform(slot, word) => {
                k.mov_i32(d, if word == 0 { u(slot).w0() } else { u(slot).w1() });
            }
            Op::And(a, b) => {
                k.lshift_and_i32(d, rr(a), Lut::ZERO, Byte::B0, rr(b));
            }
            Op::Or(a, b) => {
                k.lshift_or_i32(d, rr(a), Lut::ZERO, Byte::B0, rr(b));
            }
            Op::Xor(a, b) => {
                k.lshift_xor_i32(d, rr(a), Lut::ZERO, Byte::B0, rr(b));
            }
            Op::Shl(a, s) => {
                k.lshift_or_i32(d, rr(a), rr(s), Byte::B0, Lut::ZERO);
            }
            Op::Shr(a, s) => {
                k.rshift_or_i32(d, rr(a), rr(s), Byte::B0, Lut::ZERO);
            }
            Op::F2U(a) => {
                k.f32_to_u32(d, rr(a), Round::Rtz);
            }
            Op::F2I(a) => {
                k.f32_to_s32(d, rr(a), Round::Rtz);
            }
            Op::U2F(a) => {
                k.u32_to_f32(d, rr(a));
            }
            Op::S2F(a) => {
                k.s32_to_f32(d, rr(a));
            }
            Op::FrexpE(a, sqrt, log) => {
                k.frexpe_f32(d, rr(a), sqrt, log);
            }
            Op::FLogd(a) => {
                k.flogd_f32(d, rr(a));
            }
            Op::FAddLscale(a, b) => {
                k.fadd_lscale_f32(d, rr(a), rr(b));
            }
            Op::FExp(a, b) => {
                k.fexp_f32(d, rr(a), rr(b));
            }
            Op::Phi(init) => {
                k.mov_i32(d, rr(init));
            }
            Op::BlockBegin(m) => {
                // The invocations whose mask is clear go to the block's end, there to wait for the rest (reconverge: a divergent branch, va_insert_flow.c).
                let end = k.label();
                k.branchz_eq(rr(m), end).reconverge();
                blocks.push(end);
            }
            Op::BlockEnd => {
                // The block's last instruction falls thru to the branch's target: it carries the reconverge, or a NOP after it does where it already waits on a load.
                let end = blocks.pop().expect("valhall::ssa: a block's end without its begin");
                if k.last_flow() != 0 {
                    k.nop().reconverge();
                } else {
                    k.last().reconverge();
                }
                k.bind(end);
                at_addr = None;
                hi_slot = None;
            }
            Op::LoopHead => {
                // The loop's hoisted constants, once, before its top.
                for &(c, hr) in &held[loop_no] {
                    if let Some(p) = param_of(c) {
                        patches.push((p, k.len() as u32));
                    }
                    k.iadd_imm_i32(r(hr), Lut::ZERO, imm_bits(c).unwrap());
                }
                // The instruction before the loop's top carries reconverge (the Mandelbrot's form): a write to the scratch register.
                k.iadd_imm_i32(r(SCRATCH), Lut::ZERO, 0).reconverge();
                top = Some(k.here());
                loop_start = k.len();
                at_addr = None;
                hi_slot = None;
            }
            Op::LoopBreak(running) => {
                // An invocation no longer running leaves with the head's state: the phis take the head's outputs (for every invocation here: the tail reads the head's outputs, never a phi the head changed), then the stopped ones branch out, to wait at the loop's exit for the rest (reconverge: the block ends in a conditional branch, va_insert_flow.c).
                let ex = &rec.loops[loop_no].exits;
                let moves = ex.iter().filter(|&&(_, o)| !is_imm(o)).map(|&(p, o)| (reg[p as usize], reg[o as usize])).collect();
                let cs: Vec<(u8, u32, Option<u32>)> = ex.iter().filter(|&&(_, o)| is_imm(o)).map(|&(p, o)| (reg[p as usize], imm_bits(o).unwrap(), param_of(o))).collect();
                parallel_copy(&mut k, moves, &cs, &mut patches);
                let out = k.label();
                k.branchz_eq(rr(running), out).reconverge();
                exit = Some(out);
            }
            Op::LoopEnd(cont) => {
                // The phis take their updates all at once.
                let ca = &rec.loops[loop_no].carried;
                let moves = ca.iter().filter(|&&(_, u)| !is_imm(u)).map(|&(p, u)| (reg[p as usize], reg[u as usize])).collect();
                let cs: Vec<(u8, u32, Option<u32>)> = ca.iter().filter(|&&(_, u)| is_imm(u)).map(|&(p, u)| (reg[p as usize], imm_bits(u).unwrap(), param_of(u))).collect();
                parallel_copy(&mut k, moves, &cs, &mut patches);
                k.branchz(rr(cont), top.take().expect("valhall::ssa: loop end without head")).reconverge();
                // The stopped invocations wait here for the rest.
                k.bind(exit.take().expect("valhall::ssa: loop end without its break"));
                loop_words += k.len() - loop_start;
                loop_no += 1;
                at_addr = None;
                hi_slot = None;
            }
            Op::Store { slot, shift, offset, index, value } => {
                if at_addr != Some((slot, index, shift)) {
                    address(&mut k, slot, rr(index), shift, hi_slot == Some(slot));
                    at_addr = Some((slot, index, shift));
                    hi_slot = Some(slot);
                }
                // A store takes its value from a register only: a constant one goes through a constant register.
                let v = match rr(value) {
                    Src::Reg(x) => x,
                    other => {
                        k.mov_i32(r(CONST_REGS[2]), other);
                        r(CONST_REGS[2])
                    }
                };
                k.store_i32(v, pair(ADDR), offset, Slot::S0);
            }
            Op::Load { slot, shift, offset, index } => {
                if at_addr != Some((slot, index, shift)) {
                    address(&mut k, slot, rr(index), shift, hi_slot == Some(slot));
                    at_addr = Some((slot, index, shift));
                    hi_slot = Some(slot);
                }
                // The wait on the load itself, so the next instruction sees the value (the flow field acts after the instruction carrying it); none where the next is a load of the same address, whose wait covers both.
                let batched = next_emitted(i).is_some_and(|j| matches!(nodes[j], Op::Load { slot: s2, shift: h2, index: x2, .. } if (s2, x2, h2) == (slot, index, shift)));
                let pl = k.load_i32(d, pair(ADDR), offset, Slot::S0);
                if !batched {
                    pl.wait(Slot::S0);
                }
            }
        }
    }
    k.last().end();
    let words = k.finish().to_vec();
    for (p, at) in patches {
        params[p as usize].words.push(at);
    }
    let mut kinds: Vec<(&'static str, usize)> = Vec::new();
    for (i, op) in nodes.iter().enumerate() {
        if keep[i] {
            let name = op.name();
            match kinds.iter_mut().find(|k| k.0 == name) {
                Some(k) => k.1 += 1,
                None => kinds.push((name, 1)),
            }
        }
    }
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    Compiled { words, regs64, nodes: n, kept, peak, loop_words, loop_consts, consts, kinds, params, reserved }
}

/// Registers taking new values all at once (a parallel copy): each (destination, source) pair, then each (destination, constant, the param it is if one). A copy waits until no other pending copy still reads its destination, and a ring of them goes through the scratch register; constants last, as nothing reads them from a register. Used where a loop's phis take their values: a value can be another field's phi (a fold handing back a node already recorded).
fn parallel_copy(k: &mut Kernel<16384>, mut moves: Vec<(u8, u8)>, consts: &[(u8, u32, Option<u32>)], patches: &mut Vec<(u32, u32)>) {
    for (j, m) in moves.iter().enumerate() {
        // Two values into one register would be an allocation bug.
        assert!(!moves[..j].iter().any(|o| o.0 == m.0), "valhall::ssa: two loop copies into r{}", m.0);
    }
    moves.retain(|m| m.0 != m.1);
    while !moves.is_empty() {
        match moves.iter().position(|&(d, _)| !moves.iter().any(|&(_, s)| s == d)) {
            Some(j) => {
                let (d, s) = moves.remove(j);
                k.mov_i32(r(d), r(s));
            }
            None => {
                // A ring: park one destination's old value, which its readers then take from the scratch register.
                let d = moves[0].0;
                k.mov_i32(r(SCRATCH), r(d));
                for m in moves.iter_mut() {
                    if m.1 == d {
                        m.1 = SCRATCH;
                    }
                }
            }
        }
    }
    for &(d, bits, param) in consts {
        if let Some(p) = param {
            patches.push((p, k.len() as u32));
        }
        k.iadd_imm_i32(r(d), Lut::ZERO, bits);
    }
}
/// Whether an instruction form already reads the LUT's zero itself (a multiply's −0 addend, a negation's, a compare's `or`, the shift group's shift or second operand, a narrowing's or a made broadcast's −0): it counts against the FAU's two words.
fn fixed_zero(op: &Op) -> bool {
    matches!(op, Op::FMul(..) | Op::FNeg(_) | Op::FCmp(..) | Op::ICmp(..) | Op::And(..) | Op::Or(..) | Op::Xor(..) | Op::Shl(..) | Op::Shr(..) | Op::HMul(..) | Op::HNeg(_) | Op::HPack(..) | Op::HNarrow(_) | Op::HBroadcast(..) | Op::HWiden(..))
}

/// r58:r59 = the address in uniform `slot` plus `index << shift`, in 32 bits (the slot's high word kept: the layer arena lies below 4 GiB, so the low word's add never carries). The shift amount is a byte of the LUT's byte-index entries, so it needs no register. `hi_set`: r59 holds the slot's high word already (the last address was this slot's, on every path here).
fn address(k: &mut Kernel<16384>, slot: u8, index: Src, shift: u8, hi_set: bool) {
    // WHY: the byte-index entries name shifts 0 to 7; a larger one would silently shift by 7.
    assert!(shift < 8, "valhall::ssa: an address shift of {shift} (0 to 7 only)");
    let (lut, byte) = match shift {
        0 => (Lut::BYTES_0_3, Byte::B0),
        1 => (Lut::BYTES_0_3, Byte::B1),
        2 => (Lut::BYTES_0_3, Byte::B2),
        3 => (Lut::BYTES_0_3, Byte::B3),
        4 => (Lut::BYTES_4_7, Byte::B0),
        5 => (Lut::BYTES_4_7, Byte::B1),
        6 => (Lut::BYTES_4_7, Byte::B2),
        _ => (Lut::BYTES_4_7, Byte::B3),
    };
    k.lshift_or_i32(r(ADDR), index, lut, byte, Lut::ZERO);
    k.iadd_u32(r(ADDR), r(ADDR), u(slot).w0());
    if !hi_set {
        k.mov_i32(r(ADDR + 1), u(slot).w1());
    }
}
