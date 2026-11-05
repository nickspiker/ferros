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
    /// A loop-carried value: its value before the loop.
    Phi(u32),
    /// The loop's top (after its phis).
    LoopHead,
    /// The loop's bottom: each phi takes its update, and the loop goes round again while `cont` is set. Its updates are in [`Rec::loops`].
    LoopEnd(u32),
    /// value → *(uniform slot's address + (gid << shift) + offset).
    Store { slot: u8, shift: u8, offset: i16, value: u32 },
}

impl Op {
    fn sources(&self, mut f: impl FnMut(u32)) {
        use Op::*;
        match *self {
            Const(_) | Gid | LoopHead => {}
            FNeg(a) | FRcp(a) | FRsq(a) | FFloor(a) | F2U(a) | F2I(a) | U2F(a) | Phi(a) | LoopEnd(a) => f(a),
            FAdd(a, b) | FMul(a, b) | FMin(a, b) | FMax(a, b) | FCmp(_, a, b) | IMul(a, b) | And(a, b) | Or(a, b) | Xor(a, b) | Shl(a, b) | Shr(a, b) => {
                f(a);
                f(b)
            }
            FFma(a, b, c) | Sel(a, b, c) => {
                f(a);
                f(b);
                f(c)
            }
            Store { value, .. } => f(value),
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
}

/// The recording in progress. Single-threaded by construction: a kernel is recorded on one core, between [`begin`] and [`compile`].
static mut REC: Option<Rec> = None;

fn rec() -> &'static mut Rec {
    // SAFETY: one recording at a time on one thread (the module's contract, above); `begin` makes it Some before any handle exists.
    unsafe { (*(&raw mut REC)).as_mut().expect("valhall::ssa: record between begin() and compile()") }
}

fn push(op: Op) -> u32 {
    let r = rec();
    r.nodes.push(op);
    (r.nodes.len() - 1) as u32
}

/// Start recording a kernel.
pub fn begin() {
    unsafe { *(&raw mut REC) = Some(Rec { nodes: Vec::new(), loops: Vec::new() }) };
}

/// The invocation's global id (X), preloaded in r60.
pub fn gid() -> U {
    U(push(Op::Gid))
}

/// `value` to the address in uniform `slot` (64-bit) plus `gid << shift` plus `offset` bytes.
pub fn store(slot: u8, shift: u8, offset: i16, value: U) {
    push(Op::Store { slot, shift, offset, value: value.0 });
}

impl F {
    pub fn splat(v: f32) -> F {
        F(push(Op::Const(v.to_bits())))
    }
    fn op(op: Op) -> F {
        F(push(op))
    }
    /// 1 / self, refined by one Newton step from the special function unit's estimate.
    pub fn rcp(self) -> F {
        let r = F::op(Op::FRcp(self.0));
        let e = F::op(Op::FFma(self.neg().0, r.0, F::splat(1.).0));
        F::op(Op::FFma(r.0, e.0, r.0))
    }
    /// √self from the reciprocal square root, refined once (within an ulp, near or at the correctly rounded value), 0 at 0.
    pub fn sqrt(self) -> F {
        let rs = F::op(Op::FRsq(self.0));
        let s = self * rs;
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
    /// selfⁿ by squaring and multiplying, n known when recorded.
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
        let r = o.rcp();
        let q = self * r;
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
    let nodes = &rec.nodes;
    let n = nodes.len();
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
                    if upd != phi && (upd as usize) > h && (upd as usize) < i && !is_const(upd) && inner <= upd as usize {
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
    // Registers: linear scan over the kept nodes that hold a value (constants and the global id excepted).
    let mut reg = alloc::vec![u8::MAX; n];
    let mut free: Vec<u8> = (0..POOL).rev().collect();
    let mut live: Vec<(usize, u8)> = Vec::new();
    let (mut high, mut peak) = (0u8, 0usize);
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
        if matches!(op, Op::Gid) {
            reg[i] = 60;
        } else if let Some(phi) = coalesce[i] {
            // The phi's register, which its own entry keeps to the loop's end: the update is written over the phi after the phi's last read.
            reg[i] = reg[phi as usize];
        } else if op.has_value() && !is_const(i as u32) {
            let Some(r) = free.pop() else {
                panic!("valhall::ssa: out of registers at node {i} of {n} ({} live; no spilling yet)", live.len());
            };
            high = high.max(r);
            reg[i] = r;
            live.push((last[i].max(i), r));
            peak = peak.max(live.len());
        }
    }
    let regs64 = high > 15;
    let kept = keep.iter().filter(|&&k| k).count();
    // Emission.
    // SAFETY: an all-zero Kernel is the empty one (no words, no labels bound: None is the zero discriminant, no branches); built in place on the heap, its 128 KiB never on the stack.
    let mut k: Box<Kernel<16384>> = unsafe { Box::<Kernel<16384>>::new_zeroed().assume_init() };
    let mut top = None;
    let mut loop_no = 0usize;
    for (i, op) in nodes.iter().enumerate() {
        if !keep[i] || is_const(i as u32) {
            continue;
        }
        // This instruction's constant sources, each loaded into its own constant register first.
        let mut consts = [(u32::MAX, 0u8); 3];
        let mut nc = 0;
        op.sources(|s| {
            if is_const(s) && !consts[..nc].iter().any(|c| c.0 == s) {
                consts[nc] = (s, CONST_REGS[nc]);
                nc += 1;
            }
        });
        for &(s, cr) in &consts[..nc] {
            let Op::Const(bits) = nodes[s as usize] else { unreachable!() };
            k.iadd_imm_i32(r(cr), Lut::ZERO, bits);
        }
        let rr = |s: u32| -> Reg {
            match consts[..nc].iter().find(|c| c.0 == s) {
                Some(c) => r(c.1),
                None => r(reg[s as usize]),
            }
        };
        let d = if op.has_value() { r(reg[i]) } else { r(0) };
        match *op {
            Op::Const(_) | Op::Gid => {}
            Op::FAdd(a, b) => {
                k.fadd_f32(d, rr(a), rr(b));
            }
            Op::FMul(a, b) => {
                k.fma_f32(d, rr(a), rr(b), Lut::ZERO.neg());
            }
            Op::FFma(a, b, c) => {
                k.fma_f32(d, rr(a), rr(b), rr(c));
            }
            Op::FNeg(a) => {
                k.fadd_f32(d, rr(a).neg(), Lut::ZERO.neg());
            }
            Op::FMin(a, b) => {
                k.fmin_f32(d, rr(a), rr(b));
            }
            Op::FMax(a, b) => {
                k.fmax_f32(d, rr(a), rr(b));
            }
            Op::FRcp(a) => {
                k.frcp_f32(d, rr(a));
            }
            Op::FRsq(a) => {
                k.frsq_f32(d, rr(a));
            }
            Op::FFloor(a) => {
                k.fround_f32(d, rr(a), Round::Rtn);
            }
            Op::FCmp(c, a, b) => {
                k.fcmp_or_f32(d, c, CmpResult::M1, rr(a), rr(b), Lut::ZERO);
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
            Op::Phi(init) => {
                k.mov_i32(d, rr(init));
            }
            Op::LoopHead => {
                // The instruction before the loop's top carries reconverge (the Mandelbrot's form): a write to the scratch register.
                k.iadd_imm_i32(r(SCRATCH), Lut::ZERO, 0).reconverge();
                top = Some(k.here());
            }
            Op::LoopEnd(cont) => {
                // Every update is already in its own register (a select or the counter's add): the phis take them in any order.
                for &(phi, upd) in &rec.loops[loop_no].carried {
                    if let Op::Const(bits) = nodes[upd as usize] {
                        k.iadd_imm_i32(r(reg[phi as usize]), Lut::ZERO, bits);
                    } else if upd != phi && reg[upd as usize] != reg[phi as usize] {
                        k.mov_i32(r(reg[phi as usize]), rr(upd));
                    }
                }
                k.branchz(rr(cont), top.take().expect("valhall::ssa: loop end without head")).reconverge();
                loop_no += 1;
            }
            Op::Store { slot, shift, offset, value } => {
                k.shaddx_u64(pair(ADDR), u(slot), pair(60), shift);
                k.store_i32(rr(value), pair(ADDR), offset, Slot::S0);
            }
        }
    }
    k.last().end();
    let words = k.finish().to_vec();
    Compiled { words, regs64, nodes: n, kept, peak }
}
