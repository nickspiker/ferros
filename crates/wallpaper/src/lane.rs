//! The lane the picture is written over (tapuwae's route, its lanes.rs; GPU-KISTS.md plan step 0): one source for the CPU and the GPU.
//! On the CPU a lane is an `f32` and every operation is the IEEE one the crate always did, libm's logarithms and sine included, so the pictures and the pinned hashes stay as they were.
//! On the GPU (`gpu.rs`) it is valhall's recording type: the same code, run once, records a kernel.
//! One lane's meaning on both: an invocation is one pixel or one sample, and the orbit's loop leaves for it alone (`Lane::iterate`).
//! A choice that depends on the pixel is a `select` between two computed values; `pick` works the chosen one out only where it is needed (a branch on the CPU; on the GPU both are recorded and selected, as every invocation runs every block).
//! The generic code keeps the lane on the left of every operation (`x * 0.5`, `L::splat(1.) - x`), as tapuwae's does, and with + and × commuting exactly in IEEE that order changes no bit.

use core::ops::{Add, BitAnd, BitOr, Div, Mul, Neg, Not, Sub};

/// A truth per invocation.
pub trait Mask: Copy + BitAnd<Output = Self> + BitOr<Output = Self> + Not<Output = Self> {
    /// Whether any invocation needs the guarded block: always on the GPU, which records it.
    fn any(self) -> bool;
}

/// An f32 per invocation.
pub trait Lane: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> + Neg<Output = Self> + Add<f32, Output = Self> + Sub<f32, Output = Self> + Mul<f32, Output = Self> + Div<f32, Output = Self> {
    type M: Mask;
    fn splat(v: f32) -> Self;
    /// √x: correctly rounded on the CPU, the special function unit's on the GPU.
    fn sqrt(self) -> Self;
    /// log₂ x and ln x: libm's on the CPU, the special function unit's on the GPU.
    fn log2(self) -> Self;
    fn ln(self) -> Self;
    /// sin 2πt, t in turns: libm's sinf(t · 2π) on the CPU (the product rounded once, as the crate always took it), a polynomial on the GPU.
    fn sin_turns(self) -> Self;
    fn gt(self, o: Self) -> Self::M;
    fn ge(self, o: Self) -> Self::M;
    fn lt(self, o: Self) -> Self::M;
    fn select(m: Self::M, a: Self, b: Self) -> Self;
    /// The orbit's loop, as a hook (tapuwae's `Lane::iterate`): pass n (0 to `max` − 1, as a lane) through `head`; leave with its state when it is no longer running, otherwise through `tail`. On the CPU the plain loop; the GPU records `head` and `tail` once (valhall::ssa::iterate), an invocation that stops leaving after the head.
    #[inline(always)]
    fn iterate<S: Copy>(max: u32, init: S, mut head: impl FnMut(Self, S) -> (S, Self::M), mut tail: impl FnMut(Self, S) -> S) -> S {
        let mut s = init;
        for n in 0..max {
            let (h, running) = head(Self::splat(n as f32), s);
            if !running.any() {
                return h;
            }
            s = tail(Self::splat(n as f32), h);
        }
        s
    }
}

/// `then()` where `m` holds and `otherwise` elsewhere, `then` worked out only when some invocation needs it, through `sel` (a value's select, field by field).
#[inline(always)]
pub fn pick<L: Lane, T: Copy>(m: L::M, then: impl FnOnce() -> T, otherwise: T, sel: impl FnOnce(L::M, T, T) -> T) -> T {
    if m.any() { sel(m, then(), otherwise) } else { otherwise }
}

impl Mask for bool {
    #[inline(always)]
    fn any(self) -> bool {
        self
    }
}

impl Lane for f32 {
    type M = bool;
    #[inline(always)]
    fn splat(v: f32) -> f32 {
        v
    }
    #[inline(always)]
    fn sqrt(self) -> f32 {
        libm::sqrtf(self)
    }
    #[inline(always)]
    fn log2(self) -> f32 {
        libm::log2f(self)
    }
    #[inline(always)]
    fn ln(self) -> f32 {
        libm::logf(self)
    }
    #[inline(always)]
    fn sin_turns(self) -> f32 {
        libm::sinf(self * core::f32::consts::TAU)
    }
    #[inline(always)]
    fn gt(self, o: f32) -> bool {
        self > o
    }
    #[inline(always)]
    fn ge(self, o: f32) -> bool {
        self >= o
    }
    #[inline(always)]
    fn lt(self, o: f32) -> bool {
        self < o
    }
    #[inline(always)]
    fn select(m: bool, a: f32, b: f32) -> f32 {
        if m { a } else { b }
    }
}
