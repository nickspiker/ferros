//! A recorder and compiler for whole kernels (GPU.md, "The resident service, and the attest mark's path"): code written once against a lane type runs on the CPU's lanes as it always did, and run once with this module's [`F`], [`M`] and [`U`] it records every operation into an SSA graph, which [`compile`] turns into Valhall words through the builder.
//!
//! The values are handles: a `u32` naming a node. The arithmetic on them records a node and returns its handle, so the code being recorded neither knows nor cares that it is not computing.
//! A recorded program has one shape: straight-line code, at most one loop at a time (no nesting) whose carried state is a `Copy` value made only of handles, and stores at the end.
//!
//! # The loop
//! [`iterate`] records the loop body once. On the CPU the same hook runs `head`, leaves when no lane is still running, then runs `tail`; here both are recorded and the tail runs on the pass that stops too, leaving the state as it was for an invocation that stopped (the several-lane code's own selects), which gives the values a break would have left with no branch but the backward one (the form the device ran for the Mandelbrot).
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

#[derive(Clone, Copy, Debug)]
enum Op {
    Const(u32),
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
    /// value to *(uniform slot's address + (index << shift) + offset).
    Store { slot: u8, shift: u8, offset: i16, index: u32, value: u32 },
    /// *(uniform slot's address + (index << shift) + offset), 32 bits.
    Load { slot: u8, shift: u8, offset: i16, index: u32 },
}

impl Op {
    fn sources(&self, mut f: impl FnMut(u32)) {
        use Op::*;
        match *self {
            Const(_) | Gid | LoopHead => {}
            FNeg(a) | FRcp(a) | FRsq(a) | FFloor(a) | F2U(a) | F2I(a) | U2F(a) | S2F(a) | FrexpE(a, _, _) | FLogd(a) | Phi(a) | LoopEnd(a) => f(a),
            FAddLscale(a, b) | FExp(a, b) => {
                f(a);
                f(b)
            }
            FAdd(a, b) | FMul(a, b) | FMin(a, b) | FMax(a, b) | FCmp(_, a, b) | IMul(a, b) | And(a, b) | Or(a, b) | Xor(a, b) | Shl(a, b) | Shr(a, b) => {
                f(a);
                f(b)
            }
            FFma(a, b, c) | Sel(a, b, c) => {
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
    /// The kind's name, for `Compiled::kinds`.
    fn name(&self) -> &'static str {
        use Op::*;
        match self {
            Const(_) => "const",
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
            Store { .. } => "store",
            Load { .. } => "load",
        }
    }
    fn has_value(&self) -> bool {
        !matches!(self, Op::LoopHead | Op::LoopEnd(_) | Op::Store { .. })
    }
}

struct Loop {
    /// (phi node, its update node).
    carried: Vec<(u32, u32)>,
}

struct Rec {
    nodes: Vec<Op>,
    loops: Vec<Loop>,
    /// Uniform slots 0 to reserved - 1 are the caller's (its addresses); the constants go from there.
    reserved: u8,
    /// Divides, reciprocals and square roots refined to the CPU's correctly rounded results, and no multiply fused into an add (`exact`); otherwise the special function unit's values as they come (within an ulp) and contraction, which is faster and looks the same.
    exact: bool,
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
    unsafe { *(&raw mut REC) = Some(Rec { nodes: Vec::new(), loops: Vec::new(), reserved: 1, exact: false }) };
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
    push(Op::Store { slot, shift, offset, index: index.0, value: value.0 });
}

/// 32 bits from the address in uniform `slot` plus `index << shift` plus `offset` bytes (the same address as `store_at`).
pub fn load(slot: u8, index: U, shift: u8, offset: i16) -> U {
    U(push(Op::Load { slot, shift, offset, index: index.0 }))
}

/// Record the exact forms (see `Rec::exact`) from here, for a test against the CPU's pixels bit for bit. Fast by default.
pub fn exact(on: bool) {
    rec().exact = on;
}

/// Uniform slots 0 to n - 1 for the caller (its addresses, set before `begin`'s recording uses any constant): the kernel's constants go from slot n. One by default.
pub fn reserve(n: u8) {
    rec().reserved = n;
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
    let after_tail = tail(n, after_head);
    // The tail leaves a stopped invocation's state as it was (its own selects on `running`, as tapuwae's several-lane code freezes a stopped lane), so its output is the update: no select here, and no second copy of the state alive to the loop's end (it ran the register file out on the attest mark, 2026-11-05).
    let updates: Vec<u32> = as_ids(&after_tail);
    let n1 = n + 1.;
    let more = n1.lt(F::splat(max as f32));
    let cont = running & more;
    let mut carried = Vec::with_capacity(words + 1);
    carried.push((n.0, n1.0));
    carried.extend(phis.iter().copied().zip(updates));
    let r = rec();
    r.loops.push(Loop { carried });
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
}

/// The value registers: r0-r53. r54-r56 take constants at their uses, r57 the loop head's write, r58:r59 the store address, r60:r61 the global id (preloaded).
const POOL: u8 = 54;
const CONST_REGS: [u8; 3] = [54, 55, 56];
const SCRATCH: u8 = 57;
const ADDR: u8 = 58;

/// The recording to Valhall words; the recording ends.
/// Dead code goes first (the recorder runs every block `any()` guards; what reaches no store and no loop state is dropped). Constants hold no register: each use loads its constant into r54-r56 just before the instruction, so a constant made once and used everywhere costs no register across the kernel.
/// Panics if the program still needs more value registers than r0-r53 (no spilling yet), naming how many it needed.
pub fn compile() -> Compiled {
    let rec = unsafe { (*(&raw mut REC)).take().expect("valhall::ssa: compile without begin") };
    let n = rec.nodes.len();
    // Negations folded into the operands of the float operations that read them (Valhall negates a float source for free): a source that is FNeg(x) becomes x with that operand's bit in `negmask` set, nested ones cancelling; a negation nothing else reads then goes with the dead code (509 of the attest ball's 4836 nodes were negations, each an instruction).
    let mut work = rec.nodes.clone();
    let mut negmask = alloc::vec![0u8; n];
    for i in 0..n {
        let mut srcs = [0u32; 3];
        let ns = match work[i] {
            Op::FAdd(a, b) | Op::FMul(a, b) | Op::FMin(a, b) | Op::FMax(a, b) | Op::FCmp(_, a, b) => {
                srcs[..2].copy_from_slice(&[a, b]);
                2
            }
            Op::FFma(a, b, c) => {
                srcs = [a, b, c];
                3
            }
            Op::FRcp(a) | Op::FRsq(a) | Op::FNeg(a) => {
                srcs[0] = a;
                1
            }
            _ => 0,
        };
        for p in 0..ns {
            while let Op::FNeg(x) = work[srcs[p] as usize] {
                srcs[p] = x;
                negmask[i] ^= 1 << p;
            }
        }
        work[i] = match work[i] {
            Op::FAdd(..) => Op::FAdd(srcs[0], srcs[1]),
            Op::FMul(..) => Op::FMul(srcs[0], srcs[1]),
            Op::FMin(..) => Op::FMin(srcs[0], srcs[1]),
            Op::FMax(..) => Op::FMax(srcs[0], srcs[1]),
            Op::FCmp(c, ..) => Op::FCmp(c, srcs[0], srcs[1]),
            Op::FFma(..) => Op::FFma(srcs[0], srcs[1], srcs[2]),
            Op::FRcp(_) => Op::FRcp(srcs[0]),
            Op::FRsq(_) => Op::FRsq(srcs[0]),
            Op::FNeg(_) => Op::FNeg(srcs[0]),
            other => other,
        };
    }
    // Contraction (not when exact): an add of a product nothing else reads becomes one fused multiply-add, the product's negations carried into its factors and the add's into the sign of the product or of the addend; the multiply then goes with the dead code.
    if !rec.exact {
        let mut users = alloc::vec![0u32; n];
        for op in &work {
            op.sources(|s| users[s as usize] += 1);
        }
        for i in 0..n {
            let Op::FAdd(a, b) = work[i] else { continue };
            for (p, m, other) in [(0u32, a, b), (1, b, a)] {
                let Op::FMul(x, y) = work[m as usize] else { continue };
                if users[m as usize] != 1 {
                    continue;
                }
                let product = (negmask[m as usize] & 3) ^ (negmask[i] >> p & 1);
                let addend = negmask[i] >> (1 - p) & 1;
                work[i] = Op::FFma(x, y, other);
                negmask[i] = product | addend << 2;
                users[m as usize] = 0;
                break;
            }
        }
    }
    let nodes = &work;
    let is_const = |i: u32| matches!(nodes[i as usize], Op::Const(_));
    // Dead code: live from the stores, the loop ends' conditions and every loop's state, backwards.
    let mut keep = alloc::vec![false; n];
    for (i, op) in nodes.iter().enumerate() {
        if matches!(op, Op::Store { .. } | Op::LoopHead | Op::LoopEnd(_)) {
            keep[i] = true;
        }
    }
    for l in &rec.loops {
        for &(phi, upd) in &l.carried {
            keep[phi as usize] = true;
            keep[upd as usize] = true;
        }
    }
    for i in (0..n).rev() {
        if keep[i] {
            nodes[i].sources(|s| keep[s as usize] = true);
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
                for &(phi, upd) in &rec.loops[loop_no].carried {
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
                    if upd != phi && (upd as usize) > h && (upd as usize) < i && !is_const(upd) && inner <= upd as usize && !read_at_end {
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
    let allocate = |hoist: &[Vec<u32>]| -> Result<(Vec<u8>, u8, usize, Vec<usize>), usize> {
        let mut reg = alloc::vec![u8::MAX; n];
        let mut free: Vec<u8> = (0..POOL).rev().collect();
        let mut live: Vec<(usize, u8)> = Vec::new();
        let (mut high, mut peak) = (0u8, 0usize);
        let mut loop_peak = alloc::vec![0usize; spans.len()];
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
                    reg[c as usize] = r;
                    live.push((spans[l].1, r));
                }
            }
            if matches!(op, Op::Gid) {
                reg[i] = 60;
            } else if let Some(phi) = coalesce[i] {
                // The phi's register, which its own entry keeps to the loop's end: the update is written over the phi after the phi's last read.
                reg[i] = reg[phi as usize];
            } else if op.has_value() && !is_const(i as u32) {
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
        Ok((reg, high, peak, loop_peak))
    };
    // First without hoisting, for each loop's pressure; then each loop's most-loaded constants into the registers it leaves free (owner, 2026-11-05: the mark within a 120 Hz frame; 188 of the ball loop's 625 instructions were constant loads).
    let none: Vec<Vec<u32>> = spans.iter().map(|_| Vec::new()).collect();
    let (reg0, high0, peak0, loop_peak) = allocate(&none).unwrap_or_else(|i| panic!("valhall::ssa: out of registers at node {i} of {n} (no spilling yet)"));
    let mut hoist: Vec<Vec<u32>> = Vec::new();
    for (l, &(h, e)) in spans.iter().enumerate() {
        let mut uses: Vec<(u32, usize)> = Vec::new();
        for j in h..e {
            if keep[j] {
                nodes[j].sources(|s| {
                    if is_const(s) {
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
    let (reg, high, peak, _) = allocate(&hoist).unwrap_or((reg0, high0, peak0, loop_peak));
    // Where each hoisted constant holds its register: from its loop's head to its end, and only there (a use before or after the loop loads it as any other constant; reading the register there read whatever value had taken it: 595 black pixels on the attest ball, 2026-11-05).
    let mut hoist_span = alloc::vec![(usize::MAX, 0usize); n];
    for (l, cs) in hoist.iter().enumerate() {
        for &c in cs {
            hoist_span[c as usize] = spans[l];
        }
    }
    let hoisted_at = |s: u32, at: usize| reg[s as usize] != u8::MAX && at > hoist_span[s as usize].0 && at < hoist_span[s as usize].1;
    // Uniform words for the constants used most (a use inside a loop counting 2^6 times one outside), slots 1 to 15 (slot 0 is the caller's output address): read by the instruction itself, no load. LUT values and hoisted ones need none.
    let mut weight: Vec<(u32, usize)> = Vec::new();
    for (j, op) in nodes.iter().enumerate() {
        if !keep[j] {
            continue;
        }
        let in_loop = spans.iter().any(|s| j > s.0 && j < s.1);
        op.sources(|s| {
            if is_const(s) && !hoisted_at(s, j) {
                let Op::Const(bits) = nodes[s as usize] else { unreachable!() };
                if !LUT.contains(&bits) {
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
    // The service passes 16 uniform slots: the caller's `reserved` first, then two constants a slot.
    let reserved = rec.reserved;
    let ufau: Vec<(u32, (u8, u8))> = weight.iter().take(2 * (16 - reserved as usize)).enumerate().map(|(k, w)| (w.0, (reserved + (k / 2) as u8, (k % 2) as u8))).collect();
    let mut consts = alloc::vec![0u64; ufau.len().div_ceil(2)];
    for &(s, (sl, w)) in &ufau {
        let Op::Const(bits) = nodes[s as usize] else { unreachable!() };
        consts[(sl - reserved) as usize] |= (bits as u64) << (32 * w as u32);
    }
    let regs64 = high > 15;
    let kept = keep.iter().filter(|&&k| k).count();
    // Emission.
    // SAFETY: an all-zero Kernel is the empty one (no words, no labels bound: None is the zero discriminant, no branches); built in place on the heap, its 128 KiB never on the stack.
    let mut k: Box<Kernel<16384>> = unsafe { Box::<Kernel<16384>>::new_zeroed().assume_init() };
    let (mut loop_consts, mut loop_start, mut loop_words) = (0usize, 0usize, 0usize);
    let mut top = None;
    let mut loop_no = 0usize;
    for (i, op) in nodes.iter().enumerate() {
        if !keep[i] || is_const(i as u32) {
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
            if !is_const(s) || hoisted_at(s, i) || place[..np].iter().any(|p| p.0 == s) {
                return;
            }
            let Op::Const(bits) = nodes[s as usize] else { unreachable!() };
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
            let src = if let Some(li) = LUT.iter().position(|&v| v == bits) {
                if take(0x100 | li as u16, None) { Some(Src::Lut(Lut::index(li as u8))) } else { None }
            } else if let Some(&(sl, w)) = ufau.iter().find(|u| u.0 == s).map(|u| &u.1) {
                if take((sl as u16) << 1 | w as u16, Some(sl)) { Some(Src::Uniform(if w == 0 { u(sl).w0() } else { u(sl).w1() })) } else { None }
            } else {
                None
            };
            let src = src.unwrap_or_else(|| {
                let cr = CONST_REGS[loads];
                loads += 1;
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
                None => Src::Reg(r(reg[s as usize])),
            }
        };
        // A float operand as read, with the negation folded into it above.
        let fl = |s: u32, p: u32| -> Float {
            let f = Float { src: rr(s), neg: false, abs: false };
            if negmask[i] >> p & 1 == 1 { f.neg() } else { f }
        };
        let d = if op.has_value() { r(reg[i]) } else { r(0) };
        match *op {
            Op::Const(_) | Op::Gid => {}
            Op::FAdd(a, b) => {
                k.fadd_f32(d, fl(a, 0), fl(b, 1));
            }
            Op::FMul(a, b) => {
                k.fma_f32(d, fl(a, 0), fl(b, 1), Lut::ZERO.neg());
            }
            Op::FFma(a, b, c) => {
                k.fma_f32(d, fl(a, 0), fl(b, 1), fl(c, 2));
            }
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
            Op::Sel(m, a, b) => {
                k.mux_i32(d, Mux::Bit, rr(a), rr(b), rr(m));
            }
            Op::IMul(a, b) => {
                k.imul_i32(d, rr(a), rr(b));
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
            Op::LoopHead => {
                // The loop's hoisted constants, once, before its top.
                for &c in &hoist[loop_no] {
                    let Op::Const(bits) = nodes[c as usize] else { unreachable!() };
                    k.iadd_imm_i32(r(reg[c as usize]), Lut::ZERO, bits);
                }
                // The instruction before the loop's top carries reconverge (the Mandelbrot's form): a write to the scratch register.
                k.iadd_imm_i32(r(SCRATCH), Lut::ZERO, 0).reconverge();
                top = Some(k.here());
                loop_start = k.len();
            }
            Op::LoopEnd(cont) => {
                // The phis take their updates all at once (a parallel copy): an update can be another field's phi (a fold handing back a node already recorded), so a copy waits until no other pending copy still reads its register, and a ring of them goes through the scratch register. Constants last, as nothing reads them from a register.
                let mut moves: Vec<(u8, u8)> = Vec::new();
                for &(phi, upd) in &rec.loops[loop_no].carried {
                    if !is_const(upd) && reg[upd as usize] != reg[phi as usize] {
                        let (d, s) = (reg[phi as usize], reg[upd as usize]);
                        // Two fields with one phi register would be an allocation bug.
                        assert!(!moves.iter().any(|m| m.0 == d), "valhall::ssa: two loop copies into r{d}");
                        moves.push((d, s));
                    }
                }
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
                for &(phi, upd) in &rec.loops[loop_no].carried {
                    if let Op::Const(bits) = nodes[upd as usize] {
                        k.iadd_imm_i32(r(reg[phi as usize]), Lut::ZERO, bits);
                    }
                }
                k.branchz(rr(cont), top.take().expect("valhall::ssa: loop end without head")).reconverge();
                loop_words += k.len() - loop_start;
                loop_no += 1;
            }
            Op::Store { slot, shift, offset, index, value } => {
                address(&mut k, slot, rr(index), shift);
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
                address(&mut k, slot, rr(index), shift);
                // The wait on the load itself: the flow field acts after the instruction carrying it, so the next one sees the value.
                k.load_i32(d, pair(ADDR), offset, Slot::S0).wait(Slot::S0);
            }
        }
    }
    k.last().end();
    let words = k.finish().to_vec();
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
    Compiled { words, regs64, nodes: n, kept, peak, loop_words, loop_consts, consts, kinds }
}

/// Whether an instruction form already reads the LUT's zero itself (a multiply's −0 addend, a negation's, a compare's `or`, the shift group's shift or second operand): it counts against the FAU's two words.
fn fixed_zero(op: &Op) -> bool {
    matches!(op, Op::FMul(..) | Op::FNeg(_) | Op::FCmp(..) | Op::And(..) | Op::Or(..) | Op::Xor(..) | Op::Shl(..) | Op::Shr(..))
}

/// r58:r59 = the address in uniform `slot` plus `index << shift`, in 32 bits (the slot's high word kept: the layer arena lies below 4 GiB, so the low word's add never carries). The shift amount is a byte of the LUT's byte-index entries, so it needs no register.
fn address(k: &mut Kernel<16384>, slot: u8, index: Src, shift: u8) {
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
    k.mov_i32(r(ADDR + 1), u(slot).w1());
}
